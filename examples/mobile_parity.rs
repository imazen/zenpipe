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
const MAX_DIM: u32 = 768;

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

    // ── Try Apple ProfileToneCurve ──────────────────────────────────────
    let dng_profile = zenraw::apple::extract_dng_profile(dng_bytes);
    let tone_curve_lut = dng_profile.as_ref()
        .and_then(|p| p.tone_curve.as_deref())
        .and_then(ToneCurveLut::from_profile_tone_curve);

    let mut parity_uniform = 0.0f64;
    let mut parity_rgb = 0.0f64;
    let mut optimal_mult = bl_mult;
    let mut rgb_mult = [bl_mult; 3];
    let best_small;
    let mut method = "dt_sigmoid";

    if let Some(ref lut) = tone_curve_lut {
        println!("  Profile: {:?}", dng_profile.as_ref().and_then(|p| p.name.as_deref()));
        lut.print_diagnostics();

        // ── Luminance-preserving Apple curve with optimized exposure + saturation ──
        // Optimize: [R_mult, G_mult, B_mult, saturation]
        let (lum_mult, lum_score) = golden_search(
            |mult| {
                let out = apply_apple_curve_luminance(&small_linear, [mult; 3], lut, 1.0);
                zensim_score(&out, &small_ref, cw, ch, zs)
            },
            search_lo, search_hi,
        );
        println!("  Apple lum-curve: uniform {lum_mult:.3}x → {lum_score:.1}");

        // Per-channel + saturation optimization on luminance curve
        let mut lum_rgb = [lum_mult; 3];
        let mut lum_sat = 1.0f32;
        let mut lum_best = lum_score;

        for _ in 0..4 {
            // Optimize each RGB channel
            for c in 0..3usize {
                let lo = (lum_rgb[c] * 0.5).max(search_lo);
                let hi = (lum_rgb[c] * 2.0).min(search_hi);
                let (val, score) = golden_search(
                    |v| {
                        let mut m = lum_rgb;
                        m[c] = v;
                        let out = apply_apple_curve_luminance(&small_linear, m, lut, lum_sat);
                        zensim_score(&out, &small_ref, cw, ch, zs)
                    },
                    lo, hi,
                );
                if score > lum_best + 0.05 {
                    lum_rgb[c] = val;
                    lum_best = score;
                }
            }
            // Optimize saturation
            let (val, score) = golden_search(
                |sat| {
                    let out = apply_apple_curve_luminance(&small_linear, lum_rgb, lut, sat);
                    zensim_score(&out, &small_ref, cw, ch, zs)
                },
                0.3, 2.5,
            );
            if score > lum_best + 0.05 {
                lum_sat = val;
                lum_best = score;
            }
        }
        println!("  Apple lum-curve: RGB [{:.3},{:.3},{:.3}] sat={lum_sat:.2} → {lum_best:.1}",
            lum_rgb[0], lum_rgb[1], lum_rgb[2]);

        if lum_best > parity_rgb {
            parity_uniform = lum_score;
            parity_rgb = lum_best;
            optimal_mult = lum_rgb[1]; // G as reference
            rgb_mult = lum_rgb;
            method = "apple_lum";
        }

        // Save diagnostic
        let prefix = format!("{OUTPUT_DIR}/{label}");
        let lum_out = apply_apple_curve_luminance(&small_linear, lum_rgb, lut, lum_sat);
        save_rgb8_jpeg(&lum_out, cw, ch, &format!("{prefix}_apple_lum.jpg"));
    }

    // ── Try basic dt_sigmoid with RGB exposure ─────────────────────────
    let (sig_mult, sig_uniform) = optimize_dt_sigmoid_exposure(
        &small_linear, cw, ch, &small_ref, cw, ch, zs, search_lo, search_hi,
    );
    let (sig_rgb, sig_rgb_score) = optimize_rgb_exposure_range(
        &small_linear, cw, ch, &small_ref, cw, ch, zs, sig_mult, search_lo, search_hi,
    );
    println!("  dt_sigmoid basic: uniform {sig_mult:.3}x → {sig_uniform:.1}, RGB → {sig_rgb_score:.1} ({:.1}s)",
        t0.elapsed().as_secs_f32());

    if sig_rgb_score > parity_rgb {
        parity_uniform = sig_uniform;
        parity_rgb = sig_rgb_score;
        optimal_mult = sig_mult;
        rgb_mult = sig_rgb;
        method = "dt_sigmoid";
    }

    // ── Enhanced pipeline: dt_sigmoid + contrast/skew/saturation tuning ──
    let (enhanced_params, enhanced_score) = optimize_enhanced_pipeline(
        &small_linear, cw, ch, &small_ref, zs, bl_mult, search_lo, search_hi,
    );
    println!("  Enhanced: c={:.2} sk={:.2} sat={:.2} RGB=[{:.2},{:.2},{:.2}] curves={} → {enhanced_score:.1} ({:.1}s)",
        enhanced_params.contrast, enhanced_params.skew, enhanced_params.saturation,
        enhanced_params.rgb_mult[0], enhanced_params.rgb_mult[1], enhanced_params.rgb_mult[2],
        if enhanced_params.curves.is_identity() { "identity" } else { "custom" },
        t0.elapsed().as_secs_f32());
    if !enhanced_params.curves.is_identity() {
        let p = &enhanced_params.curves.points;
        println!("    R curve: [{:.2},{:.2},{:.2},{:.2}]", p[0], p[1], p[2], p[3]);
        println!("    G curve: [{:.2},{:.2},{:.2},{:.2}]", p[4], p[5], p[6], p[7]);
        println!("    B curve: [{:.2},{:.2},{:.2},{:.2}]", p[8], p[9], p[10], p[11]);
    }

    if enhanced_score > parity_rgb {
        parity_uniform = 0.0; // not applicable for enhanced
        parity_rgb = enhanced_score;
        optimal_mult = enhanced_params.rgb_mult[1]; // use G channel as reference
        rgb_mult = enhanced_params.rgb_mult;
        method = "enhanced";
    }

    println!("  BEST method: {method} → parity={parity_rgb:.1} ({:.1}s)",
        t0.elapsed().as_secs_f32());

    // Generate output with best method
    best_small = match method {
        "enhanced" => {
            apply_enhanced_pipeline(&small_linear, cw, ch, &enhanced_params)
        }
        "apple_lum" => {
            let lut = tone_curve_lut.as_ref().unwrap();
            // Re-extract saturation from earlier optimization (stored implicitly)
            apply_apple_curve_luminance(&small_linear, rgb_mult, lut, 1.0)
        }
        _ => {
            apply_dt_sigmoid_rgb(&small_linear, cw, ch, rgb_mult)
        }
    };

    // ── Histogram matching oracle (ceiling estimate) ──────────────────
    let histmatch = histogram_match(&best_small, &small_ref);
    let hm_score = zensim_score(&histmatch, &small_ref, cw, ch, zs);
    println!("  Histogram match ceiling: {hm_score:.1}");
    let prefix = format!("{OUTPUT_DIR}/{label}");
    save_rgb8_jpeg(&histmatch, cw, ch, &format!("{prefix}_histmatch.jpg"));

    // Per-channel error analysis
    print_diff_stats("vs preview", &best_small, &small_ref);

    // Regional comparison
    let regional = regional_compare_srgb(&best_small, &small_ref, cw, ch, m1);
    print_regional(&regional);

    // Save comparison images
    let prefix = format!("{OUTPUT_DIR}/{label}");
    save_rgb8_jpeg(&small_ref, cw, ch, &format!("{prefix}_apple_preview.jpg"));
    save_rgb8_jpeg(&best_small, cw, ch, &format!("{prefix}_our_{method}.jpg"));
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
        reference_source: method,
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

    // Enhanced pipeline
    let mut parity_rgb = parity_rgb;
    let mut rgb_mult = rgb_mult;
    let mut optimal_mult = optimal_mult;
    let mut reference_source = "darktable";
    let (enhanced_params, enhanced_score) = optimize_enhanced_pipeline(
        &small_linear, cw, ch, &small_ref, zs, bl_mult.max(1.0), search_lo, search_hi,
    );
    println!("  Enhanced: c={:.2} sk={:.2} sat={:.2} curves={} → {enhanced_score:.1} ({:.1}s)",
        enhanced_params.contrast, enhanced_params.skew, enhanced_params.saturation,
        if enhanced_params.curves.is_identity() { "identity" } else { "custom" },
        t0.elapsed().as_secs_f32());
    if enhanced_score > parity_rgb {
        parity_rgb = enhanced_score;
        rgb_mult = enhanced_params.rgb_mult;
        optimal_mult = enhanced_params.rgb_mult[1];
        reference_source = "enhanced";
    }

    // Regional comparison
    let best_small = if reference_source == "enhanced" {
        apply_enhanced_pipeline(&small_linear, cw, ch, &enhanced_params)
    } else {
        apply_dt_sigmoid_rgb(&small_linear, cw, ch, rgb_mult)
    };
    print_diff_stats("vs darktable", &best_small, &small_ref);
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

    // Histogram match ceiling
    let histmatch = histogram_match(&best_small, &small_ref);
    let hm_score = zensim_score(&histmatch, &small_ref, cw, ch, zs);
    println!("  Histogram match ceiling: {hm_score:.1}");

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
        reference_source,
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

// ── Apple ProfileToneCurve pipeline ──────────────────────────────────────

/// LUT built from Apple's ProfileToneCurve (257 control points → fast lookup).
struct ToneCurveLut {
    lut: Vec<f32>,
}

impl ToneCurveLut {
    /// Build from raw ProfileToneCurve data (514 floats = 257 x,y pairs).
    fn from_profile_tone_curve(tc: &[f32]) -> Option<Self> {
        let n_points = tc.len() / 2;
        if n_points < 2 { return None; }

        let points: Vec<(f32, f32)> = (0..n_points)
            .map(|i| (tc[i * 2], tc[i * 2 + 1]))
            .collect();

        let lut_size = 4096usize;
        let mut lut = vec![0.0f32; lut_size + 1];
        for i in 0..=lut_size {
            let x = i as f32 / lut_size as f32;
            lut[i] = interpolate_curve(&points, x);
        }
        Some(ToneCurveLut { lut })
    }

    /// Evaluate the curve at x (clamped to [0, 1]).
    fn eval(&self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        let lut_max = (self.lut.len() - 1) as f32;
        let idx_f = x * lut_max;
        let idx = idx_f as usize;
        let frac = idx_f - idx as f32;
        if idx >= self.lut.len() - 1 {
            self.lut[self.lut.len() - 1]
        } else {
            self.lut[idx] * (1.0 - frac) + self.lut[idx + 1] * frac
        }
    }

    /// Print diagnostic info about the curve shape.
    fn print_diagnostics(&self) {
        let mid = self.eval(0.18);
        let half = self.eval(0.5);
        let quarter = self.eval(0.25);
        let max = self.eval(1.0);
        println!("    curve(0.18)={mid:.4} curve(0.25)={quarter:.4} curve(0.50)={half:.4} curve(1.0)={max:.4}");
        // If curve(0.18) > 0.35, the curve likely includes gamma-like encoding
        if mid > 0.35 {
            println!("    → Curve appears to include gamma encoding (skip sRGB)");
        } else {
            println!("    → Curve output is linear (apply sRGB gamma)");
        }
    }
}

fn interpolate_curve(points: &[(f32, f32)], x: f32) -> f32 {
    if x <= points[0].0 { return points[0].1; }
    if x >= points[points.len() - 1].0 { return points[points.len() - 1].1; }
    let idx = points.partition_point(|p| p.0 < x);
    if idx == 0 { return points[0].1; }
    let (x0, y0) = points[idx - 1];
    let (x1, y1) = points[idx];
    let t = if (x1 - x0).abs() < 1e-10 { 0.0 } else { (x - x0) / (x1 - x0) };
    y0 + t * (y1 - y0)
}

/// Apply Apple ProfileToneCurve with per-channel exposure, output sRGB gamma.
fn apply_apple_curve_rgb(
    linear_f32: &[f32], rgb_mult: [f32; 3], lut: &ToneCurveLut, apply_srgb_gamma: bool,
) -> Vec<u8> {
    let n = linear_f32.len();
    let mut output = vec![0u8; n];
    let npix = n / 3;
    for i in 0..npix {
        let base = i * 3;
        for c in 0..3 {
            let v = linear_f32[base + c] * rgb_mult[c];
            let mapped = lut.eval(v);
            let final_v = if apply_srgb_gamma {
                // Curve output is linear → apply sRGB transfer
                let m = mapped.clamp(0.0, 1.0);
                if m <= 0.003_130_8 { m * 12.92 } else { 1.055 * m.powf(1.0 / 2.4) - 0.055 }
            } else {
                // Curve already includes gamma-like encoding
                mapped.clamp(0.0, 1.0)
            };
            output[base + c] = (final_v * 255.0 + 0.5) as u8;
        }
    }
    output
}

fn apply_apple_curve_uniform(
    linear_f32: &[f32], mult: f32, lut: &ToneCurveLut, apply_srgb_gamma: bool,
) -> Vec<u8> {
    apply_apple_curve_rgb(linear_f32, [mult, mult, mult], lut, apply_srgb_gamma)
}

/// Apply Apple curve on luminance, preserving color ratios.
///
/// Instead of applying the curve per-channel (which amplifies color imbalance),
/// compute luminance, apply curve to it, and scale all channels by the same ratio.
fn apply_apple_curve_luminance(
    linear_f32: &[f32], rgb_mult: [f32; 3], lut: &ToneCurveLut,
    saturation: f32,
) -> Vec<u8> {
    let npix = linear_f32.len() / 3;
    let mut output = vec![0u8; linear_f32.len()];
    for i in 0..npix {
        let base = i * 3;
        let r = linear_f32[base] * rgb_mult[0];
        let g = linear_f32[base + 1] * rgb_mult[1];
        let b = linear_f32[base + 2] * rgb_mult[2];

        // Rec.709 luminance
        let lum = (0.2126 * r + 0.7152 * g + 0.0722 * b).max(0.0);

        if lum < 1e-10 {
            output[base] = 0;
            output[base + 1] = 0;
            output[base + 2] = 0;
            continue;
        }

        // Apply curve to luminance
        let mapped_lum = lut.eval(lum);
        let ratio = mapped_lum / lum;

        // Scale channels, with optional saturation adjustment
        let (out_r, out_g, out_b) = if (saturation - 1.0).abs() > 0.01 {
            let mr = mapped_lum + (r * ratio - mapped_lum) * saturation;
            let mg = mapped_lum + (g * ratio - mapped_lum) * saturation;
            let mb = mapped_lum + (b * ratio - mapped_lum) * saturation;
            (mr, mg, mb)
        } else {
            (r * ratio, g * ratio, b * ratio)
        };

        // sRGB OETF
        for (c, v) in [(base, out_r), (base + 1, out_g), (base + 2, out_b)] {
            let v = v.clamp(0.0, 1.0);
            let srgb = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
            output[c] = (srgb * 255.0 + 0.5) as u8;
        }
    }
    output
}

/// Histogram matching: transform per-channel CDF of `source` to match `target`.
/// Returns the mapped u8 image. This is an oracle — shows the ceiling
/// for any per-channel tonal adjustment.
fn histogram_match(source: &[u8], target: &[u8]) -> Vec<u8> {
    assert_eq!(source.len(), target.len());
    let npix = source.len() / 3;

    let mut output = vec![0u8; source.len()];

    for ch in 0..3 {
        // Build histograms
        let mut src_hist = [0u32; 256];
        let mut tgt_hist = [0u32; 256];
        for i in 0..npix {
            src_hist[source[i * 3 + ch] as usize] += 1;
            tgt_hist[target[i * 3 + ch] as usize] += 1;
        }

        // Build CDFs
        let mut src_cdf = [0u32; 256];
        let mut tgt_cdf = [0u32; 256];
        src_cdf[0] = src_hist[0];
        tgt_cdf[0] = tgt_hist[0];
        for i in 1..256 {
            src_cdf[i] = src_cdf[i - 1] + src_hist[i];
            tgt_cdf[i] = tgt_cdf[i - 1] + tgt_hist[i];
        }

        // Build mapping: for each source value, find target value with closest CDF
        let mut mapping = [0u8; 256];
        for s in 0..256 {
            let s_val = src_cdf[s];
            // Binary search in target CDF
            let mut best = 0usize;
            let mut best_diff = u32::MAX;
            for t in 0..256 {
                let diff = (s_val as i64 - tgt_cdf[t] as i64).unsigned_abs() as u32;
                if diff < best_diff {
                    best_diff = diff;
                    best = t;
                }
            }
            mapping[s] = best as u8;
        }

        // Apply mapping
        for i in 0..npix {
            output[i * 3 + ch] = mapping[source[i * 3 + ch] as usize];
        }
    }

    output
}

fn optimize_apple_curve_exposure(
    linear_f32: &[f32], w: u32, h: u32,
    reference: &[u8], zs: &Zensim, lut: &ToneCurveLut,
    lo: f32, hi: f32, apply_srgb_gamma: bool,
) -> (f32, f64) {
    golden_search(
        |mult| {
            let out = apply_apple_curve_uniform(linear_f32, mult, lut, apply_srgb_gamma);
            zensim_score(&out, reference, w, h, zs)
        },
        lo, hi,
    )
}

fn optimize_apple_rgb_exposure(
    linear_f32: &[f32], w: u32, h: u32,
    reference: &[u8], zs: &Zensim, lut: &ToneCurveLut,
    uniform_mult: f32, range_lo: f32, range_hi: f32, apply_srgb_gamma: bool,
) -> ([f32; 3], f64) {
    let mut rgb = [uniform_mult; 3];
    let mut best_score = 0.0f64;

    let eval = |m: [f32; 3]| -> f64 {
        let out = apply_apple_curve_rgb(linear_f32, m, lut, apply_srgb_gamma);
        zensim_score(&out, reference, w, h, zs)
    };

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

// ── Enhanced pipeline: dt_sigmoid + per-channel curves ───────────────────

/// Per-channel correction curve: 4 control points per channel mapping
/// sRGB [0,1] → [0,1]. Control points at x = 0.0, 0.33, 0.67, 1.0.
/// Linear interpolation between points.
#[derive(Clone, Copy, Debug)]
struct ChannelCurves {
    /// [R0, R1, R2, R3, G0, G1, G2, G3, B0, B1, B2, B3]
    /// where Ri is the output value at x = i/3 for the red channel.
    points: [f32; 12],
}

impl ChannelCurves {
    fn identity() -> Self {
        Self {
            // y = x at each control point: 0, 0.333, 0.667, 1.0
            points: [0.0, 0.333, 0.667, 1.0, 0.0, 0.333, 0.667, 1.0, 0.0, 0.333, 0.667, 1.0],
        }
    }

    /// Evaluate channel curve at x ∈ [0, 1].
    fn eval(&self, ch: usize, x: f32) -> f32 {
        let base = ch * 4;
        let x = x.clamp(0.0, 1.0);
        let t = x * 3.0; // scale to [0, 3]
        let seg = (t as usize).min(2); // segment 0, 1, or 2
        let frac = t - seg as f32;
        let y0 = self.points[base + seg];
        let y1 = self.points[base + seg + 1];
        (y0 + (y1 - y0) * frac).clamp(0.0, 1.0)
    }

    fn is_identity(&self) -> bool {
        let id = Self::identity();
        self.points.iter().zip(id.points.iter())
            .all(|(a, b)| (a - b).abs() < 0.005)
    }
}

/// Tunable parameters for the enhanced pipeline.
#[derive(Clone, Copy, Debug)]
struct PipelineParams {
    /// Per-channel exposure multipliers.
    rgb_mult: [f32; 3],
    /// dt_sigmoid contrast (default 1.5).
    contrast: f32,
    /// dt_sigmoid skew (default 0.0).
    skew: f32,
    /// Post-sigmoid saturation multiplier (1.0 = no change).
    saturation: f32,
    /// Per-channel correction curves applied after sRGB encode.
    curves: ChannelCurves,
}

/// Total parameter count: 3 (RGB mult) + 3 (sigmoid) + 12 (curves) = 18
const N_PARAMS: usize = 18;

impl PipelineParams {
    fn from_uniform(mult: f32) -> Self {
        Self {
            rgb_mult: [mult; 3],
            contrast: 1.5,
            skew: 0.0,
            saturation: 1.0,
            curves: ChannelCurves::identity(),
        }
    }

    fn to_array(&self) -> [f32; N_PARAMS] {
        let mut a = [0.0f32; N_PARAMS];
        a[0] = self.rgb_mult[0];
        a[1] = self.rgb_mult[1];
        a[2] = self.rgb_mult[2];
        a[3] = self.contrast;
        a[4] = self.skew;
        a[5] = self.saturation;
        a[6..18].copy_from_slice(&self.curves.points);
        a
    }

    fn from_array(a: &[f32; N_PARAMS]) -> Self {
        let mut points = [0.0f32; 12];
        points.copy_from_slice(&a[6..18]);
        // Ensure monotonicity within each channel
        for ch in 0..3 {
            let base = ch * 4;
            for i in 1..4 {
                points[base + i] = points[base + i].max(points[base + i - 1]);
            }
            // Clamp to [0, 1]
            for i in 0..4 {
                points[base + i] = points[base + i].clamp(0.0, 1.0);
            }
        }
        Self {
            rgb_mult: [a[0].max(0.01), a[1].max(0.01), a[2].max(0.01)],
            contrast: a[3].clamp(0.5, 5.0),
            skew: a[4].clamp(-1.0, 1.0),
            saturation: a[5].clamp(0.0, 3.0),
            curves: ChannelCurves { points },
        }
    }
}

/// Apply enhanced pipeline: exposure → dt_sigmoid → saturation → sRGB → per-channel curves.
fn apply_enhanced_pipeline(linear_f32: &[f32], _w: u32, _h: u32, p: &PipelineParams) -> Vec<u8> {
    let params = dt_sigmoid::compute_params(p.contrast, p.skew, 100.0, 0.0152, 1.0);

    let mut rgb = linear_f32.to_vec();
    let n = rgb.len() / 3;

    // Apply per-channel exposure
    for i in 0..n {
        let base = i * 3;
        rgb[base] *= p.rgb_mult[0];
        rgb[base + 1] *= p.rgb_mult[1];
        rgb[base + 2] *= p.rgb_mult[2];
    }

    // Apply dt_sigmoid tone mapping
    dt_sigmoid::apply_dt_sigmoid(&mut rgb, &params);

    // Apply saturation adjustment in linear RGB
    if (p.saturation - 1.0).abs() > 0.01 {
        for i in 0..n {
            let base = i * 3;
            let r = rgb[base];
            let g = rgb[base + 1];
            let b = rgb[base + 2];
            let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            rgb[base] = lum + (r - lum) * p.saturation;
            rgb[base + 1] = lum + (g - lum) * p.saturation;
            rgb[base + 2] = lum + (b - lum) * p.saturation;
        }
    }

    // Convert to sRGB u8 with per-channel correction curves
    let use_curves = !p.curves.is_identity();
    let mut output = vec![0u8; rgb.len()];
    for i in 0..n {
        let base = i * 3;
        for c in 0..3 {
            let v = rgb[base + c].clamp(0.0, 1.0);
            // sRGB OETF
            let mut srgb = if v <= 0.003_130_8 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            };
            // Per-channel correction curve
            if use_curves {
                srgb = p.curves.eval(c, srgb);
            }
            output[base + c] = (srgb.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
    output
}

/// Nelder-Mead simplex optimizer. Maximizes `f` over N_PARAMS dimensions.
/// Returns (best_params, best_score).
fn nelder_mead(
    f: &dyn Fn(&[f32; N_PARAMS]) -> f64,
    initial: &[f32; N_PARAMS],
    ranges: &[(f32, f32); N_PARAMS],
    max_evals: usize,
) -> ([f32; N_PARAMS], f64) {
    let n = N_PARAMS;
    let mut simplex: Vec<([f32; N_PARAMS], f64)> = Vec::with_capacity(n + 1);

    // Initialize simplex: initial point + perturbations along each axis
    let score = f(initial);
    simplex.push((*initial, score));

    for dim in 0..n {
        let mut point = *initial;
        let range = ranges[dim].1 - ranges[dim].0;
        let step = range * 0.1; // 10% of range as initial step
        point[dim] = (point[dim] + step).min(ranges[dim].1);
        let s = f(&point);
        simplex.push((point, s));
    }

    let alpha = 1.0f32; // reflection
    let gamma_nm = 2.0f32; // expansion
    let rho = 0.5f32; // contraction
    let sigma = 0.5f32; // shrink

    let mut evals = n + 1;

    while evals < max_evals {
        // Sort: highest score first (we're maximizing)
        simplex.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let best_score = simplex[0].1;
        let worst_score = simplex[n].1;
        let second_worst = simplex[n - 1].1;

        // Convergence check
        if (best_score - worst_score).abs() < 0.01 && evals > 50 {
            break;
        }

        // Centroid of all points except worst
        let mut centroid = [0.0f32; N_PARAMS];
        for i in 0..n {
            for d in 0..n {
                centroid[d] += simplex[i].0[d];
            }
        }
        for d in 0..n {
            centroid[d] /= n as f32;
        }

        // Reflection
        let mut reflected = [0.0f32; N_PARAMS];
        for d in 0..n {
            reflected[d] = (centroid[d] + alpha * (centroid[d] - simplex[n].0[d]))
                .clamp(ranges[d].0, ranges[d].1);
        }
        let reflected_score = f(&reflected);
        evals += 1;

        if reflected_score > second_worst && reflected_score <= best_score {
            simplex[n] = (reflected, reflected_score);
            continue;
        }

        if reflected_score > best_score {
            // Expansion
            let mut expanded = [0.0f32; N_PARAMS];
            for d in 0..n {
                expanded[d] = (centroid[d] + gamma_nm * (reflected[d] - centroid[d]))
                    .clamp(ranges[d].0, ranges[d].1);
            }
            let expanded_score = f(&expanded);
            evals += 1;

            if expanded_score > reflected_score {
                simplex[n] = (expanded, expanded_score);
            } else {
                simplex[n] = (reflected, reflected_score);
            }
            continue;
        }

        // Contraction
        let mut contracted = [0.0f32; N_PARAMS];
        let contract_from = if reflected_score > worst_score {
            &reflected
        } else {
            &simplex[n].0
        };
        for d in 0..n {
            contracted[d] = (centroid[d] + rho * (contract_from[d] - centroid[d]))
                .clamp(ranges[d].0, ranges[d].1);
        }
        let contracted_score = f(&contracted);
        evals += 1;

        if contracted_score > worst_score {
            simplex[n] = (contracted, contracted_score);
            continue;
        }

        // Shrink: move all points toward best
        let best = simplex[0].0;
        for i in 1..=n {
            for d in 0..n {
                simplex[i].0[d] = (best[d] + sigma * (simplex[i].0[d] - best[d]))
                    .clamp(ranges[d].0, ranges[d].1);
            }
            simplex[i].1 = f(&simplex[i].0);
            evals += 1;
        }
    }

    simplex.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    (simplex[0].0, simplex[0].1)
}

/// Optimize enhanced pipeline: phase 1 golden search for exposure,
/// phase 2 coordinate descent for sigmoid params,
/// phase 3 Nelder-Mead for full 18-param optimization including per-channel curves.
fn optimize_enhanced_pipeline(
    linear_f32: &[f32], w: u32, h: u32,
    reference: &[u8], zs: &Zensim,
    initial_mult: f32, range_lo: f32, range_hi: f32,
) -> (PipelineParams, f64) {
    let eval = |a: &[f32; N_PARAMS]| -> f64 {
        let p = PipelineParams::from_array(a);
        let out = apply_enhanced_pipeline(linear_f32, w, h, &p);
        zensim_score(&out, reference, w, h, zs)
    };

    // Phase 1: Find best uniform exposure
    let (best_mult, _) = golden_search(
        |mult| {
            let p = PipelineParams::from_uniform(mult);
            let out = apply_enhanced_pipeline(linear_f32, w, h, &p);
            zensim_score(&out, reference, w, h, zs)
        },
        range_lo, range_hi,
    );
    let mut params = PipelineParams::from_uniform(best_mult);

    // Phase 2: Coordinate descent for exposure + sigmoid params (fast, 6 dims)
    let core_ranges: [(f32, f32); 6] = [
        (range_lo, range_hi), (range_lo, range_hi), (range_lo, range_hi),
        (0.5, 4.0), (-0.8, 0.8), (0.3, 2.5),
    ];
    let mut best_score = eval(&params.to_array());
    for _ in 0..4 {
        let mut improved = false;
        for dim in 0..6 {
            let mut arr = params.to_array();
            let lo = if dim < 3 { (arr[dim] * 0.5).max(core_ranges[dim].0) } else { core_ranges[dim].0 };
            let hi = if dim < 3 { (arr[dim] * 2.0).min(core_ranges[dim].1) } else { core_ranges[dim].1 };
            let (val, score) = golden_search(
                |v| { let mut t = arr; t[dim] = v; eval(&t) },
                lo, hi,
            );
            if score > best_score + 0.05 {
                arr[dim] = val;
                params = PipelineParams::from_array(&arr);
                best_score = score;
                improved = true;
            }
        }
        if !improved { break; }
    }

    // Phase 3: Nelder-Mead on all 18 params including per-channel curves
    let mut full_ranges = [(0.0f32, 1.0f32); N_PARAMS];
    for i in 0..3 { full_ranges[i] = (range_lo, range_hi); }
    full_ranges[3] = (0.5, 4.0);   // contrast
    full_ranges[4] = (-0.8, 0.8);  // skew
    full_ranges[5] = (0.3, 2.5);   // saturation
    // Curve points: R0,R1,R2,R3, G0,G1,G2,G3, B0,B1,B2,B3
    for ch in 0..3 {
        full_ranges[6 + ch * 4] = (0.0, 0.15);     // x=0 (shadows)
        full_ranges[6 + ch * 4 + 1] = (0.1, 0.6);  // x=0.33
        full_ranges[6 + ch * 4 + 2] = (0.4, 0.9);  // x=0.67
        full_ranges[6 + ch * 4 + 3] = (0.8, 1.0);  // x=1.0 (highlights)
    }

    let initial = params.to_array();
    let (best_arr, best_nm_score) = nelder_mead(&eval, &initial, &full_ranges, 600);

    if best_nm_score > best_score {
        params = PipelineParams::from_array(&best_arr);
        best_score = best_nm_score;
    }

    (params, best_score)
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

/// Print per-channel error statistics between two sRGB images.
fn print_diff_stats(label: &str, a: &[u8], b: &[u8]) {
    let npix = a.len() / 3;
    if npix == 0 { return; }
    let mut sum_r = 0i64;
    let mut sum_g = 0i64;
    let mut sum_b = 0i64;
    let mut sum_abs_r = 0u64;
    let mut sum_abs_g = 0u64;
    let mut sum_abs_b = 0u64;
    let mut max_err = 0u32;

    for i in 0..npix {
        let idx = i * 3;
        let dr = a[idx] as i64 - b[idx] as i64;
        let dg = a[idx + 1] as i64 - b[idx + 1] as i64;
        let db = a[idx + 2] as i64 - b[idx + 2] as i64;
        sum_r += dr;
        sum_g += dg;
        sum_b += db;
        sum_abs_r += dr.unsigned_abs();
        sum_abs_g += dg.unsigned_abs();
        sum_abs_b += db.unsigned_abs();
        max_err = max_err.max(dr.unsigned_abs() as u32)
            .max(dg.unsigned_abs() as u32)
            .max(db.unsigned_abs() as u32);
    }

    let n = npix as f64;
    println!("  {label} error: bias=[{:+.1},{:+.1},{:+.1}] MAE=[{:.1},{:.1},{:.1}] max={max_err}",
        sum_r as f64 / n, sum_g as f64 / n, sum_b as f64 / n,
        sum_abs_r as f64 / n, sum_abs_g as f64 / n, sum_abs_b as f64 / n);
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
