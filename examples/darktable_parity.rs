//! Measure darktable parity: compare zenfilters pipeline output against
//! darktable's default display-referred processing.
//!
//! For each DNG test image, produces four comparisons:
//! - **Parity**: our pipeline (from linear DNG) vs darktable display output
//! - **Ceiling**: darktable display vs expert edit (best case)
//! - **Quality**: our pipeline (from JPEG) vs expert edit
//! - **Baseline**: untouched original vs expert (worst case)
//!
//! Usage: cargo run --release --features experimental --example darktable_parity
//!
//! Requires darktable-cli in PATH.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use image::imageops::FilterType;
use image::RgbImage;
use zenfilters::filters::*;
use zenfilters::{
    FilterContext, OklabPlanes, Pipeline, PipelineConfig, gather_oklab_to_srgb_u8,
    scatter_srgb_u8_to_oklab, scatter_to_oklab,
};
use zenpixels::ColorPrimaries;
use zenpixels_convert::gamut::GamutMatrix;
use zenpixels_convert::oklab;
use zenraw::darktable::{self, DtConfig};
use zensim::{RgbSlice, Zensim, ZensimProfile};

const DNG_DIR: &str = "/mnt/v/input/fivek/dng";
const ORIGINAL_DIR: &str = "/mnt/v/input/fivek/original";
const EXPERT_DIR: &str = "/mnt/v/input/fivek/expert_c";
const OUTPUT_DIR: &str = "/mnt/v/output/zenfilters/parity";
const MAX_DIM: u32 = 512;
const NUM_SAMPLES: usize = 32;
const N_FEAT: usize = 142;
const N_PARAMS: usize = 18;
const TRAINING_DIR: &str = "/mnt/v/output/zenfilters/training";

fn load_f32s(path: &Path) -> Vec<f32> {
    let bytes = fs::read(path).expect("failed to read file");
    bytemuck::cast_slice(&bytes).to_vec()
}

fn zensim_score(a: &[u8], b: &[u8], w: u32, h: u32, zs: &Zensim) -> f64 {
    let expected = w as usize * h as usize * 3;
    if a.len() != expected || b.len() != expected {
        eprintln!(
            "    zensim: buffer mismatch: a={} b={} expected={} ({}x{})",
            a.len(),
            b.len(),
            expected,
            w,
            h
        );
        return 0.0;
    }
    let a_rgb: &[[u8; 3]] = bytemuck::cast_slice(a);
    let b_rgb: &[[u8; 3]] = bytemuck::cast_slice(b);
    let sa = RgbSlice::new(a_rgb, w as usize, h as usize);
    let sb = RgbSlice::new(b_rgb, w as usize, h as usize);
    match zs.compute(&sa, &sb) {
        Ok(r) => r.score(),
        Err(e) => {
            eprintln!("    zensim error: {e} ({}x{})", w, h);
            0.0
        }
    }
}

fn array_to_params(a: &[f32]) -> TunedParams {
    TunedParams {
        exposure: a[0],
        contrast: a[1],
        highlights: a[2],
        shadows: a[3],
        saturation: a[4],
        vibrance: a[5],
        temperature: a[6],
        tint: a[7],
        black_point: a[8],
        white_point: a[9],
        sigmoid_contrast: a[10],
        sigmoid_skew: a[11],
        clarity: a[12],
        sharpen: a[13],
        highlight_recovery: a[14],
        shadow_lift: a[15],
        local_tonemap: a[16],
        gamut_expand: a[17],
    }
}

fn build_pipeline(params: &TunedParams) -> Pipeline {
    let mut pipeline = Pipeline::new(PipelineConfig::default()).unwrap();
    let mut fused = FusedAdjust::new();
    fused.exposure = params.exposure;
    fused.contrast = params.contrast;
    fused.highlights = params.highlights;
    fused.shadows = params.shadows;
    fused.saturation = params.saturation;
    fused.vibrance = params.vibrance;
    fused.temperature = params.temperature;
    fused.tint = params.tint;
    fused.black_point = params.black_point;
    fused.white_point = params.white_point;
    pipeline.push(Box::new(fused));

    if (params.sigmoid_contrast - 1.0).abs() > 0.01 || (params.sigmoid_skew - 0.5).abs() > 0.01 {
        let mut sig = Sigmoid::default();
        sig.contrast = params.sigmoid_contrast;
        sig.skew = params.sigmoid_skew;
        pipeline.push(Box::new(sig));
    }
    if params.highlight_recovery > 0.01 {
        let mut hr = HighlightRecovery::default();
        hr.strength = params.highlight_recovery;
        pipeline.push(Box::new(hr));
    }
    if params.shadow_lift > 0.01 {
        let mut sl = ShadowLift::default();
        sl.strength = params.shadow_lift;
        pipeline.push(Box::new(sl));
    }
    if params.local_tonemap > 0.01 {
        let mut ltm = LocalToneMap::default();
        ltm.compression = params.local_tonemap;
        pipeline.push(Box::new(ltm));
    }
    if params.clarity > 0.01 {
        let mut c = Clarity::default();
        c.amount = params.clarity;
        pipeline.push(Box::new(c));
    }
    if params.sharpen > 0.01 {
        let mut s = AdaptiveSharpen::default();
        s.amount = params.sharpen;
        pipeline.push(Box::new(s));
    }
    if params.gamut_expand > 0.01 {
        let mut ge = GamutExpand::default();
        ge.strength = params.gamut_expand;
        pipeline.push(Box::new(ge));
    }
    pipeline
}

/// Apply filter pipeline to sRGB u8 input, return sRGB u8 output.
fn apply_pipeline_srgb(
    src: &[u8],
    w: u32,
    h: u32,
    params: &TunedParams,
    m1: &GamutMatrix,
    m1_inv: &GamutMatrix,
    ctx: &mut FilterContext,
) -> Vec<u8> {
    let mut planes = OklabPlanes::new(w, h);
    scatter_srgb_u8_to_oklab(src, &mut planes, 3, m1);
    let pipeline = build_pipeline(params);
    pipeline.apply_planar(&mut planes, ctx);
    let mut output = vec![0u8; (w as usize) * (h as usize) * 3];
    gather_oklab_to_srgb_u8(&planes, &mut output, 3, m1_inv);
    output
}

/// Build a pipeline for linear (scene-referred) input.
/// Prepends a base tone mapping step (Sigmoid) to convert scene→display
/// before applying the artistic adjustments.
fn build_pipeline_linear(params: &TunedParams) -> Pipeline {
    let mut pipeline = Pipeline::new(PipelineConfig::default()).unwrap();

    // Base tone mapping: approximate darktable's base curve.
    // Linear sensor data needs an S-curve to look like a camera JPEG.
    // Contrast ~1.6 provides a moderate base curve with shadow lift.
    let mut base_sig = Sigmoid::default();
    base_sig.contrast = 1.6;
    base_sig.skew = 0.55; // slight shadow lift
    pipeline.push(Box::new(base_sig));

    // Now apply artistic adjustments (same as JPEG path)
    let mut fused = FusedAdjust::new();
    fused.exposure = params.exposure;
    fused.contrast = params.contrast;
    fused.highlights = params.highlights;
    fused.shadows = params.shadows;
    fused.saturation = params.saturation;
    fused.vibrance = params.vibrance;
    fused.temperature = params.temperature;
    fused.tint = params.tint;
    fused.black_point = params.black_point;
    fused.white_point = params.white_point;
    pipeline.push(Box::new(fused));

    // Additional artistic sigmoid on top of base (only if cluster requests it)
    if (params.sigmoid_contrast - 1.0).abs() > 0.01 || (params.sigmoid_skew - 0.5).abs() > 0.01 {
        let mut sig = Sigmoid::default();
        sig.contrast = params.sigmoid_contrast;
        sig.skew = params.sigmoid_skew;
        pipeline.push(Box::new(sig));
    }
    if params.highlight_recovery > 0.01 {
        let mut hr = HighlightRecovery::default();
        hr.strength = params.highlight_recovery;
        pipeline.push(Box::new(hr));
    }
    if params.shadow_lift > 0.01 {
        let mut sl = ShadowLift::default();
        sl.strength = params.shadow_lift;
        pipeline.push(Box::new(sl));
    }
    if params.local_tonemap > 0.01 {
        let mut ltm = LocalToneMap::default();
        ltm.compression = params.local_tonemap;
        pipeline.push(Box::new(ltm));
    }
    if params.clarity > 0.01 {
        let mut c = Clarity::default();
        c.amount = params.clarity;
        pipeline.push(Box::new(c));
    }
    if params.sharpen > 0.01 {
        let mut s = AdaptiveSharpen::default();
        s.amount = params.sharpen;
        pipeline.push(Box::new(s));
    }
    if params.gamut_expand > 0.01 {
        let mut ge = GamutExpand::default();
        ge.strength = params.gamut_expand;
        pipeline.push(Box::new(ge));
    }
    pipeline
}

/// Apply filter pipeline to linear f32 RGB input, return sRGB u8 output.
/// Includes base tone mapping for scene-to-display conversion.
fn apply_pipeline_linear(
    linear_f32: &[f32],
    w: u32,
    h: u32,
    params: &TunedParams,
    m1: &GamutMatrix,
    m1_inv: &GamutMatrix,
    ctx: &mut FilterContext,
) -> Vec<u8> {
    let mut planes = OklabPlanes::new(w, h);
    scatter_to_oklab(linear_f32, &mut planes, 3, m1, 1.0);
    let pipeline = build_pipeline_linear(params);
    pipeline.apply_planar(&mut planes, ctx);
    let mut output = vec![0u8; (w as usize) * (h as usize) * 3];
    gather_oklab_to_srgb_u8(&planes, &mut output, 3, m1_inv);
    output
}

/// Get darktable's display-referred sRGB output for a DNG file.
/// This uses darktable's default workflow (basecurve tone mapping).
fn darktable_display_output(dng_path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_dir = PathBuf::from(format!("/tmp/dt_parity_{}_{}", std::process::id(), id));
    fs::create_dir_all(&tmp_dir).ok()?;
    let out_path = tmp_dir.join("output.tif");

    let status = Command::new("darktable-cli")
        .arg(dng_path)
        .arg(&out_path)
        .arg("--icc-type")
        .arg("SRGB")
        .arg("--apply-custom-presets")
        .arg("false")
        .arg("--core")
        .arg("--library")
        .arg(":memory:")
        .arg("--configdir")
        .arg(tmp_dir.join("dtconf"))
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .status()
        .ok()?;

    if !status.success() {
        let _ = fs::remove_dir_all(&tmp_dir);
        return None;
    }

    let img = image::open(&out_path).ok()?;
    let _ = fs::remove_dir_all(&tmp_dir);
    let rgb = img.to_rgb8();
    let w = rgb.width();
    let h = rgb.height();
    Some((rgb.into_raw(), w, h))
}

/// Resize and crop two images to common dimensions <= MAX_DIM.
fn resize_pair(
    a: &[u8],
    aw: u32,
    ah: u32,
    b: &[u8],
    bw: u32,
    bh: u32,
) -> (Vec<u8>, Vec<u8>, u32, u32) {
    let img_a = image::DynamicImage::ImageRgb8(RgbImage::from_raw(aw, ah, a.to_vec()).unwrap());
    let img_b = image::DynamicImage::ImageRgb8(RgbImage::from_raw(bw, bh, b.to_vec()).unwrap());
    let ra = img_a.resize(MAX_DIM, MAX_DIM, FilterType::Triangle);
    let rb = img_b.resize(MAX_DIM, MAX_DIM, FilterType::Triangle);
    let w = ra.width().min(rb.width());
    let h = ra.height().min(rb.height());
    let ca = ra.crop_imm(0, 0, w, h).to_rgb8().into_raw();
    let cb = rb.crop_imm(0, 0, w, h).to_rgb8().into_raw();
    (ca, cb, w, h)
}


fn save_rgb(data: &[u8], w: u32, h: u32, path: &str) {
    if let Some(img) = RgbImage::from_raw(w, h, data.to_vec()) {
        let _ = img.save(path);
    }
}

struct ImageResult {
    name: String,
    parity: f64,       // our DNG pipeline (base+cluster) vs darktable display
    parity_base: f64,  // our DNG pipeline (base only, no cluster) vs darktable display
    ceiling: f64,      // darktable display vs expert
    quality: f64,      // our JPEG pipeline vs expert
    baseline: f64,     // untouched original vs expert
}

fn main() {
    fs::create_dir_all(OUTPUT_DIR).unwrap();

    if !darktable::is_available() {
        eprintln!("ERROR: darktable-cli not found in PATH");
        std::process::exit(1);
    }
    println!("darktable: {}", darktable::version().unwrap_or_default());

    // Load cluster model
    let centroids_flat = load_f32s(&PathBuf::from(TRAINING_DIR).join("centroids.bin"));
    let params_flat = load_f32s(&PathBuf::from(TRAINING_DIR).join("cluster_params.bin"));
    let n_clusters = centroids_flat.len() / N_FEAT;
    println!("Loaded {n_clusters} clusters");

    // Find images with DNG + JPEG + expert
    let mut triples: Vec<(PathBuf, PathBuf, PathBuf)> = Vec::new();
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

    for entry in &entries {
        let stem = entry
            .path()
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let dng_path = PathBuf::from(DNG_DIR).join(format!("{stem}.dng"));
        let expert_path = PathBuf::from(EXPERT_DIR).join(entry.file_name());
        if dng_path.exists() && expert_path.exists() {
            triples.push((entry.path(), dng_path, expert_path));
        }
    }
    println!("Found {} images with DNG + JPEG + Expert", triples.len());

    let step = triples.len() / NUM_SAMPLES;
    let samples: Vec<_> = (0..NUM_SAMPLES).map(|i| &triples[i * step]).collect();

    let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
    let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();
    let zs = Zensim::new(ZensimProfile::latest());
    let mut ctx = FilterContext::new();
    let dt_config = DtConfig::new();

    let mut results: Vec<ImageResult> = Vec::new();

    for (si, (orig_path, dng_path, expert_path)) in samples.iter().enumerate() {
        let stem = orig_path.file_stem().unwrap().to_str().unwrap();
        println!("\n[{}/{}] {}", si + 1, NUM_SAMPLES, stem);

        // Load expert
        let expert_img = match image::open(expert_path) {
            Ok(i) => i,
            Err(_) => {
                println!("  SKIP: can't load expert");
                continue;
            }
        };
        let expert_rgb = expert_img.to_rgb8();
        let (ew, eh) = (expert_rgb.width(), expert_rgb.height());
        let expert_raw = expert_rgb.into_raw();

        // Load original JPEG
        let orig_img = match image::open(orig_path) {
            Ok(i) => i,
            Err(_) => {
                println!("  SKIP: can't load original");
                continue;
            }
        };
        let orig_rgb = orig_img.to_rgb8();
        let (ow, oh) = (orig_rgb.width(), orig_rgb.height());
        let orig_raw = orig_rgb.into_raw();

        // --- Baseline: original vs expert (no processing) ---
        let (orig_r, expert_r, w, h) = resize_pair(&orig_raw, ow, oh, &expert_raw, ew, eh);
        let baseline = zensim_score(&orig_r, &expert_r, w, h, &zs);

        // --- JPEG path: extract features, find cluster, apply pipeline ---
        let mut feat_planes = OklabPlanes::new(w, h);
        scatter_srgb_u8_to_oklab(&orig_r, &mut feat_planes, 3, &m1);
        let features = ImageFeatures::extract(&feat_planes);

        let input = features.to_tensor();
        let mut best_cluster = 0;
        let mut best_dist = f32::MAX;
        for c in 0..n_clusters {
            let centroid = &centroids_flat[c * N_FEAT..(c + 1) * N_FEAT];
            let dist: f32 = input
                .iter()
                .zip(centroid.iter())
                .map(|(a, b)| (a - b) * (a - b))
                .sum();
            if dist < best_dist {
                best_dist = dist;
                best_cluster = c;
            }
        }

        let cluster_p = &params_flat[best_cluster * N_PARAMS..(best_cluster + 1) * N_PARAMS];
        let params = array_to_params(cluster_p);
        let jpeg_out = apply_pipeline_srgb(&orig_r, w, h, &params, &m1, &m1_inv, &mut ctx);
        let quality = zensim_score(&jpeg_out, &expert_r, w, h, &zs);

        // --- DNG path: darktable linear → our pipeline → compare vs darktable display ---
        let (parity, parity_base, ceiling) = match process_dng_parity(
            dng_path,
            &expert_raw,
            ew,
            eh,
            &params,
            &dt_config,
            &m1,
            &m1_inv,
            &mut ctx,
            &zs,
            &format!("{OUTPUT_DIR}/{stem}"),
        ) {
            Some(r) => r,
            None => {
                println!("  DNG failed");
                (-1.0, -1.0, -1.0)
            }
        };

        println!(
            "  C{best_cluster:02} | parity={parity:.1} base={parity_base:.1} ceiling={ceiling:.1} quality={quality:.1} baseline={baseline:.1}"
        );

        // Save comparison images
        let prefix = format!("{OUTPUT_DIR}/{stem}");
        save_rgb(&orig_r, w, h, &format!("{prefix}_1_orig.jpg"));
        save_rgb(&jpeg_out, w, h, &format!("{prefix}_2_ours.jpg"));
        save_rgb(&expert_r, w, h, &format!("{prefix}_3_expert.jpg"));

        results.push(ImageResult {
            name: stem.to_string(),
            parity,
            parity_base,
            ceiling,
            quality,
            baseline,
        });
    }

    // Summary
    println!("\n\n=== RESULTS ===");
    println!("parity  = our DNG pipeline (base+cluster) vs darktable display");
    println!("base    = our DNG pipeline (base sigmoid only) vs darktable display");
    println!("ceiling = darktable display vs expert");
    println!("quality = our JPEG pipeline vs expert");
    println!("baseline = untouched original vs expert\n");

    println!(
        "{:<35} {:>7} {:>7} {:>7} {:>7} {:>7}",
        "Image", "Parity", "Base", "Ceil", "Qual", "Base0"
    );
    println!("{}", "-".repeat(80));

    let (mut sp, mut spb, mut sc, mut sq, mut sb) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let mut np = 0;

    for r in &results {
        let fmt = |v: f64| -> String {
            if v < 0.0 {
                "FAIL".to_string()
            } else {
                format!("{v:.1}")
            }
        };
        println!(
            "{:<35} {:>7} {:>7} {:>7} {:>7.1} {:>7.1}",
            &r.name[..r.name.len().min(35)],
            fmt(r.parity),
            fmt(r.parity_base),
            fmt(r.ceiling),
            r.quality,
            r.baseline
        );
        if r.parity >= 0.0 {
            sp += r.parity;
            spb += r.parity_base;
            sc += r.ceiling;
            np += 1;
        }
        sq += r.quality;
        sb += r.baseline;
    }

    let n = results.len() as f64;
    println!("{}", "-".repeat(80));
    println!(
        "{:<35} {:>7.1} {:>7.1} {:>7.1} {:>7.1} {:>7.1}",
        "MEAN",
        if np > 0 { sp / np as f64 } else { 0.0 },
        if np > 0 { spb / np as f64 } else { 0.0 },
        if np > 0 { sc / np as f64 } else { 0.0 },
        sq / n,
        sb / n
    );

    // Write TSV
    let tsv_path = format!("{OUTPUT_DIR}/parity_results.tsv");
    let mut tsv = String::new();
    tsv.push_str("image\tparity\tparity_base\tceiling\tquality\tbaseline\n");
    for r in &results {
        tsv.push_str(&format!(
            "{}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\n",
            r.name, r.parity, r.parity_base, r.ceiling, r.quality, r.baseline
        ));
    }
    fs::write(&tsv_path, &tsv).unwrap();
    println!("\nResults saved to {tsv_path}");
}

/// Process DNG: get darktable display output + our pipeline output, compare.
/// Returns (parity_score, parity_base_score, ceiling_score).
#[allow(clippy::too_many_arguments)]
fn process_dng_parity(
    dng_path: &Path,
    expert_raw: &[u8],
    ew: u32,
    eh: u32,
    params: &TunedParams,
    dt_config: &DtConfig,
    m1: &GamutMatrix,
    m1_inv: &GamutMatrix,
    ctx: &mut FilterContext,
    zs: &Zensim,
    out_prefix: &str,
) -> Option<(f64, f64, f64)> {
    // 1. Get darktable display-referred output (default tone mapping)
    let (dt_display, dtw, dth) = darktable_display_output(dng_path)?;

    // 2. Get darktable linear output for our pipeline
    let output = darktable::decode_file(dng_path, dt_config).ok()?;
    let pixels = output.pixels;
    let dw = pixels.width();
    let dh = pixels.height();
    let raw_bytes = pixels.copy_to_contiguous_bytes();
    let linear_f32: &[f32] = bytemuck::cast_slice(&raw_bytes);

    // 3. Apply our pipeline with cluster params (base sigmoid + artistic adjustments)
    let our_srgb = apply_pipeline_linear(linear_f32, dw, dh, params, m1, m1_inv, ctx);

    // 4. Apply base-only pipeline (just base sigmoid, no cluster adjustments)
    let identity_params = TunedParams::default();
    let base_only_srgb =
        apply_pipeline_linear(linear_f32, dw, dh, &identity_params, m1, m1_inv, ctx);

    // 5. Resize all to common dimensions for comparison
    let (our_r, dt_r, w, h) = resize_pair(&our_srgb, dw, dh, &dt_display, dtw, dth);
    let parity = zensim_score(&our_r, &dt_r, w, h, zs);

    let (base_r, dt_r3, w3, h3) = resize_pair(&base_only_srgb, dw, dh, &dt_display, dtw, dth);
    let parity_base = zensim_score(&base_r, &dt_r3, w3, h3, zs);

    // Darktable display vs expert → ceiling
    let (dt_r2, expert_r, w2, h2) = resize_pair(&dt_display, dtw, dth, expert_raw, ew, eh);
    let ceiling = zensim_score(&dt_r2, &expert_r, w2, h2, zs);

    // Save DNG-specific comparison images
    save_rgb(&our_r, w, h, &format!("{out_prefix}_4_dng_ours.jpg"));
    save_rgb(&dt_r, w, h, &format!("{out_prefix}_5_dng_dt.jpg"));
    save_rgb(&base_r, w3, h3, &format!("{out_prefix}_6_dng_base.jpg"));

    Some((parity, parity_base, ceiling))
}
