//! ai_corpus_flatten — batch-flatten the AI image corpus, whole-frame.
//!
//! Mapping (top-level dirs under `--root`, recursed):
//!   - `icons`, `infographics`, `marketing` → `ClipartFlatten` cartoon
//!   - `products`                            → `BackgroundFlatten` white-flatten
//!
//! Re-run-safe naming (per image, alongside it):
//!   - `_orig_<name>`  — pristine original, written ONCE, never overwritten
//!   - `<name>`        — the flattened result (takes the original filename)
//!   - `_diff_<name>.png` — coloured 10× diff (orig vs flattened)
//!   - `_skip_<name>`  — marker for a non-candidate (and `<name>` is restored to the original)
//! Every run reads the pristine `_orig_` as its source, so repeated runs never
//! degrade the image and never clobber the original.
//!
//! White flatten is bounded to the TRUE background: after flattening, a strict
//! border-seeded flood fill through near-white / low-saturation pixels defines
//! the background; the original is restored everywhere outside it. So the diff is
//! always a single contiguous region from the edges inward — light products,
//! coloured products, and shadows are never touched in their interior.

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

// ─── pipeline bridge (whole-frame: BackgroundFlatten/ClipartFlatten are
//     neighborhood filters, so the pipeline processes the full frame at once) ──

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
            let mut f = BackgroundFlatten::default();
            f.auto_skip = false; // candidacy + strict clip handle non-white; don't let the
                                 // central-subject gate no-op clean shots
            run_filter(img, Box::new(f))
        }
        Mode::Cartoon => {
            let mut f = ClipartFlatten::default();
            f.cartoon = 1.0;
            run_filter(img, Box::new(f))
        }
    }
}

// ─── luminance / saturation helpers ─────────────────────────────────

#[inline]
fn luma(p: &Rgb<u8>) -> f32 {
    (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0
}
#[inline]
fn sat(p: &Rgb<u8>) -> f32 {
    let mx = p[0].max(p[1]).max(p[2]) as f32;
    let mn = p[0].min(p[1]).min(p[2]) as f32;
    (mx - mn) / 255.0
}

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

/// Mean luminance of the four corner patches (reliable background sample).
fn corner_bg_luma(img: &RgbImage) -> f32 {
    let (w, h) = img.dimensions();
    let cs = (w.min(h) / 8).max(4);
    0.25 * (region_luma(img, 0, cs, 0, cs)
        + region_luma(img, w - cs, w, 0, cs)
        + region_luma(img, 0, cs, h - cs, h)
        + region_luma(img, w - cs, w, h - cs, h))
}

// ─── candidacy / skip ────────────────────────────────────────────────

/// Judged from the corners (centred / on-model subjects contaminate edge bands
/// but not corners). Skip a strong corner gradient (backdrop we won't climb);
/// white mode also skips non-near-white corners (nothing to flatten).
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
    let vgrad = (0.5 * (tl + tr) - 0.5 * (bl + br)).abs();
    let hgrad = (0.5 * (tl + bl) - 0.5 * (tr + br)).abs();
    if vgrad > grad_thresh || hgrad > grad_thresh {
        return false;
    }
    if mode == Mode::White && 0.25 * (tl + tr + bl + br) < 0.80 {
        return false;
    }
    true
}

// ─── strict background clip (white mode) ─────────────────────────────

/// Restore the original everywhere that is NOT part of the contiguous,
/// border-connected near-white background. A 4-connected flood fill seeds from
/// the image border and grows only through pixels that are near the background
/// level (`luma > bg - luma_margin`) and low-saturation (`sat < sat_thresh`),
/// so it halts at any product edge — including a light/cream product or a
/// coloured one. The flattened result is kept only inside that mask.
fn clip_to_background(orig: &RgbImage, flat: &RgbImage, luma_margin: f32, sat_thresh: f32) -> RgbImage {
    let (w, h) = orig.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    let bg = corner_bg_luma(orig);
    let lo = bg - luma_margin;

    let keep = |x: u32, y: u32| -> bool {
        let p = orig.get_pixel(x, y);
        luma(p) > lo && sat(p) < sat_thresh
    };

    let mut mask = vec![0u8; wu * hu];
    let mut stack: Vec<u32> = Vec::new();
    let push = |x: u32, y: u32, mask: &mut [u8], stack: &mut Vec<u32>| {
        let i = (y as usize) * wu + x as usize;
        if mask[i] == 0 && keep(x, y) {
            mask[i] = 1;
            stack.push(i as u32);
        }
    };
    for x in 0..w {
        push(x, 0, &mut mask, &mut stack);
        push(x, h - 1, &mut mask, &mut stack);
    }
    for y in 0..h {
        push(0, y, &mut mask, &mut stack);
        push(w - 1, y, &mut mask, &mut stack);
    }
    while let Some(idx) = stack.pop() {
        let i = idx as usize;
        let (x, y) = ((i % wu) as u32, (i / wu) as u32);
        if x > 0 {
            push(x - 1, y, &mut mask, &mut stack);
        }
        if x + 1 < w {
            push(x + 1, y, &mut mask, &mut stack);
        }
        if y > 0 {
            push(x, y - 1, &mut mask, &mut stack);
        }
        if y + 1 < h {
            push(x, y + 1, &mut mask, &mut stack);
        }
    }

    let mut out = orig.clone();
    for y in 0..h {
        for x in 0..w {
            if mask[(y as usize) * wu + x as usize] == 1 {
                out.put_pixel(x, y, *flat.get_pixel(x, y));
            }
        }
    }
    out
}

// ─── coloured, magnified diff ────────────────────────────────────────

fn color_diff(orig: &RgbImage, flat: &RgbImage, white_u8: u8, amp: u32) -> RgbImage {
    let (w, h) = orig.dimensions();
    let mut out = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let o = orig.get_pixel(x, y);
            let f = flat.get_pixel(x, y);
            let d = (0..3).map(|c| (o[c] as i32 - f[c] as i32).unsigned_abs()).max().unwrap_or(0);
            let mag = (d * amp).min(255);
            if mag == 0 {
                continue;
            }
            let is_white = o[0] >= white_u8 && o[1] >= white_u8 && o[2] >= white_u8;
            let (cr, cg, cb) = if is_white { (255u32, 0, 0) } else { (255u32, 165, 0) };
            out.put_pixel(x, y, Rgb([(cr * mag / 255) as u8, (cg * mag / 255) as u8, (cb * mag / 255) as u8]));
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
    name.starts_with("_orig_") || name.starts_with("_diff_") || name.starts_with("_skip_")
}

fn process_image(path: &Path, mode: Mode, grad_thresh: f32, amp: u32, lm: f32, st: f32, stats: &mut Stats) {
    let parent = match path.parent() {
        Some(p) => p,
        None => return,
    };
    let name = match path.file_name().and_then(|s| s.to_str()) {
        Some(s) => s.to_string(),
        None => return,
    };
    let stem = Path::new(&name).file_stem().and_then(|s| s.to_str()).unwrap_or(&name).to_string();
    let orig_path = parent.join(format!("_orig_{name}"));

    // Write-once pristine backup: copy the (still-original) working file ONCE.
    // On re-runs `_orig_` already exists and is never overwritten.
    if !orig_path.exists() {
        if let Err(e) = std::fs::copy(path, &orig_path) {
            eprintln!("error  backup {}: {e}", orig_path.display());
            stats.errors += 1;
            return; // never overwrite the working file if the backup failed
        }
    }
    // Always flatten FROM the pristine original.
    let img = match image::open(&orig_path) {
        Ok(im) => im.to_rgb8(),
        Err(e) => {
            eprintln!("error  open {}: {e}", orig_path.display());
            stats.errors += 1;
            return;
        }
    };

    if !is_candidate(&img, mode, grad_thresh) {
        // Restore the original into the working slot and mark skipped.
        let _ = std::fs::copy(&orig_path, path);
        let _ = std::fs::copy(&orig_path, parent.join(format!("_skip_{name}")));
        let _ = std::fs::remove_file(parent.join(format!("_diff_{stem}.png")));
        stats.skipped += 1;
        return;
    }

    let flat0 = match flatten(&img, mode) {
        Some(f) => f,
        None => {
            eprintln!("error  flatten {}", path.display());
            stats.errors += 1;
            return;
        }
    };
    // White flatten is bounded to the true background; cartoon is intentionally
    // whole-image.
    let flat = if mode == Mode::White {
        clip_to_background(&img, &flat0, lm, st)
    } else {
        flat0
    };
    let diff = color_diff(&img, &flat, 242, amp);

    let mut ok = true;
    if let Err(e) = flat.save(path) {
        eprintln!("error  save {}: {e}", path.display());
        ok = false;
    }
    if let Err(e) = diff.save(parent.join(format!("_diff_{stem}.png"))) {
        eprintln!("error  save diff {stem}: {e}");
        ok = false;
    }
    let _ = std::fs::remove_file(parent.join(format!("_skip_{name}")));
    if ok {
        stats.processed += 1;
    } else {
        stats.errors += 1;
    }
}

fn walk(dir: &Path, mode: Mode, grad_thresh: f32, amp: u32, lm: f32, st: f32, stats: &mut Stats) {
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
            walk(&path, mode, grad_thresh, amp, lm, st, stats);
            continue;
        }
        if let Some(n) = path.file_name().and_then(|s| s.to_str()) {
            if is_image(n) && !is_output(n) {
                process_image(&path, mode, grad_thresh, amp, lm, st, stats);
            }
        }
    }
}

fn main() {
    let mut root = String::from("/mnt/v/zen/ai-corpus");
    let mut grad_thresh = 0.08f32;
    let mut amp = 10u32;
    let mut luma_margin = 0.06f32; // how far below the corner background a pixel may be and still be background
    let mut sat_thresh = 0.10f32; // max saturation for a background pixel
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        let next = || args.get(i + 1).cloned();
        match args[i].as_str() {
            "--root" => {
                if let Some(v) = next() {
                    root = v;
                    i += 1;
                }
            }
            "--grad-thresh" => {
                if let Some(v) = next() {
                    grad_thresh = v.parse().unwrap_or(grad_thresh);
                    i += 1;
                }
            }
            "--diff-amp" => {
                if let Some(v) = next() {
                    amp = v.parse().unwrap_or(amp);
                    i += 1;
                }
            }
            "--luma-margin" => {
                if let Some(v) = next() {
                    luma_margin = v.parse().unwrap_or(luma_margin);
                    i += 1;
                }
            }
            "--sat-thresh" => {
                if let Some(v) = next() {
                    sat_thresh = v.parse().unwrap_or(sat_thresh);
                    i += 1;
                }
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
        "ai_corpus_flatten: root={} grad_thresh={grad_thresh} diff_amp={amp} luma_margin={luma_margin} sat_thresh={sat_thresh}",
        root.display()
    );
    let mut total = Stats::default();
    for (dir, mode) in jobs {
        let path = root.join(dir);
        if !path.is_dir() {
            eprintln!("missing dir, skipping: {}", path.display());
            continue;
        }
        let mut s = Stats::default();
        walk(&path, mode, grad_thresh, amp, luma_margin, sat_thresh, &mut s);
        let mn = if mode == Mode::Cartoon { "cartoon" } else { "white" };
        println!("  {dir:14} [{mn:7}]  processed={:<5} skipped={:<5} errors={}", s.processed, s.skipped, s.errors);
        total.processed += s.processed;
        total.skipped += s.skipped;
        total.errors += s.errors;
    }
    println!("DONE  processed={} skipped={} errors={}", total.processed, total.skipped, total.errors);
}
