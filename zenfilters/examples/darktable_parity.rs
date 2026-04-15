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

use imgref::ImgVec;
use rgb::Rgb;
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat};
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
use zenresize::{Filter, PixelDescriptor, ResizeConfig, Resizer};
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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

/// Apply basecurve tone mapping (camera-matched) in linear RGB space, then convert to sRGB u8.
///
/// Basecurve is applied per-channel in linear RGB before Oklab conversion,
/// matching how darktable applies it (in linear space, not perceptual).
#[allow(dead_code)]
fn apply_pipeline_basecurve(
    linear_f32: &[f32],
    w: u32,
    h: u32,
    maker: &str,
    model: &str,
    _m1: &GamutMatrix,
    _m1_inv: &GamutMatrix,
) -> Vec<u8> {
    let bc = BasecurveToneMap::from_camera(maker, model, 0.0);
    // Apply basecurve in linear RGB space (per-channel)
    let mut rgb = linear_f32.to_vec();
    bc.apply_linear_rgb(&mut rgb);
    // Now convert display-referred linear RGB to sRGB u8 (apply sRGB gamma)
    let n = (w as usize) * (h as usize);
    let mut output = vec![0u8; n * 3];
    for i in 0..n * 3 {
        let v = rgb[i].clamp(0.0, 1.0);
        // sRGB gamma: linear → sRGB
        let srgb = if v <= 0.003_130_8 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        output[i] = (srgb * 255.0 + 0.5) as u8;
    }
    output
}

/// Apply darktable-compatible sigmoid tone mapping in linear RGB space.
///
/// Uses the exact generalized log-logistic sigmoid from darktable's sigmoid module
/// with per-channel processing and hue preservation.
///
/// `exposure_mult`: Linear multiplier to approximate darktable's color calibration
/// and input profile normalization (~1.85x matches darktable's scene-referred pipeline for DSLRs).
fn apply_dt_sigmoid_pipeline(linear_f32: &[f32], w: u32, h: u32, exposure_mult: f32) -> Vec<u8> {
    use zenfilters::filters::dt_sigmoid;
    let params = dt_sigmoid::default_params();
    let mut rgb = linear_f32.to_vec();
    // Apply exposure correction to approximate darktable's pre-sigmoid processing
    if (exposure_mult - 1.0).abs() > 1e-6 {
        for v in rgb.iter_mut() {
            *v *= exposure_mult;
        }
    }
    dt_sigmoid::apply_dt_sigmoid(&mut rgb, &params);
    linear_to_srgb_u8(&rgb, w, h)
}

/// Apply dt_sigmoid with per-channel exposure multipliers [R, G, B].
///
/// Each channel gets its own multiplier to capture color calibration differences.
fn apply_dt_sigmoid_rgb(linear_f32: &[f32], w: u32, h: u32, rgb_mult: [f32; 3]) -> Vec<u8> {
    use zenfilters::filters::dt_sigmoid;
    let params = dt_sigmoid::default_params();
    let mut rgb = linear_f32.to_vec();
    let n = rgb.len() / 3;
    for i in 0..n {
        let base = i * 3;
        rgb[base] *= rgb_mult[0];
        rgb[base + 1] *= rgb_mult[1];
        rgb[base + 2] *= rgb_mult[2];
    }
    dt_sigmoid::apply_dt_sigmoid(&mut rgb, &params);
    linear_to_srgb_u8(&rgb, w, h)
}

/// Convert linear f32 RGB [0,1] to sRGB u8.
fn linear_to_srgb_u8(rgb: &[f32], w: u32, h: u32) -> Vec<u8> {
    let n = (w as usize) * (h as usize);
    let mut output = vec![0u8; n * 3];
    for i in 0..n * 3 {
        let v = rgb[i].clamp(0.0, 1.0);
        let srgb = if v <= 0.003_130_8 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        output[i] = (srgb * 255.0 + 0.5) as u8;
    }
    output
}

/// Golden section search for scalar optimization.
/// Maximizes `f` over `[lo, hi]`. Returns (best_x, best_score).
fn golden_search(f: impl Fn(f32) -> f64, lo: f32, hi: f32) -> (f32, f64) {
    let phi = (5.0f32.sqrt() - 1.0) / 2.0;
    let mut a = lo;
    let mut b = hi;
    let mut c = b - phi * (b - a);
    let mut d = a + phi * (b - a);
    let mut fc = f(c);
    let mut fd = f(d);
    for _ in 0..25 {
        if fc > fd {
            b = d;
            d = c;
            fd = fc;
            c = b - phi * (b - a);
            fc = f(c);
        } else {
            a = c;
            c = d;
            fc = fd;
            d = a + phi * (b - a);
            fd = f(d);
        }
        if (b - a).abs() < 0.005 {
            break;
        }
    }
    let best = (a + b) / 2.0;
    (best, f(best))
}

/// Find the optimal uniform exposure multiplier for dt_sigmoid.
fn optimize_dt_sigmoid_exposure(
    linear_f32: &[f32],
    w: u32,
    h: u32,
    reference: &[u8],
    ref_w: u32,
    ref_h: u32,
    zs: &Zensim,
    lo: f32,
    hi: f32,
) -> (f32, f64) {
    golden_search(
        |mult| {
            let out = apply_dt_sigmoid_pipeline(linear_f32, w, h, mult);
            let (a, b, rw, rh) = resize_pair(&out, w, h, reference, ref_w, ref_h);
            zensim_score(&a, &b, rw, rh, zs)
        },
        lo,
        hi,
    )
}

/// Find optimal per-channel [R, G, B] multipliers via coordinate descent.
///
/// Starts from the uniform optimum, then optimizes each channel independently,
/// repeating for `rounds` iterations.
fn optimize_rgb_exposure(
    linear_f32: &[f32],
    w: u32,
    h: u32,
    reference: &[u8],
    ref_w: u32,
    ref_h: u32,
    zs: &Zensim,
    uniform_mult: f32,
) -> ([f32; 3], f64) {
    let mut rgb = [uniform_mult; 3];
    let mut best_score = 0.0f64;

    let eval = |m: [f32; 3]| -> f64 {
        let out = apply_dt_sigmoid_rgb(linear_f32, w, h, m);
        let (a, b, rw, rh) = resize_pair(&out, w, h, reference, ref_w, ref_h);
        zensim_score(&a, &b, rw, rh, zs)
    };

    // 3 rounds of coordinate descent
    for _ in 0..3 {
        for ch in 0..3 {
            let (best_val, score) = golden_search(
                |v| {
                    let mut m = rgb;
                    m[ch] = v;
                    eval(m)
                },
                (rgb[ch] * 0.6).max(0.5),
                rgb[ch] * 1.6,
            );
            rgb[ch] = best_val;
            best_score = score;
        }
    }

    (rgb, best_score)
}

/// Generate a per-pixel absolute difference heatmap between two sRGB u8 images.
/// Returns a colorized heatmap where:
///   black = identical, blue = small diff, red = large diff, white = extreme diff
fn diff_heatmap(a: &[u8], b: &[u8], w: u32, h: u32) -> Vec<u8> {
    let n = (w as usize) * (h as usize);
    let mut out = vec![0u8; n * 3];
    for i in 0..n {
        let idx = i * 3;
        // Compute per-channel absolute difference, take max
        let dr = (a[idx] as i32 - b[idx] as i32).unsigned_abs();
        let dg = (a[idx + 1] as i32 - b[idx + 1] as i32).unsigned_abs();
        let db = (a[idx + 2] as i32 - b[idx + 2] as i32).unsigned_abs();
        let d = dr.max(dg).max(db).min(255) as u8;
        // Colorize: scale 4x so diffs are visible, then blue->red->white
        let v = (d as u32 * 4).min(255) as u8;
        if v < 128 {
            // black -> blue -> cyan
            out[idx] = 0;
            out[idx + 1] = v;
            out[idx + 2] = v * 2;
        } else {
            // yellow -> red -> white
            let t = v - 128;
            out[idx] = 128 + t;
            out[idx + 1] = 128 - t;
            out[idx + 2] = t;
        }
    }
    out
}

/// Generate a side-by-side comparison image: [A | B | diff_heatmap]
fn side_by_side(a: &[u8], b: &[u8], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    let heat = diff_heatmap(a, b, w, h);
    let total_w = w * 3;
    let stride_a = (w as usize) * 3;
    let stride_out = (total_w as usize) * 3;
    let mut out = vec![0u8; stride_out * (h as usize)];
    for y in 0..h as usize {
        let row_a = &a[y * stride_a..(y + 1) * stride_a];
        let row_b = &b[y * stride_a..(y + 1) * stride_a];
        let row_h = &heat[y * stride_a..(y + 1) * stride_a];
        let row_out = &mut out[y * stride_out..(y + 1) * stride_out];
        row_out[..stride_a].copy_from_slice(row_a);
        row_out[stride_a..stride_a * 2].copy_from_slice(row_b);
        row_out[stride_a * 2..stride_a * 3].copy_from_slice(row_h);
    }
    (out, total_w, h)
}

/// Get darktable's display-referred sRGB output for a DNG file.
/// This uses darktable's default workflow (basecurve tone mapping).
/// Get darktable sRGB output for a DNG file with a specific workflow.
/// `workflow`: "scene-referred (sigmoid)" (default), "display-referred", or "none"
fn darktable_render(dng_path: &Path, workflow: &str) -> Option<(Vec<u8>, u32, u32)> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_dir = PathBuf::from(format!("/tmp/dt_parity_{}_{}", std::process::id(), id));
    fs::create_dir_all(&tmp_dir).ok()?;
    let out_path = tmp_dir.join("output.png");

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
        .arg("--conf")
        .arg(format!("plugins/darkroom/workflow={workflow}"))
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .status()
        .ok()?;

    if !status.success() {
        let _ = fs::remove_dir_all(&tmp_dir);
        return None;
    }

    let png_bytes = fs::read(&out_path).ok()?;
    let _ = fs::remove_dir_all(&tmp_dir);
    let decoded = DecodeRequest::new(&png_bytes).decode().ok()?;
    let w = decoded.width();
    let h = decoded.height();
    use zenpixels_convert::PixelBufferConvertTypedExt;
    let rgb8_buf = decoded.into_buffer().to_rgb8();
    let bytes = rgb8_buf.copy_to_contiguous_bytes();
    Some((bytes, w, h))
}

/// Get darktable's default (scene-referred sigmoid) sRGB output.
fn darktable_display_output(dng_path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    darktable_render(dng_path, "scene-referred (sigmoid)")
}

/// Get darktable's display-referred (basecurve) sRGB output.
#[allow(dead_code)]
fn darktable_basecurve_output(dng_path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    darktable_render(dng_path, "display-referred")
}

/// Downscale sRGB u8 RGB data to fit within max_dim using zenresize (SIMD).
fn downscale_rgb8(data: &[u8], w: u32, h: u32, max_dim: u32) -> (Vec<u8>, u32, u32) {
    if w <= max_dim && h <= max_dim {
        return (data.to_vec(), w, h);
    }
    let scale = max_dim as f64 / w.max(h) as f64;
    let nw = ((w as f64 * scale) as u32).max(1);
    let nh = ((h as f64 * scale) as u32).max(1);
    let config = ResizeConfig::builder(w, h, nw, nh)
        .filter(Filter::Lanczos)
        .format(PixelDescriptor::RGB8_SRGB)
        .build();
    let mut resizer = Resizer::new(&config);
    (resizer.resize(data), nw, nh)
}

/// Crop u8 RGB data to target dimensions (top-left).
fn crop_u8(data: &[u8], w: u32, h: u32, tw: u32, th: u32) -> Vec<u8> {
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

/// Resize and crop two images to common dimensions <= MAX_DIM.
fn resize_pair(
    a: &[u8],
    aw: u32,
    ah: u32,
    b: &[u8],
    bw: u32,
    bh: u32,
) -> (Vec<u8>, Vec<u8>, u32, u32) {
    let (ra, raw, rah) = downscale_rgb8(a, aw, ah, MAX_DIM);
    let (rb, rbw, rbh) = downscale_rgb8(b, bw, bh, MAX_DIM);
    let w = raw.min(rbw);
    let h = rah.min(rbh);
    let ca = crop_u8(&ra, raw, rah, w, h);
    let cb = crop_u8(&rb, rbw, rbh, w, h);
    (ca, cb, w, h)
}

fn save_rgb(data: &[u8], w: u32, h: u32, path: &str) {
    let pixels: &[Rgb<u8>] = bytemuck::cast_slice(data);
    let img = ImgVec::new(pixels.to_vec(), w as usize, h as usize);
    match EncodeRequest::new(ImageFormat::Jpeg)
        .with_quality(90.0)
        .encode_rgb8(img.as_ref())
    {
        Ok(encoded) => {
            let _ = fs::write(path, encoded.data());
        }
        Err(e) => eprintln!("    save error: {e}"),
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
    parity_dt_opt: f64, // dt_sigmoid with per-image optimized uniform exposure
    parity_rgb: f64,    // dt_sigmoid with per-channel [R,G,B] optimized exposure
    optimal_mult: f32,  // the per-image optimal uniform exposure multiplier
    rgb_mult: [f32; 3], // per-channel optimal multipliers
    ceiling: f64,       // darktable sigmoid vs expert
    quality_k3: f64,    // our JPEG cluster pipeline (k=3 blend) vs expert
    quality_rule: f64,  // our JPEG rule-based pipeline vs expert
    baseline: f64,      // untouched original vs expert
    illuminant_xy: Option<(f32, f32)>,
    regional_dng: Option<RegionalComparison>,
    regional_jpeg: Option<RegionalComparison>,
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
        let expert_bytes = match fs::read(expert_path) {
            Ok(b) => b,
            Err(_) => {
                println!("  SKIP: can't load expert");
                continue;
            }
        };
        let expert_decoded = match DecodeRequest::new(&expert_bytes).decode() {
            Ok(d) => d,
            Err(_) => {
                println!("  SKIP: can't decode expert");
                continue;
            }
        };
        let (ew, eh) = (expert_decoded.width(), expert_decoded.height());
        let expert_raw = {
            use zenpixels_convert::PixelBufferConvertTypedExt;
            expert_decoded
                .into_buffer()
                .to_rgb8()
                .copy_to_contiguous_bytes()
        };

        // Load original JPEG
        let orig_bytes = match fs::read(orig_path) {
            Ok(b) => b,
            Err(_) => {
                println!("  SKIP: can't load original");
                continue;
            }
        };
        let orig_decoded = match DecodeRequest::new(&orig_bytes).decode() {
            Ok(d) => d,
            Err(_) => {
                println!("  SKIP: can't decode original");
                continue;
            }
        };
        let (ow, oh) = (orig_decoded.width(), orig_decoded.height());
        let orig_raw = {
            use zenpixels_convert::PixelBufferConvertTypedExt;
            orig_decoded
                .into_buffer()
                .to_rgb8()
                .copy_to_contiguous_bytes()
        };

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
        let (_quality, quality_k3, _best_cluster, jpeg_best) = if n_clusters > 0 {
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
        let dng_result = process_dng_parity(
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
        );
        let (
            parity_dt_opt,
            parity_rgb,
            optimal_mult,
            rgb_mult,
            ceiling,
            illuminant_xy,
            regional_dng,
        ) = match dng_result {
            Some(r) => (
                r.parity_dt_opt,
                r.parity_rgb,
                r.optimal_mult,
                r.rgb_mult,
                r.ceiling,
                r.illuminant_xy,
                Some(r.regional),
            ),
            None => {
                println!("  DNG failed");
                (-1.0, -1.0, 0.0, [0.0; 3], -1.0, None, None)
            }
        };

        // --- Regional analysis: best JPEG pipeline vs expert ---
        let regional_jpeg = {
            let best_jpeg = jpeg_best.as_deref().unwrap_or(&jpeg_rule);
            Some(regional_compare_srgb(best_jpeg, &expert_r, w, h, &m1))
        };

        let rgb_str = format!("[{:.2},{:.2},{:.2}]", rgb_mult[0], rgb_mult[1], rgb_mult[2]);
        let delta = parity_rgb - parity_dt_opt;
        println!(
            "  dtO={parity_dt_opt:.1}({optimal_mult:.2}x) RGB={parity_rgb:.1}{rgb_str} Δ={delta:+.1} ceil={ceiling:.1} k3={quality_k3:.1} base0={baseline:.1}"
        );
        if let Some(ref reg) = regional_dng {
            print_regional("DNG→dt", reg);
        }

        // Save comparison images
        let prefix = format!("{OUTPUT_DIR}/{stem}");
        save_rgb(&orig_r, w, h, &format!("{prefix}_1_orig.jpg"));
        save_rgb(&expert_r, w, h, &format!("{prefix}_3_expert.jpg"));

        results.push(ImageResult {
            name: stem.to_string(),
            parity_dt_opt,
            parity_rgb,
            optimal_mult,
            rgb_mult,
            ceiling,
            quality_k3,
            quality_rule,
            baseline,
            illuminant_xy,
            regional_dng,
            regional_jpeg,
        });
    }

    // Summary
    println!("\n\n=== RESULTS ===");
    println!("dtOpt   = dt_sigmoid, per-image optimized uniform exposure");
    println!("RGB     = dt_sigmoid, per-channel [R,G,B] optimized exposure");
    println!("ceil    = darktable display vs expert");
    println!("k3      = our JPEG cluster pipeline (3-blend) vs expert\n");

    println!(
        "{:<28} {:>6} {:>6} {:>6} {:>20} {:>6} {:>6} {:>6}",
        "Image", "dtOpt", "mult", "RGB", "R,G,B mults", "Ceil", "K3", "Base0"
    );
    println!("{}", "-".repeat(104));

    let (mut sdto, mut srgb, mut sc, mut sq3, mut sb) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let mut np = 0;

    for r in &results {
        let fmt = |v: f64| -> String {
            if v < 0.0 {
                "---".to_string()
            } else {
                format!("{v:.1}")
            }
        };
        let rgb_str = format!(
            "{:.2},{:.2},{:.2}",
            r.rgb_mult[0], r.rgb_mult[1], r.rgb_mult[2]
        );
        println!(
            "{:<28} {:>6} {:>6} {:>6} {:>20} {:>6} {:>6} {:>6.1}",
            &r.name[..r.name.len().min(28)],
            fmt(r.parity_dt_opt),
            if r.optimal_mult > 0.0 {
                format!("{:.2}x", r.optimal_mult)
            } else {
                "---".to_string()
            },
            fmt(r.parity_rgb),
            rgb_str,
            fmt(r.ceiling),
            fmt(r.quality_k3),
            r.baseline
        );
        if r.parity_dt_opt >= 0.0 {
            sdto += r.parity_dt_opt;
            srgb += r.parity_rgb;
            sc += r.ceiling;
            np += 1;
        }
        sq3 += r.quality_k3;
        sb += r.baseline;
    }

    let n = results.len() as f64;
    println!("{}", "-".repeat(104));
    let mean_dto = if np > 0 { sdto / np as f64 } else { 0.0 };
    let mean_rgb = if np > 0 { srgb / np as f64 } else { 0.0 };
    println!(
        "{:<28} {:>6.1} {:>6} {:>6.1} {:>20} {:>6.1} {:>6.1} {:>6.1}",
        "MEAN",
        mean_dto,
        "",
        mean_rgb,
        "",
        if np > 0 { sc / np as f64 } else { 0.0 },
        sq3 / n,
        sb / n,
    );
    println!(
        "{:<28} {:>6} {:>6} {:>+6.1}",
        "RGB vs uniform",
        "",
        "",
        mean_rgb - mean_dto,
    );

    // Exposure multiplier statistics
    if np > 0 {
        let mults: Vec<f32> = results
            .iter()
            .filter(|r| r.optimal_mult > 0.0)
            .map(|r| r.optimal_mult)
            .collect();
        if !mults.is_empty() {
            let mean_m: f32 = mults.iter().sum::<f32>() / mults.len() as f32;
            let min_m = mults.iter().copied().reduce(f32::min).unwrap();
            let max_m = mults.iter().copied().reduce(f32::max).unwrap();
            println!("\nUniform mult:  mean={mean_m:.2}x  range=[{min_m:.2}, {max_m:.2}]");
        }

        // Per-channel stats
        let rgb_mults: Vec<[f32; 3]> = results
            .iter()
            .filter(|r| r.rgb_mult[0] > 0.0)
            .map(|r| r.rgb_mult)
            .collect();
        if !rgb_mults.is_empty() {
            let n = rgb_mults.len() as f32;
            let mean_r = rgb_mults.iter().map(|m| m[0]).sum::<f32>() / n;
            let mean_g = rgb_mults.iter().map(|m| m[1]).sum::<f32>() / n;
            let mean_b = rgb_mults.iter().map(|m| m[2]).sum::<f32>() / n;
            println!("RGB mults:     mean=[{mean_r:.2}, {mean_g:.2}, {mean_b:.2}]");
            // Show ratio R/G and B/G to understand color shift
            println!("  R/G={:.3}  B/G={:.3}", mean_r / mean_g, mean_b / mean_g);
        }

        // Illuminant xy statistics
        let xys: Vec<(f32, f32)> = results.iter().filter_map(|r| r.illuminant_xy).collect();
        if !xys.is_empty() {
            let mean_x = xys.iter().map(|xy| xy.0).sum::<f32>() / xys.len() as f32;
            let mean_y = xys.iter().map(|xy| xy.1).sum::<f32>() / xys.len() as f32;
            println!(
                "Illuminant xy: mean=({mean_x:.4}, {mean_y:.4})  D65=(0.3127, 0.3290)  n={}",
                xys.len()
            );
        }
    }

    // Regional summary
    let labels = RegionalComparison::zone_labels();
    {
        let dng_regs: Vec<&RegionalComparison> = results
            .iter()
            .filter_map(|r| r.regional_dng.as_ref())
            .collect();
        let jpeg_regs: Vec<&RegionalComparison> = results
            .iter()
            .filter_map(|r| r.regional_jpeg.as_ref())
            .collect();

        if !dng_regs.is_empty() {
            println!(
                "\n=== REGIONAL: DNG base vs darktable ({} images) ===",
                dng_regs.len()
            );
            print_zone_summary("  Luminance", labels.luminance, &dng_regs, |r| {
                &r.lum_zone_dist
            });
            print_zone_summary("  Hue      ", labels.hue, &dng_regs, |r| &r.hue_sector_dist);
            print_zone_summary("  Chroma   ", labels.chroma, &dng_regs, |r| {
                &r.chroma_zone_dist
            });
            print_zone_summary("  Texture  ", labels.texture, &dng_regs, |r| {
                &r.texture_zone_dist
            });
            let mean_agg: f32 =
                dng_regs.iter().map(|r| r.aggregate).sum::<f32>() / dng_regs.len() as f32;
            println!("  Aggregate: {mean_agg:.4}");
        }

        if !jpeg_regs.is_empty() {
            println!(
                "\n=== REGIONAL: JPEG pipeline vs expert ({} images) ===",
                jpeg_regs.len()
            );
            print_zone_summary("  Luminance", labels.luminance, &jpeg_regs, |r| {
                &r.lum_zone_dist
            });
            print_zone_summary("  Hue      ", labels.hue, &jpeg_regs, |r| {
                &r.hue_sector_dist
            });
            print_zone_summary("  Chroma   ", labels.chroma, &jpeg_regs, |r| {
                &r.chroma_zone_dist
            });
            print_zone_summary("  Texture  ", labels.texture, &jpeg_regs, |r| {
                &r.texture_zone_dist
            });
            let mean_agg: f32 =
                jpeg_regs.iter().map(|r| r.aggregate).sum::<f32>() / jpeg_regs.len() as f32;
            println!("  Aggregate: {mean_agg:.4}");
        }
    }

    // Write TSV
    let tsv_path = format!("{OUTPUT_DIR}/parity_results.tsv");
    let mut tsv = String::new();
    tsv.push_str(
        "image\tparity_dt_opt\topt_mult\tparity_rgb\trgb_r\trgb_g\trgb_b\tilluminant_x\tilluminant_y\tceiling\tquality_k3\tquality_rule\tbaseline\tregional_dng\tregional_jpeg\n",
    );
    for r in &results {
        let reg_dng = r.regional_dng.as_ref().map_or(-1.0, |r| r.aggregate);
        let reg_jpeg = r.regional_jpeg.as_ref().map_or(-1.0, |r| r.aggregate);
        let (ix, iy) = r.illuminant_xy.unwrap_or((-1.0, -1.0));
        tsv.push_str(&format!(
            "{}\t{:.2}\t{:.3}\t{:.2}\t{:.3}\t{:.3}\t{:.3}\t{:.4}\t{:.4}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.4}\t{:.4}\n",
            r.name,
            r.parity_dt_opt,
            r.optimal_mult,
            r.parity_rgb,
            r.rgb_mult[0], r.rgb_mult[1], r.rgb_mult[2],
            ix, iy,
            r.ceiling,
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

/// DNG parity result: scores for dt_sigmoid with uniform and per-channel exposure.
struct DngParityResult {
    parity_dt_opt: f64, // dt_sigmoid with per-image optimized uniform exposure
    parity_rgb: f64,    // dt_sigmoid with per-channel [R,G,B] optimized exposure
    optimal_mult: f32,  // optimal uniform exposure multiplier
    rgb_mult: [f32; 3], // per-channel optimal multipliers
    ceiling: f64,       // darktable vs expert
    illuminant_xy: Option<(f32, f32)>,
    regional: RegionalComparison,
}

/// Process DNG: compare our dt_sigmoid pipeline against darktable's output.
#[allow(clippy::too_many_arguments)]
fn process_dng_parity(
    dng_path: &Path,
    expert_raw: &[u8],
    ew: u32,
    eh: u32,
    dt_config: &DtConfig,
    m1: &GamutMatrix,
    _m1_inv: &GamutMatrix,
    _ctx: &mut FilterContext,
    zs: &Zensim,
    out_prefix: &str,
) -> Option<DngParityResult> {
    // 1. Get darktable scene-referred (sigmoid) output — the default in dt 5.5
    let (dt_sig_out, dtw, dth) = darktable_display_output(dng_path)?;

    // 2. Get darktable linear output for our pipeline
    let output = darktable::decode_file(dng_path, dt_config).ok()?;
    let pixels = output.pixels;
    let dw = pixels.width();
    let dh = pixels.height();
    let raw_bytes = pixels.copy_to_contiguous_bytes();
    let linear_f32: &[f32] = bytemuck::cast_slice(&raw_bytes);

    // 3. Extract illuminant xy from EXIF
    let dng_bytes = std::fs::read(dng_path).ok()?;
    let exif = zenraw::exif::read_metadata(&dng_bytes);
    use zenfilters::filters::cat16;
    let illuminant_xy = exif.as_ref().and_then(|e| {
        let cm = if e.calibration_illuminant_2 == Some(21) {
            e.color_matrix_2.as_deref().or(e.color_matrix_1.as_deref())
        } else {
            e.color_matrix_1.as_deref()
        };
        cat16::illuminant_xy_from_dng(e.as_shot_white_xy, e.as_shot_neutral.as_deref(), cm)
    });

    // 4. Optimize uniform exposure multiplier
    let (optimal_mult, parity_dt_opt) =
        optimize_dt_sigmoid_exposure(linear_f32, dw, dh, &dt_sig_out, dtw, dth, zs, 1.0, 4.0);

    // 5. Optimize per-channel [R,G,B] exposure multipliers
    let (rgb_mult, parity_rgb) =
        optimize_rgb_exposure(linear_f32, dw, dh, &dt_sig_out, dtw, dth, zs, optimal_mult);

    // 6. Darktable sigmoid vs expert → ceiling
    let (dt_r2, expert_r, w2, h2) = resize_pair(&dt_sig_out, dtw, dth, expert_raw, ew, eh);
    let ceiling = zensim_score(&dt_r2, &expert_r, w2, h2, zs);

    // 7. Regional comparison: our best (RGB) vs darktable
    let best_out = apply_dt_sigmoid_rgb(linear_f32, dw, dh, rgb_mult);
    let (best_r, dt_r, w, h) = resize_pair(&best_out, dw, dh, &dt_sig_out, dtw, dth);
    let regional = regional_compare_srgb(&best_r, &dt_r, w, h, m1);

    // Save comparison images
    save_rgb(&dt_r, w, h, &format!("{out_prefix}_6_dt_reference.jpg"));
    save_rgb(&best_r, w, h, &format!("{out_prefix}_4d_dng_rgb_opt.jpg"));
    let (sbs, sbs_w, sbs_h) = side_by_side(&best_r, &dt_r, w, h);
    save_rgb(&sbs, sbs_w, sbs_h, &format!("{out_prefix}_diff_sbs.jpg"));

    Some(DngParityResult {
        parity_dt_opt,
        parity_rgb,
        optimal_mult,
        rgb_mult,
        ceiling,
        illuminant_xy,
        regional,
    })
}
