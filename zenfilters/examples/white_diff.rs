//! white_diff — near-white-amplified diff tuned to catch edge jaggies.
//!
//! Ordinary amplified abs-diffs are too gentle near pure white: a 254→255 change
//! is one level, and against a white page the jaggy stair-steps of an edge are
//! invisible. This tool boosts changes *much* harder where either the before or
//! after pixel is near pure white, and renders them near-binary onto black, so a
//! single-level change and the stair-step profile of an aliased edge both pop.
//!
//! For each matching image in two directories it writes:
//!   - `wdiff/<name>.png` — red→yellow near-white-amplified change map (black =
//!     unchanged). Edge jaggies introduced/moved by a filter show as crisp
//!     red staircases.
//!   - `wmag/<name>.png` — "white magnifier" of the AFTER image: the near-white
//!     tonal range is stretched so anything that is not pure white (edges,
//!     jaggies, halos, residual noise) shows as bright structure on black.
//!
//!   cargo run --release --features experimental --example white_diff -- \
//!     --before /mnt/v/zen/ai/_clipartflatten/before \
//!     --after  /mnt/v/zen/ai/_clipartflatten/after  \
//!     --out    /mnt/v/zen/ai/_clipartflatten
//!
//! Tuning: --gain (overall, default 8), --white-boost (extra gain near white,
//! default 6), --mag-gain (magnifier gain, default 14).

#![allow(clippy::needless_range_loop)]

use image::{GrayImage, Luma, Rgb, RgbImage};
use std::path::{Path, PathBuf};

/// 1.0 when a pixel is near pure white (all channels high), 0.0 otherwise.
#[inline]
fn whiteness(p: &Rgb<u8>) -> f32 {
    let m = p[0].min(p[1]).min(p[2]) as f32;
    // smoothstep 235 → 252
    let t = ((m - 235.0) / 17.0).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Near-white-amplified change map for one image pair.
fn white_diff(before: &RgbImage, after: &RgbImage, gain: f32, white_boost: f32) -> RgbImage {
    let (w, h) = before.dimensions();
    let mut out = RgbImage::new(w, h);
    for (px, (a, b)) in out.pixels_mut().zip(before.pixels().zip(after.pixels())) {
        let d = (0..3)
            .map(|c| (a[c] as i32 - b[c] as i32).unsigned_abs())
            .max()
            .unwrap_or(0) as f32;
        // Extra gain where EITHER side is near pure white.
        let wl = whiteness(a).max(whiteness(b));
        let score = (d * gain * (1.0 + white_boost * wl)).clamp(0.0, 255.0) as i32;
        // black → red → yellow: red ramps first, green joins past the midpoint.
        let r = score.min(255) as u8;
        let g = ((score - 128).max(0) * 2).min(255) as u8;
        *px = Rgb([r, g, 0]);
    }
    out
}

/// "White magnifier": stretch the near-white tonal range of `img` so structure
/// hidden in the white (edges, jaggies, halos, noise) shows as bright on black.
fn white_magnifier(img: &RgbImage, mag_gain: f32) -> GrayImage {
    let (w, h) = img.dimensions();
    let mut out = GrayImage::new(w, h);
    for (px, p) in out.pixels_mut().zip(img.pixels()) {
        let m = p[0].min(p[1]).min(p[2]) as f32; // brightest-common channel
        let dev = (255.0 - m).max(0.0); // 0 at pure white, grows as it darkens/colours
        let v = (dev * mag_gain).clamp(0.0, 255.0) as u8;
        *px = Luma([v]);
    }
    out
}

fn list_images(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let ext = p
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x.to_ascii_lowercase());
            if !matches!(ext.as_deref(), Some("png" | "jpg" | "jpeg" | "webp")) {
                continue;
            }
            let rel = p.strip_prefix(dir).unwrap_or(&p);
            let name = rel.with_extension("").to_string_lossy().replace('\\', "/");
            out.push((name, p));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn main() {
    let mut before = PathBuf::new();
    let mut after = PathBuf::new();
    let mut out = PathBuf::new();
    let mut gain = 8.0f32;
    let mut white_boost = 6.0f32;
    let mut mag_gain = 14.0f32;
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--before" if i + 1 < args.len() => {
                before = PathBuf::from(&args[i + 1]);
                i += 1;
            }
            "--after" if i + 1 < args.len() => {
                after = PathBuf::from(&args[i + 1]);
                i += 1;
            }
            "--out" if i + 1 < args.len() => {
                out = PathBuf::from(&args[i + 1]);
                i += 1;
            }
            "--gain" if i + 1 < args.len() => {
                gain = args[i + 1].parse().unwrap_or(8.0);
                i += 1;
            }
            "--white-boost" if i + 1 < args.len() => {
                white_boost = args[i + 1].parse().unwrap_or(6.0);
                i += 1;
            }
            "--mag-gain" if i + 1 < args.len() => {
                mag_gain = args[i + 1].parse().unwrap_or(14.0);
                i += 1;
            }
            other => eprintln!("ignoring unknown arg: {other}"),
        }
        i += 1;
    }
    if before.as_os_str().is_empty() || after.as_os_str().is_empty() {
        eprintln!(
            "usage: white_diff --before <dir> --after <dir> [--out <dir>] [--gain G] [--white-boost B] [--mag-gain M]"
        );
        std::process::exit(2);
    }
    if out.as_os_str().is_empty() {
        out = after.parent().unwrap_or(Path::new(".")).to_path_buf();
    }
    std::fs::create_dir_all(out.join("wdiff")).expect("mkdir wdiff");
    std::fs::create_dir_all(out.join("wmag")).expect("mkdir wmag");

    let items = list_images(&before);
    println!(
        "white_diff — {} images, gain={gain}, white_boost={white_boost}, mag_gain={mag_gain}\n  out={}",
        items.len(),
        out.display()
    );
    let mut done = 0usize;
    let mut max_white_change = 0u8;
    for (name, bpath) in &items {
        let apath = after.join(format!("{name}.png"));
        let (bi, ai) = match (image::open(bpath), image::open(&apath)) {
            (Ok(b), Ok(a)) => (b.to_rgb8(), a.to_rgb8()),
            _ => continue,
        };
        if bi.dimensions() != ai.dimensions() {
            eprintln!("skip {name}: dimension mismatch");
            continue;
        }
        // Track the largest change that occurred on a near-white pixel.
        for (a, b) in bi.pixels().zip(ai.pixels()) {
            if whiteness(a).max(whiteness(b)) > 0.5 {
                let d = (0..3)
                    .map(|c| (a[c] as i32 - b[c] as i32).unsigned_abs() as u8)
                    .max()
                    .unwrap_or(0);
                max_white_change = max_white_change.max(d);
            }
        }
        let wd = white_diff(&bi, &ai, gain, white_boost);
        let wm = white_magnifier(&ai, mag_gain);
        let dpath = out.join("wdiff").join(format!("{name}.png"));
        let mpath = out.join("wmag").join(format!("{name}.png"));
        if let Some(p) = dpath.parent() {
            std::fs::create_dir_all(p).ok();
        }
        if let Some(p) = mpath.parent() {
            std::fs::create_dir_all(p).ok();
        }
        wd.save(&dpath).ok();
        wm.save(&mpath).ok();
        done += 1;
    }
    println!(
        "wrote {done} wdiff + wmag pairs to {} (max near-white per-channel change across corpus: {max_white_change})",
        out.display()
    );
}
