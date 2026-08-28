//! Wall-clock cost of the three skew detectors on a 4000×3000 ruled
//! fixture (the zenpipe#27 acceptance budget is < 50 ms mean per detection
//! on the analysis downsample). Run with `--release`:
//!
//! ```text
//! cargo run --release -p zenlayout --example deskew_timing
//! ```
//!
//! Prints mean / min over 20 runs per method and the detected angle, so the
//! number is a measurement of this machine, not an estimate.

use std::time::Instant;
use zenlayout::deskew::{
    detect_skew_gradient_moment, detect_skew_hough, detect_skew_projection_variance,
};

/// Parallel dark lines (3 px thick, 24 px apart), anti-aliased, rotated
/// by `angle_deg` in `RotateEffect`'s convention.
fn ruled(w: usize, h: usize, angle_deg: f32) -> Vec<u8> {
    let (s, c) = angle_deg.to_radians().sin_cos();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let mut px = vec![255u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let p = (y as f32 - cy) * c - (x as f32 - cx) * s;
            let d = ((p % 24.0) + 24.0) % 24.0;
            let cov = (d + 0.5).min(3.5 - d).clamp(0.0, 1.0);
            px[y * w + x] = (255.0 * (1.0 - cov)).round() as u8;
        }
    }
    px
}

fn time<F: FnMut() -> Option<f32>>(name: &str, runs: u32, mut f: F) {
    let mut best = f64::MAX;
    let mut total = 0.0;
    let mut angle = None;
    for _ in 0..runs {
        let t = Instant::now();
        angle = f();
        let ms = t.elapsed().as_secs_f64() * 1e3;
        best = best.min(ms);
        total += ms;
    }
    println!(
        "{name:<22} mean {:7.2} ms  min {:7.2} ms  detected {:?}",
        total / f64::from(runs),
        best,
        angle
    );
}

fn main() {
    let (w, h) = (4000usize, 3000usize);
    let skew = 3.7f32;
    let img = ruled(w, h, skew);
    let (wu, hu) = (w as u32, h as u32);
    println!(
        "{w}x{h} ruled fixture skewed {skew}°, step = {}",
        w.max(h).div_ceil(1000)
    );
    let runs = 20;
    time("gradient_moment", runs, || {
        detect_skew_gradient_moment(&img, wu, hu, w, 10.0)
    });
    time("projection_variance", runs, || {
        detect_skew_projection_variance(&img, wu, hu, w, 10.0)
    });
    time("hough", runs, || {
        detect_skew_hough(&img, wu, hu, w, 10.0, 0.2)
    });
    let conf = zenlayout::deskew::detect_skew_hough_with_confidence(&img, wu, hu, w, 10.0);
    println!("hough (angle, confidence) = {conf:?}");
    for (w, h) in [(1000usize, 750usize), (2000, 1500)] {
        let img = ruled(w, h, skew);
        let conf =
            zenlayout::deskew::detect_skew_hough_with_confidence(&img, w as u32, h as u32, w, 10.0);
        let pv = detect_skew_projection_variance(&img, w as u32, h as u32, w, 10.0);
        println!("{w}x{h}: hough {conf:?}  projection {pv:?}");
    }
}
