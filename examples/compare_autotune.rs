//! Generate side-by-side comparison images for auto-tune models.
//!
//! Produces montages: Original | Rule-Based | Cluster Model | Expert C
//! with zensim scores annotated.
//!
//! Usage: cargo run --release --example compare_autotune

use std::fs;
use std::path::{Path, PathBuf};

use image::imageops::FilterType;
use image::{GenericImageView, RgbImage};
use zenfilters::filters::*;
use zenfilters::{
    FilterContext, OklabPlanes, Pipeline, PipelineConfig, gather_oklab_to_srgb_u8,
    scatter_srgb_u8_to_oklab,
};
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
const NUM_SAMPLES: usize = 16; // One per cluster

fn load_f32s(path: &Path) -> Vec<f32> {
    let bytes = fs::read(path).expect("failed to read file");
    bytemuck::cast_slice(&bytes).to_vec()
}

fn load_u32s(path: &Path) -> Vec<u32> {
    let bytes = fs::read(path).expect("failed to read file");
    bytemuck::cast_slice(&bytes).to_vec()
}

fn load_resized(path: &Path, max_dim: u32) -> Option<(Vec<u8>, u32, u32)> {
    let img = image::open(path).ok()?;
    let resized = img.resize(max_dim, max_dim, FilterType::Triangle);
    let (w, h) = resized.dimensions();
    Some((resized.to_rgb8().into_raw(), w, h))
}

fn load_pair(
    orig_path: &Path,
    expert_path: &Path,
    max_dim: u32,
) -> Option<(Vec<u8>, Vec<u8>, u32, u32)> {
    let orig_img = image::open(orig_path).ok()?;
    let expert_img = image::open(expert_path).ok()?;
    let orig_r = orig_img.resize(max_dim, max_dim, FilterType::Triangle);
    let expert_r = expert_img.resize(max_dim, max_dim, FilterType::Triangle);
    let (ow, oh) = orig_r.dimensions();
    let (ew, eh) = expert_r.dimensions();
    let w = ow.min(ew);
    let h = oh.min(eh);
    let orig_c = orig_r.crop_imm(0, 0, w, h).to_rgb8();
    let expert_c = expert_r.crop_imm(0, 0, w, h).to_rgb8();
    Some((orig_c.into_raw(), expert_c.into_raw(), w, h))
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

fn reconstruct_features(feat: &[f32]) -> ImageFeatures {
    ImageFeatures {
        l_histogram: {
            let mut h = [0.0f32; 64];
            h.copy_from_slice(&feat[..64]);
            h
        },
        a_histogram: {
            let mut h = [0.0f32; 32];
            h.copy_from_slice(&feat[64..96]);
            h
        },
        b_histogram: {
            let mut h = [0.0f32; 32];
            h.copy_from_slice(&feat[96..128]);
            h
        },
        l_percentiles: {
            let mut p = [0.0f32; 7];
            p.copy_from_slice(&feat[128..135]);
            p
        },
        channel_stats: {
            let mut s = [0.0f32; 6];
            s.copy_from_slice(&feat[135..141]);
            s
        },
        dynamic_range: feat[141],
    }
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

    for (si, &idx) in sample_indices.iter().enumerate().take(NUM_SAMPLES) {
        let (orig_path, expert_path) = &pairs[idx];
        let cluster_id = assignments[idx] as usize;
        let name = orig_path.file_stem().unwrap().to_str().unwrap();

        println!(
            "\n[{}/{}] Cluster {cluster_id}: {}",
            si + 1,
            NUM_SAMPLES,
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
            RgbImage::from_raw(w, h, pixels.to_vec())
                .unwrap()
                .save(&path)
                .unwrap();
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
    }

    println!("\n\nSaved {} images to {OUTPUT_DIR}/", all_paths.len());
    println!("Use montage to create comparison grid.");
}
