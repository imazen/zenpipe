//! Compare zenfilters vs ImageMagick on real photographs.
//!
//! For each operation, applies three versions:
//! 1. **Oklab** — zenfilters' perceptually correct path (default)
//! 2. **sRGB** — zenfilters' sRGB-compat filters (matching ImageMagick's formulas)
//! 3. **ImageMagick** — reference output via `convert` command
//!
//! Measures zensim agreement between sRGB and IM to validate IM compatibility.
//! Also shows Oklab vs source to demonstrate perceptual quality.
//!
//! Run: cargo test --test imageflow_comparison -- --nocapture

use image::{ImageBuffer, RgbImage};
use std::path::Path;
use std::process::Command;
use zenfilters::filters::*;
use zenfilters::*;
use zensim::{RgbSlice, Zensim, ZensimProfile};

// ─── Constructors for #[non_exhaustive] filter structs ─────────────

fn make_sharpen(amount: f32, sigma: f32) -> Box<dyn Filter> {
    let mut s = Sharpen::default();
    s.amount = amount;
    s.sigma = sigma;
    Box::new(s)
}

fn make_blur(sigma: f32) -> Box<dyn Filter> {
    let mut b = Blur::default();
    b.sigma = sigma;
    Box::new(b)
}

fn make_edge_detect(mode: EdgeMode, strength: f32) -> Box<dyn Filter> {
    let mut e = EdgeDetect::default();
    e.mode = mode;
    e.strength = strength;
    Box::new(e)
}

fn make_morphology(op: MorphOp, radius: u32, process_chroma: bool) -> Box<dyn Filter> {
    let mut m = Morphology::default();
    m.op = op;
    m.radius = radius;
    m.process_chroma = process_chroma;
    Box::new(m)
}

fn make_motion_blur(angle: f32, length: f32) -> Box<dyn Filter> {
    let mut mb = MotionBlur::default();
    mb.angle = angle;
    mb.length = length;
    Box::new(mb)
}

fn make_sigmoidal_contrast(amount: f32) -> Box<dyn Filter> {
    let mut c = SigmoidalContrast::default();
    c.amount = amount;
    Box::new(c)
}

fn make_linear_brightness(offset: f32) -> Box<dyn Filter> {
    let mut b = LinearBrightness::default();
    b.offset = offset;
    Box::new(b)
}

fn make_hsl_saturate(factor: f32) -> Box<dyn Filter> {
    let mut s = HslSaturate::default();
    s.factor = factor;
    Box::new(s)
}

fn make_channel_posterize(levels: u32) -> Box<dyn Filter> {
    let mut p = ChannelPosterize::default();
    p.levels = levels;
    Box::new(p)
}

fn make_channel_solarize(threshold: f32) -> Box<dyn Filter> {
    let mut s = ChannelSolarize::default();
    s.threshold = threshold;
    Box::new(s)
}

// ─── Image loading ─────────────────────────────────────────────────

fn load_test_image(name: &str) -> Option<RgbImage> {
    let paths = [
        format!(
            "/home/lilith/work/imageflow/.image-cache/sources/imageflow-resources/test_inputs/{name}"
        ),
        format!("/home/lilith/work/zen/jxl-encoder/jxl-encoder/tests/images/{name}"),
    ];
    for path in &paths {
        if Path::new(path).exists() {
            return image::open(path).ok().map(|img| img.to_rgb8());
        }
    }
    None
}

fn source_path(name: &str) -> Option<String> {
    let paths = [
        format!(
            "/home/lilith/work/imageflow/.image-cache/sources/imageflow-resources/test_inputs/{name}"
        ),
        format!("/home/lilith/work/zen/jxl-encoder/jxl-encoder/tests/images/{name}"),
    ];
    for path in &paths {
        if Path::new(path).exists() {
            return Some(path.clone());
        }
    }
    None
}

// ─── ImageMagick reference ─────────────────────────────────────────

fn imagemagick_op(source_path: &str, args: &[&str], output_path: &Path) -> Option<RgbImage> {
    let mut cmd = Command::new("convert");
    cmd.arg(source_path);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.arg(output_path.to_str().unwrap());
    let status = cmd.status().ok()?;
    if !status.success() {
        eprintln!("  ImageMagick failed: convert {source_path} {}", args.join(" "));
        return None;
    }
    image::open(output_path).ok().map(|img| img.to_rgb8())
}

// ─── Apply helpers ─────────────────────────────────────────────────

fn apply_oklab(img: &RgbImage, filter: Box<dyn Filter>) -> RgbImage {
    apply_with_config(img, filter, PipelineConfig::default())
}

fn apply_srgb(img: &RgbImage, filter: Box<dyn Filter>) -> RgbImage {
    apply_with_config(img, filter, PipelineConfig::srgb_compat())
}

fn apply_with_config(img: &RgbImage, filter: Box<dyn Filter>, config: PipelineConfig) -> RgbImage {
    let (w, h) = img.dimensions();
    let input_bytes: Vec<u8> = img.as_raw().clone();
    let desc = zenpixels::PixelDescriptor::RGB8_SRGB;
    let input_buf = zenpixels::buffer::PixelBuffer::from_vec(input_bytes, w, h, desc).unwrap();
    let mut pipeline = Pipeline::new(config).unwrap();
    pipeline.push(filter);
    let mut ctx = FilterContext::new();
    let output_buf = apply_to_buffer(&pipeline, &input_buf, true, &mut ctx).unwrap();
    ImageBuffer::from_raw(w, h, output_buf.copy_to_contiguous_bytes()).unwrap()
}

fn zensim_score(a: &RgbImage, b: &RgbImage) -> f64 {
    let (w, h) = a.dimensions();
    assert_eq!(a.dimensions(), b.dimensions());
    let a_pixels: &[[u8; 3]] = bytemuck::cast_slice(a.as_raw());
    let b_pixels: &[[u8; 3]] = bytemuck::cast_slice(b.as_raw());
    let z = Zensim::new(ZensimProfile::latest()).with_parallel(false);
    z.compute(
        &RgbSlice::new(a_pixels, w as usize, h as usize),
        &RgbSlice::new(b_pixels, w as usize, h as usize),
    )
    .unwrap()
    .score()
}

fn save_image(img: &RgbImage, path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    img.save(path).unwrap();
}

// ─── Comparison harness ────────────────────────────────────────────

struct ComparisonResult {
    image_name: String,
    filter_name: String,
    oklab_vs_src: f64,
    im_vs_src: f64,
    srgb_vs_im: f64,
}

/// Compare an operation across all three paths (Oklab, sRGB, ImageMagick).
///
/// `make_oklab`: creates the Oklab-native filter (or None to skip Oklab)
/// `make_srgb`: creates the sRGB-compat filter
/// `im_args`: ImageMagick convert arguments
fn compare_op(
    source: &RgbImage,
    source_path: &str,
    image_name: &str,
    filter_name: &str,
    make_oklab: Option<&dyn Fn() -> Box<dyn Filter>>,
    make_srgb: &dyn Fn() -> Box<dyn Filter>,
    im_args: &[&str],
    output_dir: &Path,
) -> Option<ComparisonResult> {
    // IM reference
    let im_out = output_dir.join(image_name).join(format!("{filter_name}_im.png"));
    let im_result = imagemagick_op(source_path, im_args, &im_out)?;

    // sRGB zenfilters
    let srgb_result = apply_srgb(source, make_srgb());

    if srgb_result.dimensions() != im_result.dimensions() {
        eprintln!("  {filter_name}: dimension mismatch, skipping");
        return None;
    }

    // Oklab zenfilters (optional — some ops only have sRGB versions)
    let oklab_vs_src = if let Some(mk) = make_oklab {
        let oklab_result = apply_oklab(source, mk());
        let dir = output_dir.join(image_name);
        save_image(&oklab_result, &dir.join(format!("{filter_name}_oklab.png")));
        zensim_score(source, &oklab_result)
    } else {
        f64::NAN
    };

    let im_vs_src = zensim_score(source, &im_result);
    let srgb_vs_im = zensim_score(&srgb_result, &im_result);

    let dir = output_dir.join(image_name);
    save_image(&srgb_result, &dir.join(format!("{filter_name}_srgb.png")));

    if oklab_vs_src.is_nan() {
        eprintln!(
            "  {filter_name:25}  oklab=  N/A  im={im_vs_src:6.1}  srgb_vs_im={srgb_vs_im:6.1}"
        );
    } else {
        eprintln!(
            "  {filter_name:25}  oklab={oklab_vs_src:6.1}  im={im_vs_src:6.1}  srgb_vs_im={srgb_vs_im:6.1}"
        );
    }

    Some(ComparisonResult {
        image_name: image_name.to_string(),
        filter_name: filter_name.to_string(),
        oklab_vs_src,
        im_vs_src,
        srgb_vs_im,
    })
}

// ─── Test suite ────────────────────────────────────────────────────

fn run_suite(
    source: &RgbImage,
    source_path: &str,
    image_name: &str,
    output_dir: &Path,
) -> Vec<ComparisonResult> {
    let mut results = Vec::new();
    save_image(source, &output_dir.join(image_name).join("source.png"));

    eprintln!("\n=== {image_name} ({}x{}) ===", source.width(), source.height());
    eprintln!("  {:25}  {:>6}  {:>6}  {:>10}", "filter", "oklab", "im", "srgb_vs_im");

    // ─── Contrast: Oklab power curve vs sRGB linear ────────────
    for &(amount, im_pct) in &[(0.3f32, "30"), (0.6, "60"), (-0.3, "-30")] {
        let label = format!("contrast_{amount:+.1}");
        if let Some(r) = compare_op(
            source, source_path, image_name, &label,
            Some(&|| { let mut c = Contrast::default(); c.amount = amount; Box::new(c) }),
            &|| make_sigmoidal_contrast(amount),
            &["-brightness-contrast", &format!("0x{im_pct}")],
            output_dir,
        ) { results.push(r); }
    }

    // ─── Brightness: Oklab exposure vs sRGB additive ───────────
    for &(stops, im_pct) in &[(0.5f32, "15"), (1.0, "30"), (-0.5, "-15")] {
        let label = format!("brightness_{stops:+.1}");
        if let Some(r) = compare_op(
            source, source_path, image_name, &label,
            Some(&|| { let mut e = Exposure::default(); e.stops = stops; Box::new(e) }),
            &|| make_linear_brightness(stops * 0.15),
            &["-brightness-contrast", &format!("{im_pct}x0")],
            output_dir,
        ) { results.push(r); }
    }

    // ─── Saturation: Oklab chroma vs sRGB HSL ──────────────────
    for &(factor, im_mod) in &[(1.5f32, "150"), (0.5, "50"), (0.0, "0")] {
        let label = format!("saturation_{factor:.1}");
        if let Some(r) = compare_op(
            source, source_path, image_name, &label,
            Some(&|| { let mut s = Saturation::default(); s.factor = factor; Box::new(s) }),
            &|| make_hsl_saturate(factor),
            &["-modulate", &format!("100,{im_mod},100")],
            output_dir,
        ) { results.push(r); }
    }

    // ─── Grayscale: Oklab zero-chroma vs sRGB Rec.709 luma ────
    if let Some(r) = compare_op(
        source, source_path, image_name, "grayscale",
        Some(&|| Box::new(Grayscale::default())),
        &|| Box::new(LumaGrayscale::default()),
        &["-colorspace", "Gray"],
        output_dir,
    ) { results.push(r); }

    // ─── Generic filters (same filter for both paths) ──────────

    // Sharpen (generic — works in any space)
    for &(amount, im_sigma) in &[(0.5f32, "0x1"), (1.0, "0x2")] {
        let label = format!("sharpen_{amount:.1}");
        if let Some(r) = compare_op(
            source, source_path, image_name, &label,
            Some(&|| make_sharpen(amount, 1.0)),
            &|| make_sharpen(amount, 1.0),
            &["-sharpen", im_sigma],
            output_dir,
        ) { results.push(r); }
    }

    // Blur (generic)
    for &(sigma, im_sigma) in &[(2.0f32, "0x2"), (5.0, "0x5")] {
        let label = format!("blur_{sigma:.0}");
        if let Some(r) = compare_op(
            source, source_path, image_name, &label,
            Some(&|| make_blur(sigma)),
            &|| make_blur(sigma),
            &["-blur", im_sigma],
            output_dir,
        ) { results.push(r); }
    }

    // Emboss (generic convolution on all planes)
    if let Some(r) = compare_op(
        source, source_path, image_name, "emboss",
        Some(&|| Box::new(Convolve::new(ConvolutionKernel::emboss()).with_bias(0.5).with_target(ConvolveTarget::All))),
        &|| Box::new(Convolve::new(ConvolutionKernel::emboss()).with_bias(0.5).with_target(ConvolveTarget::All)),
        &["-emboss", "1"],
        output_dir,
    ) { results.push(r); }

    // Edge detect (generic)
    if let Some(r) = compare_op(
        source, source_path, image_name, "edge_detect",
        Some(&|| make_edge_detect(EdgeMode::Sobel, 1.0)),
        &|| make_edge_detect(EdgeMode::Sobel, 1.0),
        &["-edge", "1"],
        output_dir,
    ) { results.push(r); }

    // Posterize: Oklab L-only vs sRGB all-channel
    if let Some(r) = compare_op(
        source, source_path, image_name, "posterize_4",
        None, // no Oklab equivalent that matches IM
        &|| make_channel_posterize(4),
        &["-posterize", "4"],
        output_dir,
    ) { results.push(r); }

    // Solarize: Oklab L-only vs sRGB all-channel
    if let Some(r) = compare_op(
        source, source_path, image_name, "solarize_50",
        None,
        &|| make_channel_solarize(0.5),
        &["-solarize", "50%"],
        output_dir,
    ) { results.push(r); }

    // Morphology (generic — dilate on all planes)
    if let Some(r) = compare_op(
        source, source_path, image_name, "dilate_1",
        Some(&|| make_morphology(MorphOp::Dilate, 1, true)),
        &|| make_morphology(MorphOp::Dilate, 1, true),
        &["-morphology", "Dilate", "Square:1"],
        output_dir,
    ) { results.push(r); }

    if let Some(r) = compare_op(
        source, source_path, image_name, "erode_1",
        Some(&|| make_morphology(MorphOp::Erode, 1, true)),
        &|| make_morphology(MorphOp::Erode, 1, true),
        &["-morphology", "Erode", "Square:1"],
        output_dir,
    ) { results.push(r); }

    // Motion blur (generic)
    if let Some(r) = compare_op(
        source, source_path, image_name, "motion_blur_0_15",
        Some(&|| make_motion_blur(0.0, 15.0)),
        &|| make_motion_blur(0.0, 15.0),
        &["-motion-blur", "0x15+0"],
        output_dir,
    ) { results.push(r); }

    results
}

// ─── Main test ─────────────────────────────────────────────────────

#[test]
fn compare_zenfilters_vs_imagemagick() {
    let output_dir = Path::new("/mnt/v/output/zenfilters/comparison");

    let test_images = [
        ("frymire.png", "frymire"),
        ("waterhouse.jpg", "waterhouse"),
        ("red-night.png", "red_night"),
        ("dice.png", "dice"),
        ("gradients.png", "gradients"),
    ];

    let mut all_results = Vec::new();
    let mut images_tested = 0;

    for (filename, label) in &test_images {
        let src_path = match source_path(filename) {
            Some(p) => p,
            None => { eprintln!("SKIP: {filename} not found"); continue; }
        };
        let img = match load_test_image(filename) {
            Some(i) => i,
            None => continue,
        };
        all_results.extend(run_suite(&img, &src_path, label, output_dir));
        images_tested += 1;
    }

    // ─── Summary ───────────────────────────────────────────────────
    eprintln!("\n{}", "=".repeat(70));
    eprintln!("SUMMARY: {images_tested} images tested\n");

    let mut filter_names: Vec<String> = all_results.iter().map(|r| r.filter_name.clone()).collect();
    filter_names.sort();
    filter_names.dedup();

    eprintln!("  {:25}  {:>8}  {:>8}  {:>10}", "filter", "avg_oklab", "avg_im", "avg_srgb_im");
    for fname in &filter_names {
        let m: Vec<_> = all_results.iter().filter(|r| &r.filter_name == fname).collect();
        let n = m.len() as f64;
        let avg_oklab: f64 = m.iter().map(|r| if r.oklab_vs_src.is_nan() { 0.0 } else { r.oklab_vs_src }).sum::<f64>() / n;
        let avg_im: f64 = m.iter().map(|r| r.im_vs_src).sum::<f64>() / n;
        let avg_agree: f64 = m.iter().map(|r| r.srgb_vs_im).sum::<f64>() / n;
        eprintln!("  {fname:25}  {avg_oklab:8.1}  {avg_im:8.1}  {avg_agree:10.1}");
    }

    assert!(images_tested >= 3, "Need at least 3 test images, got {images_tested}");

    // Morphology in sRGB mode should be pixel-perfect with ImageMagick
    for r in all_results.iter().filter(|r| {
        r.filter_name.starts_with("dilate") || r.filter_name.starts_with("erode")
    }) {
        assert!(
            r.srgb_vs_im > 90.0,
            "{}/{}: srgb_vs_im={:.1} should be >90 for morphology",
            r.image_name, r.filter_name, r.srgb_vs_im
        );
    }

    // Desaturation and grayscale should closely match IM
    for r in all_results.iter().filter(|r| {
        r.filter_name == "grayscale" || r.filter_name == "saturation_0.0" || r.filter_name == "saturation_0.5"
    }) {
        assert!(
            r.srgb_vs_im > 85.0,
            "{}/{}: srgb_vs_im={:.1} should be >85 for desaturation/grayscale",
            r.image_name, r.filter_name, r.srgb_vs_im
        );
    }

    // Posterize and solarize should be near-perfect (same formula, same space)
    for r in all_results.iter().filter(|r| {
        r.filter_name.starts_with("posterize") || r.filter_name.starts_with("solarize")
    }) {
        assert!(
            r.srgb_vs_im > 90.0,
            "{}/{}: srgb_vs_im={:.1} should be >90 for posterize/solarize",
            r.image_name, r.filter_name, r.srgb_vs_im
        );
    }
}
