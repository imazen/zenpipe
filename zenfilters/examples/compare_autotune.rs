//! Generate side-by-side comparison images for auto-tune models.
//!
//! Produces montages: Original | Rule-Based | Cluster Model | Expert C
//! with zensim scores annotated.
//!
//! Usage: cargo run --release --example compare_autotune

use std::fs;
use std::path::{Path, PathBuf};

use imgref::ImgVec;
use rgb::Rgb;
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat};
use zenfilters::filters::*;
use zenfilters::{FilterContext, OklabPlanes, gather_oklab_to_srgb_u8, scatter_srgb_u8_to_oklab};
use zenpixels::ColorPrimaries;
use zenpixels_convert::gamut::GamutMatrix;
use zenpixels_convert::oklab;
use zensim::{RgbSlice, Zensim, ZensimProfile};

const ORIGINAL_DIR: &str = "/mnt/v/input/fivek/original";
const EXPERT_DIR: &str = "/mnt/v/input/fivek/expert_c";
const TRAINING_DIR: &str = "/mnt/v/output/zenfilters/training";
const OUTPUT_DIR: &str = "/mnt/v/output/zenfilters/compare";
const N_FEAT: usize = 142;
const N_PARAMS: usize = 18;
const COMPARE_WIDTH: u32 = 512;
const DEFAULT_SAMPLES: usize = 32;

fn load_f32s(path: &Path) -> Vec<f32> {
    let bytes = fs::read(path).expect("failed to read file");
    bytemuck::cast_slice(&bytes).to_vec()
}

fn load_u32s(path: &Path) -> Vec<u32> {
    let bytes = fs::read(path).expect("failed to read file");
    bytemuck::cast_slice(&bytes).to_vec()
}

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
    Some((resizer.resize(&rgb8), nw, nh))
}

fn crop_rgb8(data: &[u8], w: u32, h: u32, tw: u32, th: u32) -> Vec<u8> {
    if tw == w && th == h {
        return data.to_vec();
    }
    let tw = tw.min(w);
    let th = th.min(h);
    let mut out = vec![0u8; (tw as usize) * (th as usize) * 3];
    for y in 0..th as usize {
        let src_off = y * (w as usize) * 3;
        let dst_off = y * (tw as usize) * 3;
        let row_bytes = (tw as usize) * 3;
        out[dst_off..dst_off + row_bytes].copy_from_slice(&data[src_off..src_off + row_bytes]);
    }
    out
}

fn load_pair(
    orig_path: &Path,
    expert_path: &Path,
    max_dim: u32,
) -> Option<(Vec<u8>, Vec<u8>, u32, u32)> {
    let (orig_px, ow, oh) = load_resized(orig_path, max_dim)?;
    let (expert_px, ew, eh) = load_resized(expert_path, max_dim)?;
    let w = ow.min(ew);
    let h = oh.min(eh);
    let orig_c = crop_rgb8(&orig_px, ow, oh, w, h);
    let expert_c = crop_rgb8(&expert_px, ew, eh, w, h);
    Some((orig_c, expert_c, w, h))
}

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

fn zensim_score(a: &[u8], b: &[u8], w: u32, h: u32, zs: &Zensim) -> f64 {
    let a_rgb: &[[u8; 3]] = bytemuck::cast_slice(a);
    let b_rgb: &[[u8; 3]] = bytemuck::cast_slice(b);
    let sa = RgbSlice::new(a_rgb, w as usize, h as usize);
    let sb = RgbSlice::new(b_rgb, w as usize, h as usize);
    zs.compute(&sa, &sb).map(|r| r.score()).unwrap_or(0.0)
}

fn array_to_params(a: &[f32]) -> TunedParams {
    let arr: &[f32; 18] = a.try_into().expect("expected 18-float param slice");
    TunedParams::from_array(arr)
}

fn reconstruct_features(feat: &[f32]) -> ImageFeatures {
    ImageFeatures::from_tensor(feat)
}

fn main() {
    fs::create_dir_all(OUTPUT_DIR).unwrap();

    // Load cached training data
    let features_flat = load_f32s(&PathBuf::from(TRAINING_DIR).join("features.bin"));
    let assignments = load_u32s(&PathBuf::from(TRAINING_DIR).join("clusters.bin"));
    let centroids_flat = load_f32s(&PathBuf::from(TRAINING_DIR).join("centroids.bin"));
    let params_flat = load_f32s(&PathBuf::from(TRAINING_DIR).join("cluster_params.bin"));

    let n_images = features_flat.len() / N_FEAT;
    let n_clusters = centroids_flat.len() / N_FEAT;

    println!("Loaded {n_images} features, {n_clusters} clusters");

    // Discover image pairs (same order as training)
    let mut pairs: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(ORIGINAL_DIR)
        .unwrap()
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
    assert_eq!(pairs.len(), n_images);

    // Pick one representative image per cluster (closest to centroid)
    let mut sample_indices = Vec::new();
    for cluster_id in 0..n_clusters {
        let centroid = &centroids_flat[cluster_id * N_FEAT..(cluster_id + 1) * N_FEAT];
        let mut best_idx = 0;
        let mut best_dist = f32::MAX;
        for (i, a) in assignments.iter().enumerate() {
            if *a == cluster_id as u32 {
                let feat = &features_flat[i * N_FEAT..(i + 1) * N_FEAT];
                let dist: f32 = feat
                    .iter()
                    .zip(centroid.iter())
                    .map(|(x, y)| (x - y) * (x - y))
                    .sum();
                if dist < best_dist {
                    best_dist = dist;
                    best_idx = i;
                }
            }
        }
        sample_indices.push(best_idx);
    }

    let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
    let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();
    let zs = Zensim::new(ZensimProfile::latest());
    let mut ctx = FilterContext::new();

    let mut all_paths: Vec<String> = Vec::new();
    let mut sum_orig = 0.0f64;
    let mut sum_rule = 0.0f64;
    let mut sum_cluster = 0.0f64;
    let mut count = 0usize;

    let num_samples: usize = std::env::var("ZEN_SAMPLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_SAMPLES.min(n_clusters));

    for (si, &idx) in sample_indices.iter().enumerate().take(num_samples) {
        let (orig_path, expert_path) = &pairs[idx];
        let cluster_id = assignments[idx] as usize;
        let name = orig_path.file_stem().unwrap().to_str().unwrap();

        println!(
            "\n[{}/{}] Cluster {cluster_id}: {}",
            si + 1,
            num_samples,
            name
        );

        let (orig_px, expert_px, w, h) = match load_pair(orig_path, expert_path, COMPARE_WIDTH) {
            Some(v) => v,
            None => {
                println!("  SKIP: failed to load");
                continue;
            }
        };

        // Get features for this image
        let feat = &features_flat[idx * N_FEAT..(idx + 1) * N_FEAT];
        let img_features = reconstruct_features(feat);

        // Rule-based params
        let rule_params = rule_based_tune(&img_features);
        let rule_px = apply_params(&orig_px, w, h, &rule_params, &m1, &m1_inv, &mut ctx);

        // Cluster model params
        let cluster_p = &params_flat[cluster_id * N_PARAMS..(cluster_id + 1) * N_PARAMS];
        let cluster_params = array_to_params(cluster_p);
        let cluster_px = apply_params(&orig_px, w, h, &cluster_params, &m1, &m1_inv, &mut ctx);

        // Zensim scores (all compared to expert_c)
        let score_orig = zensim_score(&orig_px, &expert_px, w, h, &zs);
        let score_rule = zensim_score(&rule_px, &expert_px, w, h, &zs);
        let score_cluster = zensim_score(&cluster_px, &expert_px, w, h, &zs);

        println!("  Original:  {score_orig:.1}");
        println!("  Rule-based: {score_rule:.1}");
        println!("  Cluster:    {score_cluster:.1}");

        // Save individual images
        let prefix = format!("{OUTPUT_DIR}/c{cluster_id:02}_{name}");
        let save = |pixels: &[u8], suffix: &str| {
            let path = format!("{prefix}_{suffix}.jpg");
            let rgb_pixels: Vec<Rgb<u8>> = pixels
                .chunks_exact(3)
                .map(|c| Rgb {
                    r: c[0],
                    g: c[1],
                    b: c[2],
                })
                .collect();
            let img = ImgVec::new(rgb_pixels, w as usize, h as usize);
            let encoded = EncodeRequest::new(ImageFormat::Jpeg)
                .with_quality(90.0)
                .encode_rgb8(img.as_ref())
                .expect("JPEG encode failed");
            fs::write(&path, encoded.data()).expect("failed to write JPEG");
            path
        };

        let p_orig = save(&orig_px, &format!("1orig_{score_orig:.0}"));
        let p_rule = save(&rule_px, &format!("2rule_{score_rule:.0}"));
        let p_cluster = save(&cluster_px, &format!("3cluster_{score_cluster:.0}"));
        let p_expert = save(&expert_px, "4expert");

        all_paths.push(p_orig);
        all_paths.push(p_rule);
        all_paths.push(p_cluster);
        all_paths.push(p_expert);

        sum_orig += score_orig;
        sum_rule += score_rule;
        sum_cluster += score_cluster;
        count += 1;
    }

    if count > 0 {
        let n = count as f64;
        println!("\n=== Summary ({count} images) ===");
        println!("  Original:   {:.1}", sum_orig / n);
        println!("  Rule-based: {:.1}", sum_rule / n);
        println!("  Cluster:    {:.1}", sum_cluster / n);
        println!(
            "  Improvement (cluster over original): {:+.1}",
            (sum_cluster - sum_orig) / n
        );
    }

    println!("\nSaved {} images to {OUTPUT_DIR}/", all_paths.len());
}
