//! Compare zenfilters (Oklab) vs ImageMagick (sRGB) on real photographs.
//!
//! For each operation, applies both zenfilters and ImageMagick's equivalent,
//! then scores both against the source using zensim. This validates that
//! zenfilters' Oklab-space processing produces comparable or better results
//! than the industry-standard sRGB-space processing in ImageMagick.
//!
//! Test images: frymire (textured), waterhouse (landscape), red-night (dark),
//! dice (colored objects), gradients (banding detection).
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

fn make_posterize(levels: u32, posterize_chroma: bool) -> Box<dyn Filter> {
    let mut p = Posterize::default();
    p.levels = levels;
    p.posterize_chroma = posterize_chroma;
    Box::new(p)
}

fn make_solarize(threshold: f32, solarize_chroma: bool) -> Box<dyn Filter> {
    let mut s = Solarize::default();
    s.threshold = threshold;
    s.solarize_chroma = solarize_chroma;
    Box::new(s)
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

// ─── ImageMagick reference operations ──────────────────────────────

/// Run ImageMagick convert with the given arguments and load the result.
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

// ─── Zenfilters application ────────────────────────────────────────

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

// ─── Scoring ───────────────────────────────────────────────────────

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

fn save_image(img: &RgbImage, path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    img.save(path).unwrap();
}

struct ComparisonResult {
    image_name: String,
    filter_name: String,
    zen_vs_src: f64,
    im_vs_src: f64,
    zen_vs_im: f64,
}

// ─── Comparison harness ────────────────────────────────────────────

fn compare_op(
    source: &RgbImage,
    source_path: &str,
    image_name: &str,
    filter_name: &str,
    zen_filter: Box<dyn Filter>,
    im_args: &[&str],
    output_dir: &Path,
) -> Option<ComparisonResult> {
    let zen_result = apply_zenfilter(source, zen_filter);

    let im_out_path = output_dir
        .join(image_name)
        .join(format!("{filter_name}_im.png"));
    let im_result = imagemagick_op(source_path, im_args, &im_out_path)?;

    // Ensure same dimensions (IM might change them for some ops)
    if zen_result.dimensions() != im_result.dimensions() {
        eprintln!(
            "  {filter_name}: dimension mismatch zen={}x{} im={}x{}, skipping",
            zen_result.width(),
            zen_result.height(),
            im_result.width(),
            im_result.height()
        );
        return None;
    }

    let zen_vs_src = zensim_score(source, &zen_result);
    let im_vs_src = zensim_score(source, &im_result);
    let zen_vs_im = zensim_score(&zen_result, &im_result);

    let zen_path = output_dir
        .join(image_name)
        .join(format!("{filter_name}_zen.png"));
    save_image(&zen_result, &zen_path);

    let winner = if zen_vs_src >= im_vs_src {
        "ZEN"
    } else {
        "IM"
    };
    eprintln!(
        "  {filter_name:25}  zen={zen_vs_src:6.1}  im={im_vs_src:6.1}  agreement={zen_vs_im:5.1}  {winner}"
    );

    Some(ComparisonResult {
        image_name: image_name.to_string(),
        filter_name: filter_name.to_string(),
        zen_vs_src,
        im_vs_src,
        zen_vs_im,
    })
}

fn run_suite(
    source: &RgbImage,
    source_path: &str,
    image_name: &str,
    output_dir: &Path,
) -> Vec<ComparisonResult> {
    let mut results = Vec::new();

    // Save source
    let src_out = output_dir.join(image_name).join("source.png");
    save_image(source, &src_out);

    eprintln!(
        "\n=== {image_name} ({}x{}) ===",
        source.width(),
        source.height()
    );
    eprintln!(
        "  {:25}  {:>6}  {:>6}  {:>9}  {}",
        "filter", "zen", "im", "agreement", "winner"
    );

    // --- Contrast ---
    for &(amount, im_pct) in &[(0.3f32, "30"), (0.6, "60"), (-0.3, "-30")] {
        let label = format!("contrast_{amount:+.1}");
        // IM: -brightness-contrast 0x{pct}
        if let Some(r) = compare_op(
            source,
            source_path,
            image_name,
            &label,
            {
                let mut c = Contrast::default();
                c.amount = amount;
                Box::new(c)
            },
            &["-brightness-contrast", &format!("0x{im_pct}")],
            output_dir,
        ) {
            results.push(r);
        }
    }

    // --- Brightness (Exposure) ---
    for &(stops, im_pct) in &[(0.5f32, "15"), (1.0, "30"), (-0.5, "-15")] {
        let label = format!("exposure_{stops:+.1}EV");
        if let Some(r) = compare_op(
            source,
            source_path,
            image_name,
            &label,
            {
                let mut e = Exposure::default();
                e.stops = stops;
                Box::new(e)
            },
            &["-brightness-contrast", &format!("{im_pct}x0")],
            output_dir,
        ) {
            results.push(r);
        }
    }

    // --- Saturation ---
    for &(factor, im_mod) in &[(1.5f32, "150"), (0.5, "50"), (0.0, "0")] {
        let label = format!("saturation_{factor:.1}");
        if let Some(r) = compare_op(
            source,
            source_path,
            image_name,
            &label,
            {
                let mut s = Saturation::default();
                s.factor = factor;
                Box::new(s)
            },
            &["-modulate", &format!("100,{im_mod},100")],
            output_dir,
        ) {
            results.push(r);
        }
    }

    // --- Grayscale ---
    if let Some(r) = compare_op(
        source,
        source_path,
        image_name,
        "grayscale",
        Box::new(Grayscale::default()),
        &["-colorspace", "Gray"],
        output_dir,
    ) {
        results.push(r);
    }

    // --- Sharpen ---
    for &(amount, im_sigma) in &[(0.5f32, "0x1"), (1.0, "0x2")] {
        let label = format!("sharpen_{amount:.1}");
        if let Some(r) = compare_op(
            source,
            source_path,
            image_name,
            &label,
            make_sharpen(amount, 1.0),
            &["-sharpen", im_sigma],
            output_dir,
        ) {
            results.push(r);
        }
    }

    // --- Blur ---
    for &(sigma, im_sigma) in &[(2.0f32, "0x2"), (5.0, "0x5")] {
        let label = format!("blur_{sigma:.0}");
        if let Some(r) = compare_op(
            source,
            source_path,
            image_name,
            &label,
            make_blur(sigma),
            &["-blur", im_sigma],
            output_dir,
        ) {
            results.push(r);
        }
    }

    // --- Emboss (new filter) ---
    if let Some(r) = compare_op(
        source,
        source_path,
        image_name,
        "emboss",
        Box::new(Convolve::new(ConvolutionKernel::emboss()).with_bias(0.5)),
        &["-emboss", "1"],
        output_dir,
    ) {
        results.push(r);
    }

    // --- Edge detect ---
    if let Some(r) = compare_op(
        source,
        source_path,
        image_name,
        "edge_detect",
        make_edge_detect(EdgeMode::Sobel, 1.0),
        &["-edge", "1"],
        output_dir,
    ) {
        results.push(r);
    }

    // --- Posterize (new filter) ---
    if let Some(r) = compare_op(
        source,
        source_path,
        image_name,
        "posterize_4",
        make_posterize(4, false),
        &["-posterize", "4"],
        output_dir,
    ) {
        results.push(r);
    }

    // --- Solarize (new filter) ---
    if let Some(r) = compare_op(
        source,
        source_path,
        image_name,
        "solarize_50",
        make_solarize(0.5, false),
        &["-solarize", "50%"],
        output_dir,
    ) {
        results.push(r);
    }

    // --- Morphology: Dilate (new filter) ---
    if let Some(r) = compare_op(
        source,
        source_path,
        image_name,
        "dilate_1",
        make_morphology(MorphOp::Dilate, 1, false),
        &["-morphology", "Dilate", "Square:1"],
        output_dir,
    ) {
        results.push(r);
    }

    // --- Morphology: Erode (new filter) ---
    if let Some(r) = compare_op(
        source,
        source_path,
        image_name,
        "erode_1",
        make_morphology(MorphOp::Erode, 1, false),
        &["-morphology", "Erode", "Square:1"],
        output_dir,
    ) {
        results.push(r);
    }

    // --- Motion blur (new filter) ---
    if let Some(r) = compare_op(
        source,
        source_path,
        image_name,
        "motion_blur_0_15",
        make_motion_blur(0.0, 15.0),
        &["-motion-blur", "0x15+0"],
        output_dir,
    ) {
        results.push(r);
    }

    // --- Zenfilters-only effects (no IM equivalent needed) ---
    {
        let zen_result = apply_zenfilter(
            source,
            Box::new(Convolve::new(ConvolutionKernel::ridge_detect())),
        );
        let path = output_dir
            .join(image_name)
            .join("ridge_detect_zen.png");
        save_image(&zen_result, &path);
        let score = zensim_score(source, &zen_result);
        eprintln!("  {:25}  zen={score:6.1}  (zen only)", "ridge_detect");
    }

    results
}

// ─── Main test ─────────────────────────────────────────────────────

#[test]
fn compare_zenfilters_vs_imagemagick() {
    let output_dir = Path::new("/mnt/v/output/zenfilters/comparison");

    let test_images: Vec<(&str, &str)> = vec![
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
            None => {
                eprintln!("SKIP: {filename} not found");
                continue;
            }
        };
        let img = match load_test_image(filename) {
            Some(i) => i,
            None => continue,
        };
        let results = run_suite(&img, &src_path, label, output_dir);
        all_results.extend(results);
        images_tested += 1;
    }

    // ─── Summary ───────────────────────────────────────────────────
    eprintln!("\n{}", "=".repeat(70));
    eprintln!("SUMMARY: {images_tested} images tested\n");

    // Aggregate by filter
    let mut filter_names: Vec<String> = all_results.iter().map(|r| r.filter_name.clone()).collect();
    filter_names.sort();
    filter_names.dedup();

    eprintln!(
        "  {:25}  {:>8}  {:>8}  {:>9}  {}",
        "filter", "avg_zen", "avg_im", "avg_agree", "zen_wins"
    );
    for fname in &filter_names {
        let matching: Vec<&ComparisonResult> =
            all_results.iter().filter(|r| &r.filter_name == fname).collect();
        let n = matching.len() as f64;
        let avg_zen: f64 = matching.iter().map(|r| r.zen_vs_src).sum::<f64>() / n;
        let avg_im: f64 = matching.iter().map(|r| r.im_vs_src).sum::<f64>() / n;
        let avg_agree: f64 = matching.iter().map(|r| r.zen_vs_im).sum::<f64>() / n;
        let wins = matching
            .iter()
            .filter(|r| r.zen_vs_src >= r.im_vs_src)
            .count();
        eprintln!(
            "  {fname:25}  {avg_zen:8.1}  {avg_im:8.1}  {avg_agree:9.1}  {wins}/{}",
            matching.len()
        );
    }

    // Assertions
    assert!(
        images_tested >= 3,
        "Need at least 3 test images, got {images_tested}"
    );

    // Every filter should produce non-garbage output
    for r in &all_results {
        assert!(
            r.zen_vs_src > 15.0,
            "{}/{}: zen score {:.1} is too low",
            r.image_name, r.filter_name, r.zen_vs_src
        );
    }
}
