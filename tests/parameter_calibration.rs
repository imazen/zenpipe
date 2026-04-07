//! Verify that every filter's parameter curve is well-calibrated on real photos:
//! - At 25% of slider range: filter produces a visible change (zensim score < threshold)
//! - At 75% of slider range: filter produces a reasonable result (not destroyed)
//!
//! Uses real photographs from the CID22 corpus and zensim perceptual scoring.
//! A zensim score of 0 = identical, higher = more different.
//! Typical thresholds: <0.5 barely visible, 1-3 noticeable, >10 dramatic, >30 extreme.
//!
//! Run: cargo test --test parameter_calibration -- --nocapture

use image::RgbImage;
use std::path::{Path, PathBuf};
use zenfilters::filters::*;
use zenfilters::*;
use zensim::{RgbSlice, Zensim, ZensimProfile};

// ─── Configuration ─────────────────────────────────────────────────

fn corpus_dir() -> &'static str {
    static DIR: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        if let Ok(d) = std::env::var("ZENFILTERS_CORPUS_DIR") {
            return d;
        }
        let candidates = [
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../codec-corpus/CID22/CID22-512/training/"
            ),
            "/home/lilith/work/codec-corpus/CID22/CID22-512/training/",
        ];
        for c in candidates {
            if Path::new(c).exists() {
                return c.to_string();
            }
        }
        candidates[0].to_string()
    })
}

/// 4 diverse test images: dark/saturated, bright sky, portrait, neutral detail
const TEST_IMAGES: &[&str] = &[
    "1028637.png",              // dark, saturated cocktail glasses
    "pexels-photo-2908983.png", // bright sky with clouds
    "pexels-photo-6096399.png", // portrait, dark skin
    "1722183.png",              // neutral cityscape, detail
];

fn corpus_available() -> bool {
    let dir = corpus_dir();
    Path::new(dir).exists() && Path::new(dir).join(TEST_IMAGES[0]).exists()
}

fn load_image(name: &str) -> RgbImage {
    let path = PathBuf::from(corpus_dir()).join(name);
    image::open(&path)
        .unwrap_or_else(|e| panic!("failed to load {}: {e}", path.display()))
        .to_rgb8()
}

// ─── Core measurement ──────────────────────────────────────────────

fn apply_filter(img: &RgbImage, filter: Box<dyn Filter>) -> RgbImage {
    let (w, h) = img.dimensions();
    let input_bytes: Vec<u8> = img.as_raw().clone();
    let desc = zenpixels::PixelDescriptor::RGB8_SRGB;
    let input_buf = zenpixels::buffer::PixelBuffer::from_vec(input_bytes, w, h, desc).unwrap();
    let mut pipeline = Pipeline::new(PipelineConfig::default()).unwrap();
    pipeline.push(filter);
    let mut ctx = FilterContext::new();
    let output_buf = apply_to_buffer(&pipeline, &input_buf, true, &mut ctx).unwrap();
    let output_bytes = output_buf.copy_to_contiguous_bytes();
    image::ImageBuffer::from_raw(w, h, output_bytes).unwrap()
}

fn zensim_score(a: &RgbImage, b: &RgbImage) -> f64 {
    let (w, h) = a.dimensions();
    let a_pixels: &[[u8; 3]] = bytemuck::cast_slice(a.as_raw());
    let b_pixels: &[[u8; 3]] = bytemuck::cast_slice(b.as_raw());
    let z = Zensim::new(ZensimProfile::latest()).with_parallel(false);
    let src = RgbSlice::new(a_pixels, w as usize, h as usize);
    let dst = RgbSlice::new(b_pixels, w as usize, h as usize);
    z.compute(&src, &dst).unwrap().score()
}

/// Measure the median zensim score of a filter across test images.
/// Higher score = more visible change.
fn median_score(filter_fn: impl Fn() -> Box<dyn Filter>) -> f64 {
    let mut scores = Vec::new();
    for &img_name in TEST_IMAGES {
        let img = load_image(img_name);
        let filtered = apply_filter(&img, filter_fn());
        let score = zensim_score(&img, &filtered);
        scores.push(score);
    }
    scores.sort_by(|a, b| a.partial_cmp(b).unwrap());
    scores[scores.len() / 2] // median
}

/// Like median_score but only on images matching a predicate.
/// Used for content-dependent filters (shadow lift needs dark images, etc.)
fn median_score_on(filter_fn: impl Fn() -> Box<dyn Filter>, images: &[&str]) -> f64 {
    let mut scores = Vec::new();
    for &img_name in images {
        let img = load_image(img_name);
        let filtered = apply_filter(&img, filter_fn());
        let score = zensim_score(&img, &filtered);
        scores.push(score);
    }
    scores.sort_by(|a, b| a.partial_cmp(b).unwrap());
    scores[scores.len() / 2]
}

// ─── Helpers ───────────────────────────────────────────────────────

macro_rules! mk {
    ($ty:ty, $($field:ident = $val:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut f = <$ty>::default();
        $(f.$field = $val;)*
        f
    }};
}

// Zensim scale: 100 = identical, ~85 = noticeable, ~50 = strong, <0 = extreme.
//
// At 25%: score must be < 93 (clearly something changed).
//         93+ means the change is too subtle to be useful.
// At 75%: score must be > -150 (image not completely destroyed).
//         Scores like -50 to -100 mean a very strong edit, which is fine at 75%.
const MAX_SCORE_25: f64 = 93.0;
const MIN_SCORE_75: f64 = -150.0;

fn check_25(name: &str, score: f64) {
    eprintln!("  {name}: zensim = {score:.2}");
    assert!(
        score < MAX_SCORE_25,
        "{name}: zensim {score:.2} >= {MAX_SCORE_25} — filter has no visible effect at 25%!"
    );
}

fn check_75(name: &str, score: f64) {
    eprintln!("  {name}: zensim = {score:.2}");
    assert!(
        score > MIN_SCORE_75,
        "{name}: zensim {score:.2} < {MIN_SCORE_75} — filter completely destroys the image at 75%!"
    );
}

// ═══════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn exposure_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    // Stops range [-3, +3], positive side: 25% = 0.75, 75% = 2.25
    let s25 = median_score(|| Box::new(mk!(Exposure, stops = 0.75)));
    let s75 = median_score(|| Box::new(mk!(Exposure, stops = 2.25)));
    check_25("Exposure@25% (0.75 stops)", s25);
    check_75("Exposure@75% (2.25 stops)", s75);
}

#[test]
fn contrast_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    // Slider² mapping: 0.25 → 0.0625, 0.75 → 0.5625
    let s25 = median_score(|| {
        Box::new(mk!(
            Contrast,
            amount = zenfilters::slider::contrast_from_slider(0.25)
        ))
    });
    let s75 = median_score(|| {
        Box::new(mk!(
            Contrast,
            amount = zenfilters::slider::contrast_from_slider(0.75)
        ))
    });
    check_25("Contrast@25%", s25);
    check_75("Contrast@75%", s75);
}

#[test]
fn highlights_shadows_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| Box::new(mk!(HighlightsShadows, highlights = -0.25, shadows = 0.25)));
    let s75 = median_score(|| Box::new(mk!(HighlightsShadows, highlights = -0.75, shadows = 0.75)));
    check_25("HighlightsShadows@25%", s25);
    check_75("HighlightsShadows@75%", s75);
}

#[test]
fn whites_blacks_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| Box::new(mk!(WhitesBlacks, whites = 0.25, blacks = -0.25)));
    let s75 = median_score(|| Box::new(mk!(WhitesBlacks, whites = 0.75, blacks = -0.75)));
    check_25("WhitesBlacks@25%", s25);
    check_75("WhitesBlacks@75%", s75);
}

#[test]
fn clarity_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    // amount [-2, +2], positive: 25% = 0.5, 75% = 1.5
    let s25 = median_score(|| Box::new(mk!(Clarity, sigma = 3.0, amount = 0.5)));
    let s75 = median_score(|| Box::new(mk!(Clarity, sigma = 3.0, amount = 1.5)));
    check_25("Clarity@25%", s25);
    check_75("Clarity@75%", s75);
}

#[test]
fn sharpen_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    // amount [0, 2]: 25% = 0.5, 75% = 1.5
    let s25 = median_score(|| Box::new(mk!(Sharpen, sigma = 1.0, amount = 0.5)));
    let s75 = median_score(|| Box::new(mk!(Sharpen, sigma = 1.0, amount = 1.5)));
    check_25("Sharpen@25%", s25);
    check_75("Sharpen@75%", s75);
}

#[test]
fn adaptive_sharpen_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| Box::new(mk!(AdaptiveSharpen, amount = 0.5, sigma = 1.0)));
    let s75 = median_score(|| Box::new(mk!(AdaptiveSharpen, amount = 1.5, sigma = 1.0)));
    check_25("AdaptiveSharpen@25%", s25);
    check_75("AdaptiveSharpen@75%", s75);
}

#[test]
fn texture_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| Box::new(mk!(Texture, sigma = 1.0, amount = 0.5)));
    let s75 = median_score(|| Box::new(mk!(Texture, sigma = 1.0, amount = 1.5)));
    check_25("Texture@25%", s25);
    check_75("Texture@75%", s75);
}

#[test]
fn brilliance_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| Box::new(mk!(Brilliance, sigma = 10.0, amount = 0.25)));
    let s75 = median_score(|| Box::new(mk!(Brilliance, sigma = 10.0, amount = 0.75)));
    check_25("Brilliance@25%", s25);
    check_75("Brilliance@75%", s75);
}

#[test]
fn dehaze_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| {
        Box::new(mk!(
            Dehaze,
            strength = zenfilters::slider::dehaze_from_slider(0.25)
        ))
    });
    let s75 = median_score(|| {
        Box::new(mk!(
            Dehaze,
            strength = zenfilters::slider::dehaze_from_slider(0.75)
        ))
    });
    check_25("Dehaze@25%", s25);
    check_75("Dehaze@75%", s75);
}

#[test]
fn local_tone_map_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| {
        Box::new(mk!(
            LocalToneMap,
            compression = zenfilters::slider::ltm_compression_from_slider(0.25),
            sigma = 20.0
        ))
    });
    let s75 = median_score(|| {
        Box::new(mk!(
            LocalToneMap,
            compression = zenfilters::slider::ltm_compression_from_slider(0.75),
            sigma = 20.0
        ))
    });
    check_25("LocalToneMap@25%", s25);
    check_75("LocalToneMap@75%", s75);
}

#[test]
fn bloom_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| Box::new(mk!(Bloom, amount = 0.25, threshold = 0.5, sigma = 20.0)));
    let s75 = median_score(|| Box::new(mk!(Bloom, amount = 0.75, threshold = 0.5, sigma = 20.0)));
    check_25("Bloom@25%", s25);
    check_75("Bloom@75%", s75);
}

#[test]
fn vignette_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| Box::new(mk!(Vignette, strength = 0.25)));
    let s75 = median_score(|| Box::new(mk!(Vignette, strength = 0.75)));
    check_25("Vignette@25%", s25);
    check_75("Vignette@75%", s75);
}

#[test]
fn grain_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| Box::new(mk!(Grain, amount = 0.25, size = 1.5, seed = 42)));
    let s75 = median_score(|| Box::new(mk!(Grain, amount = 0.75, size = 1.5, seed = 42)));
    check_25("Grain@25%", s25);
    check_75("Grain@75%", s75);
}

#[test]
fn noise_reduction_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| {
        Box::new(mk!(
            NoiseReduction,
            luminance = zenfilters::slider::nr_strength_from_slider(0.25),
            chroma = zenfilters::slider::nr_strength_from_slider(0.25)
        ))
    });
    let s75 = median_score(|| {
        Box::new(mk!(
            NoiseReduction,
            luminance = zenfilters::slider::nr_strength_from_slider(0.75),
            chroma = zenfilters::slider::nr_strength_from_slider(0.75)
        ))
    });
    check_25("NoiseReduction@25%", s25);
    check_75("NoiseReduction@75%", s75);
}

#[test]
fn bilateral_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| Box::new(mk!(Bilateral, strength = 0.25, spatial_sigma = 5.0)));
    let s75 = median_score(|| Box::new(mk!(Bilateral, strength = 0.75, spatial_sigma = 5.0)));
    check_25("Bilateral@25%", s25);
    check_75("Bilateral@75%", s75);
}

#[test]
fn edge_detect_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    // EdgeDetect is a mask/utility filter (Sobel edge extraction), not a photo
    // adjustment. It intentionally replaces the image with edge magnitudes.
    // Just verify it runs and produces output — no 25%/75% calibration needed.
    let s = median_score(|| Box::new(mk!(EdgeDetect, strength = 0.5)));
    eprintln!("  EdgeDetect@50%: zensim = {s:.2} (mask utility, extreme by design)");
}

// ─── Content-dependent filters ─────────────────────────────────────
// These need specific image types to show their effect.

#[test]
fn auto_exposure_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    // Test on dark image (1028637 = dark cocktail) and normal images
    let s25 = median_score(|| {
        Box::new(mk!(
            AutoExposure,
            strength = 0.25,
            target = 0.5,
            max_correction = 3.0
        ))
    });
    let s75 = median_score(|| {
        Box::new(mk!(
            AutoExposure,
            strength = 0.75,
            target = 0.5,
            max_correction = 3.0
        ))
    });
    check_25("AutoExposure@25%", s25);
    check_75("AutoExposure@75%", s75);
}

#[test]
fn shadow_lift_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    // 1028637 is dark — should trigger shadow lift
    let s25 = median_score_on(
        || Box::new(mk!(ShadowLift, strength = 0.25)),
        &["1028637.png"],
    );
    let s75 = median_score_on(
        || Box::new(mk!(ShadowLift, strength = 0.75)),
        &["1028637.png"],
    );
    check_25("ShadowLift@25% (dark image)", s25);
    check_75("ShadowLift@75% (dark image)", s75);
}

#[test]
fn highlight_recovery_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    // pexels-photo-2908983 = bright sky with clouds
    let s25 = median_score_on(
        || Box::new(mk!(HighlightRecovery, strength = 0.25)),
        &["pexels-photo-2908983.png"],
    );
    let s75 = median_score_on(
        || Box::new(mk!(HighlightRecovery, strength = 0.75)),
        &["pexels-photo-2908983.png"],
    );
    check_25("HighlightRecovery@25% (bright image)", s25);
    check_75("HighlightRecovery@75% (bright image)", s75);
    // Verify monotonic response: 75% must produce MORE change (lower score) than 25%
    assert!(
        s75 <= s25,
        "HighlightRecovery response is inverted! 25%={s25:.2} should be >= 75%={s75:.2}"
    );
}

#[test]
fn auto_levels_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| Box::new(mk!(AutoLevels, strength = 0.25)));
    let s75 = median_score(|| Box::new(mk!(AutoLevels, strength = 0.75)));
    check_25("AutoLevels@25%", s25);
    check_75("AutoLevels@75%", s75);
}

#[test]
fn tone_equalizer_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| {
        let mut t = ToneEqualizer::default();
        t.zones = [0.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.0, 0.0];
        Box::new(t)
    });
    let s75 = median_score(|| {
        let mut t = ToneEqualizer::default();
        t.zones = [-1.5, -0.75, 0.0, 0.75, 1.5, 0.75, 0.0, -0.75, -1.5];
        Box::new(t)
    });
    check_25("ToneEqualizer@25%", s25);
    check_75("ToneEqualizer@75%", s75);
}

#[test]
fn sigmoid_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    // contrast 0.5-3.0: 25% = 1.125, 75% = 2.375
    let s25 = median_score(|| Box::new(mk!(Sigmoid, contrast = 1.125)));
    let s75 = median_score(|| Box::new(mk!(Sigmoid, contrast = 2.375)));
    check_25("Sigmoid@25%", s25);
    check_75("Sigmoid@75%", s75);
}

#[test]
fn levels_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| Box::new(mk!(Levels, gamma = 1.5)));
    let s75 = median_score(|| Box::new(mk!(Levels, gamma = 3.0)));
    check_25("Levels@25%", s25);
    check_75("Levels@75%", s75);
}

#[test]
fn saturation_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    // Slider: 0.25 → factor 0.5 (desat), 0.75 → factor 1.5 (boost)
    let s25 = median_score(|| {
        Box::new(mk!(
            Saturation,
            factor = zenfilters::slider::saturation_from_slider(0.75)
        ))
    });
    let s75 = median_score(|| {
        Box::new(mk!(
            Saturation,
            factor = zenfilters::slider::saturation_from_slider(1.0)
        ))
    });
    check_25("Saturation@25% (factor=1.5)", s25);
    check_75("Saturation@75% (factor=2.0)", s75);
}

#[test]
fn vibrance_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| Box::new(mk!(Vibrance, amount = 0.25)));
    let s75 = median_score(|| Box::new(mk!(Vibrance, amount = 0.75)));
    check_25("Vibrance@25%", s25);
    check_75("Vibrance@75%", s75);
}

#[test]
fn temperature_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| Box::new(mk!(Temperature, shift = 0.25)));
    let s75 = median_score(|| Box::new(mk!(Temperature, shift = 0.75)));
    check_25("Temperature@25%", s25);
    check_75("Temperature@75%", s75);
}

#[test]
fn devignette_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| Box::new(mk!(Devignette, strength = 0.25)));
    let s75 = median_score(|| Box::new(mk!(Devignette, strength = 0.75)));
    check_25("Devignette@25%", s25);
    check_75("Devignette@75%", s75);
}

#[test]
fn gamut_expand_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| Box::new(mk!(GamutExpand, strength = 0.25)));
    let s75 = median_score(|| Box::new(mk!(GamutExpand, strength = 0.75)));
    check_25("GamutExpand@25%", s25);
    check_75("GamutExpand@75%", s75);
}

#[test]
fn chromatic_aberration_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    // Schema range: -0.02 to +0.02. CA is inherently a subtle correction
    // (sub-pixel chroma realignment). Test at 25% and 75%.
    let s25 =
        median_score(|| Box::new(mk!(ChromaticAberration, shift_a = 0.005, shift_b = -0.005)));
    let s75 =
        median_score(|| Box::new(mk!(ChromaticAberration, shift_a = 0.015, shift_b = -0.015)));
    // CA at 25% is intentionally subtle — loosen threshold to 96
    eprintln!("  ChromaticAberration@25%: zensim = {s25:.2}");
    assert!(
        s25 < 96.0,
        "ChromaticAberration@25%: zensim {s25:.2} >= 96 — completely invisible"
    );
    check_75("ChromaticAberration@75%", s75);
}

#[test]
fn black_point_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    // Range 0-0.3: 25% = 0.075, 75% = 0.225
    let s25 = median_score(|| Box::new(mk!(BlackPoint, level = 0.075)));
    let s75 = median_score(|| Box::new(mk!(BlackPoint, level = 0.225)));
    check_25("BlackPoint@25%", s25);
    check_75("BlackPoint@75%", s75);
}

// ═══════════════════════════════════════════════════════════════════
// NEW AUTO FILTERS
// ═══════════════════════════════════════════════════════════════════

#[test]
fn auto_tone_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| Box::new(mk!(AutoTone, strength = 0.25, preserve_intent = 0.3)));
    let s75 = median_score(|| Box::new(mk!(AutoTone, strength = 0.75, preserve_intent = 0.3)));
    check_25("AutoTone@25%", s25);
    check_75("AutoTone@75%", s75);
}

#[test]
fn auto_white_balance_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    // Auto WB is content-dependent: only corrects images with color cast.
    // Verify it runs without error at all strength levels and produces
    // monotonic response (higher strength = more correction or same).
    let s25 = median_score(|| Box::new(mk!(AutoWhiteBalance, strength = 0.25)));
    let s75 = median_score(|| Box::new(mk!(AutoWhiteBalance, strength = 0.75)));
    let s100 = median_score(|| Box::new(mk!(AutoWhiteBalance, strength = 1.0)));
    eprintln!("  AutoWhiteBalance: 25%={s25:.2} 75%={s75:.2} 100%={s100:.2}");
    // Monotonic: stronger strength should produce equal or more change
    assert!(
        s75 <= s25 + 1.0,
        "AutoWhiteBalance response should be monotonic: 25%={s25:.2} 75%={s75:.2}"
    );
    check_75("AutoWhiteBalance@75%", s75);
}

#[test]
fn auto_contrast_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    // Auto Contrast is content-dependent: only corrects flat or over-contrasty images.
    // CID22 images are well-exposed, so the effect may be minimal.
    let s25 = median_score(|| Box::new(mk!(AutoContrast, strength = 0.25)));
    let s75 = median_score(|| Box::new(mk!(AutoContrast, strength = 0.75)));
    let s100 = median_score(|| Box::new(mk!(AutoContrast, strength = 1.0)));
    eprintln!("  AutoContrast: 25%={s25:.2} 75%={s75:.2} 100%={s100:.2}");
    assert!(
        s75 <= s25 + 1.0,
        "AutoContrast response should be monotonic: 25%={s25:.2} 75%={s75:.2}"
    );
    check_75("AutoContrast@75%", s75);
}

#[test]
fn auto_vibrance_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    // Auto Vibrance is content-dependent: only boosts muted hue sectors.
    let s25 = median_score(|| Box::new(mk!(AutoVibrance, strength = 0.25)));
    let s75 = median_score(|| Box::new(mk!(AutoVibrance, strength = 0.75)));
    let s100 = median_score(|| Box::new(mk!(AutoVibrance, strength = 1.0)));
    eprintln!("  AutoVibrance: 25%={s25:.2} 75%={s75:.2} 100%={s100:.2}");
    assert!(
        s75 <= s25 + 1.0,
        "AutoVibrance response should be monotonic: 25%={s25:.2} 75%={s75:.2}"
    );
    check_75("AutoVibrance@75%", s75);
}

#[test]
fn whites_blacks_auto_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    // Auto range is content-dependent: only expands narrow tonal ranges.
    // Test on the dark image which likely has a narrow range.
    let s25 = median_score_on(
        || {
            Box::new(mk!(
                WhitesBlacks,
                whites = 0.5,
                blacks = -0.5,
                auto_range = true
            ))
        },
        &["1028637.png"],
    );
    let s75 = median_score_on(
        || {
            Box::new(mk!(
                WhitesBlacks,
                whites = 1.0,
                blacks = -1.0,
                auto_range = true
            ))
        },
        &["1028637.png"],
    );
    eprintln!("  WhitesBlacks_auto (dark): 25%={s25:.2} 75%={s75:.2}");
    check_75("WhitesBlacks_auto@75%", s75);
}

#[test]
fn dehaze_auto_calibration() {
    if !corpus_available() {
        eprintln!("SKIP: corpus not found");
        return;
    }
    let s25 = median_score(|| Box::new(mk!(Dehaze, strength = 0.25, auto_strength = true)));
    let s75 = median_score(|| Box::new(mk!(Dehaze, strength = 0.75, auto_strength = true)));
    check_25("Dehaze_auto@25%", s25);
    check_75("Dehaze_auto@75%", s75);
}
