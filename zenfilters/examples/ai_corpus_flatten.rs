//! ai_corpus_flatten — batch-flatten the AI image corpus, whole-frame, re-run-safe.
//!
//! Mapping (top-level dirs under `--root`, recursed):
//!   - `icons`, `infographics`, `marketing` → `ClipartFlatten` cartoon (`--cartoon`)
//!   - `products`                            → conservative white-background snap
//!
//! White snap (ported from the gen-clothing `clean_bg.clean_array` approach that
//! the corpus was validated on): only the *very*-near-white, border-connected
//! background is touched. The editable band is a TINY measured range around the
//! image's average white (mean/std of its near-white pixels); anything darker —
//! shadows, the product, its interior whites — is below the band and left
//! exactly as-is, so shadow edges stay soft (no hard mask boundary). The snap is
//! feathered across the band and skips non-white / coloured backgrounds.
//!
//! Re-run-safe naming (per image, alongside it):
//!   - `_orig_<name>`  — pristine original, written ONCE, never overwritten
//!   - `<name>`        — the flattened result (takes the original filename)
//!   - `_diff_<name>.png` — coloured 10× diff (orig vs flattened)
//!   - `_skip_<name>`  — marker for a non-candidate (and `<name>` is the original)
//! Every run reads the pristine `_orig_` as its source, so repeated runs never
//! degrade the image and never clobber the original.

#![allow(clippy::needless_range_loop)]

use std::path::Path;

use image::{ImageBuffer, Rgb, RgbImage};

use zenfilters::filters::ClipartFlatten;
use zenfilters::{FilterContext, Pipeline, PipelineConfig, apply_to_buffer};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Cartoon,
    White,
}

// ─── cartoon (ClipartFlatten) — whole-frame neighborhood filter ──────

fn cartoon_flatten(img: &RgbImage, cartoon: f32, waviness: f32, flatness: f32) -> Option<RgbImage> {
    let (w, h) = img.dimensions();
    let desc = zenpixels::PixelDescriptor::RGB8_SRGB;
    let input = zenpixels::buffer::PixelBuffer::from_vec(img.as_raw().clone(), w, h, desc).ok()?;
    let mut pipeline = Pipeline::new(PipelineConfig::default()).ok()?;
    let mut f = ClipartFlatten::default();
    f.cartoon = cartoon;
    // The guided-filter base (edge-preserving, no posterization) is what removes
    // AI "undulation"/bubble-noise. Its scale must match the undulation size, so
    // expose it; larger eps flattens more low-variance texture.
    f.waviness_scale = waviness;
    f.flatness = flatness;
    pipeline.push(Box::new(f));
    let mut ctx = FilterContext::new();
    let out = apply_to_buffer(&pipeline, &input, true, &mut ctx).ok()?;
    ImageBuffer::from_raw(w, h, out.copy_to_contiguous_bytes())
}

// ─── helpers ─────────────────────────────────────────────────────────

#[inline]
fn min_chan(p: &Rgb<u8>) -> u8 {
    p[0].min(p[1]).min(p[2])
}
#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if (e1 - e0).abs() < 1e-6 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ─── cartoon candidacy: flat-art vs photographic ─────────────────────

/// True if the image is flat vector-style art (clipart / logos / icons /
/// illustrations) rather than a photograph. Cartoon flattening only suits flat
/// art; photographic heroes and continuous-tone images are skipped.
///
/// Flat art has FEW distinct colours (a handful of constant fills) — clipart
/// ~12k, logos ~21k — whereas photos/complex infographics have 90k–120k. A
/// flat-fraction alone fails (a smooth photographic sky reads as "flat"), so the
/// primary test is a unique-colour cap (with early-exit), backed by a tight
/// near-constant-fill fraction.
fn is_flat_art(img: &RgbImage, max_colors: u32, min_flat_frac: f32) -> bool {
    let (w, h) = img.dimensions();
    if w < 8 || h < 8 {
        return false;
    }
    let mut set: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for p in img.pixels() {
        set.insert(((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32);
        if set.len() as u32 > max_colors {
            return false; // too many distinct colours → photographic / continuous-tone
        }
    }
    // Tight near-constant-fill fraction (subsampled): flat art is mostly exact fills.
    let step = ((w.max(h) as f32) / 220.0).ceil() as u32;
    let step = step.max(1);
    let mut flat = 0u32;
    let mut tot = 0u32;
    let mut y = 0;
    while y + step < h {
        let mut x = 0;
        while x + step < w {
            let p = img.get_pixel(x, y);
            let r = img.get_pixel(x + step, y);
            let d = img.get_pixel(x, y + step);
            let g = (0..3)
                .map(|c| {
                    (p[c] as i32 - r[c] as i32)
                        .abs()
                        .max((p[c] as i32 - d[c] as i32).abs())
                })
                .max()
                .unwrap_or(0);
            if g <= 2 {
                flat += 1;
            }
            tot += 1;
            x += step;
        }
        y += step;
    }
    tot > 0 && (flat as f32 / tot as f32) >= min_flat_frac
}

// ─── conservative white-background snap ──────────────────────────────

/// Two-pass chamfer distance (in px) to the nearest `source[i] == 0` pixel.
fn chamfer(source: &[u8], w: usize, h: usize) -> Vec<f32> {
    const BIG: f32 = 1e9;
    const D1: f32 = 1.0;
    const D2: f32 = std::f32::consts::SQRT_2;
    let mut d: Vec<f32> = source
        .iter()
        .map(|&s| if s == 0 { 0.0 } else { BIG })
        .collect();
    if w == 0 || h == 0 {
        return d;
    }
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if d[i] == 0.0 {
                continue;
            }
            let mut b = d[i];
            if x > 0 {
                b = b.min(d[i - 1] + D1);
            }
            if y > 0 {
                b = b.min(d[i - w] + D1);
                if x > 0 {
                    b = b.min(d[i - w - 1] + D2);
                }
                if x + 1 < w {
                    b = b.min(d[i - w + 1] + D2);
                }
            }
            d[i] = b;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            if d[i] == 0.0 {
                continue;
            }
            let mut b = d[i];
            if x + 1 < w {
                b = b.min(d[i + 1] + D1);
            }
            if y + 1 < h {
                b = b.min(d[i + w] + D1);
                if x + 1 < w {
                    b = b.min(d[i + w + 1] + D2);
                }
                if x > 0 {
                    b = b.min(d[i + w - 1] + D2);
                }
            }
            d[i] = b;
        }
    }
    d
}

/// Snap the border-connected, very-near-white background to pure white, within
/// a tiny measured band around the image's average white. The snap is faded out
/// within `shadow_radius` px of any non-near-white ("no-go") pixel, so it never
/// runs right up against a shadow/product edge (avoids a hard boundary). Returns
/// (result, snapped); `snapped == false` => non-white background / nothing to do.
fn white_snap(orig: &RgbImage, skip_floor: u8, ramp: f32, shadow_radius: f32) -> (RgbImage, bool) {
    let (w, h) = orig.dimensions();
    let (wu, hu) = (w as usize, h as usize);

    // Border min-channel stats (the background sample).
    let mut border: Vec<u8> = Vec::new();
    for x in 0..w {
        border.push(min_chan(orig.get_pixel(x, 0)));
        border.push(min_chan(orig.get_pixel(x, h - 1)));
    }
    for y in 0..h {
        border.push(min_chan(orig.get_pixel(0, y)));
        border.push(min_chan(orig.get_pixel(w - 1, y)));
    }
    border.sort_unstable();
    let median = border[border.len() / 2];
    if median < skip_floor {
        return (orig.clone(), false); // not a near-white background shot
    }

    // Average white = mean/std of the near-white border pixels (min-chan >= 244).
    let near: Vec<f32> = border
        .iter()
        .filter(|&&v| v >= 244)
        .map(|&v| v as f32)
        .collect();
    let (white_mean, white_std) = if near.len() >= 8 {
        let m = near.iter().sum::<f32>() / near.len() as f32;
        let var = near.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / near.len() as f32;
        (m, var.sqrt())
    } else {
        (252.0, 1.0)
    };
    // Tiny editable band: a few levels / std below the average white.
    let thresh = (white_mean - (5.0 + 4.0 * white_std)).clamp(244.0, 252.0);
    let thresh_u8 = thresh as u8;

    // Flood-fill the border-connected near-white region (min-chan >= thresh).
    let keep = |x: u32, y: u32| min_chan(orig.get_pixel(x, y)) >= thresh_u8;
    let mut mask = vec![0u8; wu * hu];
    let mut stack: Vec<u32> = Vec::new();
    let push = |x: u32, y: u32, mask: &mut [u8], st: &mut Vec<u32>| {
        let i = (y as usize) * wu + x as usize;
        if mask[i] == 0 && keep(x, y) {
            mask[i] = 1;
            st.push(i as u32);
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
    if stack.is_empty() {
        return (orig.clone(), false);
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

    // Spatial distance from the nearest "no-go" pixel (anything not near-white:
    // shadows, the product). The snap is faded out within `shadow_radius` px of
    // those, so it stays a radius away from shadow/product edges.
    let nogo_src: Vec<u8> = (0..wu * hu)
        .map(|i| {
            let (x, y) = ((i % wu) as u32, (i / wu) as u32);
            if min_chan(orig.get_pixel(x, y)) >= thresh_u8 {
                1
            } else {
                0
            }
        })
        .collect();
    let dist = chamfer(&nogo_src, wu, hu);
    let radius = shadow_radius.max(0.5);

    // Snap to PURE white (255) — a clean white background, no off-white clash
    // with the white page. The edge line is avoided not by under-brightening but
    // by a LARGE spatial feather: the snap eases from full 255 (open background)
    // down to zero over `shadow_radius` px as it approaches any shadow/product,
    // so the 255→original transition is spread out and imperceptible. The feather
    // only ever touches near-white background pixels (the flood mask); it fades to
    // zero at the no-go boundary and never modifies product/shadow pixels.
    let snap_target = 255.0f32;
    let mut out = orig.clone();
    for y in 0..h {
        for x in 0..w {
            let i = (y as usize) * wu + x as usize;
            if mask[i] == 0 {
                continue;
            }
            let p = *orig.get_pixel(x, y);
            let w_lum = smoothstep(thresh, thresh + ramp, min_chan(&p) as f32);
            let w_spatial = smoothstep(0.0, radius, dist[i]);
            let wgt = w_lum * w_spatial;
            if wgt <= 0.0 {
                continue;
            }
            let mix = |c: u8| {
                (c as f32 + (snap_target - c as f32) * wgt)
                    .round()
                    .clamp(0.0, 255.0) as u8
            };
            out.put_pixel(x, y, Rgb([mix(p[0]), mix(p[1]), mix(p[2])]));
        }
    }
    (out, true)
}

// ─── coloured, magnified diff ────────────────────────────────────────

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
                continue;
            }
            let is_white = o[0] >= white_u8 && o[1] >= white_u8 && o[2] >= white_u8;
            let (cr, cg, cb) = if is_white {
                (255u32, 0, 0)
            } else {
                (255u32, 165, 0)
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
    name.starts_with("_orig_") || name.starts_with("_diff_") || name.starts_with("_skip_")
}

struct Cfg {
    max_colors: u32,
    min_flat_frac: f32,
    amp: u32,
    cartoon: f32,
    waviness: f32,
    flatness: f32,
    skip_floor: u8,
    ramp: f32,
    shadow_radius: f32,
}

fn process_image(path: &Path, mode: Mode, cfg: &Cfg, stats: &mut Stats) {
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
    let orig_path = parent.join(format!("_orig_{name}"));

    if !orig_path.exists() {
        if let Err(e) = std::fs::copy(path, &orig_path) {
            eprintln!("error  backup {}: {e}", orig_path.display());
            stats.errors += 1;
            return;
        }
    }
    let img = match image::open(&orig_path) {
        Ok(im) => im.to_rgb8(),
        Err(e) => {
            eprintln!("error  open {}: {e}", orig_path.display());
            stats.errors += 1;
            return;
        }
    };

    let (flat, processed) = match mode {
        Mode::White => white_snap(&img, cfg.skip_floor, cfg.ramp, cfg.shadow_radius),
        Mode::Cartoon => {
            if is_flat_art(&img, cfg.max_colors, cfg.min_flat_frac) {
                match cartoon_flatten(&img, cfg.cartoon, cfg.waviness, cfg.flatness) {
                    Some(f) => (f, true),
                    None => {
                        eprintln!("error  cartoon {}", path.display());
                        stats.errors += 1;
                        return;
                    }
                }
            } else {
                (img.clone(), false)
            }
        }
    };

    if !processed {
        let _ = std::fs::copy(&orig_path, path);
        let _ = std::fs::copy(&orig_path, parent.join(format!("_skip_{name}")));
        let _ = std::fs::remove_file(parent.join(format!("_diff_{stem}.png")));
        stats.skipped += 1;
        return;
    }

    let diff = color_diff(&img, &flat, 242, cfg.amp);
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

fn walk(dir: &Path, mode: Mode, cfg: &Cfg, stats: &mut Stats) {
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
            walk(&path, mode, cfg, stats);
            continue;
        }
        if let Some(n) = path.file_name().and_then(|s| s.to_str()) {
            if is_image(n) && !is_output(n) {
                process_image(&path, mode, cfg, stats);
            }
        }
    }
}

fn main() {
    let mut root = String::from("/mnt/v/zen/ai-corpus");
    let mut cfg = Cfg {
        max_colors: 80000,
        min_flat_frac: 0.55,
        amp: 10,
        cartoon: 1.0,
        waviness: 3.0,
        flatness: 0.0010,
        skip_floor: 235,
        ramp: 6.0,
        shadow_radius: 64.0,
    };
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        let val = args.get(i + 1).cloned();
        let mut took = false;
        match args[i].as_str() {
            "--root" => {
                if let Some(v) = val {
                    root = v;
                    took = true;
                }
            }
            "--min-flat-frac" => {
                if let Some(v) = val {
                    cfg.min_flat_frac = v.parse().unwrap_or(cfg.min_flat_frac);
                    took = true;
                }
            }
            "--max-colors" => {
                if let Some(v) = val {
                    cfg.max_colors = v.parse().unwrap_or(cfg.max_colors);
                    took = true;
                }
            }
            "--diff-amp" => {
                if let Some(v) = val {
                    cfg.amp = v.parse().unwrap_or(cfg.amp);
                    took = true;
                }
            }
            "--cartoon" => {
                if let Some(v) = val {
                    cfg.cartoon = v.parse().unwrap_or(cfg.cartoon);
                    took = true;
                }
            }
            "--waviness" => {
                if let Some(v) = val {
                    cfg.waviness = v.parse().unwrap_or(cfg.waviness);
                    took = true;
                }
            }
            "--flatness" => {
                if let Some(v) = val {
                    cfg.flatness = v.parse().unwrap_or(cfg.flatness);
                    took = true;
                }
            }
            "--skip-floor" => {
                if let Some(v) = val {
                    cfg.skip_floor = v.parse().unwrap_or(cfg.skip_floor);
                    took = true;
                }
            }
            "--white-ramp" => {
                if let Some(v) = val {
                    cfg.ramp = v.parse().unwrap_or(cfg.ramp);
                    took = true;
                }
            }
            "--shadow-radius" => {
                if let Some(v) = val {
                    cfg.shadow_radius = v.parse().unwrap_or(cfg.shadow_radius);
                    took = true;
                }
            }
            other => eprintln!("ignoring arg: {other}"),
        }
        i += if took { 2 } else { 1 };
    }

    let root = Path::new(&root);
    // Cartoon dirs are flat-art candidates; `is_flat_art` skips photographic
    // content (e.g. marketing heroes) within them. Products get the white snap.
    // `clipart` is deliberately EXCLUDED — it was already hand-fixed; do not touch
    // it. `illustrations` are detailed painterly art (~290k colours), not flat
    // clipart, so they're excluded too. infographics stay in the list but
    // is_flat_art skips the complex/photographic ones.
    let jobs: [(&str, Mode); 4] = [
        ("icons", Mode::Cartoon),
        ("infographics", Mode::Cartoon),
        ("marketing", Mode::Cartoon),
        ("products", Mode::White),
    ];
    println!(
        "ai_corpus_flatten: root={} cartoon={} min_flat_frac={} skip_floor={} white_ramp={} shadow_radius={} diff_amp={}",
        root.display(),
        cfg.cartoon,
        cfg.min_flat_frac,
        cfg.skip_floor,
        cfg.ramp,
        cfg.shadow_radius,
        cfg.amp
    );
    let mut total = Stats::default();
    for (dir, mode) in jobs {
        let path = root.join(dir);
        if !path.is_dir() {
            eprintln!("missing dir, skipping: {}", path.display());
            continue;
        }
        let mut s = Stats::default();
        walk(&path, mode, &cfg, &mut s);
        let mn = if mode == Mode::Cartoon {
            "cartoon"
        } else {
            "white"
        };
        println!(
            "  {dir:14} [{mn:7}]  processed={:<5} skipped={:<5} errors={}",
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
