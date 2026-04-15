//! Training harness for the auto-tune clustering model.
//!
//! Loads image pairs from the MIT-Adobe FiveK dataset, extracts histogram
//! features, clusters them via K-means, and optimizes per-cluster filter
//! parameters using Nelder-Mead with zensim as the loss function.
//!
//! Phases (each caches its output, re-run skips completed phases):
//!   1. Feature extraction → features.bin
//!   2. Baseline evaluation → baseline_scores.bin
//!   3. K-means clustering → clusters.bin
//!   4. Per-cluster optimization → cluster_params.bin
//!   5. Evaluation + export → model_weights.rs
//!
//! Usage: cargo run --release --example train_autotune

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use zencodecs::DecodeRequest;
use zenfilters::filters::*;
use zenfilters::{FilterContext, OklabPlanes, gather_oklab_to_srgb_u8, scatter_srgb_u8_to_oklab};
use zenpixels::ColorPrimaries;
use zenpixels_convert::gamut::GamutMatrix;
use zenpixels_convert::oklab;
use zensim::{RgbSlice, Zensim, ZensimProfile};

// ── Directories ──────────────────────────────────────────────────────
const ORIGINAL_DIR: &str = "/mnt/v/input/fivek/original";
const EXPERT_DIR: &str = "/mnt/v/input/fivek/expert_c";
const OUTPUT_DIR: &str = "/mnt/v/output/zenfilters/training";

// ── Tuning knobs ─────────────────────────────────────────────────────
const MAX_DIM: u32 = 384; // Working resolution for optimization speed
const NUM_CLUSTERS: usize = 64;
const KMEANS_ITERS: usize = 30;
const OPTIM_EVALS: usize = 400; // Nelder-Mead budget per cluster
const CLUSTER_SAMPLE: usize = 20; // Images per cluster for optimization

const N_FEAT: usize = 142;
const N_PARAMS: usize = 18;

// Parameter bounds [min, max] indexed by TunedParams field order
const P_MIN: [f32; N_PARAMS] = [
    -3.0, -0.5, -1.0, -1.0, // exposure, contrast, highlights, shadows
    0.0, -0.5, -0.5, -0.5, // saturation, vibrance, temperature, tint
    0.0, 0.5, // black_point, white_point
    0.5, 0.1, // sigmoid_contrast, sigmoid_skew
    0.0, 0.0, // clarity, sharpen
    0.0, 0.0, // highlight_recovery, shadow_lift
    0.0, 0.0, // local_tonemap, gamut_expand
];
const P_MAX: [f32; N_PARAMS] = [
    3.0, 0.5, 1.0, 1.0, //
    2.0, 1.0, 0.5, 0.5, //
    0.2, 1.0, //
    3.0, 0.9, //
    1.0, 1.0, //
    1.0, 1.0, //
    1.0, 1.0, //
];

// Initial Nelder-Mead step sizes per parameter
const P_STEP: [f32; N_PARAMS] = [
    0.4, 0.08, 0.15, 0.15, // exposure, contrast, highlights, shadows
    0.15, 0.12, 0.08, 0.08, // saturation, vibrance, temperature, tint
    0.02, 0.08, // black_point, white_point
    0.25, 0.08, // sigmoid_contrast, sigmoid_skew
    0.12, 0.12, // clarity, sharpen
    0.12, 0.12, // highlight_recovery, shadow_lift
    0.12, 0.12, // local_tonemap, gamut_expand
];

// ── Helpers ──────────────────────────────────────────────────────────

/// Xorshift64 PRNG — simple, fast, deterministic.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn usize(&mut self) -> usize {
        self.next_u64() as usize
    }
    fn f32(&mut self) -> f32 {
        (self.next_u64() % 1_000_000) as f32 / 1_000_000.0
    }
}

fn clamp_params(a: &mut [f32; N_PARAMS]) {
    for i in 0..N_PARAMS {
        a[i] = a[i].clamp(P_MIN[i], P_MAX[i]);
    }
}

fn params_to_array(p: &TunedParams) -> [f32; N_PARAMS] {
    p.to_array()
}

fn array_to_params(a: &[f32; N_PARAMS]) -> TunedParams {
    TunedParams::from_array(a)
}

fn squared_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| (x - y) * (x - y)).sum()
}

// ── Binary I/O ───────────────────────────────────────────────────────

fn save_f32s(path: &Path, data: &[f32]) {
    let bytes: &[u8] = bytemuck::cast_slice(data);
    fs::write(path, bytes).expect("failed to write cache file");
}

fn load_f32s(path: &Path) -> Vec<f32> {
    let bytes = fs::read(path).expect("failed to read cache file");
    bytemuck::cast_slice(&bytes).to_vec()
}

fn save_u32s(path: &Path, data: &[u32]) {
    let bytes: &[u8] = bytemuck::cast_slice(data);
    fs::write(path, bytes).expect("failed to write cache file");
}

fn load_u32s(path: &Path) -> Vec<u32> {
    let bytes = fs::read(path).expect("failed to read cache file");
    bytemuck::cast_slice(&bytes).to_vec()
}

fn save_f64s(path: &Path, data: &[f64]) {
    let bytes: &[u8] = bytemuck::cast_slice(data);
    fs::write(path, bytes).expect("failed to write cache file");
}

fn load_f64s(path: &Path) -> Vec<f64> {
    let bytes = fs::read(path).expect("failed to read cache file");
    bytemuck::cast_slice(&bytes).to_vec()
}

// ── Image loading ────────────────────────────────────────────────────

/// Discover matching image pairs in original/ and expert_c/.
fn discover_pairs() -> Vec<(PathBuf, PathBuf)> {
    let mut pairs = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(ORIGINAL_DIR)
        .expect("cannot read original dir")
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("jpg"))
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let name = entry.file_name();
        let expert_path = PathBuf::from(EXPERT_DIR).join(&name);
        if expert_path.exists() {
            pairs.push((entry.path(), expert_path));
        }
    }
    pairs
}

/// Load a JPEG and resize to fit within max_dim, returning (pixels, w, h).
fn load_resized(path: &Path, max_dim: u32) -> Option<(Vec<u8>, u32, u32)> {
    use zenresize::{Filter, PixelDescriptor, ResizeConfig, Resizer};
    let bytes = fs::read(path).ok()?;
    let decoded = DecodeRequest::new(&bytes).decode().ok()?;
    let (iw, ih) = (decoded.width(), decoded.height());
    use zenpixels_convert::PixelBufferConvertTypedExt;
    let rgb8 = decoded.into_buffer().to_rgb8().copy_to_contiguous_bytes();

    if iw <= max_dim && ih <= max_dim {
        return Some((rgb8, iw, ih));
    }
    let scale = max_dim as f64 / iw.max(ih) as f64;
    let nw = ((iw as f64 * scale) as u32).max(1);
    let nh = ((ih as f64 * scale) as u32).max(1);
    let config = ResizeConfig::builder(iw, ih, nw, nh)
        .filter(Filter::Lanczos)
        .format(PixelDescriptor::RGB8_SRGB)
        .build();
    let mut resizer = Resizer::new(&config);
    let resized = resizer.resize(&rgb8);
    Some((resized, nw, nh))
}

/// Load a pair of images resized to matching dimensions.
fn load_pair(
    orig_path: &Path,
    expert_path: &Path,
    max_dim: u32,
) -> Option<(Vec<u8>, Vec<u8>, u32, u32)> {
    let (orig_px, ow, oh) = load_resized(orig_path, max_dim)?;
    let (expert_px, ew, eh) = load_resized(expert_path, max_dim)?;

    // Handle rounding differences by cropping to common size
    let w = ow.min(ew);
    let h = oh.min(eh);

    if ow == w && oh == h && ew == w && eh == h {
        return Some((orig_px, expert_px, w, h));
    }

    // Crop both to common dimensions
    fn crop_rgb8(data: &[u8], w: u32, _h: u32, tw: u32, th: u32) -> Vec<u8> {
        let mut out = vec![0u8; (tw as usize) * (th as usize) * 3];
        for y in 0..th as usize {
            let src_off = y * (w as usize) * 3;
            let dst_off = y * (tw as usize) * 3;
            let row_bytes = (tw as usize) * 3;
            out[dst_off..dst_off + row_bytes].copy_from_slice(&data[src_off..src_off + row_bytes]);
        }
        out
    }
    let orig_cropped = crop_rgb8(&orig_px, ow, oh, w, h);
    let expert_cropped = crop_rgb8(&expert_px, ew, eh, w, h);
    Some((orig_cropped, expert_cropped, w, h))
}

// ── Feature extraction ───────────────────────────────────────────────

fn extract_features_from_pixels(pixels: &[u8], w: u32, h: u32, m1: &GamutMatrix) -> [f32; N_FEAT] {
    let mut planes = OklabPlanes::new(w, h);
    scatter_srgb_u8_to_oklab(pixels, &mut planes, 3, m1);
    let features = ImageFeatures::extract(&planes);
    let tensor = features.to_tensor();
    let mut arr = [0.0f32; N_FEAT];
    arr.copy_from_slice(&tensor);
    arr
}

// ── Pipeline application ─────────────────────────────────────────────

fn build_pipeline(params: &TunedParams) -> zenfilters::Pipeline {
    params.build_pipeline()
}

fn apply_params(
    orig_u8: &[u8],
    w: u32,
    h: u32,
    params: &TunedParams,
    m1: &GamutMatrix,
    m1_inv: &GamutMatrix,
    ctx: &mut FilterContext,
) -> Vec<u8> {
    let mut planes = OklabPlanes::new(w, h);
    scatter_srgb_u8_to_oklab(orig_u8, &mut planes, 3, m1);

    let pipeline = build_pipeline(params);
    pipeline.apply_planar(&mut planes, ctx);

    let mut output = vec![0u8; (w as usize) * (h as usize) * 3];
    gather_oklab_to_srgb_u8(&planes, &mut output, 3, m1_inv);
    output
}

#[allow(clippy::too_many_arguments)]
fn score_params(
    orig_u8: &[u8],
    expert_u8: &[u8],
    w: u32,
    h: u32,
    params: &TunedParams,
    m1: &GamutMatrix,
    m1_inv: &GamutMatrix,
    ctx: &mut FilterContext,
    zs: &Zensim,
) -> f64 {
    let filtered = apply_params(orig_u8, w, h, params, m1, m1_inv, ctx);
    let filtered_rgb: &[[u8; 3]] = bytemuck::cast_slice(&filtered);
    let expert_rgb: &[[u8; 3]] = bytemuck::cast_slice(expert_u8);
    let source = RgbSlice::new(filtered_rgb, w as usize, h as usize);
    let target = RgbSlice::new(expert_rgb, w as usize, h as usize);
    match zs.compute(&source, &target) {
        Ok(result) => result.score(),
        Err(_) => 0.0,
    }
}

// ── K-means ──────────────────────────────────────────────────────────

fn kmeans(data: &[[f32; N_FEAT]], k: usize, max_iters: usize) -> (Vec<[f32; N_FEAT]>, Vec<u32>) {
    let n = data.len();
    assert!(k <= n);
    let mut rng = Rng::new(42);

    // K-means++ initialization
    let mut centroids: Vec<[f32; N_FEAT]> = Vec::with_capacity(k);
    centroids.push(data[rng.usize() % n]);

    for _ in 1..k {
        let dists: Vec<f32> = data
            .iter()
            .map(|x| {
                centroids
                    .iter()
                    .map(|c| squared_distance(x, c))
                    .fold(f32::MAX, f32::min)
            })
            .collect();
        let total: f32 = dists.iter().sum();
        let threshold = rng.f32() * total;
        let mut cumsum = 0.0;
        let mut chosen = 0;
        for (i, &d) in dists.iter().enumerate() {
            cumsum += d;
            if cumsum >= threshold {
                chosen = i;
                break;
            }
        }
        centroids.push(data[chosen]);
    }

    // Iterate
    let mut assignments = vec![0u32; n];
    for iter in 0..max_iters {
        let mut changed = 0usize;
        for (i, x) in data.iter().enumerate() {
            let nearest = centroids
                .iter()
                .enumerate()
                .map(|(j, c)| (j, squared_distance(x, c)))
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                .unwrap()
                .0;
            if assignments[i] != nearest as u32 {
                changed += 1;
                assignments[i] = nearest as u32;
            }
        }

        if changed == 0 {
            println!("  K-means converged at iteration {iter}");
            break;
        }

        // Recompute centroids
        let mut sums = vec![[0.0f64; N_FEAT]; k];
        let mut counts = vec![0usize; k];
        for (i, x) in data.iter().enumerate() {
            let c = assignments[i] as usize;
            counts[c] += 1;
            for j in 0..N_FEAT {
                sums[c][j] += x[j] as f64;
            }
        }
        for c in 0..k {
            if counts[c] > 0 {
                for j in 0..N_FEAT {
                    centroids[c][j] = (sums[c][j] / counts[c] as f64) as f32;
                }
            }
        }

        if (iter + 1) % 10 == 0 || iter == 0 {
            println!("  K-means iteration {}: {changed} changed", iter + 1);
        }
    }

    (centroids, assignments)
}

// ── Nelder-Mead optimizer ────────────────────────────────────────────

/// Nelder-Mead simplex minimizer for N_PARAMS dimensions.
///
/// `f(x)` returns the value to minimize (negate zensim score).
/// Returns (best_point, best_value).
fn nelder_mead(
    f: &mut impl FnMut(&[f32; N_PARAMS]) -> f32,
    x0: &[f32; N_PARAMS],
    scales: &[f32; N_PARAMS],
    max_evals: usize,
) -> ([f32; N_PARAMS], f32) {
    let n = N_PARAMS;

    // Initialize simplex: x0 + one vertex per axis
    let mut simplex: Vec<([f32; N_PARAMS], f32)> = Vec::with_capacity(n + 1);
    let mut x = *x0;
    clamp_params(&mut x);
    simplex.push((x, f(&x)));

    for i in 0..n {
        let mut xi = *x0;
        xi[i] += scales[i];
        clamp_params(&mut xi);
        simplex.push((xi, f(&xi)));
    }

    let mut evals = n + 1;

    while evals < max_evals {
        // Sort by value (ascending — minimizing)
        simplex.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        let best_val = simplex[0].1;
        let worst_val = simplex[n].1;
        let second_worst_val = simplex[n - 1].1;

        if (worst_val - best_val).abs() < 1e-6 {
            break;
        }

        // Centroid of all except worst
        let mut centroid = [0.0f32; N_PARAMS];
        for v in simplex.iter().take(n) {
            for (j, c) in centroid.iter_mut().enumerate() {
                *c += v.0[j];
            }
        }
        for c in &mut centroid {
            *c /= n as f32;
        }

        // Reflection
        let mut reflected = [0.0f32; N_PARAMS];
        for j in 0..N_PARAMS {
            reflected[j] = 2.0 * centroid[j] - simplex[n].0[j];
        }
        clamp_params(&mut reflected);
        let fr = f(&reflected);
        evals += 1;

        if fr < second_worst_val && fr >= best_val {
            simplex[n] = (reflected, fr);
            continue;
        }

        if fr < best_val {
            // Expansion
            let mut expanded = [0.0f32; N_PARAMS];
            for j in 0..N_PARAMS {
                expanded[j] = centroid[j] + 2.0 * (reflected[j] - centroid[j]);
            }
            clamp_params(&mut expanded);
            let fe = f(&expanded);
            evals += 1;

            simplex[n] = if fe < fr {
                (expanded, fe)
            } else {
                (reflected, fr)
            };
            continue;
        }

        // Contraction
        let mut contracted = [0.0f32; N_PARAMS];
        if fr < worst_val {
            for j in 0..N_PARAMS {
                contracted[j] = centroid[j] + 0.5 * (reflected[j] - centroid[j]);
            }
        } else {
            for j in 0..N_PARAMS {
                contracted[j] = centroid[j] + 0.5 * (simplex[n].0[j] - centroid[j]);
            }
        }
        clamp_params(&mut contracted);
        let fc = f(&contracted);
        evals += 1;

        if fc < worst_val.min(fr) {
            simplex[n] = (contracted, fc);
            continue;
        }

        // Shrink toward best
        let best_point = simplex[0].0;
        for vertex in simplex.iter_mut().take(n + 1).skip(1) {
            for (j, bp) in best_point.iter().enumerate() {
                vertex.0[j] = bp + 0.5 * (vertex.0[j] - bp);
            }
            clamp_params(&mut vertex.0);
            vertex.1 = f(&vertex.0);
            evals += 1;
        }
    }

    simplex.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    (simplex[0].0, simplex[0].1)
}

// ── Image data for a cluster ─────────────────────────────────────────

struct ClusterImages {
    originals: Vec<Vec<u8>>,
    experts: Vec<Vec<u8>>,
    widths: Vec<u32>,
    heights: Vec<u32>,
}

// ── Training phases ──────────────────────────────────────────────────

fn phase_extract_features(pairs: &[(PathBuf, PathBuf)]) -> Vec<[f32; N_FEAT]> {
    let cache = PathBuf::from(OUTPUT_DIR).join("features.bin");
    if cache.exists() {
        println!("Loading cached features from {}", cache.display());
        let flat = load_f32s(&cache);
        let n = flat.len() / N_FEAT;
        assert_eq!(n, pairs.len(), "cache size mismatch — delete features.bin");
        return flat
            .chunks_exact(N_FEAT)
            .map(|c| {
                let mut arr = [0.0f32; N_FEAT];
                arr.copy_from_slice(c);
                arr
            })
            .collect();
    }

    let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
    let mut features = Vec::with_capacity(pairs.len());
    let t0 = Instant::now();

    for (i, (orig_path, _)) in pairs.iter().enumerate() {
        if let Some((pixels, w, h)) = load_resized(orig_path, MAX_DIM) {
            features.push(extract_features_from_pixels(&pixels, w, h, &m1));
        } else {
            // Placeholder for failed loads — zero features
            features.push([0.0f32; N_FEAT]);
        }

        if (i + 1) % 500 == 0 || i + 1 == pairs.len() {
            let elapsed = t0.elapsed().as_secs_f64();
            let rate = (i + 1) as f64 / elapsed;
            println!(
                "  Features: {}/{} ({:.0} img/s, {:.0}s remaining)",
                i + 1,
                pairs.len(),
                rate,
                (pairs.len() - i - 1) as f64 / rate
            );
        }
    }

    let flat: Vec<f32> = features.iter().flat_map(|f| f.iter().copied()).collect();
    save_f32s(&cache, &flat);
    println!(
        "  Saved features to {} ({:.1} MB)",
        cache.display(),
        flat.len() as f64 * 4.0 / 1e6
    );
    features
}

fn phase_baseline(pairs: &[(PathBuf, PathBuf)], features: &[[f32; N_FEAT]]) -> Vec<f64> {
    let cache = PathBuf::from(OUTPUT_DIR).join("baseline_scores.bin");
    if cache.exists() {
        println!("Loading cached baseline scores from {}", cache.display());
        let scores = load_f64s(&cache);
        assert_eq!(
            scores.len(),
            pairs.len(),
            "cache size mismatch — delete baseline_scores.bin"
        );
        return scores;
    }

    let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
    let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();
    let zs = Zensim::new(ZensimProfile::latest());
    let mut ctx = FilterContext::new();
    let mut scores = Vec::with_capacity(pairs.len());
    let t0 = Instant::now();

    for (i, (orig_path, expert_path)) in pairs.iter().enumerate() {
        let score = match load_pair(orig_path, expert_path, MAX_DIM) {
            Some((orig_px, expert_px, w, h)) => {
                let img_features = reconstruct_features(&features[i]);
                let params = rule_based_tune(&img_features);
                score_params(
                    &orig_px, &expert_px, w, h, &params, &m1, &m1_inv, &mut ctx, &zs,
                )
            }
            None => 0.0,
        };
        scores.push(score);

        if (i + 1) % 200 == 0 || i + 1 == pairs.len() {
            let elapsed = t0.elapsed().as_secs_f64();
            let rate = (i + 1) as f64 / elapsed;
            let mean: f64 = scores.iter().sum::<f64>() / scores.len() as f64;
            println!(
                "  Baseline: {}/{} ({:.0} img/s) mean={:.2}",
                i + 1,
                pairs.len(),
                rate,
                mean
            );
        }
    }

    save_f64s(&cache, &scores);
    scores
}

fn phase_cluster(features: &[[f32; N_FEAT]]) -> (Vec<[f32; N_FEAT]>, Vec<u32>) {
    let cache = PathBuf::from(OUTPUT_DIR).join("clusters.bin");
    if cache.exists() {
        println!("Loading cached clusters from {}", cache.display());
        let flat_centroids = load_f32s(&PathBuf::from(OUTPUT_DIR).join("centroids.bin"));
        let assignments = load_u32s(&cache);
        let centroids: Vec<[f32; N_FEAT]> = flat_centroids
            .chunks_exact(N_FEAT)
            .map(|c| {
                let mut arr = [0.0f32; N_FEAT];
                arr.copy_from_slice(c);
                arr
            })
            .collect();
        return (centroids, assignments);
    }

    let (centroids, assignments) = kmeans(features, NUM_CLUSTERS, KMEANS_ITERS);

    let flat_centroids: Vec<f32> = centroids.iter().flat_map(|c| c.iter().copied()).collect();
    save_f32s(
        &PathBuf::from(OUTPUT_DIR).join("centroids.bin"),
        &flat_centroids,
    );
    save_u32s(&cache, &assignments);

    (centroids, assignments)
}

fn phase_optimize(
    pairs: &[(PathBuf, PathBuf)],
    features: &[[f32; N_FEAT]],
    assignments: &[u32],
) -> Vec<[f32; N_PARAMS]> {
    let cache = PathBuf::from(OUTPUT_DIR).join("cluster_params.bin");
    if cache.exists() {
        println!("Loading cached cluster params from {}", cache.display());
        let flat = load_f32s(&cache);
        return flat
            .chunks_exact(N_PARAMS)
            .map(|c| {
                let mut arr = [0.0f32; N_PARAMS];
                arr.copy_from_slice(c);
                arr
            })
            .collect();
    }

    let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
    let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();
    let zs = Zensim::new(ZensimProfile::latest());

    let mut cluster_params = Vec::with_capacity(NUM_CLUSTERS);
    let total_t0 = Instant::now();

    for cluster_id in 0..NUM_CLUSTERS {
        let t0 = Instant::now();

        // Collect indices for this cluster
        let indices: Vec<usize> = assignments
            .iter()
            .enumerate()
            .filter(|&(_, a)| *a == cluster_id as u32)
            .map(|(i, _)| i)
            .collect();

        println!(
            "\n  Cluster {cluster_id}/{NUM_CLUSTERS}: {} images",
            indices.len()
        );

        if indices.is_empty() {
            cluster_params.push(params_to_array(&TunedParams::default()));
            continue;
        }

        // Sample images for optimization
        let mut rng = Rng::new(cluster_id as u64 + 100);
        let sample_indices: Vec<usize> = if indices.len() <= CLUSTER_SAMPLE {
            indices.clone()
        } else {
            let mut sampled = Vec::with_capacity(CLUSTER_SAMPLE);
            let mut available = indices.clone();
            for _ in 0..CLUSTER_SAMPLE {
                let idx = rng.usize() % available.len();
                sampled.push(available.swap_remove(idx));
            }
            sampled
        };

        // Pre-load sample images
        let mut cluster_data = ClusterImages {
            originals: Vec::new(),
            experts: Vec::new(),
            widths: Vec::new(),
            heights: Vec::new(),
        };
        for &idx in &sample_indices {
            let (orig_path, expert_path) = &pairs[idx];
            if let Some((orig_px, expert_px, w, h)) = load_pair(orig_path, expert_path, MAX_DIM) {
                cluster_data.originals.push(orig_px);
                cluster_data.experts.push(expert_px);
                cluster_data.widths.push(w);
                cluster_data.heights.push(h);
            }
        }

        let n_loaded = cluster_data.originals.len();
        println!("    Loaded {n_loaded} images for optimization");

        if n_loaded == 0 {
            cluster_params.push(params_to_array(&TunedParams::default()));
            continue;
        }

        // Compute initial point: mean rule_based_tune across cluster
        let mut initial = [0.0f32; N_PARAMS];
        for &idx in &indices {
            let feat_array = &features[idx];
            let img_features = reconstruct_features(feat_array);
            let p = rule_based_tune(&img_features);
            let pa = params_to_array(&p);
            for j in 0..N_PARAMS {
                initial[j] += pa[j];
            }
        }
        for v in &mut initial {
            *v /= indices.len() as f32;
        }
        clamp_params(&mut initial);

        // Objective: negative mean zensim score (Nelder-Mead minimizes)
        let mut ctx = FilterContext::new();
        let mut eval_count = 0u32;
        let mut best_seen = f32::MAX;

        let mut objective = |params: &[f32; N_PARAMS]| -> f32 {
            eval_count += 1;
            let tuned = array_to_params(params);
            let mut total = 0.0f64;
            for i in 0..n_loaded {
                let filtered = apply_params(
                    &cluster_data.originals[i],
                    cluster_data.widths[i],
                    cluster_data.heights[i],
                    &tuned,
                    &m1,
                    &m1_inv,
                    &mut ctx,
                );
                let filtered_rgb: &[[u8; 3]] = bytemuck::cast_slice(&filtered);
                let expert_rgb: &[[u8; 3]] = bytemuck::cast_slice(&cluster_data.experts[i]);
                let source = RgbSlice::new(
                    filtered_rgb,
                    cluster_data.widths[i] as usize,
                    cluster_data.heights[i] as usize,
                );
                let target = RgbSlice::new(
                    expert_rgb,
                    cluster_data.widths[i] as usize,
                    cluster_data.heights[i] as usize,
                );
                if let Ok(result) = zs.compute(&source, &target) {
                    total += result.score();
                }
            }
            let mean = -(total / n_loaded as f64) as f32;
            if mean < best_seen {
                best_seen = mean;
                if eval_count.is_multiple_of(50) {
                    println!("    eval {eval_count}: score={:.2}", -mean);
                }
            }
            mean
        };

        let (best_params, best_neg_score) =
            nelder_mead(&mut objective, &initial, &P_STEP, OPTIM_EVALS);

        println!(
            "    Cluster {cluster_id}: score={:.2} ({} evals, {:.1}s)",
            -best_neg_score,
            eval_count,
            t0.elapsed().as_secs_f64()
        );
        cluster_params.push(best_params);
    }

    // Save
    let flat: Vec<f32> = cluster_params
        .iter()
        .flat_map(|p| p.iter().copied())
        .collect();
    save_f32s(&cache, &flat);
    println!(
        "\nOptimization complete in {:.1}s",
        total_t0.elapsed().as_secs_f64()
    );

    cluster_params
}

fn reconstruct_features(feat_array: &[f32; N_FEAT]) -> ImageFeatures {
    ImageFeatures::from_tensor(feat_array)
}

// ── Evaluation & export ──────────────────────────────────────────────

fn phase_evaluate(
    pairs: &[(PathBuf, PathBuf)],
    _features: &[[f32; N_FEAT]],
    _centroids: &[[f32; N_FEAT]],
    assignments: &[u32],
    cluster_params: &[[f32; N_PARAMS]],
    baseline_scores: &[f64],
) {
    let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
    let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();
    let zs = Zensim::new(ZensimProfile::latest());
    let mut ctx = FilterContext::new();

    let mut cluster_baseline = [0.0f64; NUM_CLUSTERS];
    let mut cluster_optimized = [0.0f64; NUM_CLUSTERS];
    let mut cluster_counts = [0usize; NUM_CLUSTERS];
    let mut improved = 0usize;
    let mut degraded = 0usize;

    let t0 = Instant::now();
    let eval_count = pairs.len().min(500); // Evaluate on subset for speed

    let mut rng = Rng::new(999);
    let eval_indices: Vec<usize> = if pairs.len() <= eval_count {
        (0..pairs.len()).collect()
    } else {
        let mut indices: Vec<usize> = (0..pairs.len()).collect();
        // Fisher-Yates shuffle first eval_count elements
        for i in 0..eval_count {
            let j = i + rng.usize() % (indices.len() - i);
            indices.swap(i, j);
        }
        indices[..eval_count].to_vec()
    };

    for (progress, &idx) in eval_indices.iter().enumerate() {
        let (orig_path, expert_path) = &pairs[idx];
        let c = assignments[idx] as usize;

        if let Some((orig_px, expert_px, w, h)) = load_pair(orig_path, expert_path, MAX_DIM) {
            // Cluster model score
            let tuned = array_to_params(&cluster_params[c]);
            let opt_score = score_params(
                &orig_px, &expert_px, w, h, &tuned, &m1, &m1_inv, &mut ctx, &zs,
            );

            let bl_score = baseline_scores[idx];

            cluster_baseline[c] += bl_score;
            cluster_optimized[c] += opt_score;
            cluster_counts[c] += 1;

            if opt_score > bl_score + 0.5 {
                improved += 1;
            } else if opt_score < bl_score - 0.5 {
                degraded += 1;
            }
        }

        if (progress + 1) % 100 == 0 || progress + 1 == eval_indices.len() {
            println!(
                "  Eval: {}/{} ({:.1}s)",
                progress + 1,
                eval_indices.len(),
                t0.elapsed().as_secs_f64()
            );
        }
    }

    // Print per-cluster results
    println!(
        "\n  {:>3}  {:>5}  {:>8}  {:>8}  {:>8}",
        "C", "N", "Base", "Opt", "Delta"
    );
    println!("  {}", "-".repeat(40));
    let mut total_base = 0.0f64;
    let mut total_opt = 0.0f64;
    let mut total_n = 0usize;
    for c in 0..NUM_CLUSTERS {
        if cluster_counts[c] > 0 {
            let base = cluster_baseline[c] / cluster_counts[c] as f64;
            let opt = cluster_optimized[c] / cluster_counts[c] as f64;
            println!(
                "  {:>3}  {:>5}  {:>8.2}  {:>8.2}  {:>+8.2}",
                c,
                cluster_counts[c],
                base,
                opt,
                opt - base
            );
            total_base += cluster_baseline[c];
            total_opt += cluster_optimized[c];
            total_n += cluster_counts[c];
        }
    }
    if total_n > 0 {
        println!("  {}", "-".repeat(40));
        println!(
            "  {:>3}  {:>5}  {:>8.2}  {:>8.2}  {:>+8.2}",
            "ALL",
            total_n,
            total_base / total_n as f64,
            total_opt / total_n as f64,
            (total_opt - total_base) / total_n as f64
        );
    }
    println!(
        "\n  Improved: {improved}, Degraded: {degraded}, Neutral: {}",
        eval_indices.len() - improved - degraded
    );
}

fn phase_export(centroids: &[[f32; N_FEAT]], cluster_params: &[[f32; N_PARAMS]]) {
    let path = PathBuf::from(OUTPUT_DIR).join("model_weights.rs");
    let mut out = String::new();

    out.push_str("// Auto-generated by train_autotune. Do not edit.\n\n");
    out.push_str(&format!(
        "const CLUSTER_COUNT: usize = {};\n\n",
        centroids.len()
    ));

    // Centroids
    out.push_str(&format!(
        "const CENTROIDS: [[f32; {N_FEAT}]; {}] = [\n",
        centroids.len()
    ));
    for centroid in centroids {
        out.push_str("    [\n");
        for chunk in centroid.chunks(8) {
            out.push_str("        ");
            for (j, v) in chunk.iter().enumerate() {
                if j > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format!("{v:.6}"));
            }
            out.push_str(",\n");
        }
        out.push_str("    ],\n");
    }
    out.push_str("];\n\n");

    // Params
    let param_names = [
        "exposure",
        "contrast",
        "highlights",
        "shadows",
        "saturation",
        "vibrance",
        "temperature",
        "tint",
        "black_point",
        "white_point",
        "sigmoid_contrast",
        "sigmoid_skew",
        "clarity",
        "sharpen",
        "highlight_recovery",
        "shadow_lift",
        "local_tonemap",
        "gamut_expand",
    ];

    out.push_str(&format!(
        "const CLUSTER_PARAMS: [[f32; {N_PARAMS}]; {}] = [\n",
        cluster_params.len()
    ));
    for (ci, params) in cluster_params.iter().enumerate() {
        out.push_str(&format!("    // Cluster {ci}\n    [\n"));
        for (j, v) in params.iter().enumerate() {
            out.push_str(&format!("        {v:.6}, // {}\n", param_names[j]));
        }
        out.push_str("    ],\n");
    }
    out.push_str("];\n");

    fs::write(&path, &out).expect("failed to write export file");
    println!("Exported model to {}", path.display());
    println!(
        "  {} centroids × {N_FEAT} features = {} floats",
        centroids.len(),
        centroids.len() * N_FEAT
    );
    println!(
        "  {} clusters × {N_PARAMS} params = {} floats",
        cluster_params.len(),
        cluster_params.len() * N_PARAMS
    );
    println!(
        "  Total: {} floats = {:.1} KB",
        centroids.len() * N_FEAT + cluster_params.len() * N_PARAMS,
        (centroids.len() * N_FEAT + cluster_params.len() * N_PARAMS) as f64 * 4.0 / 1024.0
    );
}

// ── Main ─────────────────────────────────────────────────────────────

fn main() {
    let start = Instant::now();
    fs::create_dir_all(OUTPUT_DIR).expect("cannot create output dir");

    // Phase 1: Discover pairs
    println!("=== Discovering image pairs ===");
    let pairs = discover_pairs();
    println!("Found {} image pairs\n", pairs.len());

    // Phase 2: Extract features
    println!("=== Phase 1: Feature Extraction ===");
    let features = phase_extract_features(&pairs);
    println!();

    // Phase 3: Baseline evaluation
    println!("=== Phase 2: Baseline Evaluation ===");
    let baseline_scores = phase_baseline(&pairs, &features);
    let baseline_mean: f64 = baseline_scores.iter().sum::<f64>() / baseline_scores.len() as f64;
    println!("Baseline mean zensim: {baseline_mean:.2}\n");

    // Phase 4: Clustering
    println!("=== Phase 3: K-Means Clustering ===");
    let (centroids, assignments) = phase_cluster(&features);
    let mut sizes = vec![0usize; NUM_CLUSTERS];
    for &a in &assignments {
        sizes[a as usize] += 1;
    }
    println!("Cluster sizes: {sizes:?}\n");

    // Phase 5: Per-cluster optimization
    println!("=== Phase 4: Per-Cluster Optimization ===");
    let cluster_params = phase_optimize(&pairs, &features, &assignments);

    // Phase 6: Evaluation
    println!("\n=== Phase 5: Evaluation ===");
    phase_evaluate(
        &pairs,
        &features,
        &centroids,
        &assignments,
        &cluster_params,
        &baseline_scores,
    );

    // Phase 7: Export
    println!("\n=== Phase 6: Export ===");
    phase_export(&centroids, &cluster_params);

    println!(
        "\nTotal training time: {:.1}s ({:.1}m)",
        start.elapsed().as_secs_f64(),
        start.elapsed().as_secs_f64() / 60.0
    );
}
