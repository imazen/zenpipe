//! Reproduce the horizontal-band artifact against the REAL ClipartFlatten filter
//! on a synthetic gradient-background image (the content that triggers it).
//!
//! Hypothesis: banding is the region-mean snap stepping where a smooth gradient
//! crosses a quantization boundary — an algorithmic artifact, not a SIMD bug.
//! This test localizes per-row darkening introduced by the filter and reports
//! which `cartoon` setting triggers it.

use zenfilters::filters::ClipartFlatten;
use zenfilters::{Filter, FilterContext, OklabPlanes};

/// A near-flat light background with a gentle vertical gradient (0.78..0.97 in L)
/// plus a centered dark "subject". This is exactly the regime where AI product
/// shots / infographics band.
fn gradient_bg(w: usize, h: usize) -> OklabPlanes {
    let mut p = OklabPlanes::new(w as u32, h as u32);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let g = 0.78 + 0.19 * (y as f32 / (h as f32 - 1.0));
            let ripple = (((x * 3 + y * 7) % 9) as f32 / 9.0 - 0.5) * 0.006;
            p.l[i] = g + ripple;
            p.a[i] = 0.0;
            p.b[i] = 0.0;
        }
    }
    // centered dark subject block
    let (x0, x1, y0, y1) = (w * 2 / 5, w * 3 / 5, h * 2 / 5, h * 3 / 5);
    for y in y0..y1 {
        for x in x0..x1 {
            p.l[y * w + x] = 0.25;
            p.a[y * w + x] = 0.03;
        }
    }
    p
}

/// Worst localized per-row darkening the filter introduced, vs neighbours ±k
/// rows. A band = a row darkened much more than the rows above/below (a step).
fn worst_band(orig: &[f32], out: &[f32], w: usize, h: usize) -> (usize, f32) {
    // per-row mean delta (orig - out); positive = filter darkened the row
    let row_delta: Vec<f32> = (0..h)
        .map(|y| {
            let mut s = 0.0f32;
            for x in 0..w {
                s += orig[y * w + x] - out[y * w + x];
            }
            s / w as f32
        })
        .collect();
    let k = 10usize;
    let mut worst = 0.0f32;
    let mut wy = 0;
    for y in 0..h {
        let lo = y.saturating_sub(k);
        let hi = (y + k).min(h - 1);
        let local = row_delta[y] - 0.5 * (row_delta[lo] + row_delta[hi]);
        if local.abs() > worst {
            worst = local.abs();
            wy = y;
        }
    }
    (wy, worst)
}

fn run(cartoon: f32) -> (usize, f32) {
    let (w, h) = (256usize, 384usize);
    let p = gradient_bg(w, h);
    let orig_l = p.l.clone();
    let mut out = p;
    let mut f = ClipartFlatten::default();
    f.cartoon = cartoon;
    f.apply(&mut out, &mut FilterContext::new());
    worst_band(&orig_l, &out.l, w, h)
}

#[test]
fn clipart_gradient_bg_band_profile() {
    for &c in &[0.0f32, 0.5, 1.0] {
        let (y, mag) = run(c);
        // L is 0..1; a band of 0.02 L (~5/255) is clearly visible.
        eprintln!("cartoon={c}: worst row-band = {mag:.4} L at row {y} (>0.02 = visible)");
    }
}

/// Gentle mode (cartoon=0, eases toward the edge-preserving guided base) must
/// NOT introduce visible horizontal steps on a smooth gradient background.
#[test]
fn clipart_gentle_no_visible_band() {
    let (y, mag) = run(0.0);
    assert!(
        mag < 0.02,
        "gentle ClipartFlatten introduced a visible horizontal band: {mag:.4} L at row {y}"
    );
}

/// FULL cartoon mode must ALSO not band a smooth gradient background: the
/// soft palette-blend snap is ≈ identity along an evenly-quantized gradient, so
/// quantization-cell boundaries don't become visible steps.
#[test]
fn clipart_cartoon_no_visible_band_on_gradient() {
    let (y, mag) = run(1.0);
    assert!(
        mag < 0.02,
        "cartoon ClipartFlatten banded a smooth gradient: {mag:.4} L at row {y}"
    );
}

/// A multi-flat-colour "logo" with undulation: several large constant-colour
/// blocks, each carrying low-amplitude waviness. Full cartoon mode MUST still
/// collapse the undulation inside the flat blocks (that's its whole purpose for
/// logos / marketing heroes / clipart). This guards against the anti-banding
/// change turning cartoon into a no-op.
fn flat_blocks_with_undulation(w: usize, h: usize) -> OklabPlanes {
    let mut p = OklabPlanes::new(w as u32, h as u32);
    // Four constant base colours in quadrants (distinct in L and chroma).
    let bases = [
        [0.55f32, 0.10, -0.05],
        [0.72, -0.08, 0.06],
        [0.40, 0.04, 0.10],
        [0.85, 0.0, 0.0],
    ];
    for y in 0..h {
        for x in 0..w {
            let q = (if x >= w / 2 { 1 } else { 0 }) + (if y >= h / 2 { 2 } else { 0 });
            let b = bases[q];
            // Low-amplitude undulation (the artifact to remove) — smooth, not noise.
            let u = ((x as f32 * 0.13).sin() + (y as f32 * 0.11).sin()) * 0.012;
            let i = y * w + x;
            p.l[i] = b[0] + u;
            p.a[i] = b[1] + u * 0.3;
            p.b[i] = b[2] - u * 0.3;
        }
    }
    p
}

fn region_l_var(l: &[f32], w: usize, x0: usize, x1: usize, y0: usize, y1: usize) -> f32 {
    let mut s = 0.0f32;
    let mut c = 0usize;
    for y in y0..y1 {
        for x in x0..x1 {
            s += l[y * w + x];
            c += 1;
        }
    }
    let m = s / c as f32;
    let mut v = 0.0f32;
    for y in y0..y1 {
        for x in x0..x1 {
            let d = l[y * w + x] - m;
            v += d * d;
        }
    }
    v / c as f32
}

#[test]
fn clipart_cartoon_still_flattens_flat_blocks() {
    let (w, h) = (256usize, 256usize);
    let p = flat_blocks_with_undulation(w, h);
    let before = region_l_var(&p.l, w, 8, w / 2 - 8, 8, h / 2 - 8); // top-left block interior
    let mut out = p;
    let mut f = ClipartFlatten::default();
    f.cartoon = 1.0;
    f.apply(&mut out, &mut FilterContext::new());
    let after = region_l_var(&out.l, w, 8, w / 2 - 8, 8, h / 2 - 8);
    assert!(
        after < before * 0.5,
        "cartoon must still collapse undulation in flat blocks: var {before:.6} -> {after:.6}"
    );
}
