//! Compares the inline-scalar `scatter_srgb_passthrough` (baked into this
//! file as `scatter_baseline`) against the garb-routed version exported
//! by zenfilters. Same shape, same allocator, same measurement loop.

use std::time::Instant;
use zenfilters::{scatter_srgb_passthrough, OklabPlanes};

/// The pre-garb body verbatim — naive scalar deinterleave.
fn scatter_baseline(src: &[f32], planes: &mut OklabPlanes, channels: u32) {
    let n = planes.pixel_count();
    let ch = channels as usize;
    debug_assert!(ch == 3 || ch == 4);
    debug_assert!(src.len() >= n * ch);

    for i in 0..n {
        planes.l[i] = src[i * ch];
        planes.a[i] = src[i * ch + 1];
        planes.b[i] = src[i * ch + 2];
    }

    if ch == 4
        && let Some(alpha) = &mut planes.alpha
    {
        for i in 0..n {
            alpha[i] = src[i * ch + 3];
        }
    }
}

fn make_input(pixels: usize, channels: usize) -> Vec<f32> {
    (0..pixels * channels).map(|i| (i as f32) * 0.0125 - 8.0).collect()
}

fn bench(label: &str, iters: usize, mut f: impl FnMut()) -> f64 {
    for _ in 0..3 {
        f();
    }
    let t0 = Instant::now();
    for _ in 0..iters {
        f();
    }
    let dt = t0.elapsed();
    let per_us = dt.as_secs_f64() * 1e6 / iters as f64;
    println!("  {:55} mean = {:9.3} µs / iter", label, per_us);
    per_us
}

fn main() {
    // Strip-sized inputs that match what `Pipeline::apply_*` typically passes.
    // Pipeline strip_height clamps total working set to ~4 MB, so:
    //   1024w → ~341 rows → ~349K pixels
    //   2048w → ~170 rows → ~348K pixels
    //   4096w → ~85  rows → ~348K pixels (RGBA: ~64 rows → ~262K)
    for &(label, w, h) in &[
        ("64K   pixels (256x256)", 256_u32, 256_u32),
        ("262K  pixels (512x512)", 512, 512),
        ("349K  pixels (typical strip)", 1024, 341),
        ("1MP   pixels (1024x1024)", 1024, 1024),
    ] {
        println!("\n=== {} ===", label);
        let pixels = (w as usize) * (h as usize);

        for &(ch_label, ch) in &[("RGB (3-ch)", 3usize), ("RGBA (4-ch)", 4usize)] {
            println!("--- {} ---", ch_label);
            let src = make_input(pixels, ch);
            // Allocate planes ONCE per (size, channels) — reuse across all
            // measured iterations so allocator noise doesn't dominate.
            let mut planes = if ch == 4 {
                OklabPlanes::with_alpha(w, h)
            } else {
                OklabPlanes::new(w, h)
            };
            let iters = if pixels > 500_000 { 200 } else { 1000 };

            let baseline = bench(&format!("baseline (scalar inline)        ch={}", ch), iters, || {
                scatter_baseline(&src, &mut planes, ch as u32);
                std::hint::black_box(&planes);
            });
            let garb = bench(&format!("garb deinterleave dispatch      ch={}", ch), iters, || {
                scatter_srgb_passthrough(&src, &mut planes, ch as u32);
                std::hint::black_box(&planes);
            });
            println!("    speedup: {:.2}× ({:+.1}%)", baseline / garb, (baseline / garb - 1.0) * 100.0);
        }
    }
}
