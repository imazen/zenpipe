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

use image::RgbImage;
use image::imageops::FilterType;
use zenfilters::filters::*;
use zenfilters::regional::{RegionalComparison, RegionalFeatures};
use zenfilters::{
    FilterContext, OklabPlanes, Pipeline, gather_oklab_to_srgb_u8, scatter_srgb_u8_to_oklab,
    scatter_to_oklab,
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
const DEFAULT_SAMPLES: usize = 32;
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
    let arr: &[f32; 18] = a.try_into().expect("expected 18-float param slice");
    TunedParams::from_array(arr)
}

fn build_pipeline(params: &TunedParams) -> Pipeline {
    params.build_pipeline()
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
    let base_contrast: f32 = std::env::var("ZEN_BASE_CONTRAST")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.4);
    let base_skew: f32 = std::env::var("ZEN_BASE_SKEW")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.58);
    params.build_pipeline_linear(base_contrast, base_skew)
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

/// Compare two sRGB u8 images regionally via Oklab feature extraction.
fn regional_compare_srgb(
    a: &[u8],
    b: &[u8],
    w: u32,
    h: u32,
    m1: &GamutMatrix,
) -> RegionalComparison {
    let mut planes_a = OklabPlanes::new(w, h);
    scatter_srgb_u8_to_oklab(a, &mut planes_a, 3, m1);
    let mut planes_b = OklabPlanes::new(w, h);
    scatter_srgb_u8_to_oklab(b, &mut planes_b, 3, m1);
    let fa = RegionalFeatures::extract(&planes_a);
    let fb = RegionalFeatures::extract(&planes_b);
    RegionalComparison::compare(&fa, &fb)
}

/// Print compact regional breakdown highlighting worst-offending zones.
fn print_regional(label: &str, r: &RegionalComparison) {
    let labels = RegionalComparison::zone_labels();
    let (li, &lv) = r
        .lum_zone_dist
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .unwrap();
    let (hi, &hv) = r
        .hue_sector_dist
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .unwrap();
    let (ti, &tv) = r
        .texture_zone_dist
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .unwrap();
    println!(
        "  {label}: L:{}={:.3} H:{}={:.3} T:{}={:.3} agg={:.3}",
        labels.luminance[li], lv, labels.hue[hi], hv, labels.texture[ti], tv, r.aggregate,
    );
}

/// Print mean per-zone distances for a set of regional comparisons.
fn print_zone_summary<F, const N: usize>(
    prefix: &str,
    labels: &[&str],
    regs: &[&RegionalComparison],
    extract: F,
) where
    F: Fn(&RegionalComparison) -> &[f32; N],
{
    let n = regs.len() as f32;
    let mut means = vec![0.0f32; N];
    for r in regs {
        let dists = extract(r);
        for (i, &d) in dists.iter().enumerate() {
            means[i] += d;
        }
    }
    for m in &mut means {
        *m /= n;
    }
    let parts: Vec<String> = labels
        .iter()
        .zip(means.iter())
        .map(|(lbl, &m)| format!("{lbl}={m:.3}"))
        .collect();
    println!("{prefix}: {}", parts.join("  "));
}

struct ImageResult {
    name: String,
    parity_base: f64, // our DNG pipeline (base only, no cluster) vs darktable display
    parity_rule_dng: f64, // our DNG pipeline (rule-based) vs darktable display
    ceiling: f64,     // darktable display vs expert
    quality: f64,     // our JPEG cluster pipeline (k=1) vs expert
    quality_k3: f64,  // our JPEG cluster pipeline (k=3 blend) vs expert
    quality_rule: f64, // our JPEG rule-based pipeline vs expert
    baseline: f64,    // untouched original vs expert
    regional_dng: Option<RegionalComparison>,  // DNG base vs darktable
    regional_jpeg: Option<RegionalComparison>, // best JPEG pipeline vs expert
}

fn main() {
    fs::create_dir_all(OUTPUT_DIR).unwrap();

    if !darktable::is_available() {
        eprintln!("ERROR: darktable-cli not found in PATH");
        std::process::exit(1);
    }
    println!("darktable: {}", darktable::version().unwrap_or_default());

    // Load cluster model (optional — run rule-based only if not available)
    let centroids_path = PathBuf::from(TRAINING_DIR).join("centroids.bin");
    let params_path = PathBuf::from(TRAINING_DIR).join("cluster_params.bin");
    let (centroids_flat, params_flat, n_clusters) = if centroids_path.exists()
        && params_path.exists()
    {
        let c = load_f32s(&centroids_path);
        let p = load_f32s(&params_path);
        let nc = c.len() / N_FEAT;
        let np = p.len() / N_PARAMS;
        if nc == np && nc > 0 {
            println!("Loaded {nc} clusters");
            (c, p, nc)
        } else {
            eprintln!(
                "WARNING: cluster model size mismatch ({nc} centroids, {np} params) — using rule-based only"
            );
            (vec![], vec![], 0)
        }
    } else {
        println!("No cluster model found — using rule-based only");
        (vec![], vec![], 0)
    };

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

    let num_samples: usize = std::env::var("ZEN_SAMPLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_SAMPLES);
    let step = triples.len() / num_samples;
    let samples: Vec<_> = (0..num_samples).map(|i| &triples[i * step]).collect();

    let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
    let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();
    let zs = Zensim::new(ZensimProfile::latest());
    let mut ctx = FilterContext::new();
    let dt_config = DtConfig::new();

    let mut results: Vec<ImageResult> = Vec::new();

    for (si, (orig_path, dng_path, expert_path)) in samples.iter().enumerate() {
        let stem = orig_path.file_stem().unwrap().to_str().unwrap();
        println!("\n[{}/{}] {}", si + 1, num_samples, stem);

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

        // --- JPEG path: extract features, apply pipeline ---
        let mut feat_planes = OklabPlanes::new(w, h);
        scatter_srgb_u8_to_oklab(&orig_r, &mut feat_planes, 3, &m1);
        let features = ImageFeatures::extract(&feat_planes);

        // Rule-based pipeline
        let rule_params = rule_based_tune(&features);
        let jpeg_rule = apply_pipeline_srgb(&orig_r, w, h, &rule_params, &m1, &m1_inv, &mut ctx);
        let quality_rule = zensim_score(&jpeg_rule, &expert_r, w, h, &zs);

        // Cluster model pipeline (if available)
        // Returns (k1_score, k3_score, best_cluster, k3_output_for_regional)
        let (quality, quality_k3, best_cluster, jpeg_best) = if n_clusters > 0 {
            let input = features.to_tensor();
            // Find nearest centroid
            let mut dists: Vec<(usize, f32)> = (0..n_clusters)
                .map(|c| {
                    let centroid = &centroids_flat[c * N_FEAT..(c + 1) * N_FEAT];
                    let dist: f32 = input
                        .iter()
                        .zip(centroid.iter())
                        .map(|(a, b)| (a - b) * (a - b))
                        .sum();
                    (c, dist)
                })
                .collect();
            dists.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            let bc = dists[0].0;

            // k=1: nearest neighbor
            let cluster_p = &params_flat[bc * N_PARAMS..(bc + 1) * N_PARAMS];
            let params = array_to_params(cluster_p);
            let jpeg_out = apply_pipeline_srgb(&orig_r, w, h, &params, &m1, &m1_inv, &mut ctx);
            let q1 = zensim_score(&jpeg_out, &expert_r, w, h, &zs);

            // k=3: weighted blend
            let k = 3.min(n_clusters);
            let mut blended = [0.0f32; N_PARAMS];
            let mut total_w = 0.0f32;
            for &(idx, dist) in &dists[..k] {
                let w_val = 1.0 / (dist.sqrt() + 1e-6);
                total_w += w_val;
                let p = &params_flat[idx * N_PARAMS..(idx + 1) * N_PARAMS];
                for (j, val) in p.iter().enumerate() {
                    blended[j] += val * w_val;
                }
            }
            for v in &mut blended {
                *v /= total_w;
            }
            let blend_params = array_to_params(&blended);
            let jpeg_blend =
                apply_pipeline_srgb(&orig_r, w, h, &blend_params, &m1, &m1_inv, &mut ctx);
            let q3 = zensim_score(&jpeg_blend, &expert_r, w, h, &zs);

            (q1, q3, bc, Some(jpeg_blend))
        } else {
            (-1.0, -1.0, 0, None)
        };

        // --- DNG path: darktable linear → our pipeline → compare vs darktable display ---
        let (parity_base, parity_rule_dng, ceiling, regional_dng) = match process_dng_parity(
            dng_path,
            &expert_raw,
            ew,
            eh,
            &dt_config,
            &m1,
            &m1_inv,
            &mut ctx,
            &zs,
            &format!("{OUTPUT_DIR}/{stem}"),
        ) {
            Some((pb, pr, c, reg)) => (pb, pr, c, Some(reg)),
            None => {
                println!("  DNG failed");
                (-1.0, -1.0, -1.0, None)
            }
        };

        // --- Regional analysis: best JPEG pipeline vs expert ---
        let regional_jpeg = {
            let best_jpeg = jpeg_best.as_deref().unwrap_or(&jpeg_rule);
            Some(regional_compare_srgb(best_jpeg, &expert_r, w, h, &m1))
        };

        println!(
            "  C{best_cluster:02} | base={parity_base:.1} rDNG={parity_rule_dng:.1} ceil={ceiling:.1} k1={quality:.1} k3={quality_k3:.1} rule={quality_rule:.1} base0={baseline:.1}"
        );
        if let Some(ref reg) = regional_dng {
            print_regional("DNG→dt", reg);
        }
        if let Some(ref reg) = regional_jpeg {
            print_regional("JPEG→ex", reg);
        }

        // Save comparison images
        let prefix = format!("{OUTPUT_DIR}/{stem}");
        save_rgb(&orig_r, w, h, &format!("{prefix}_1_orig.jpg"));
        save_rgb(&jpeg_rule, w, h, &format!("{prefix}_2_rule.jpg"));
        save_rgb(&expert_r, w, h, &format!("{prefix}_3_expert.jpg"));

        results.push(ImageResult {
            name: stem.to_string(),
            parity_base,
            parity_rule_dng,
            ceiling,
            quality,
            quality_k3,
            quality_rule,
            baseline,
            regional_dng,
            regional_jpeg,
        });
    }

    // Summary
    println!("\n\n=== RESULTS ===");
    println!("base    = our DNG pipeline (base sigmoid only) vs darktable display");
    println!("rDNG    = our DNG pipeline (rule-based) vs darktable display");
    println!("ceil    = darktable display vs expert");
    println!("k1      = our JPEG cluster pipeline (nearest) vs expert");
    println!("k3      = our JPEG cluster pipeline (3-blend) vs expert");
    println!("rule    = our JPEG rule-based pipeline vs expert");
    println!("base0   = untouched original vs expert\n");

    println!(
        "{:<35} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}",
        "Image", "Base", "rDNG", "Ceil", "K1", "K3", "Rule", "Base0"
    );
    println!("{}", "-".repeat(98));

    let (mut spb, mut spr, mut sc, mut sq, mut sq3, mut sqr, mut sb) =
        (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    let mut np = 0;

    for r in &results {
        let fmt = |v: f64| -> String {
            if v < 0.0 {
                "---".to_string()
            } else {
                format!("{v:.1}")
            }
        };
        println!(
            "{:<35} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7.1} {:>7.1}",
            &r.name[..r.name.len().min(35)],
            fmt(r.parity_base),
            fmt(r.parity_rule_dng),
            fmt(r.ceiling),
            fmt(r.quality),
            fmt(r.quality_k3),
            r.quality_rule,
            r.baseline
        );
        if r.parity_base >= 0.0 {
            spb += r.parity_base;
            spr += r.parity_rule_dng;
            sc += r.ceiling;
            np += 1;
        }
        sq += r.quality;
        sq3 += r.quality_k3;
        sqr += r.quality_rule;
        sb += r.baseline;
    }

    let n = results.len() as f64;
    println!("{}", "-".repeat(98));
    let mean_k1 = sq / n;
    let mean_k3 = sq3 / n;
    let mean_rule = sqr / n;
    let mean_base0 = sb / n;
    println!(
        "{:<35} {:>7.1} {:>7.1} {:>7.1} {:>7.1} {:>7.1} {:>7.1} {:>7.1}",
        "MEAN",
        if np > 0 { spb / np as f64 } else { 0.0 },
        if np > 0 { spr / np as f64 } else { 0.0 },
        if np > 0 { sc / np as f64 } else { 0.0 },
        mean_k1,
        mean_k3,
        mean_rule,
        mean_base0
    );
    println!(
        "{:<35} {:>7} {:>7} {:>7} {:>+7.1} {:>+7.1} {:>+7.1} {:>7}",
        "DELTA vs base0",
        "",
        "",
        "",
        mean_k1 - mean_base0,
        mean_k3 - mean_base0,
        mean_rule - mean_base0,
        ""
    );

    // Regional summary
    let labels = RegionalComparison::zone_labels();
    {
        let dng_regs: Vec<&RegionalComparison> =
            results.iter().filter_map(|r| r.regional_dng.as_ref()).collect();
        let jpeg_regs: Vec<&RegionalComparison> =
            results.iter().filter_map(|r| r.regional_jpeg.as_ref()).collect();

        if !dng_regs.is_empty() {
            println!("\n=== REGIONAL: DNG base vs darktable ({} images) ===", dng_regs.len());
            print_zone_summary("  Luminance", labels.luminance, &dng_regs, |r| &r.lum_zone_dist);
            print_zone_summary("  Hue      ", labels.hue, &dng_regs, |r| &r.hue_sector_dist);
            print_zone_summary("  Chroma   ", labels.chroma, &dng_regs, |r| &r.chroma_zone_dist);
            print_zone_summary("  Texture  ", labels.texture, &dng_regs, |r| &r.texture_zone_dist);
            let mean_agg: f32 = dng_regs.iter().map(|r| r.aggregate).sum::<f32>() / dng_regs.len() as f32;
            println!("  Aggregate: {mean_agg:.4}");
        }

        if !jpeg_regs.is_empty() {
            println!("\n=== REGIONAL: JPEG pipeline vs expert ({} images) ===", jpeg_regs.len());
            print_zone_summary("  Luminance", labels.luminance, &jpeg_regs, |r| &r.lum_zone_dist);
            print_zone_summary("  Hue      ", labels.hue, &jpeg_regs, |r| &r.hue_sector_dist);
            print_zone_summary("  Chroma   ", labels.chroma, &jpeg_regs, |r| &r.chroma_zone_dist);
            print_zone_summary("  Texture  ", labels.texture, &jpeg_regs, |r| &r.texture_zone_dist);
            let mean_agg: f32 = jpeg_regs.iter().map(|r| r.aggregate).sum::<f32>() / jpeg_regs.len() as f32;
            println!("  Aggregate: {mean_agg:.4}");
        }
    }

    // Write TSV
    let tsv_path = format!("{OUTPUT_DIR}/parity_results.tsv");
    let mut tsv = String::new();
    tsv.push_str(
        "image\tparity_base\tparity_rule_dng\tceiling\tquality_k1\tquality_k3\tquality_rule\tbaseline\tregional_dng\tregional_jpeg\n",
    );
    for r in &results {
        let reg_dng = r.regional_dng.as_ref().map_or(-1.0, |r| r.aggregate);
        let reg_jpeg = r.regional_jpeg.as_ref().map_or(-1.0, |r| r.aggregate);
        tsv.push_str(&format!(
            "{}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.4}\t{:.4}\n",
            r.name,
            r.parity_base,
            r.parity_rule_dng,
            r.ceiling,
            r.quality,
            r.quality_k3,
            r.quality_rule,
            r.baseline,
            reg_dng,
            reg_jpeg,
        ));
    }
    fs::write(&tsv_path, &tsv).unwrap();
    println!("\nResults saved to {tsv_path}");
}

/// Process DNG: get darktable display output + our base pipeline output, compare.
/// Returns (parity_base_score, parity_rule_score, ceiling_score, regional_comparison).
#[allow(clippy::too_many_arguments)]
fn process_dng_parity(
    dng_path: &Path,
    expert_raw: &[u8],
    ew: u32,
    eh: u32,
    dt_config: &DtConfig,
    m1: &GamutMatrix,
    m1_inv: &GamutMatrix,
    ctx: &mut FilterContext,
    zs: &Zensim,
    out_prefix: &str,
) -> Option<(f64, f64, f64, RegionalComparison)> {
    // 1. Get darktable display-referred output (default tone mapping)
    let (dt_display, dtw, dth) = darktable_display_output(dng_path)?;

    // 2. Get darktable linear output for our pipeline
    let output = darktable::decode_file(dng_path, dt_config).ok()?;
    let pixels = output.pixels;
    let dw = pixels.width();
    let dh = pixels.height();
    let raw_bytes = pixels.copy_to_contiguous_bytes();
    let linear_f32: &[f32] = bytemuck::cast_slice(&raw_bytes);

    // 3. Apply base-only pipeline (just base sigmoid, no artistic adjustments)
    let identity_params = TunedParams::default();
    let base_only_srgb =
        apply_pipeline_linear(linear_f32, dw, dh, &identity_params, m1, m1_inv, ctx);

    // 4. Resize and compare base-only vs darktable
    let (base_r, dt_r, w, h) = resize_pair(&base_only_srgb, dw, dh, &dt_display, dtw, dth);
    let parity_base = zensim_score(&base_r, &dt_r, w, h, zs);

    // 5. Apply rule-based adjustments on top of base sigmoid
    // Extract features from the base-sigmoid output (now display-referred)
    let mut feat_planes = OklabPlanes::new(w, h);
    scatter_srgb_u8_to_oklab(&base_r, &mut feat_planes, 3, m1);
    let features = ImageFeatures::extract(&feat_planes);
    let rule_params = rule_based_tune(&features);
    let rule_srgb = apply_pipeline_linear(linear_f32, dw, dh, &rule_params, m1, m1_inv, ctx);
    let (rule_r, dt_r3, w3, h3) = resize_pair(&rule_srgb, dw, dh, &dt_display, dtw, dth);
    let parity_rule = zensim_score(&rule_r, &dt_r3, w3, h3, zs);

    // Darktable display vs expert → ceiling
    let (dt_r2, expert_r, w2, h2) = resize_pair(&dt_display, dtw, dth, expert_raw, ew, eh);
    let ceiling = zensim_score(&dt_r2, &expert_r, w2, h2, zs);

    // Regional comparison: DNG base vs darktable display
    let regional = regional_compare_srgb(&base_r, &dt_r, w, h, m1);

    // Save DNG-specific comparison images
    save_rgb(&base_r, w, h, &format!("{out_prefix}_4_dng_base.jpg"));
    save_rgb(&rule_r, w3, h3, &format!("{out_prefix}_5_dng_rule.jpg"));
    save_rgb(&dt_r, w, h, &format!("{out_prefix}_6_dng_dt.jpg"));

    Some((parity_base, parity_rule, ceiling, regional))
}
