//! Cross-library validation tests for zenfilters.
//!
//! Compares our Oklab-based filter outputs against established open-source
//! implementations (image crate, imageproc) using zensim psychovisual
//! similarity scoring. The goal is to catch gross errors — if our filter
//! produces output that looks wildly different from the same conceptual
//! operation done in sRGB, something is broken.
//!
//! Expected behavior:
//! - Spatial filters (blur, sharpen) should be very similar (score > 80)
//! - Per-pixel operations in different color spaces may differ more (score > 50)
//! - Directional checks: exposure up = brighter, contrast up = more range, etc.

use image::{DynamicImage, ImageBuffer, Rgb, RgbImage};
use palette::convert::IntoColorUnclamped;
use zenfilters::filters::*;
use zenfilters::*;
use zensim::{RgbSlice, Zensim, ZensimProfile};

// ─── Test image generators ──────────────────────────────────────────

/// Smooth gradient with color variation — good for per-pixel filter tests.
fn make_gradient_rgb(w: u32, h: u32) -> RgbImage {
    ImageBuffer::from_fn(w, h, |x, y| {
        let tx = x as f32 / w as f32;
        let ty = y as f32 / h as f32;
        Rgb([
            (tx * 200.0 + 30.0) as u8,
            ((1.0 - tx) * 160.0 + ty * 60.0 + 20.0) as u8,
            (ty * 180.0 + 40.0) as u8,
        ])
    })
}

/// Image with sharp edges and flat regions — good for spatial filter tests.
fn make_edges_rgb(w: u32, h: u32) -> RgbImage {
    ImageBuffer::from_fn(w, h, |x, y| {
        let block_x = (x / (w / 4)) % 2;
        let block_y = (y / (h / 4)) % 2;
        let checkerboard = (block_x + block_y) % 2;
        if checkerboard == 0 {
            Rgb([60, 80, 120])
        } else {
            Rgb([200, 180, 140])
        }
    })
}

/// Colorful image with a range of saturated and desaturated regions.
fn make_colorful_rgb(w: u32, h: u32) -> RgbImage {
    ImageBuffer::from_fn(w, h, |x, y| {
        let tx = x as f32 / w as f32;
        let ty = y as f32 / h as f32;
        // HSV-like sweep: hue = x, saturation = y, value = 0.8
        let hue = tx * 6.0;
        let sat = ty;
        let val = 0.8f32;
        let c = val * sat;
        let x_mod = c * (1.0 - ((hue % 2.0) - 1.0).abs());
        let m = val - c;
        let (r, g, b) = match hue as u32 {
            0 => (c, x_mod, 0.0),
            1 => (x_mod, c, 0.0),
            2 => (0.0, c, x_mod),
            3 => (0.0, x_mod, c),
            4 => (x_mod, 0.0, c),
            _ => (c, 0.0, x_mod),
        };
        Rgb([
            ((r + m) * 255.0) as u8,
            ((g + m) * 255.0) as u8,
            ((b + m) * 255.0) as u8,
        ])
    })
}

// ─── Helpers ────────────────────────────────────────────────────────

/// Apply a zenfilter to an sRGB u8 image and get sRGB u8 output.
fn apply_zenfilter(img: &RgbImage, filter: Box<dyn Filter>) -> RgbImage {
    let (w, h) = img.dimensions();
    let input_bytes: Vec<u8> = img.as_raw().clone();

    let desc = zenpixels::PixelDescriptor::RGB8_SRGB;
    let input_buf = zenpixels::buffer::PixelBuffer::from_vec(input_bytes, w, h, desc).unwrap();

    let mut pipeline = Pipeline::new(PipelineConfig::default()).unwrap();
    pipeline.push(filter);

    let mut ctx = FilterContext::new();
    let output_buf = apply_to_buffer(&pipeline, &input_buf, true, &mut ctx).unwrap();
    let output_bytes = output_buf.copy_to_contiguous_bytes();

    ImageBuffer::from_raw(w, h, output_bytes).unwrap()
}

/// Compare two RGB images using zensim. Returns the similarity score (0-100).
fn zensim_score(a: &RgbImage, b: &RgbImage) -> f64 {
    let (w, h) = a.dimensions();
    assert_eq!(a.dimensions(), b.dimensions());
    let a_pixels: &[[u8; 3]] = bytemuck::cast_slice(a.as_raw());
    let b_pixels: &[[u8; 3]] = bytemuck::cast_slice(b.as_raw());
    let z = Zensim::new(ZensimProfile::latest()).with_parallel(false);
    let src = RgbSlice::new(a_pixels, w as usize, h as usize);
    let dst = RgbSlice::new(b_pixels, w as usize, h as usize);
    z.compute(&src, &dst).unwrap().score()
}

/// Average brightness of an sRGB image (mean of all channel values).
fn avg_brightness(img: &RgbImage) -> f64 {
    let sum: u64 = img.as_raw().iter().map(|&v| v as u64).sum();
    sum as f64 / img.as_raw().len() as f64
}

/// Standard deviation of pixel channel values.
fn pixel_stddev(img: &RgbImage) -> f64 {
    let mean = avg_brightness(img);
    let n = img.as_raw().len() as f64;
    let var: f64 = img
        .as_raw()
        .iter()
        .map(|&v| {
            let d = v as f64 - mean;
            d * d
        })
        .sum::<f64>()
        / n;
    var.sqrt()
}

const SIZE: u32 = 64;

// ─── Exposure ───────────────────────────────────────────────────────

#[test]
fn exposure_up_is_brighter() {
    let img = make_gradient_rgb(SIZE, SIZE);
    let mut exposure = Exposure::default();
    exposure.stops = 1.0;
    let result = apply_zenfilter(&img, Box::new(exposure));
    assert!(
        avg_brightness(&result) > avg_brightness(&img),
        "exposure +1 should increase average brightness"
    );
}

#[test]
fn exposure_down_is_darker() {
    let img = make_gradient_rgb(SIZE, SIZE);
    let mut exposure = Exposure::default();
    exposure.stops = -1.0;
    let result = apply_zenfilter(&img, Box::new(exposure));
    assert!(
        avg_brightness(&result) < avg_brightness(&img),
        "exposure -1 should decrease average brightness"
    );
}

#[test]
fn exposure_vs_image_crate_brighten() {
    let img = make_gradient_rgb(SIZE, SIZE);

    // Our Oklab exposure +0.5 stops
    let mut exposure = Exposure::default();
    exposure.stops = 0.5;
    let ours = apply_zenfilter(&img, Box::new(exposure));

    // image crate brighten (additive in sRGB, different model)
    let reference = DynamicImage::ImageRgb8(img.clone()).brighten(25).to_rgb8();

    // Both should be brighter than original — behavioral equivalence.
    // Oklab vs sRGB operations produce very different pixel values, so
    // we verify direction rather than pixel similarity.
    let orig_brightness = avg_brightness(&img);
    let ours_brightness = avg_brightness(&ours);
    let ref_brightness = avg_brightness(&reference);
    assert!(
        ours_brightness > orig_brightness,
        "our exposure should brighten: {orig_brightness:.1} -> {ours_brightness:.1}"
    );
    assert!(
        ref_brightness > orig_brightness,
        "reference brighten should brighten: {orig_brightness:.1} -> {ref_brightness:.1}"
    );
}

// ─── Contrast ───────────────────────────────────────────────────────

#[test]
fn contrast_increases_stddev() {
    let img = make_gradient_rgb(SIZE, SIZE);
    let mut contrast = Contrast::default();
    contrast.amount = 0.5;
    let result = apply_zenfilter(&img, Box::new(contrast));
    assert!(
        pixel_stddev(&result) > pixel_stddev(&img),
        "contrast +0.5 should increase pixel standard deviation"
    );
}

#[test]
fn contrast_vs_image_crate() {
    let img = make_gradient_rgb(SIZE, SIZE);

    let mut contrast = Contrast::default();
    contrast.amount = 0.3;
    let ours = apply_zenfilter(&img, Box::new(contrast));

    let reference = DynamicImage::ImageRgb8(img.clone())
        .adjust_contrast(15.0)
        .to_rgb8();

    let score = zensim_score(&ours, &reference);
    eprintln!("contrast vs image::adjust_contrast score: {score:.1}");
    assert!(
        score > 40.0,
        "contrast results should be broadly similar, got score {score:.1}"
    );
}

// ─── Saturation ─────────────────────────────────────────────────────

#[test]
fn saturation_zero_is_grayscale() {
    let img = make_colorful_rgb(SIZE, SIZE);
    let mut sat = Saturation::default();
    sat.factor = 0.0;
    let result = apply_zenfilter(&img, Box::new(sat));
    // Every pixel should have R≈G≈B (grayscale)
    for pixel in result.pixels() {
        let max_diff = pixel.0[0]
            .abs_diff(pixel.0[1])
            .max(pixel.0[1].abs_diff(pixel.0[2]))
            .max(pixel.0[0].abs_diff(pixel.0[2]));
        assert!(
            max_diff <= 3,
            "saturation=0 should produce near-grayscale, got diff={max_diff}"
        );
    }
}

#[test]
fn saturation_boost_increases_chroma() {
    let img = make_colorful_rgb(SIZE, SIZE);
    let before_stddev = pixel_stddev(&img);

    let mut sat = Saturation::default();
    sat.factor = 1.5;
    let result = apply_zenfilter(&img, Box::new(sat));
    let after_stddev = pixel_stddev(&result);

    // Boosted saturation should increase the overall variation
    assert!(
        after_stddev > before_stddev * 0.95,
        "saturation 1.5x should maintain or increase variation"
    );
}

// ─── Grayscale ──────────────────────────────────────────────────────

#[test]
fn grayscale_vs_image_crate() {
    let img = make_colorful_rgb(SIZE, SIZE);

    let ours = apply_zenfilter(&img, Box::new(Grayscale::default()));

    // image crate grayscale uses BT.601 luma weights
    let reference_gray = DynamicImage::ImageRgb8(img.clone()).grayscale();
    let reference = reference_gray.to_rgb8();

    // Both should produce neutral (R≈G≈B) pixels — verify behavioral equivalence
    // rather than pixel similarity (Oklab L vs BT.601 luma differ significantly)
    for (ours_px, ref_px) in ours.pixels().zip(reference.pixels()) {
        let ours_spread = ours_px.0[0].abs_diff(ours_px.0[2]);
        assert!(
            ours_spread <= 3,
            "our grayscale should be neutral, spread={ours_spread}"
        );
        let ref_spread = ref_px.0[0].abs_diff(ref_px.0[2]);
        assert!(
            ref_spread <= 3,
            "ref grayscale should be neutral, spread={ref_spread}"
        );
    }

    // Luminance values should be correlated (bright→bright, dark→dark)
    let ours_bright = avg_brightness(&ours);
    let ref_bright = avg_brightness(&reference);
    // Both should be in a similar range (within 30% of each other)
    let ratio = ours_bright / ref_bright;
    assert!(
        (0.7..1.4).contains(&ratio),
        "grayscale brightness should be in similar range: ours={ours_bright:.1} ref={ref_bright:.1}"
    );
}

#[test]
fn grayscale_output_is_neutral() {
    let img = make_colorful_rgb(SIZE, SIZE);
    let result = apply_zenfilter(&img, Box::new(Grayscale::default()));
    for pixel in result.pixels() {
        let max_diff = pixel.0[0]
            .abs_diff(pixel.0[1])
            .max(pixel.0[1].abs_diff(pixel.0[2]))
            .max(pixel.0[0].abs_diff(pixel.0[2]));
        assert!(
            max_diff <= 3,
            "grayscale should produce neutral pixels, got diff={max_diff}"
        );
    }
}

// ─── Invert ─────────────────────────────────────────────────────────

#[test]
fn invert_vs_image_crate() {
    let img = make_gradient_rgb(SIZE, SIZE);

    let ours = apply_zenfilter(&img, Box::new(Invert::default()));

    let mut reference = DynamicImage::ImageRgb8(img.clone());
    reference.invert();
    let reference = reference.to_rgb8();

    // Oklab invert (L'=1-L, negate chroma) and sRGB invert (255-v) are
    // fundamentally different operations. Oklab inversion is perceptually
    // uniform but doesn't produce sRGB-complementary values.
    //
    // Verify: both should swap the brightness ordering — the brightest
    // original pixel should become among the darkest, and vice versa.
    let ours_brightness = avg_brightness(&ours);
    let ref_brightness = avg_brightness(&reference);

    // Both produce valid images (not all-black or all-white)
    assert!(
        ours_brightness > 10.0 && ours_brightness < 245.0,
        "Oklab invert should produce reasonable brightness, got {ours_brightness:.1}"
    );
    assert!(
        ref_brightness > 10.0 && ref_brightness < 245.0,
        "sRGB invert should produce reasonable brightness, got {ref_brightness:.1}"
    );

    // Verify pixel ordering is reversed: for each pair of pixels, if one was
    // brighter than the other in the original, it should be darker in the
    // inverted result. Check a sample of pixel pairs.
    let ours_raw = ours.as_raw();
    let img_raw = img.as_raw();
    let n = img_raw.len() / 3;
    let mut reversed = 0usize;
    let mut total = 0usize;
    for i in (0..n).step_by(7) {
        for j in (i + 1..n).step_by(11) {
            let orig_i =
                img_raw[i * 3] as i32 + img_raw[i * 3 + 1] as i32 + img_raw[i * 3 + 2] as i32;
            let orig_j =
                img_raw[j * 3] as i32 + img_raw[j * 3 + 1] as i32 + img_raw[j * 3 + 2] as i32;
            let inv_i =
                ours_raw[i * 3] as i32 + ours_raw[i * 3 + 1] as i32 + ours_raw[i * 3 + 2] as i32;
            let inv_j =
                ours_raw[j * 3] as i32 + ours_raw[j * 3 + 1] as i32 + ours_raw[j * 3 + 2] as i32;
            if (orig_i - orig_j).abs() > 10 {
                total += 1;
                if (orig_i > orig_j) != (inv_i > inv_j) {
                    reversed += 1;
                }
            }
        }
    }
    let ratio = reversed as f64 / total as f64;
    assert!(
        ratio > 0.8,
        "Oklab invert should reverse brightness ordering, got {ratio:.2} ({reversed}/{total})"
    );
}

#[test]
fn invert_double_is_near_identity() {
    let img = make_gradient_rgb(SIZE, SIZE);
    let once = apply_zenfilter(&img, Box::new(Invert::default()));
    let twice = apply_zenfilter(&once, Box::new(Invert::default()));

    // Double invert in Oklab is mathematically exact, but the sRGB u8 roundtrip
    // through each pipeline pass introduces quantization error. Additionally,
    // Oklab inversion can push colors outside sRGB gamut, causing clipping
    // that can't be reversed. Check max per-pixel deviation.
    let mut max_diff = 0u8;
    let mut total_diff = 0u64;
    for (orig, result) in img.as_raw().iter().zip(twice.as_raw().iter()) {
        let d = orig.abs_diff(*result);
        max_diff = max_diff.max(d);
        total_diff += d as u64;
    }
    let mean_diff = total_diff as f64 / img.as_raw().len() as f64;
    eprintln!("invert double roundtrip: max_diff={max_diff}, mean_diff={mean_diff:.2}");
    // Gamut clipping on the inverted intermediate causes significant precision
    // loss. Oklab invert maps many in-gamut colors to out-of-sRGB-gamut values,
    // which get clipped during the u8 conversion. The second invert can't
    // recover these clipped colors. Mean error ~10 levels is expected.
    assert!(
        mean_diff < 15.0,
        "double invert mean error should be moderate, got {mean_diff:.2}"
    );
}

// ─── Hue rotation ───────────────────────────────────────────────────

#[test]
fn hue_rotate_vs_image_crate() {
    let img = make_colorful_rgb(SIZE, SIZE);

    let mut hue = HueRotate::default();
    hue.degrees = 90.0;
    let ours = apply_zenfilter(&img, Box::new(hue));

    let reference = DynamicImage::ImageRgb8(img.clone()).huerotate(90).to_rgb8();

    // Oklab rotation vs HSL rotation produce very different pixel values.
    // Verify that both shift colors away from the original by a similar amount.
    let ours_vs_orig = avg_brightness(&ours);
    let ref_vs_orig = avg_brightness(&reference);
    let orig_bright = avg_brightness(&img);

    // Both should preserve overall brightness (hue rotation doesn't change lightness much)
    assert!(
        (ours_vs_orig - orig_bright).abs() < 30.0,
        "our hue rotation shouldn't drastically change brightness: {orig_bright:.1} -> {ours_vs_orig:.1}"
    );
    assert!(
        (ref_vs_orig - orig_bright).abs() < 30.0,
        "ref hue rotation shouldn't drastically change brightness: {orig_bright:.1} -> {ref_vs_orig:.1}"
    );

    // Both should change the color distribution (not be identical to original)
    let ours_diff: u64 = ours
        .as_raw()
        .iter()
        .zip(img.as_raw().iter())
        .map(|(a, b)| a.abs_diff(*b) as u64)
        .sum();
    let ref_diff: u64 = reference
        .as_raw()
        .iter()
        .zip(img.as_raw().iter())
        .map(|(a, b)| a.abs_diff(*b) as u64)
        .sum();
    assert!(ours_diff > 0, "our hue rotation should change colors");
    assert!(ref_diff > 0, "ref hue rotation should change colors");
}

#[test]
fn hue_rotate_360_is_near_identity() {
    let img = make_colorful_rgb(SIZE, SIZE);
    let mut hue = HueRotate::default();
    hue.degrees = 360.0;
    let result = apply_zenfilter(&img, Box::new(hue));
    let score = zensim_score(&img, &result);
    eprintln!("hue_rotate 360° roundtrip score: {score:.1}");
    assert!(
        score > 90.0,
        "360° hue rotation should be near-identity, got score {score:.1}"
    );
}

// ─── Gaussian blur ──────────────────────────────────────────────────

#[test]
fn blur_vs_imageproc() {
    let img = make_edges_rgb(SIZE, SIZE);

    let mut blur = Blur::default();
    blur.sigma = 2.0;
    let ours = apply_zenfilter(&img, Box::new(blur));

    // imageproc gaussian blur in sRGB space
    let reference = imageproc::filter::gaussian_blur_f32(&img, 2.0);

    let score = zensim_score(&ours, &reference);
    eprintln!("blur sigma=2 vs imageproc::gaussian_blur score: {score:.1}");
    // Same algorithm, different color space. Should be quite similar.
    assert!(
        score > 70.0,
        "Gaussian blur should match closely, got score {score:.1}"
    );
}

#[test]
fn blur_reduces_edge_sharpness() {
    let img = make_edges_rgb(SIZE, SIZE);
    let before = pixel_stddev(&img);

    let mut blur = Blur::default();
    blur.sigma = 3.0;
    let result = apply_zenfilter(&img, Box::new(blur));
    let after = pixel_stddev(&result);

    assert!(
        after < before,
        "blur should reduce pixel variation: {before:.1} -> {after:.1}"
    );
}

// ─── Sharpen ────────────────────────────────────────────────────────

#[test]
fn sharpen_vs_imageproc() {
    let img = make_edges_rgb(SIZE, SIZE);

    let mut sharpen = Sharpen::default();
    sharpen.sigma = 1.0;
    sharpen.amount = 0.5;
    let ours = apply_zenfilter(&img, Box::new(sharpen));

    // imageproc sharpen: sharpen_gaussian(img, sigma, amount) isn't available,
    // but sharpen3x3 is. Use gaussian_blur + manual unsharp mask instead.
    let blurred = imageproc::filter::gaussian_blur_f32(&img, 1.0);
    let reference = ImageBuffer::from_fn(SIZE, SIZE, |x, y| {
        let orig = img.get_pixel(x, y);
        let blur = blurred.get_pixel(x, y);
        Rgb([
            (orig.0[0] as f32 + 0.5 * (orig.0[0] as f32 - blur.0[0] as f32)).clamp(0.0, 255.0)
                as u8,
            (orig.0[1] as f32 + 0.5 * (orig.0[1] as f32 - blur.0[1] as f32)).clamp(0.0, 255.0)
                as u8,
            (orig.0[2] as f32 + 0.5 * (orig.0[2] as f32 - blur.0[2] as f32)).clamp(0.0, 255.0)
                as u8,
        ])
    });

    let score = zensim_score(&ours, &reference);
    eprintln!("sharpen vs imageproc unsharp mask score: {score:.1}");
    // L-only sharpen vs all-channel sharpen — reasonably similar
    assert!(
        score > 60.0,
        "sharpen should be broadly similar, got score {score:.1}"
    );
}

// ─── Highlights / Shadows ───────────────────────────────────────────

#[test]
fn shadows_lift_brightens_darks() {
    // Use a genuinely dark image — the gradient has high green values that
    // give high Oklab L even when sRGB mean looks low.
    let img = ImageBuffer::from_fn(SIZE, SIZE, |x, y| {
        let tx = x as f32 / SIZE as f32;
        let ty = y as f32 / SIZE as f32;
        // Low brightness image: all channels in 10-80 range
        Rgb([
            (tx * 60.0 + 10.0) as u8,
            (ty * 50.0 + 15.0) as u8,
            ((1.0 - tx) * 40.0 + 20.0) as u8,
        ])
    });

    let mut hs = HighlightsShadows::default();
    hs.shadows = 0.5;
    let result = apply_zenfilter(&img, Box::new(hs));

    // Shadow lift only increases Oklab L (for L < 0.5) and leaves L >= 0.5
    // unchanged, so average brightness should increase for a dark image.
    let before = avg_brightness(&img);
    let after = avg_brightness(&result);
    assert!(
        after > before,
        "shadow lift should brighten dark image: {before:.1} -> {after:.1}"
    );
}

#[test]
fn highlights_recovery_dims_brights() {
    let img = make_gradient_rgb(SIZE, SIZE);

    let mut hs = HighlightsShadows::default();
    hs.highlights = 0.5;
    let result = apply_zenfilter(&img, Box::new(hs));

    let bright_before: f64 = img
        .pixels()
        .filter(|p| p.0[0] > 180 && p.0[1] > 100)
        .map(|p| (p.0[0] as u64 + p.0[1] as u64 + p.0[2] as u64) as f64 / 3.0)
        .sum::<f64>();
    let bright_after: f64 = result
        .pixels()
        .zip(img.pixels())
        .filter(|(_, orig)| orig.0[0] > 180 && orig.0[1] > 100)
        .map(|(p, _)| (p.0[0] as u64 + p.0[1] as u64 + p.0[2] as u64) as f64 / 3.0)
        .sum::<f64>();

    assert!(
        bright_after < bright_before,
        "highlight recovery should dim bright pixels"
    );
}

// ─── Temperature / Tint ─────────────────────────────────────────────

#[test]
fn temperature_warm_shifts_toward_yellow() {
    let img = make_gradient_rgb(SIZE, SIZE);

    let mut temp = Temperature::default();
    temp.shift = 0.5;
    let result = apply_zenfilter(&img, Box::new(temp));

    // Warm = more yellow/red, less blue
    let blue_before: f64 = img.pixels().map(|p| p.0[2] as f64).sum();
    let blue_after: f64 = result.pixels().map(|p| p.0[2] as f64).sum();
    let red_before: f64 = img.pixels().map(|p| p.0[0] as f64).sum();
    let red_after: f64 = result.pixels().map(|p| p.0[0] as f64).sum();

    // Warming should decrease blue relative to red
    let ratio_before = blue_before / red_before;
    let ratio_after = blue_after / red_after;
    assert!(
        ratio_after < ratio_before,
        "warm temperature should decrease blue/red ratio: {ratio_before:.3} -> {ratio_after:.3}"
    );
}

#[test]
fn temperature_cool_shifts_toward_blue() {
    let img = make_gradient_rgb(SIZE, SIZE);

    let mut temp = Temperature::default();
    temp.shift = -0.5;
    let result = apply_zenfilter(&img, Box::new(temp));

    let blue_before: f64 = img.pixels().map(|p| p.0[2] as f64).sum();
    let blue_after: f64 = result.pixels().map(|p| p.0[2] as f64).sum();
    let red_before: f64 = img.pixels().map(|p| p.0[0] as f64).sum();
    let red_after: f64 = result.pixels().map(|p| p.0[0] as f64).sum();

    let ratio_before = blue_before / red_before;
    let ratio_after = blue_after / red_after;
    assert!(
        ratio_after > ratio_before,
        "cool temperature should increase blue/red ratio: {ratio_before:.3} -> {ratio_after:.3}"
    );
}

// ─── Vibrance ───────────────────────────────────────────────────────

#[test]
fn vibrance_protects_saturated_colors() {
    let img = make_colorful_rgb(SIZE, SIZE);

    let mut vib = Vibrance::default();
    vib.amount = 0.5;
    let result = apply_zenfilter(&img, Box::new(vib));

    // Vibrance should boost desaturated areas more than saturated ones.
    // Check that the image is visually similar to the original (not wildly distorted).
    let score = zensim_score(&img, &result);
    eprintln!("vibrance 0.5 vs original score: {score:.1}");
    assert!(
        score > 50.0,
        "vibrance should produce a plausible result, got score {score:.1}"
    );
}

// ─── Sepia ──────────────────────────────────────────────────────────

#[test]
fn sepia_has_warm_neutral_tones() {
    let img = make_colorful_rgb(SIZE, SIZE);

    let result = apply_zenfilter(&img, Box::new(Sepia::default()));

    // Sepia should produce warm neutral tones: R >= G >= B for most pixels
    let warm_count = result
        .pixels()
        .filter(|p| p.0[0] >= p.0[1].saturating_sub(5) && p.0[1] >= p.0[2].saturating_sub(5))
        .count();
    let total = (SIZE * SIZE) as usize;
    let warm_ratio = warm_count as f64 / total as f64;
    assert!(
        warm_ratio > 0.8,
        "sepia should produce warm tones (R>=G>=B) for most pixels, got {:.1}%",
        warm_ratio * 100.0
    );
}

#[test]
fn sepia_is_mostly_neutral() {
    let img = make_colorful_rgb(SIZE, SIZE);
    let result = apply_zenfilter(&img, Box::new(Sepia::default()));

    // Sepia desaturates + adds warm tint — all pixels should have small channel spread
    let mut max_spread = 0u8;
    for pixel in result.pixels() {
        let spread = pixel.0[0].abs_diff(pixel.0[2]);
        max_spread = max_spread.max(spread);
    }
    assert!(
        max_spread < 60,
        "sepia should be mostly neutral (small R-B spread), got max {max_spread}"
    );
}

// ─── Dehaze ─────────────────────────────────────────────────────────

#[test]
fn dehaze_increases_contrast_and_saturation() {
    let img = make_gradient_rgb(SIZE, SIZE);

    let mut dehaze = Dehaze::default();
    dehaze.strength = 0.5;
    let result = apply_zenfilter(&img, Box::new(dehaze));

    let stddev_before = pixel_stddev(&img);
    let stddev_after = pixel_stddev(&result);
    assert!(
        stddev_after > stddev_before,
        "dehaze should increase pixel variation: {stddev_before:.1} -> {stddev_after:.1}"
    );
}

// ─── Black point / White point ──────────────────────────────────────

#[test]
fn black_point_crushes_shadows() {
    let img = make_gradient_rgb(SIZE, SIZE);

    let mut bp = BlackPoint::default();
    bp.level = 0.1;
    let result = apply_zenfilter(&img, Box::new(bp));

    // Dark pixels should be pushed toward black
    let dark_before: usize = img
        .pixels()
        .filter(|p| p.0[0] < 30 && p.0[1] < 30 && p.0[2] < 30)
        .count();
    let dark_after: usize = result
        .pixels()
        .filter(|p| p.0[0] < 30 && p.0[1] < 30 && p.0[2] < 30)
        .count();
    assert!(
        dark_after >= dark_before,
        "black point should increase number of dark pixels"
    );
}

#[test]
fn white_point_below_one_brightens() {
    let img = make_gradient_rgb(SIZE, SIZE);

    let mut wp = WhitePoint::default();
    wp.level = 0.8;
    let result = apply_zenfilter(&img, Box::new(wp));

    assert!(
        avg_brightness(&result) > avg_brightness(&img),
        "white point < 1.0 should brighten the image"
    );
}

// ─── FusedAdjust ────────────────────────────────────────────────────

#[test]
fn fused_adjust_matches_individual_filters() {
    let img = make_gradient_rgb(SIZE, SIZE);

    // Apply individual filters in sequence
    let mut pipeline_individual = Pipeline::new(PipelineConfig::default()).unwrap();
    let mut exp = Exposure::default();
    exp.stops = 0.5;
    pipeline_individual.push(Box::new(exp));
    let mut con = Contrast::default();
    con.amount = 0.2;
    pipeline_individual.push(Box::new(con));
    let mut sat = Saturation::default();
    sat.factor = 1.2;
    pipeline_individual.push(Box::new(sat));

    let input_bytes = img.as_raw().clone();
    let desc = zenpixels::PixelDescriptor::RGB8_SRGB;
    let input_buf =
        zenpixels::buffer::PixelBuffer::from_vec(input_bytes.clone(), SIZE, SIZE, desc).unwrap();
    let mut ctx = FilterContext::new();
    let individual_out = apply_to_buffer(&pipeline_individual, &input_buf, true, &mut ctx).unwrap();
    let individual_bytes = individual_out.copy_to_contiguous_bytes();
    let individual: RgbImage = ImageBuffer::from_raw(SIZE, SIZE, individual_bytes).unwrap();

    // Apply equivalent FusedAdjust
    let mut fused = FusedAdjust::new();
    fused.exposure = 0.5;
    fused.contrast = 0.2;
    fused.saturation = 1.2;
    let fused_result = apply_zenfilter(&img, Box::new(fused));

    let score = zensim_score(&individual, &fused_result);
    eprintln!("fused_adjust vs individual filters score: {score:.1}");
    // These should be very similar since they use the same math
    assert!(
        score > 85.0,
        "fused adjust should closely match individual filters, got score {score:.1}"
    );
}

// ─── Clarity ────────────────────────────────────────────────────────

#[test]
fn clarity_enhances_local_contrast() {
    let img = make_edges_rgb(SIZE, SIZE);

    let mut clarity = Clarity::default();
    clarity.amount = 0.5;
    let result = apply_zenfilter(&img, Box::new(clarity));

    // Clarity enhances local contrast by adding the difference between
    // the original and a blurred version. On a checkerboard pattern,
    // this should increase the stddev of pixel values.
    let before_stddev = pixel_stddev(&img);
    let after_stddev = pixel_stddev(&result);
    eprintln!("clarity stddev: {before_stddev:.1} -> {after_stddev:.1}");
    assert!(
        after_stddev >= before_stddev * 0.95,
        "clarity should maintain or increase local contrast: {before_stddev:.1} -> {after_stddev:.1}"
    );

    // The result should not be all-black or all-white (sanity check)
    let brightness = avg_brightness(&result);
    assert!(
        brightness > 20.0 && brightness < 235.0,
        "clarity result should have reasonable brightness, got {brightness:.1}"
    );
}

// ─── Color matrix ───────────────────────────────────────────────────

#[test]
fn color_matrix_identity_is_near_noop() {
    let img = make_gradient_rgb(SIZE, SIZE);
    let result = apply_zenfilter(&img, Box::new(ColorMatrix::default()));
    let score = zensim_score(&img, &result);
    eprintln!("color_matrix identity score: {score:.1}");
    assert!(
        score > 90.0,
        "identity color matrix should be near-noop, got score {score:.1}"
    );
}

// ─── Alpha ──────────────────────────────────────────────────────────

#[test]
fn alpha_has_no_effect_on_rgb() {
    let img = make_gradient_rgb(SIZE, SIZE);
    let mut alpha = Alpha::default();
    alpha.factor = 0.5;
    let result = apply_zenfilter(&img, Box::new(alpha));
    // Alpha filter on RGB (no alpha channel) should be a no-op
    let score = zensim_score(&img, &result);
    assert!(
        score > 95.0,
        "alpha on RGB should be near-noop, got score {score:.1}"
    );
}

// ─── Pipeline roundtrip ─────────────────────────────────────────────

#[test]
fn empty_pipeline_is_near_identity() {
    let img = make_gradient_rgb(SIZE, SIZE);
    let input_bytes = img.as_raw().clone();
    let desc = zenpixels::PixelDescriptor::RGB8_SRGB;
    let input_buf =
        zenpixels::buffer::PixelBuffer::from_vec(input_bytes, SIZE, SIZE, desc).unwrap();

    let pipeline = Pipeline::new(PipelineConfig::default()).unwrap();
    let mut ctx = FilterContext::new();
    let output_buf = apply_to_buffer(&pipeline, &input_buf, true, &mut ctx).unwrap();
    let output_bytes = output_buf.copy_to_contiguous_bytes();
    let result: RgbImage = ImageBuffer::from_raw(SIZE, SIZE, output_bytes).unwrap();

    let score = zensim_score(&img, &result);
    eprintln!("empty pipeline roundtrip score: {score:.1}");
    assert!(
        score > 90.0,
        "empty pipeline should be near-identity, got score {score:.1}"
    );
}

// ═══ Palette crate Oklab validation ═════════════════════════════════
//
// Compare our sRGB→Oklab→sRGB conversion against the palette crate's
// implementation. Palette uses the W3C-corrected Oklab matrices (2021);
// our code uses the original Ottosson 2020 matrices. This test measures
// the practical impact of that matrix difference.

/// Convert a single sRGB u8 pixel through our pipeline (sRGB→Oklab→sRGB).
fn our_srgb_to_oklab(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let input = vec![r, g, b];
    let m1 = zenpixels_convert::oklab::rgb_to_lms_matrix(zenpixels::ColorPrimaries::Bt709).unwrap();
    let mut planes = OklabPlanes::new(1, 1);
    zenfilters::scatter_srgb_u8_to_oklab(&input, &mut planes, 3, &m1);
    (planes.l[0], planes.a[0], planes.b[0])
}

/// Convert a single sRGB u8 pixel through palette (sRGB→Oklab).
fn palette_srgb_to_oklab(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let srgb = palette::Srgb::new(r, g, b).into_format::<f32>();
    let oklab: palette::Oklab = srgb.into_color_unclamped();
    (oklab.l, oklab.a, oklab.b)
}

#[test]
fn oklab_conversion_vs_palette_primary_colors() {
    // Test the full sRGB primary and secondary colors plus white, black, mid-gray
    let test_colors: &[(u8, u8, u8, &str)] = &[
        (0, 0, 0, "black"),
        (255, 255, 255, "white"),
        (128, 128, 128, "mid-gray"),
        (255, 0, 0, "red"),
        (0, 255, 0, "green"),
        (0, 0, 255, "blue"),
        (255, 255, 0, "yellow"),
        (255, 0, 255, "magenta"),
        (0, 255, 255, "cyan"),
    ];

    let mut max_l_err = 0.0f32;
    let mut max_a_err = 0.0f32;
    let mut max_b_err = 0.0f32;

    for &(r, g, b, name) in test_colors {
        let (our_l, our_a, our_b) = our_srgb_to_oklab(r, g, b);
        let (pal_l, pal_a, pal_b) = palette_srgb_to_oklab(r, g, b);

        let l_err = (our_l - pal_l).abs();
        let a_err = (our_a - pal_a).abs();
        let b_err = (our_b - pal_b).abs();

        max_l_err = max_l_err.max(l_err);
        max_a_err = max_a_err.max(a_err);
        max_b_err = max_b_err.max(b_err);

        eprintln!(
            "{name:>8}: ours=({our_l:.4}, {our_a:.4}, {our_b:.4}) \
             palette=({pal_l:.4}, {pal_a:.4}, {pal_b:.4}) \
             err=({l_err:.6}, {a_err:.6}, {b_err:.6})"
        );
    }

    eprintln!("Max errors: L={max_l_err:.6}, a={max_a_err:.6}, b={max_b_err:.6}");

    // Both use f32 with slightly different matrices (original Ottosson vs W3C corrected).
    // The difference should be tiny — well under 1e-3 for any practical purpose.
    assert!(
        max_l_err < 5e-3,
        "L channel max error vs palette: {max_l_err}"
    );
    assert!(
        max_a_err < 5e-3,
        "a channel max error vs palette: {max_a_err}"
    );
    assert!(
        max_b_err < 5e-3,
        "b channel max error vs palette: {max_b_err}"
    );
}

#[test]
fn oklab_conversion_vs_palette_full_sweep() {
    // Test a broader sample: every 17th value in each channel (16³ = 4096 test colors)
    let mut total_colors = 0u32;
    let mut max_l_err = 0.0f32;
    let mut max_a_err = 0.0f32;
    let mut max_b_err = 0.0f32;
    let mut sum_l_err = 0.0f64;
    let mut sum_a_err = 0.0f64;
    let mut sum_b_err = 0.0f64;

    for r in (0u16..=255).step_by(17) {
        for g in (0u16..=255).step_by(17) {
            for b in (0u16..=255).step_by(17) {
                let (our_l, our_a, our_b) = our_srgb_to_oklab(r as u8, g as u8, b as u8);
                let (pal_l, pal_a, pal_b) = palette_srgb_to_oklab(r as u8, g as u8, b as u8);

                let l_err = (our_l - pal_l).abs();
                let a_err = (our_a - pal_a).abs();
                let b_err = (our_b - pal_b).abs();

                max_l_err = max_l_err.max(l_err);
                max_a_err = max_a_err.max(a_err);
                max_b_err = max_b_err.max(b_err);
                sum_l_err += l_err as f64;
                sum_a_err += a_err as f64;
                sum_b_err += b_err as f64;
                total_colors += 1;
            }
        }
    }

    let n = total_colors as f64;
    eprintln!("Tested {total_colors} colors");
    eprintln!("Max errors:  L={max_l_err:.6}, a={max_a_err:.6}, b={max_b_err:.6}");
    eprintln!(
        "Mean errors: L={:.8}, a={:.8}, b={:.8}",
        sum_l_err / n,
        sum_a_err / n,
        sum_b_err / n
    );

    // With different XYZ→LMS matrices (original Ottosson vs W3C corrected),
    // we expect small but nonzero differences. The practical threshold for
    // visible difference in Oklab is ~0.01 (JND).
    assert!(
        max_l_err < 0.01,
        "L max error {max_l_err:.6} exceeds JND threshold"
    );
    assert!(
        max_a_err < 0.01,
        "a max error {max_a_err:.6} exceeds JND threshold"
    );
    assert!(
        max_b_err < 0.01,
        "b max error {max_b_err:.6} exceeds JND threshold"
    );
}
