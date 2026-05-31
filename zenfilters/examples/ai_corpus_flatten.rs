//! ai_corpus_flatten — batch-flatten the AI image corpus in place.
//!
//! Mapping (top-level dirs under `--root`, recursed):
//!   - `icons`, `infographics`, `marketing` → `ClipartFlatten` cartoon
//!   - `products`                            → `BackgroundFlatten` white-flatten
//!   (other dirs are ignored)
//!
//! Non-destructive, alongside each original `<name>`:
//!   - candidate, processed → `_flattened_<name>` + `_diff_<name>.png`
//!   - non-candidate         → copied to `_skip_<name>` (original left untouched)
//!
//! A pixel is a non-candidate (skipped) when its border shows a strong top↔bottom
//! or left↔right luminance gradient (a backdrop we don't want to "climb"); white
//! mode additionally skips images whose border is not near-white.
//!
//! The diff is the per-pixel change, magnified 10×, coloured by the *original*
//! pixel: red where it was ≥95 % white, orange where it was not (orange =
//! changes to non-white content), black where unchanged.
//!
//! Run:
//!   cargo run --release --features experimental --example ai_corpus_flatten
//!   cargo run --release --features experimental --example ai_corpus_flatten -- \
//!       --root /mnt/v/zen/ai-corpus --grad-thresh 0.08 --diff-amp 10

#![allow(clippy::needless_range_loop)]

use std::path::Path;

use image::{ImageBuffer, Rgb, RgbImage};

use zenfilters::filters::{BackgroundFlatten, ClipartFlatten};
use zenfilters::{Filter, FilterContext, Pipeline, PipelineConfig, apply_to_buffer};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Cartoon,
    White,
}

// ─── pipeline bridge ─────────────────────────────────────────────────

/// Run a single filter over an sRGB8 image and return the result.
fn run_filter(img: &RgbImage, filter: Box<dyn Filter>) -> Option<RgbImage> {
    let (w, h) = img.dimensions();
    let desc = zenpixels::PixelDescriptor::RGB8_SRGB;
    let input = zenpixels::buffer::PixelBuffer::from_vec(img.as_raw().clone(), w, h, desc).ok()?;
    let mut pipeline = Pipeline::new(PipelineConfig::default()).ok()?;
    pipeline.push(filter);
    let mut ctx = FilterContext::new();
    let out = apply_to_buffer(&pipeline, &input, true, &mut ctx).ok()?;
    ImageBuffer::from_raw(w, h, out.copy_to_contiguous_bytes())
}

fn flatten(img: &RgbImage, mode: Mode) -> Option<RgbImage> {
    match mode {
        Mode::White => {
            // This corpus is known product-on-white, and the candidacy gate
            // already routes dark/non-white backgrounds to `_skip_`. Disable the
            // filter's own auto-skip so its conservative central-subject gate
            // doesn't no-op clean ghost/flat shots that happen to be mostly white.
            let mut f = BackgroundFlatten::default();
            f.auto_skip = false;
            run_filter(img, Box::new(f))
        }
        Mode::Cartoon => {
            let mut f = ClipartFlatten::default();
            f.cartoon = 1.0;
            run_filter(img, Box::new(f))
        }
    }
}

// ─── candidacy / skip ────────────────────────────────────────────────

#[inline]
fn luma(p: &Rgb<u8>) -> f32 {
    (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0
}

/// Mean luma of a rectangular region [x0,x1) × [y0,y1).
fn region_luma(img: &RgbImage, x0: u32, x1: u32, y0: u32, y1: u32) -> f32 {
    let mut s = 0.0f32;
    let mut c = 0.0f32;
    for y in y0..y1 {
        for x in x0..x1 {
            s += luma(img.get_pixel(x, y));
            c += 1.0;
        }
    }
    if c > 0.0 { s / c } else { 0.0 }
}

/// Candidacy is judged from the four *corner* patches, which are almost always
/// background (a centred product or an on-model subject contaminates a full edge
/// band but not the corners). A strong top↔bottom or left↔right corner gradient
/// marks a backdrop gradient we don't want to climb; white mode additionally
/// requires the corners to be near-white (else it's a dark/coloured-bg shot with
/// nothing to flatten). Non-candidates are skipped.
fn is_candidate(img: &RgbImage, mode: Mode, grad_thresh: f32) -> bool {
    let (w, h) = img.dimensions();
    if w < 8 || h < 8 {
        return false;
    }
    let cs = (w.min(h) / 8).max(4);
    let tl = region_luma(img, 0, cs, 0, cs);
    let tr = region_luma(img, w - cs, w, 0, cs);
    let bl = region_luma(img, 0, cs, h - cs, h);
    let br = region_luma(img, w - cs, w, h - cs, h);

    let top_c = 0.5 * (tl + tr);
    let bot_c = 0.5 * (bl + br);
    let left_c = 0.5 * (tl + bl);
    let right_c = 0.5 * (tr + br);
    let vgrad = (top_c - bot_c).abs();
    let hgrad = (left_c - right_c).abs();
    if vgrad > grad_thresh || hgrad > grad_thresh {
        return false; // directional backdrop gradient — don't climb it
    }
    if mode == Mode::White {
        let mean_c = 0.25 * (tl + tr + bl + br);
        if mean_c < 0.80 {
            return false; // dark / coloured background — nothing to flatten
        }
    }
    true
}

// ─── coloured, magnified diff ────────────────────────────────────────

/// Per-pixel change, magnified `amp`×, coloured by the *original* pixel:
/// red where it was ≥95 % white, orange where not; black where unchanged.
fn color_diff(orig: &RgbImage, flat: &RgbImage, white_u8: u8, amp: u32) -> RgbImage {
    let (w, h) = orig.dimensions();
    let mut out = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let o = orig.get_pixel(x, y);
            let f = flat.get_pixel(x, y);
            let d = (0..3)
                .map(|c| (o[c] as i32 - f[c] as i32).unsigned_abs())
                .max()
                .unwrap_or(0);
            let mag = (d * amp).min(255);
            if mag == 0 {
                continue; // unchanged → black
            }
            let is_white = o[0] >= white_u8 && o[1] >= white_u8 && o[2] >= white_u8;
            let (cr, cg, cb) = if is_white {
                (255u32, 0u32, 0u32) // red: changed a white pixel (expected whitening)
            } else {
                (255u32, 165u32, 0u32) // orange: changed a non-white pixel
            };
            out.put_pixel(
                x,
                y,
                Rgb([
                    (cr * mag / 255) as u8,
                    (cg * mag / 255) as u8,
                    (cb * mag / 255) as u8,
                ]),
            );
        }
    }
    out
}

// ─── driver ──────────────────────────────────────────────────────────

#[derive(Default)]
struct Stats {
    processed: u32,
    skipped: u32,
    errors: u32,
}

fn is_image(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".png") || n.ends_with(".jpg") || n.ends_with(".jpeg") || n.ends_with(".webp")
}

fn is_output(name: &str) -> bool {
    name.starts_with("_flattened_") || name.starts_with("_diff_") || name.starts_with("_skip_")
}

fn process_image(path: &Path, mode: Mode, grad_thresh: f32, amp: u32, stats: &mut Stats) {
    let parent = match path.parent() {
        Some(p) => p,
        None => return,
    };
    let name = match path.file_name().and_then(|s| s.to_str()) {
        Some(s) => s.to_string(),
        None => return,
    };
    let stem = Path::new(&name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&name)
        .to_string();

    let img = match image::open(path) {
        Ok(im) => im.to_rgb8(),
        Err(e) => {
            eprintln!("error  {}: {e}", path.display());
            stats.errors += 1;
            return;
        }
    };

    if !is_candidate(&img, mode, grad_thresh) {
        let dst = parent.join(format!("_skip_{name}"));
        if let Err(e) = std::fs::copy(path, &dst) {
            eprintln!("error  copy-skip {}: {e}", dst.display());
            stats.errors += 1;
        } else {
            stats.skipped += 1;
        }
        return;
    }

    let flat = match flatten(&img, mode) {
        Some(f) => f,
        None => {
            eprintln!("error  flatten {}", path.display());
            stats.errors += 1;
            return;
        }
    };
    let diff = color_diff(&img, &flat, 242, amp);

    let flat_dst = parent.join(format!("_flattened_{name}"));
    let diff_dst = parent.join(format!("_diff_{stem}.png"));
    let mut ok = true;
    if let Err(e) = flat.save(&flat_dst) {
        eprintln!("error  save {}: {e}", flat_dst.display());
        ok = false;
    }
    if let Err(e) = diff.save(&diff_dst) {
        eprintln!("error  save {}: {e}", diff_dst.display());
        ok = false;
    }
    if ok {
        stats.processed += 1;
    } else {
        stats.errors += 1;
    }
}

fn walk(dir: &Path, mode: Mode, grad_thresh: f32, amp: u32, stats: &mut Stats) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("error  read_dir {}: {e}", dir.display());
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, mode, grad_thresh, amp, stats);
            continue;
        }
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        if is_image(name) && !is_output(name) {
            process_image(&path, mode, grad_thresh, amp, stats);
        }
    }
}

fn main() {
    let mut root = String::from("/mnt/v/zen/ai-corpus");
    let mut grad_thresh = 0.08f32;
    let mut amp = 10u32;
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--root" if i + 1 < args.len() => {
                root = args[i + 1].clone();
                i += 1;
            }
            "--grad-thresh" if i + 1 < args.len() => {
                grad_thresh = args[i + 1].parse().unwrap_or(grad_thresh);
                i += 1;
            }
            "--diff-amp" if i + 1 < args.len() => {
                amp = args[i + 1].parse().unwrap_or(amp);
                i += 1;
            }
            other => eprintln!("ignoring arg: {other}"),
        }
        i += 1;
    }

    let root = Path::new(&root);
    let jobs: [(&str, Mode); 4] = [
        ("icons", Mode::Cartoon),
        ("infographics", Mode::Cartoon),
        ("marketing", Mode::Cartoon),
        ("products", Mode::White),
    ];

    println!(
        "ai_corpus_flatten: root={} grad_thresh={grad_thresh} diff_amp={amp}",
        root.display()
    );
    let mut total = Stats::default();
    for (dir, mode) in jobs {
        let path = root.join(dir);
        if !path.is_dir() {
            eprintln!("missing dir, skipping: {}", path.display());
            continue;
        }
        let mode_name = if mode == Mode::Cartoon { "cartoon" } else { "white" };
        let mut s = Stats::default();
        walk(&path, mode, grad_thresh, amp, &mut s);
        println!(
            "  {dir:14} [{mode_name:7}]  processed={:<5} skipped={:<5} errors={}",
            s.processed, s.skipped, s.errors
        );
        total.processed += s.processed;
        total.skipped += s.skipped;
        total.errors += s.errors;
    }
    println!(
        "DONE  processed={} skipped={} errors={}",
        total.processed, total.skipped, total.errors
    );
}
