//! Mobile DNG parity: compare our dt_sigmoid pipeline against reference
//! renderings on mobile (iPhone ProRAW, Samsung Galaxy) DNGs.
//!
//! Three decode paths based on file format:
//! - **Standard DNG** (Samsung): darktable reference rendering
//! - **APPLEDNG** (iPhone 16/15 Pro): rawler decode + embedded preview reference
//!   (darktable can't handle LJPEG predictor 7 used by Apple)
//! - **AMPF** (iPhone 17 Pro): skip — processed JPEG, not raw data
//!
//! Performance: all optimization runs on downscaled (~512px) data. Full-res
//! is only used for final image output. Uses zenresize (SIMD) and zencodecs
//! instead of the image crate.
//!
//! Usage: cargo run --release --features experimental --example mobile_parity

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use enough::Unstoppable;
use imgref::ImgVec;
use rgb::Rgb;
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat};
use zenfilters::filters::cat16;
use zenfilters::filters::dt_sigmoid;
use zenfilters::regional::{RegionalComparison, RegionalFeatures};
use zenfilters::{OklabPlanes, scatter_srgb_u8_to_oklab};
use zenpixels::ColorPrimaries;
use zenpixels_convert::gamut::GamutMatrix;
use zenpixels_convert::oklab;
use zenraw::darktable::{self, DtConfig};
use zenresize::{Filter, PixelDescriptor, ResizeConfig, Resizer};
use zensim::{RgbSlice, Zensim, ZensimProfile};

const OUTPUT_DIR: &str = "/mnt/v/output/zenfilters/mobile_parity";
const MAX_DIM: u32 = 512;

/// Mobile DNG files to test.
const MOBILE_DNGS: &[(&str, &str)] = &[
    ("iPhone_3269", "/mnt/v/heic/IMG_3269.DNG"),
    ("iPhone_3270", "/mnt/v/heic/IMG_3270.DNG"),
    ("iPhone_46CD", "/mnt/v/heic/46CD6167-C36B-4F98-B386-2300D8E840F0.DNG"),
    ("iPhone_CBFA", "/mnt/v/heic/CBFA569A-5C28-468E-96B4-CFFBAEB951C7.DNG"),
    ("Samsung_Fold7", "/mnt/v/heic/android/20260220_093521.dng"),
];

/// Detected file format.
enum MobileFormat {
    /// Apple AMPF container — processed JPEG, not raw.
    Ampf,
    /// Apple ProRAW DNG (APPLEDNG) — darktable can't handle LJPEG pred 7.
    /// Use rawler decode + embedded preview as reference.
    AppleDng,
    /// Standard DNG — darktable works.
    StandardDng,
}

fn detect_format(data: &[u8]) -> MobileFormat {
    if zenraw::exif::is_ampf(data) {
        return MobileFormat::Ampf;
    }
    // APPLEDNG: TIFF with "APPLEDNG" signature at bytes 8-15
    if data.len() > 16 && zenraw::is_raw_file(data) && &data[8..16] == b"APPLEDNG" {
        return MobileFormat::AppleDng;
    }
    MobileFormat::StandardDng
}

fn main() {
    fs::create_dir_all(OUTPUT_DIR).unwrap();

    let has_dt = darktable::is_available();
    if has_dt {
        println!("darktable: {}", darktable::version().unwrap_or_default());
    } else {
        println!("WARNING: darktable-cli not found — standard DNG files will be skipped");
    }

    let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
    let zs = Zensim::new(ZensimProfile::latest());
    let dt_config = DtConfig::new();

    let mut results: Vec<MobileResult> = Vec::new();

    for (label, path) in MOBILE_DNGS {
        let dng_path = PathBuf::from(path);
        if !dng_path.exists() {
            println!("\n{label}: SKIP (file not found)");
            continue;
        }
        let file_size = fs::metadata(&dng_path).map(|m| m.len()).unwrap_or(0);
        let t0 = Instant::now();
        println!("\n=== {label} ({:.1} MB) ===", file_size as f64 / 1_048_576.0);

        let dng_bytes = fs::read(&dng_path).unwrap();
        let format = detect_format(&dng_bytes);

        // 1. Read EXIF metadata
        let exif = zenraw::exif::read_metadata(&dng_bytes);
        if let Some(ref e) = exif {
            println!("  Make:     {:?}", e.make);
            println!("  Model:    {:?}", e.model);
            println!("  ISO:      {:?}", e.iso);
            println!("  Dims:     {:?}x{:?}", e.width, e.height);
            println!("  DNG ver:  {:?}", e.dng_version);
            println!("  BaseLine: {:?} EV", e.baseline_exposure);
            println!("  AsShotN:  {:?}", e.as_shot_neutral);
        }

        match format {
            MobileFormat::Ampf => {
                println!("  Format: AMPF (processed JPEG + HDR gain map, not raw)");
                println!("  SKIP: not raw data");
                continue;
            }
            MobileFormat::AppleDng => {
                println!("  Format: APPLEDNG (rawler decode, embedded preview reference)");
                if let Some(r) = process_appledng(
                    label, &dng_bytes, &exif, &m1, &zs, &t0,
                ) {
                    results.push(r);
                }
            }
            MobileFormat::StandardDng => {
                println!("  Format: Standard DNG (darktable reference)");
                if !has_dt {
                    println!("  SKIP: darktable not available");
                    continue;
                }
                if let Some(r) = process_standard_dng(
                    label, &dng_path, &dng_bytes, &exif, &m1, &zs, &dt_config, &t0,
                ) {
                    results.push(r);
                }
            }
        }
    }

    // Summary table
    print_summary(&results);

    // Save TSV
    save_tsv(&results);
}

/// Process APPLEDNG files (iPhone 16/15 Pro) using rawler + embedded preview.
fn process_appledng(
    label: &str,
    dng_bytes: &[u8],
    exif: &Option<zenraw::exif::ExifMetadata>,
    m1: &GamutMatrix,
    zs: &Zensim,
    t0: &Instant,
) -> Option<MobileResult> {
    // Extract embedded preview as reference
    let preview_jpeg = zenraw::exif::extract_dng_preview(dng_bytes)?;
    let decoded_preview = DecodeRequest::new(&preview_jpeg).decode().ok()?;
    let pw = decoded_preview.width();
    let ph = decoded_preview.height();
    use zenpixels_convert::PixelBufferConvertTypedExt;
    let preview_rgb8 = decoded_preview.into_buffer().to_rgb8().copy_to_contiguous_bytes();
    println!("  Preview: {}x{} ({} bytes JPEG) ({:.1}s)",
        pw, ph, preview_jpeg.len(), t0.elapsed().as_secs_f32());

    // Decode raw with zenraw (rawler backend)
    let config = zenraw::RawDecodeConfig::default();
    let output = zenraw::decode(dng_bytes, &config, &Unstoppable).ok()?;
    let raw_pixels = output.pixels;
    let dw = raw_pixels.width();
    let dh = raw_pixels.height();
    let raw_bytes = raw_pixels.copy_to_contiguous_bytes();
    let linear_f32: &[f32] = bytemuck::cast_slice(&raw_bytes);
    println!("  Raw decode: {}x{} ({:.1}s)", dw, dh, t0.elapsed().as_secs_f32());

    // Analyze linear data range
    let (mean_v, mr, mg, mb, clipped_pct) = analyze_linear(linear_f32);
    println!("  Linear mean={mean_v:.4} clipped={clipped_pct:.2}%");
    println!("  Channel means: R={mr:.4} G={mg:.4} B={mb:.4}  R/G={:.3} B/G={:.3}",
        mr / mg, mb / mg);

    // Extract illuminant
    let illuminant_xy = exif.as_ref().and_then(|e| {
        let cm = if e.calibration_illuminant_2 == Some(21) {
            e.color_matrix_2.as_deref().or(e.color_matrix_1.as_deref())
        } else {
            e.color_matrix_1.as_deref()
        };
        cat16::illuminant_xy_from_dng(
            e.as_shot_white_xy,
            e.as_shot_neutral.as_deref(),
            cm,
        )
    });
    if let Some((x, y)) = illuminant_xy {
        println!("  Illuminant: ({x:.4}, {y:.4})");
    }

    // Downscale both for optimization
    let (small_linear, sw, sh) = downscale_linear_f32(linear_f32, dw, dh, MAX_DIM);
    let (small_ref, srw, srh) = downscale_rgb8(&preview_rgb8, pw, ph, MAX_DIM);
    let cw = sw.min(srw);
    let ch = sh.min(srh);
    let small_linear = crop_f32(&small_linear, sw, sh, cw, ch);
    let small_ref = crop_u8(&small_ref, srw, srh, cw, ch);
    println!("  Optimization res: {}x{} ({:.1}s)", cw, ch, t0.elapsed().as_secs_f32());

    // Derive search range from BaselineExposure (Apple underexposes raw heavily)
    let bl_ev = exif.as_ref().and_then(|e| e.baseline_exposure).unwrap_or(0.0);
    let bl_mult = 2.0f64.powf(bl_ev) as f32;
    let search_lo = (bl_mult * 0.25).max(0.5);
    let search_hi = (bl_mult * 4.0).max(8.0).min(64.0);
    println!("  Search range: {search_lo:.2}..{search_hi:.1}x (BL={bl_ev:+.2} EV → {bl_mult:.1}x)");

    // Optimize uniform exposure
    let (optimal_mult, parity_uniform) = optimize_dt_sigmoid_exposure(
        &small_linear, cw, ch, &small_ref, cw, ch, zs, search_lo, search_hi,
    );
    println!("  Uniform mult: {optimal_mult:.3}x → parity={parity_uniform:.1} ({:.1}s)",
        t0.elapsed().as_secs_f32());

    // Optimize per-channel RGB (wider range for APPLEDNG)
    let (rgb_mult, parity_rgb) = optimize_rgb_exposure_range(
        &small_linear, cw, ch, &small_ref, cw, ch, zs, optimal_mult, search_lo, search_hi,
    );
    let delta = parity_rgb - parity_uniform;
    println!("  RGB mult: [{:.3}, {:.3}, {:.3}] → parity={parity_rgb:.1} (Δ={delta:+.1}) ({:.1}s)",
        rgb_mult[0], rgb_mult[1], rgb_mult[2], t0.elapsed().as_secs_f32());
    println!("  RGB ratios: R/G={:.3}  B/G={:.3}",
        rgb_mult[0] / rgb_mult[1], rgb_mult[2] / rgb_mult[1]);

    // Regional comparison
    let best_small = apply_dt_sigmoid_rgb(&small_linear, cw, ch, rgb_mult);
    let regional = regional_compare_srgb(&best_small, &small_ref, cw, ch, m1);
    print_regional(&regional);

    // Save comparison images
    let prefix = format!("{OUTPUT_DIR}/{label}");
    save_rgb8_jpeg(&small_ref, cw, ch, &format!("{prefix}_apple_preview.jpg"));
    save_rgb8_jpeg(&best_small, cw, ch, &format!("{prefix}_our_rgb.jpg"));
    let (sbs, sbs_w, sbs_h) = side_by_side(&best_small, &small_ref, cw, ch);
    save_rgb8_jpeg(&sbs, sbs_w, sbs_h, &format!("{prefix}_sbs.jpg"));
    let heat = diff_heatmap(&best_small, &small_ref, cw, ch);
    save_rgb8_jpeg(&heat, cw, ch, &format!("{prefix}_diff.jpg"));

    println!("  Total: {:.1}s", t0.elapsed().as_secs_f32());

    Some(MobileResult {
        name: label.to_string(),
        parity_uniform,
        parity_rgb,
        optimal_mult,
        rgb_mult,
        illuminant_xy,
        make: exif.as_ref().and_then(|e| e.make.clone()),
        model: exif.as_ref().and_then(|e| e.model.clone()),
        iso: exif.as_ref().and_then(|e| e.iso),
        baseline_exposure: exif.as_ref().and_then(|e| e.baseline_exposure),
        dims: (dw, dh),
        linear_mean: mean_v as f32,
        regional,
        reference_source: "apple_preview",
    })
}

/// Process standard DNG (Samsung etc.) using darktable as reference.
#[allow(clippy::too_many_arguments)]
fn process_standard_dng(
    label: &str,
    dng_path: &Path,
    _dng_bytes: &[u8],
    exif: &Option<zenraw::exif::ExifMetadata>,
    m1: &GamutMatrix,
    zs: &Zensim,
    dt_config: &DtConfig,
    t0: &Instant,
) -> Option<MobileResult> {
    // Extract illuminant
    let illuminant_xy = exif.as_ref().and_then(|e| {
        let cm = if e.calibration_illuminant_2 == Some(21) {
            e.color_matrix_2.as_deref().or(e.color_matrix_1.as_deref())
        } else {
            e.color_matrix_1.as_deref()
        };
        cat16::illuminant_xy_from_dng(
            e.as_shot_white_xy,
            e.as_shot_neutral.as_deref(),
            cm,
        )
    });
    if let Some((x, y)) = illuminant_xy {
        println!("  Illuminant: ({x:.4}, {y:.4})");
    }

    // Render with darktable (scene-referred sigmoid)
    let dt_sigmoid = darktable_render_png(dng_path, "scene-referred (sigmoid)");
    if dt_sigmoid.is_none() {
        println!("  darktable sigmoid render FAILED");
        return None;
    }
    let (dt_sig_out, dtw, dth) = dt_sigmoid.unwrap();
    println!("  darktable sigmoid: {}x{} ({:.1}s)", dtw, dth, t0.elapsed().as_secs_f32());

    // Render with darktable (workflow=none for linear)
    let linear_output = darktable::decode_file(dng_path, dt_config);
    if linear_output.is_err() {
        println!("  darktable linear render FAILED: {:?}", linear_output.err());
        return None;
    }
    let output = linear_output.unwrap();
    let pixels = output.pixels;
    let dw = pixels.width();
    let dh = pixels.height();
    let raw_bytes = pixels.copy_to_contiguous_bytes();
    let linear_f32: &[f32] = bytemuck::cast_slice(&raw_bytes);
    println!("  darktable linear: {}x{} ({:.1}s)", dw, dh, t0.elapsed().as_secs_f32());

    // Analyze linear data
    let (mean_v, mr, mg, mb, clipped_pct) = analyze_linear(linear_f32);
    println!("  Linear range: mean={mean_v:.4} clipped={clipped_pct:.2}%");
    println!("  Channel means: R={mr:.4} G={mg:.4} B={mb:.4}  R/G={:.3} B/G={:.3}",
        mr / mg, mb / mg);

    // Downscale both for optimization
    let (small_linear, sw, sh) = downscale_linear_f32(linear_f32, dw, dh, MAX_DIM);
    let (small_ref, srw, srh) = downscale_rgb8(&dt_sig_out, dtw, dth, MAX_DIM);
    let cw = sw.min(srw);
    let ch = sh.min(srh);
    let small_linear = crop_f32(&small_linear, sw, sh, cw, ch);
    let small_ref = crop_u8(&small_ref, srw, srh, cw, ch);
    println!("  Optimization res: {}x{} ({:.1}s)", cw, ch, t0.elapsed().as_secs_f32());

    // Derive search range from BaselineExposure
    let bl_ev = exif.as_ref().and_then(|e| e.baseline_exposure).unwrap_or(0.0);
    let bl_mult = 2.0f64.powf(bl_ev) as f32;
    let search_lo = (bl_mult * 0.25).max(0.5);
    let search_hi = (bl_mult * 4.0).max(8.0).min(64.0);
    println!("  Search range: {search_lo:.2}..{search_hi:.1}x (BL={bl_ev:+.2} EV → {bl_mult:.1}x)");

    // Optimize uniform exposure
    let (optimal_mult, parity_uniform) = optimize_dt_sigmoid_exposure(
        &small_linear, cw, ch, &small_ref, cw, ch, zs, search_lo, search_hi,
    );
    println!("  Uniform mult: {optimal_mult:.3}x → parity={parity_uniform:.1} ({:.1}s)",
        t0.elapsed().as_secs_f32());

    // Optimize per-channel RGB
    let (rgb_mult, parity_rgb) = optimize_rgb_exposure_range(
        &small_linear, cw, ch, &small_ref, cw, ch, zs, optimal_mult, search_lo, search_hi,
    );
    let delta = parity_rgb - parity_uniform;
    println!("  RGB mult: [{:.3}, {:.3}, {:.3}] → parity={parity_rgb:.1} (Δ={delta:+.1}) ({:.1}s)",
        rgb_mult[0], rgb_mult[1], rgb_mult[2], t0.elapsed().as_secs_f32());
    println!("  RGB ratios: R/G={:.3}  B/G={:.3}",
        rgb_mult[0] / rgb_mult[1], rgb_mult[2] / rgb_mult[1]);

    // Regional comparison
    let best_small = apply_dt_sigmoid_rgb(&small_linear, cw, ch, rgb_mult);
    let regional = regional_compare_srgb(&best_small, &small_ref, cw, ch, m1);
    print_regional(&regional);

    // Save comparison images
    let prefix = format!("{OUTPUT_DIR}/{label}");
    save_rgb8_jpeg(&small_ref, cw, ch, &format!("{prefix}_dt_sigmoid.jpg"));
    save_rgb8_jpeg(&best_small, cw, ch, &format!("{prefix}_our_rgb.jpg"));
    let (sbs, sbs_w, sbs_h) = side_by_side(&best_small, &small_ref, cw, ch);
    save_rgb8_jpeg(&sbs, sbs_w, sbs_h, &format!("{prefix}_sbs.jpg"));
    let heat = diff_heatmap(&best_small, &small_ref, cw, ch);
    save_rgb8_jpeg(&heat, cw, ch, &format!("{prefix}_diff.jpg"));

    // Also render darktable basecurve for comparison
    let dt_basecurve = darktable_render_png(dng_path, "display-referred");
    let sig_vs_bc = dt_basecurve.as_ref().map(|(bc_out, bw, bh)| {
        let (a, b, w, h) = resize_pair_rgb8(&dt_sig_out, dtw, dth, bc_out, *bw, *bh);
        zensim_score(&a, &b, w, h, zs)
    });
    if let Some(s) = sig_vs_bc {
        println!("  dt sigmoid vs basecurve: {s:.1}");
    }

    println!("  Total: {:.1}s", t0.elapsed().as_secs_f32());

    Some(MobileResult {
        name: label.to_string(),
        parity_uniform,
        parity_rgb,
        optimal_mult,
        rgb_mult,
        illuminant_xy,
        make: exif.as_ref().and_then(|e| e.make.clone()),
        model: exif.as_ref().and_then(|e| e.model.clone()),
        iso: exif.as_ref().and_then(|e| e.iso),
        baseline_exposure: exif.as_ref().and_then(|e| e.baseline_exposure),
        dims: (dw, dh),
        linear_mean: mean_v as f32,
        regional,
        reference_source: "darktable",
    })
}

fn analyze_linear(linear_f32: &[f32]) -> (f64, f64, f64, f64, f64) {
    let n = linear_f32.len();
    let npix = n / 3;
    let (mut mr, mut mg, mut mb) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..npix {
        mr += linear_f32[i * 3] as f64;
        mg += linear_f32[i * 3 + 1] as f64;
        mb += linear_f32[i * 3 + 2] as f64;
    }
    mr /= npix as f64;
    mg /= npix as f64;
    mb /= npix as f64;
    let mean_v = (mr + mg + mb) / 3.0;
    let clipped = linear_f32.iter().filter(|&&v| v > 0.95).count();
    let clipped_pct = 100.0 * clipped as f64 / n as f64;
    (mean_v, mr, mg, mb, clipped_pct)
}

struct MobileResult {
    name: String,
    parity_uniform: f64,
    parity_rgb: f64,
    optimal_mult: f32,
    rgb_mult: [f32; 3],
    illuminant_xy: Option<(f32, f32)>,
    make: Option<String>,
    model: Option<String>,
    iso: Option<u32>,
    baseline_exposure: Option<f64>,
    dims: (u32, u32),
    linear_mean: f32,
    regional: RegionalComparison,
    reference_source: &'static str,
}

fn print_summary(results: &[MobileResult]) {
    println!("\n\n=== MOBILE DNG PARITY SUMMARY ===");
    println!(
        "{:<20} {:>8} {:>8} {:>8} {:>25} {:>8} {:>10} {:>10}",
        "Name", "Uniform", "RGB", "Δ", "R,G,B mults", "Mult", "BL Exp", "Ref"
    );
    println!("{}", "-".repeat(110));

    for r in results {
        let delta = r.parity_rgb - r.parity_uniform;
        let rgb_str = format!("{:.3},{:.3},{:.3}", r.rgb_mult[0], r.rgb_mult[1], r.rgb_mult[2]);
        let bl = r.baseline_exposure.map_or("---".to_string(), |v| format!("{v:+.2}"));
        println!(
            "{:<20} {:>8.1} {:>8.1} {:>+8.1} {:>25} {:>8.3} {:>10} {:>10}",
            r.name, r.parity_uniform, r.parity_rgb, delta, rgb_str, r.optimal_mult, bl,
            r.reference_source,
        );
    }

    if !results.is_empty() {
        println!("{}", "-".repeat(110));
        let n = results.len() as f64;
        let mean_u: f64 = results.iter().map(|r| r.parity_uniform).sum::<f64>() / n;
        let mean_rgb: f64 = results.iter().map(|r| r.parity_rgb).sum::<f64>() / n;
        let mean_mult: f32 = results.iter().map(|r| r.optimal_mult).sum::<f32>() / n as f32;
        println!(
            "{:<20} {:>8.1} {:>8.1} {:>+8.1} {:>25} {:>8.3}",
            "MEAN", mean_u, mean_rgb, mean_rgb - mean_u, "", mean_mult,
        );
    }
}

fn save_tsv(results: &[MobileResult]) {
    let tsv_path = format!("{OUTPUT_DIR}/mobile_parity.tsv");
    let mut tsv = String::new();
    tsv.push_str("name\tmake\tmodel\tiso\tbaseline_exp\tdims\tlinear_mean\tparity_uniform\tparity_rgb\topt_mult\trgb_r\trgb_g\trgb_b\tilluminant_x\tilluminant_y\tregional_agg\treference\n");
    for r in results {
        let (ix, iy) = r.illuminant_xy.unwrap_or((-1.0, -1.0));
        tsv.push_str(&format!(
            "{}\t{}\t{}\t{}\t{:.2}\t{}x{}\t{:.4}\t{:.2}\t{:.2}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.4}\t{:.4}\t{:.4}\t{}\n",
            r.name,
            r.make.as_deref().unwrap_or("?"),
            r.model.as_deref().unwrap_or("?"),
            r.iso.unwrap_or(0),
            r.baseline_exposure.unwrap_or(0.0),
            r.dims.0, r.dims.1,
            r.linear_mean,
            r.parity_uniform, r.parity_rgb,
            r.optimal_mult, r.rgb_mult[0], r.rgb_mult[1], r.rgb_mult[2],
            ix, iy,
            r.regional.aggregate,
            r.reference_source,
        ));
    }
    fs::write(&tsv_path, &tsv).unwrap();
    println!("\nResults saved to {tsv_path}");
}

// ── Downscaling (zenresize) ─────────────────────────────────────────────

fn downscale_linear_f32(data: &[f32], w: u32, h: u32, max_dim: u32) -> (Vec<f32>, u32, u32) {
    if w <= max_dim && h <= max_dim {
        return (data.to_vec(), w, h);
    }
    let scale = max_dim as f64 / w.max(h) as f64;
    let nw = ((w as f64 * scale) as u32).max(1);
    let nh = ((h as f64 * scale) as u32).max(1);
    let config = ResizeConfig::builder(w, h, nw, nh)
        .filter(Filter::Lanczos)
        .format(PixelDescriptor::RGBF32_LINEAR)
        .build();
    let mut resizer = Resizer::new(&config);
    (resizer.resize_f32(data), nw, nh)
}

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

fn crop_f32(data: &[f32], w: u32, _h: u32, tw: u32, th: u32) -> Vec<f32> {
    if tw == w { return data[..(tw as usize * th as usize * 3)].to_vec(); }
    let tw = tw.min(w);
    let mut out = vec![0.0f32; (tw as usize) * (th as usize) * 3];
    for y in 0..th as usize {
        let src_off = y * (w as usize) * 3;
        let dst_off = y * (tw as usize) * 3;
        let row = (tw as usize) * 3;
        out[dst_off..dst_off + row].copy_from_slice(&data[src_off..src_off + row]);
    }
    out
}

fn crop_u8(data: &[u8], w: u32, _h: u32, tw: u32, th: u32) -> Vec<u8> {
    if tw == w { return data[..(tw as usize * th as usize * 3)].to_vec(); }
    let tw = tw.min(w);
    let mut out = vec![0u8; (tw as usize) * (th as usize) * 3];
    for y in 0..th as usize {
        let src_off = y * (w as usize) * 3;
        let dst_off = y * (tw as usize) * 3;
        let row = (tw as usize) * 3;
        out[dst_off..dst_off + row].copy_from_slice(&data[src_off..src_off + row]);
    }
    out
}

// ── Darktable rendering ─────────────────────────────────────────────────

fn darktable_render_png(dng_path: &Path, workflow: &str) -> Option<(Vec<u8>, u32, u32)> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_dir = PathBuf::from(format!("/tmp/dt_mobile_{}_{}", std::process::id(), id));
    fs::create_dir_all(&tmp_dir).ok()?;
    let out_path = tmp_dir.join("output.png");

    let status = Command::new("darktable-cli")
        .arg(dng_path)
        .arg(&out_path)
        .arg("--icc-type").arg("SRGB")
        .arg("--apply-custom-presets").arg("false")
        .arg("--core")
        .arg("--library").arg(":memory:")
        .arg("--configdir").arg(tmp_dir.join("dtconf"))
        .arg("--conf").arg(format!("plugins/darkroom/workflow={workflow}"))
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
    Some((rgb8_buf.copy_to_contiguous_bytes(), w, h))
}

// ── dt_sigmoid pipeline ─────────────────────────────────────────────────

fn apply_dt_sigmoid_rgb(linear_f32: &[f32], _w: u32, _h: u32, rgb_mult: [f32; 3]) -> Vec<u8> {
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
    linear_to_srgb_u8(&rgb)
}

fn apply_dt_sigmoid_uniform(linear_f32: &[f32], _w: u32, _h: u32, mult: f32) -> Vec<u8> {
    let params = dt_sigmoid::default_params();
    let mut rgb = linear_f32.to_vec();
    if (mult - 1.0).abs() > 1e-6 {
        for v in rgb.iter_mut() { *v *= mult; }
    }
    dt_sigmoid::apply_dt_sigmoid(&mut rgb, &params);
    linear_to_srgb_u8(&rgb)
}

fn linear_to_srgb_u8(rgb: &[f32]) -> Vec<u8> {
    let mut output = vec![0u8; rgb.len()];
    for (i, &v) in rgb.iter().enumerate() {
        let v = v.clamp(0.0, 1.0);
        let srgb = if v <= 0.003_130_8 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        output[i] = (srgb * 255.0 + 0.5) as u8;
    }
    output
}

// ── Optimization ────────────────────────────────────────────────────────

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
            b = d; d = c; fd = fc;
            c = b - phi * (b - a);
            fc = f(c);
        } else {
            a = c; c = d; fc = fd;
            d = a + phi * (b - a);
            fd = f(d);
        }
        if (b - a).abs() < 0.005 { break; }
    }
    let best = (a + b) / 2.0;
    (best, f(best))
}

fn optimize_dt_sigmoid_exposure(
    linear_f32: &[f32], w: u32, h: u32,
    reference: &[u8], _rw: u32, _rh: u32,
    zs: &Zensim, lo: f32, hi: f32,
) -> (f32, f64) {
    golden_search(
        |mult| {
            let out = apply_dt_sigmoid_uniform(linear_f32, w, h, mult);
            zensim_score(&out, reference, w, h, zs)
        },
        lo, hi,
    )
}

fn optimize_rgb_exposure_range(
    linear_f32: &[f32], w: u32, h: u32,
    reference: &[u8], _rw: u32, _rh: u32,
    zs: &Zensim, uniform_mult: f32, range_lo: f32, range_hi: f32,
) -> ([f32; 3], f64) {
    let mut rgb = [uniform_mult; 3];
    let mut best_score = 0.0f64;

    let eval = |m: [f32; 3]| -> f64 {
        let out = apply_dt_sigmoid_rgb(linear_f32, w, h, m);
        zensim_score(&out, reference, w, h, zs)
    };

    // Per-channel: search ±50% of current value, clamped to overall range
    for _ in 0..3 {
        for ch in 0..3 {
            let lo = (rgb[ch] * 0.5).max(range_lo);
            let hi = (rgb[ch] * 2.0).min(range_hi);
            let (best_val, score) = golden_search(
                |v| { let mut m = rgb; m[ch] = v; eval(m) },
                lo, hi,
            );
            rgb[ch] = best_val;
            best_score = score;
        }
    }
    (rgb, best_score)
}

// ── Resize & comparison helpers ─────────────────────────────────────────

fn resize_pair_rgb8(
    a: &[u8], aw: u32, ah: u32,
    b: &[u8], bw: u32, bh: u32,
) -> (Vec<u8>, Vec<u8>, u32, u32) {
    let (ra, raw, rah) = downscale_rgb8(a, aw, ah, MAX_DIM);
    let (rb, rbw, rbh) = downscale_rgb8(b, bw, bh, MAX_DIM);
    let w = raw.min(rbw);
    let h = rah.min(rbh);
    let ca = crop_u8(&ra, raw, rah, w, h);
    let cb = crop_u8(&rb, rbw, rbh, w, h);
    (ca, cb, w, h)
}

fn zensim_score(a: &[u8], b: &[u8], w: u32, h: u32, zs: &Zensim) -> f64 {
    let expected = w as usize * h as usize * 3;
    if a.len() != expected || b.len() != expected {
        eprintln!("    zensim: buffer mismatch: a={} b={} expected={} ({}x{})",
            a.len(), b.len(), expected, w, h);
        return 0.0;
    }
    let a_rgb: &[[u8; 3]] = bytemuck::cast_slice(a);
    let b_rgb: &[[u8; 3]] = bytemuck::cast_slice(b);
    let sa = RgbSlice::new(a_rgb, w as usize, h as usize);
    let sb = RgbSlice::new(b_rgb, w as usize, h as usize);
    match zs.compute(&sa, &sb) {
        Ok(r) => r.score(),
        Err(e) => { eprintln!("    zensim error: {e}"); 0.0 }
    }
}

fn save_rgb8_jpeg(data: &[u8], w: u32, h: u32, path: &str) {
    let pixels: &[Rgb<u8>] = bytemuck::cast_slice(data);
    let img = ImgVec::new(pixels.to_vec(), w as usize, h as usize);
    match EncodeRequest::new(ImageFormat::Jpeg)
        .with_quality(90.0)
        .encode_rgb8(img.as_ref())
    {
        Ok(encoded) => { let _ = fs::write(path, encoded.data()); }
        Err(e) => eprintln!("    save error: {e}"),
    }
}

fn diff_heatmap(a: &[u8], b: &[u8], w: u32, h: u32) -> Vec<u8> {
    let n = (w as usize) * (h as usize);
    let mut out = vec![0u8; n * 3];
    for i in 0..n {
        let idx = i * 3;
        let dr = (a[idx] as i32 - b[idx] as i32).unsigned_abs();
        let dg = (a[idx + 1] as i32 - b[idx + 1] as i32).unsigned_abs();
        let db = (a[idx + 2] as i32 - b[idx + 2] as i32).unsigned_abs();
        let d = dr.max(dg).max(db).min(255) as u8;
        let v = (d as u32 * 4).min(255) as u8;
        if v < 128 {
            out[idx] = 0; out[idx + 1] = v; out[idx + 2] = v * 2;
        } else {
            let t = v - 128;
            out[idx] = 128 + t; out[idx + 1] = 128 - t; out[idx + 2] = t;
        }
    }
    out
}

fn side_by_side(a: &[u8], b: &[u8], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    let heat = diff_heatmap(a, b, w, h);
    let total_w = w * 3;
    let stride = (w as usize) * 3;
    let stride_out = (total_w as usize) * 3;
    let mut out = vec![0u8; stride_out * (h as usize)];
    for y in 0..h as usize {
        let row_a = &a[y * stride..(y + 1) * stride];
        let row_b = &b[y * stride..(y + 1) * stride];
        let row_h = &heat[y * stride..(y + 1) * stride];
        let row_out = &mut out[y * stride_out..(y + 1) * stride_out];
        row_out[..stride].copy_from_slice(row_a);
        row_out[stride..stride * 2].copy_from_slice(row_b);
        row_out[stride * 2..stride * 3].copy_from_slice(row_h);
    }
    (out, total_w, h)
}

fn regional_compare_srgb(
    a: &[u8], b: &[u8], w: u32, h: u32, m1: &GamutMatrix,
) -> RegionalComparison {
    let mut planes_a = OklabPlanes::new(w, h);
    scatter_srgb_u8_to_oklab(a, &mut planes_a, 3, m1);
    let mut planes_b = OklabPlanes::new(w, h);
    scatter_srgb_u8_to_oklab(b, &mut planes_b, 3, m1);
    let fa = RegionalFeatures::extract(&planes_a);
    let fb = RegionalFeatures::extract(&planes_b);
    RegionalComparison::compare(&fa, &fb)
}

fn print_regional(r: &RegionalComparison) {
    let labels = RegionalComparison::zone_labels();
    let lum: Vec<String> = labels.luminance.iter().zip(r.lum_zone_dist.iter())
        .map(|(l, v)| format!("{l}={v:.3}")).collect();
    let hue: Vec<String> = labels.hue.iter().zip(r.hue_sector_dist.iter())
        .map(|(l, v)| format!("{l}={v:.3}")).collect();
    let chr: Vec<String> = labels.chroma.iter().zip(r.chroma_zone_dist.iter())
        .map(|(l, v)| format!("{l}={v:.3}")).collect();
    println!("  Regional L: {}", lum.join("  "));
    println!("  Regional H: {}", hue.join("  "));
    println!("  Regional C: {}", chr.join("  "));
    println!("  Regional aggregate: {:.4}", r.aggregate);
}
