//! Compare zenfilters auto-tune output against darktable for DNG and JPEG sources.
//!
//! For each test image:
//! 1. DNG → darktable (linear output, no edits) → reference
//! 2. DNG → darktable → sRGB JPEG (darktable default processing)
//! 3. JPEG original → zenfilters auto-tune pipeline → our output
//! 4. Compare our output vs expert_c using zensim
//!
//! Usage: cargo run --release --features experimental --example darktable_parity
//!
//! Requires darktable-cli in PATH.

use std::fs;
use std::path::{Path, PathBuf};

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
            "    zensim: buffer size mismatch: a={} b={} expected={} ({}x{})",
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
fn apply_pipeline_u8(
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

fn main() {
    fs::create_dir_all(OUTPUT_DIR).unwrap();

    // Check darktable availability
    if !darktable::is_available() {
        eprintln!("ERROR: darktable-cli not found in PATH");
        std::process::exit(1);
    }
    println!("darktable: {}", darktable::version().unwrap_or_default());

    // Load cluster model data
    let centroids_flat = load_f32s(&PathBuf::from(TRAINING_DIR).join("centroids.bin"));
    let params_flat = load_f32s(&PathBuf::from(TRAINING_DIR).join("cluster_params.bin"));
    let n_clusters = centroids_flat.len() / N_FEAT;
    println!("Loaded {n_clusters} cluster centroids + params");

    // Discover test images (need DNG + original JPEG + expert JPEG)
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

    // Sample evenly
    let step = triples.len() / NUM_SAMPLES;
    let samples: Vec<_> = (0..NUM_SAMPLES).map(|i| &triples[i * step]).collect();

    let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
    let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();
    let zs = Zensim::new(ZensimProfile::latest());
    let mut ctx = FilterContext::new();
    let dt_config = DtConfig::new();

    let mut results: Vec<(String, f64, f64, f64, f64)> = Vec::new();

    for (si, (orig_path, dng_path, expert_path)) in samples.iter().enumerate() {
        let stem = orig_path.file_stem().unwrap().to_str().unwrap();
        println!("\n[{}/{}] {}", si + 1, NUM_SAMPLES, stem);

        // --- Path A: JPEG → zenfilters auto-tune → compare vs expert ---
        let orig_img = match image::open(orig_path) {
            Ok(i) => i,
            Err(_) => {
                println!("  SKIP: can't load original");
                continue;
            }
        };
        let orig_resized = orig_img.resize(MAX_DIM, MAX_DIM, FilterType::Triangle);
        let expert_img2 = image::open(expert_path).unwrap();
        let expert_resized = expert_img2.resize(MAX_DIM, MAX_DIM, FilterType::Triangle);

        // Crop to common dimensions
        let w = orig_resized.width().min(expert_resized.width());
        let h = orig_resized.height().min(expert_resized.height());
        let crop_orig = orig_resized.crop_imm(0, 0, w, h).to_rgb8().into_raw();
        let crop_expert = expert_resized.crop_imm(0, 0, w, h).to_rgb8().into_raw();

        // Extract features from original
        let mut feat_planes = OklabPlanes::new(w, h);
        scatter_srgb_u8_to_oklab(&crop_orig, &mut feat_planes, 3, &m1);
        let features = ImageFeatures::extract(&feat_planes);

        // Find nearest cluster
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

        // Apply cluster params
        let cluster_p = &params_flat[best_cluster * N_PARAMS..(best_cluster + 1) * N_PARAMS];
        let cluster_params = array_to_params(cluster_p);
        let jpeg_cluster =
            apply_pipeline_u8(&crop_orig, w, h, &cluster_params, &m1, &m1_inv, &mut ctx);

        // Rule-based for comparison
        let rule_params = rule_based_tune(&features);
        let jpeg_rule = apply_pipeline_u8(&crop_orig, w, h, &rule_params, &m1, &m1_inv, &mut ctx);

        let score_jpeg_cluster = zensim_score(&jpeg_cluster, &crop_expert, w, h, &zs);
        let score_jpeg_rule = zensim_score(&jpeg_rule, &crop_expert, w, h, &zs);

        // --- Path B: DNG → darktable (linear) → zenfilters → compare vs expert ---
        let (score_dng_cluster, score_dng_rule) = match process_dng(
            dng_path,
            expert_path,
            &cluster_params,
            &rule_params,
            &dt_config,
            &m1,
            &m1_inv,
            &mut ctx,
            &zs,
        ) {
            Some((sc, sr)) => (sc, sr),
            None => {
                println!("  DNG processing failed, using JPEG scores only");
                (score_jpeg_cluster, score_jpeg_rule)
            }
        };

        println!(
            "  Cluster {best_cluster} | JPEG: rule={score_jpeg_rule:.1} cluster={score_jpeg_cluster:.1} | DNG: rule={score_dng_rule:.1} cluster={score_dng_cluster:.1}"
        );

        // Save comparison images
        let prefix = format!("{OUTPUT_DIR}/{stem}");
        save_rgb(&crop_orig, w, h, &format!("{prefix}_1_orig.jpg"));
        save_rgb(&jpeg_rule, w, h, &format!("{prefix}_2_rule.jpg"));
        save_rgb(&jpeg_cluster, w, h, &format!("{prefix}_3_cluster.jpg"));
        save_rgb(&crop_expert, w, h, &format!("{prefix}_4_expert.jpg"));

        results.push((
            stem.to_string(),
            score_jpeg_rule,
            score_jpeg_cluster,
            score_dng_rule,
            score_dng_cluster,
        ));
    }

    // Summary
    println!("\n\n=== RESULTS ===\n");
    println!(
        "{:<40} {:>10} {:>10} {:>10} {:>10}",
        "Image", "JPEG Rule", "JPEG Clust", "DNG Rule", "DNG Clust"
    );
    println!("{}", "-".repeat(90));

    let mut sum_jr = 0.0;
    let mut sum_jc = 0.0;
    let mut sum_dr = 0.0;
    let mut sum_dc = 0.0;

    for (name, jr, jc, dr, dc) in &results {
        println!(
            "{:<40} {:>10.1} {:>10.1} {:>10.1} {:>10.1}",
            name, jr, jc, dr, dc
        );
        sum_jr += jr;
        sum_jc += jc;
        sum_dr += dr;
        sum_dc += dc;
    }

    let n = results.len() as f64;
    println!("{}", "-".repeat(90));
    println!(
        "{:<40} {:>10.1} {:>10.1} {:>10.1} {:>10.1}",
        "MEAN",
        sum_jr / n,
        sum_jc / n,
        sum_dr / n,
        sum_dc / n
    );

    // Write TSV results
    let tsv_path = format!("{OUTPUT_DIR}/parity_results.tsv");
    let mut tsv = String::new();
    tsv.push_str("image\tjpeg_rule\tjpeg_cluster\tdng_rule\tdng_cluster\n");
    for (name, jr, jc, dr, dc) in &results {
        tsv.push_str(&format!("{name}\t{jr:.2}\t{jc:.2}\t{dr:.2}\t{dc:.2}\n"));
    }
    fs::write(&tsv_path, &tsv).unwrap();
    println!("\nResults saved to {tsv_path}");
}

/// Process a DNG through darktable (linear output), apply zenfilters, compare vs expert.
#[allow(clippy::too_many_arguments)]
fn process_dng(
    dng_path: &Path,
    expert_path: &Path,
    cluster_params: &TunedParams,
    rule_params: &TunedParams,
    dt_config: &DtConfig,
    m1: &GamutMatrix,
    m1_inv: &GamutMatrix,
    ctx: &mut FilterContext,
    zs: &Zensim,
) -> Option<(f64, f64)> {
    // Decode DNG through darktable to get linear f32
    let output = match darktable::decode_file(dng_path, dt_config) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("    DNG decode error: {e}");
            return None;
        }
    };
    let pixels = output.pixels;
    let dw = pixels.width();
    let dh = pixels.height();

    // Get raw bytes and interpret as f32
    let raw_bytes = pixels.copy_to_contiguous_bytes();
    let linear_f32: &[f32] = bytemuck::cast_slice(&raw_bytes);

    // Convert linear f32 to sRGB u8 for resize
    let mut srgb_full = vec![0u8; dw as usize * dh as usize * 3];
    {
        let mut planes = OklabPlanes::new(dw, dh);
        scatter_to_oklab(linear_f32, &mut planes, 3, m1, 1.0);
        gather_oklab_to_srgb_u8(&planes, &mut srgb_full, 3, m1_inv);
    }

    // Load and resize both DNG output and expert to same dimensions
    let dng_img = RgbImage::from_raw(dw, dh, srgb_full)?;
    let dng_resized =
        image::DynamicImage::ImageRgb8(dng_img).resize(MAX_DIM, MAX_DIM, FilterType::Triangle);
    let expert_img = image::open(expert_path).ok()?;
    let expert_resized = expert_img.resize(MAX_DIM, MAX_DIM, FilterType::Triangle);

    // Crop to common dimensions
    let w = dng_resized.width().min(expert_resized.width());
    let h = dng_resized.height().min(expert_resized.height());
    let dng_cropped = dng_resized.crop_imm(0, 0, w, h).to_rgb8().into_raw();
    let expert_cropped = expert_resized.crop_imm(0, 0, w, h).to_rgb8().into_raw();

    // Apply cluster params to DNG-sourced pixels
    let dng_cluster = apply_pipeline_u8(&dng_cropped, w, h, cluster_params, m1, m1_inv, ctx);
    let dng_rule = apply_pipeline_u8(&dng_cropped, w, h, rule_params, m1, m1_inv, ctx);

    let score_cluster = zensim_score(&dng_cluster, &expert_cropped, w, h, zs);
    let score_rule = zensim_score(&dng_rule, &expert_cropped, w, h, zs);

    Some((score_cluster, score_rule))
}

fn save_rgb(data: &[u8], w: u32, h: u32, path: &str) {
    if let Some(img) = RgbImage::from_raw(w, h, data.to_vec()) {
        let _ = img.save(path);
    }
}
