//! whitebg_corpus — demonstrate and validate the [`BackgroundFlatten`] filter
//! on a corpus, with a full automated checks-and-balances loop:
//!
//! For every image it:
//!   1. runs `BackgroundFlatten` through the pipeline,
//!   2. scores the change with **zensim** (psychovisual similarity),
//!   3. **scales the strength back** (binary search) so the change stays just
//!      under a "barely visible" zensim threshold — or **skips** if even a
//!      faint edit is too visible,
//!   4. writes `before` / `after` / `diff` images plus a CSV report so the
//!      result can be eyeballed across the corpus.
//!
//! Inputs:
//!   - a set of built-in synthetic white-background product scenes (clean,
//!     noisy, gradient, color-cast, halo) so the demo is self-contained,
//!   - a few CID22 photos (if the corpus is present) as a *no-op safety* check
//!     — the filter must barely touch a non-white-background photo,
//!   - any real images from `--input <dir>`.
//!
//! Run:
//!   cargo run --release --features experimental --example whitebg_corpus
//!   cargo run --release --features experimental --example whitebg_corpus -- \
//!       --input /path/to/products --out /mnt/v/output/zenfilters/whitebg --min-zensim 90
//!
//! Output defaults to `/mnt/v/output/zenfilters/whitebg/`.

use image::{ImageBuffer, Rgb, RgbImage};
use std::path::{Path, PathBuf};

use zenfilters::filters::BackgroundFlatten;
use zenfilters::{FilterContext, Pipeline, PipelineConfig, apply_to_buffer};
use zensim::{RgbSlice, Zensim, ZensimProfile};

// ─── pipeline bridge ────────────────────────────────────────────────

/// Run `BackgroundFlatten` at the given strength over an sRGB8 image.
fn flatten(img: &RgbImage, strength: f32) -> RgbImage {
    let (w, h) = img.dimensions();
    let desc = zenpixels::PixelDescriptor::RGB8_SRGB;
    let input = zenpixels::buffer::PixelBuffer::from_vec(img.as_raw().clone(), w, h, desc).unwrap();

    let mut pipeline = Pipeline::new(PipelineConfig::default()).unwrap();
    let mut bg = BackgroundFlatten::default();
    bg.strength = strength;
    pipeline.push(Box::new(bg));

    let mut ctx = FilterContext::new();
    let out = apply_to_buffer(&pipeline, &input, true, &mut ctx).unwrap();
    ImageBuffer::from_raw(w, h, out.copy_to_contiguous_bytes()).unwrap()
}

/// zensim similarity (≈100 = identical, lower = more visible change).
fn zensim_score(a: &RgbImage, b: &RgbImage) -> f64 {
    let a_px: &[[u8; 3]] = bytemuck::cast_slice(a.as_raw());
    let b_px: &[[u8; 3]] = bytemuck::cast_slice(b.as_raw());
    let (w, h) = a.dimensions();
    let z = Zensim::new(ZensimProfile::latest()).with_parallel(false);
    let src = RgbSlice::new(a_px, w as usize, h as usize);
    let dst = RgbSlice::new(b_px, w as usize, h as usize);
    z.compute(&src, &dst).unwrap().score()
}

struct GateResult {
    out: RgbImage,
    strength: f32,
    score_full: f64,
    score_final: f64,
}

/// Apply the filter, score with zensim, and scale the strength back (binary
/// search) until the change is at least `min_zensim` similar — i.e. barely
/// visible. If even a faint edit drops below the threshold, the edit is skipped.
fn gated_flatten(img: &RgbImage, min_zensim: f64) -> GateResult {
    let full = flatten(img, 1.0);
    let score_full = zensim_score(img, &full);
    if score_full >= min_zensim {
        return GateResult {
            out: full,
            strength: 1.0,
            score_full,
            score_final: score_full,
        };
    }

    // Binary-search the largest strength whose change stays subtle enough.
    // `lo` is always acceptable (strength 0 ⇒ identity ⇒ score 100).
    let mut lo = 0.0f32;
    let mut hi = 1.0f32;
    let mut best = img.clone();
    let mut best_s = 0.0f32;
    let mut best_score = 100.0f64;
    for _ in 0..7 {
        let mid = 0.5 * (lo + hi);
        let candidate = flatten(img, mid);
        let score = zensim_score(img, &candidate);
        if score >= min_zensim {
            lo = mid;
            best = candidate;
            best_s = mid;
            best_score = score;
        } else {
            hi = mid;
        }
    }
    GateResult {
        out: best,
        strength: best_s,
        score_full,
        score_final: best_score,
    }
}

// ─── diff heatmap ───────────────────────────────────────────────────

/// Amplified grayscale difference map for manual inspection. Subtle, intended
/// edits are tiny, so the per-pixel max-channel delta is multiplied by `amp`.
fn diff_heatmap(a: &RgbImage, b: &RgbImage, amp: u32) -> RgbImage {
    let (w, h) = a.dimensions();
    let mut out = RgbImage::new(w, h);
    for (px, (pa, pb)) in out.pixels_mut().zip(a.pixels().zip(b.pixels())) {
        let d = (0..3)
            .map(|c| (pa[c] as i32 - pb[c] as i32).unsigned_abs())
            .max()
            .unwrap_or(0);
        let v = (d * amp).min(255) as u8;
        *px = Rgb([v, v, v]);
    }
    out
}

fn mean_max_diff(a: &RgbImage, b: &RgbImage) -> (f64, u8) {
    let mut sum = 0u64;
    let mut max = 0u8;
    for (pa, pb) in a.pixels().zip(b.pixels()) {
        for c in 0..3 {
            let d = (pa[c] as i32 - pb[c] as i32).unsigned_abs() as u8;
            sum += d as u64;
            if d > max {
                max = d;
            }
        }
    }
    (sum as f64 / (a.as_raw().len() as f64), max)
}

// ─── synthetic white-background product scenes ──────────────────────

/// Deterministic per-pixel noise in [-amp, amp].
fn noise(x: u32, y: u32, amp: i32) -> i32 {
    let h = (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)).wrapping_mul(2_654_435_761);
    ((h >> 24) as i32 % (2 * amp + 1)) - amp
}

struct SceneOpts {
    bg: u8,
    bg_noise: i32,
    gradient: bool,
    tint: [i32; 3],
    halo: bool,
}

fn draw_scene(opts: &SceneOpts) -> RgbImage {
    let (w, h) = (256u32, 256u32);
    let mut img = RgbImage::new(w, h);
    let (pcx, pcy, pr) = (128.0f32, 138.0f32, 56.0f32); // product circle
    let prod = [55i32, 115, 150]; // teal-ish product
    for y in 0..h {
        for x in 0..w {
            // Background: optionally a vertical gradient, plus tint and noise.
            let grad = if opts.gradient {
                (-14.0 + 28.0 * (y as f32 / (h as f32 - 1.0))) as i32
            } else {
                0
            };
            let mut rgb = [0i32; 3];
            for c in 0..3 {
                rgb[c] = opts.bg as i32 + grad + opts.tint[c] + noise(x, y, opts.bg_noise);
            }

            let dx = x as f32 - pcx;
            let dy = y as f32 - pcy;
            let dist = (dx * dx + dy * dy).sqrt();

            // Product disc with a little radial shading and a detail band.
            if dist <= pr {
                let shade = 1.0 - 0.25 * (dist / pr);
                for c in 0..3 {
                    rgb[c] = (prod[c] as f32 * shade) as i32 + noise(x, y, 3);
                }
                if (dy.abs() < 8.0) && (dx.abs() < pr * 0.8) {
                    for c in 0..3 {
                        rgb[c] += 30; // a lighter detail stripe
                    }
                }
            } else {
                // Soft contact shadow: an ellipse just below the product.
                let sdx = (x as f32 - pcx) / (pr * 1.1);
                let sdy = (y as f32 - (pcy + pr * 0.95)) / (pr * 0.35);
                let sd = sdx * sdx + sdy * sdy;
                if sd < 1.0 {
                    let darken = (1.0 - sd) * 70.0;
                    for c in 0..3 {
                        rgb[c] -= darken as i32;
                    }
                }
                // Bright overshoot halo ring hugging the product edge.
                if opts.halo && dist > pr && dist < pr + 3.0 {
                    for c in 0..3 {
                        rgb[c] += 18;
                    }
                }
            }

            img.put_pixel(
                x,
                y,
                Rgb([
                    rgb[0].clamp(0, 255) as u8,
                    rgb[1].clamp(0, 255) as u8,
                    rgb[2].clamp(0, 255) as u8,
                ]),
            );
        }
    }
    img
}

fn synthetic_scenes() -> Vec<(String, RgbImage)> {
    vec![
        (
            "synth_clean_white".into(),
            draw_scene(&SceneOpts {
                bg: 250,
                bg_noise: 2,
                gradient: false,
                tint: [0, 0, 0],
                halo: false,
            }),
        ),
        (
            "synth_noisy_white".into(),
            draw_scene(&SceneOpts {
                bg: 244,
                bg_noise: 8,
                gradient: false,
                tint: [0, 0, 0],
                halo: false,
            }),
        ),
        (
            "synth_gradient_white".into(),
            draw_scene(&SceneOpts {
                bg: 244,
                bg_noise: 4,
                gradient: true,
                tint: [0, 0, 0],
                halo: false,
            }),
        ),
        (
            "synth_warm_cast".into(),
            draw_scene(&SceneOpts {
                bg: 248,
                bg_noise: 4,
                gradient: false,
                tint: [4, 0, -8],
                halo: false,
            }),
        ),
        (
            "synth_halo".into(),
            draw_scene(&SceneOpts {
                bg: 247,
                bg_noise: 4,
                gradient: false,
                tint: [0, 0, 0],
                halo: true,
            }),
        ),
    ]
}

// ─── CID22 no-op safety samples (optional) ──────────────────────────

fn cid22_dir() -> Option<PathBuf> {
    for c in [
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../codec-corpus/CID22/CID22-512/training/"
        ),
        "/home/lilith/work/codec-corpus/CID22/CID22-512/training/",
    ] {
        let p = PathBuf::from(c);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn cid22_samples() -> Vec<(String, RgbImage)> {
    let dir = match cid22_dir() {
        Some(d) => d,
        None => {
            eprintln!("note: CID22 corpus not found — skipping no-op safety samples");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for name in ["1722183.png", "pexels-photo-2908983.png", "1028637.png"] {
        let p = dir.join(name);
        if let Ok(img) = image::open(&p) {
            out.push((
                format!("cid22_{}", name.trim_end_matches(".png")),
                img.to_rgb8(),
            ));
        }
    }
    out
}

// ─── driver ─────────────────────────────────────────────────────────

fn parse_args() -> (PathBuf, Option<PathBuf>, f64) {
    let mut out = PathBuf::from("/mnt/v/output/zenfilters/whitebg");
    let mut input: Option<PathBuf> = None;
    let mut min_zensim = 90.0f64;
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--out" if i + 1 < args.len() => {
                out = PathBuf::from(&args[i + 1]);
                i += 1;
            }
            "--input" if i + 1 < args.len() => {
                input = Some(PathBuf::from(&args[i + 1]));
                i += 1;
            }
            "--min-zensim" if i + 1 < args.len() => {
                min_zensim = args[i + 1].parse().unwrap_or(90.0);
                i += 1;
            }
            other => eprintln!("ignoring unknown arg: {other}"),
        }
        i += 1;
    }
    (out, input, min_zensim)
}

fn load_dir(dir: &Path) -> Vec<(String, RgbImage)> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("could not read --input {}: {e}", dir.display());
            return out;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        if !matches!(
            ext.as_deref(),
            Some("png" | "jpg" | "jpeg" | "bmp" | "tiff")
        ) {
            continue;
        }
        if let Ok(img) = image::open(&path) {
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("img");
            out.push((format!("input_{name}"), img.to_rgb8()));
        }
    }
    out
}

fn main() {
    let (out_dir, input, min_zensim) = parse_args();
    std::fs::create_dir_all(&out_dir).expect("create output dir");

    let mut images = synthetic_scenes();
    images.extend(cid22_samples());
    if let Some(dir) = &input {
        images.extend(load_dir(dir));
    }

    let mut csv = String::from(
        "name,width,height,strength,zensim_full,zensim_final,mean_diff,max_diff,verdict\n",
    );
    println!(
        "BackgroundFlatten corpus run — {} images, min_zensim={min_zensim}, out={}",
        images.len(),
        out_dir.display()
    );
    println!(
        "{:<26} {:>6} {:>10} {:>10} {:>9} {:>8}  {}",
        "image", "stren", "z_full", "z_final", "meanΔ", "maxΔ", "verdict"
    );

    for (name, img) in &images {
        let (w, h) = img.dimensions();
        let res = gated_flatten(img, min_zensim);
        let (mean_d, max_d) = mean_max_diff(img, &res.out);
        let diff = diff_heatmap(img, &res.out, 12);

        img.save(out_dir.join(format!("{name}_before.png"))).ok();
        res.out.save(out_dir.join(format!("{name}_after.png"))).ok();
        diff.save(out_dir.join(format!("{name}_diff.png"))).ok();

        let verdict = if res.strength <= 0.001 {
            "skipped"
        } else if res.strength >= 0.999 {
            "full"
        } else {
            "scaled-back"
        };
        println!(
            "{name:<26} {:>6.2} {:>10.2} {:>10.2} {:>9.3} {:>8}  {verdict}",
            res.strength, res.score_full, res.score_final, mean_d, max_d
        );
        csv.push_str(&format!(
            "{name},{w},{h},{:.3},{:.3},{:.3},{:.4},{},{verdict}\n",
            res.strength, res.score_full, res.score_final, mean_d, max_d
        ));
    }

    let csv_path = out_dir.join("report.csv");
    std::fs::write(&csv_path, csv).expect("write report.csv");
    println!("\nWrote before/after/diff PNGs + {}", csv_path.display());
}
