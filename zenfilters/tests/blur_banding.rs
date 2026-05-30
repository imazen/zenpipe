//! Regression test for content-dependent horizontal dark-strip artifacts in the
//! SIMD Gaussian blur path (the suspected source of "banding" seen in
//! ClipartFlatten / guided-filter output).
//!
//! Strategy: the scalar blur (`gaussian_blur_plane_scalar`) is the reference.
//! The dispatched SIMD blur (`gaussian_blur_plane`) must match it everywhere.
//! Any *row* where the two diverge beyond fp tolerance is a banding source —
//! and the per-row breakdown localizes which sigma regime (FIR vs stackblur) and
//! which rows are affected.
//!
//! Requires the `experimental` feature (exposes `blur_internals`).

#![cfg(feature = "experimental")]

use zenfilters::FilterContext;
use zenfilters::blur_internals::{
    GaussianKernel, gaussian_blur_plane, gaussian_blur_plane_scalar,
};

/// Build a banding-prone test plane: a large near-flat region carrying a gentle
/// vertical gradient plus low-amplitude content (the regime where AI-clipart
/// flats live), with a sharp horizontal feature partway down to seed any
/// stateful divergence.
fn make_plane(w: usize, h: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            // Gentle vertical gradient 0.80..0.98 + tiny deterministic ripple.
            let g = 0.80 + 0.18 * (y as f32 / (h as f32 - 1.0));
            let ripple = (((x * 7 + y * 13) % 11) as f32 / 11.0 - 0.5) * 0.01;
            v[y * w + x] = g + ripple;
        }
    }
    // A sharp dark horizontal stripe ~40% down (a strong vertical-gradient edge).
    let sy = (h * 2) / 5;
    for x in 0..w {
        v[sy * w + x] = 0.15;
        v[(sy + 1) * w + x] = 0.15;
    }
    v
}

/// Max absolute per-row difference between two planes → (worst_row, worst_diff).
fn worst_row_diff(a: &[f32], b: &[f32], w: usize, h: usize) -> (usize, f32) {
    let mut worst = 0.0f32;
    let mut worst_y = 0;
    for y in 0..h {
        let mut row_max = 0.0f32;
        for x in 0..w {
            let d = (a[y * w + x] - b[y * w + x]).abs();
            if d > row_max {
                row_max = d;
            }
        }
        if row_max > worst {
            worst = row_max;
            worst_y = y;
        }
    }
    (worst_y, worst)
}

fn check_sigma(w: usize, h: usize, sigma: f32) -> (usize, f32) {
    let src = make_plane(w, h);
    let kernel = GaussianKernel::new(sigma);
    let mut ctx = FilterContext::new();

    let mut simd = vec![0.0f32; w * h];
    gaussian_blur_plane(&src, &mut simd, w as u32, h as u32, &kernel, &mut ctx);

    let mut scalar = vec![0.0f32; w * h];
    gaussian_blur_plane_scalar(&src, &mut scalar, w as u32, h as u32, &kernel, &mut ctx);

    worst_row_diff(&simd, &scalar, w, h)
}

/// SIMD and scalar blur must agree to fp tolerance at every sigma regime.
/// FIR path is sigma < 6, stackblur path is sigma >= 6.
#[test]
fn simd_matches_scalar_no_banding() {
    // Non-multiple-of-8 width to exercise the SIMD tail handling too.
    let (w, h) = (200usize, 300usize);
    let sigmas = [1.0f32, 2.0, 3.0, 5.0, 5.9, 6.0, 8.0, 12.0, 20.0];
    let tol = 2e-3f32; // generous: FIR vs stackblur are different kernels at >=6
    let mut failures = Vec::new();
    for &s in &sigmas {
        let (y, d) = check_sigma(w, h, s);
        eprintln!("sigma={s:>5}: worst_row_diff={d:.5} at row {y}/{h}");
        if d > tol {
            failures.push((s, y, d));
        }
    }
    assert!(
        failures.is_empty(),
        "SIMD/scalar blur diverged (banding source): {failures:?}"
    );
}

/// Tighter check on the FIR regime alone (sigma < 6) — same kernel, so SIMD and
/// scalar should be near-identical. A large divergence here is a true bug.
#[test]
fn fir_path_simd_matches_scalar_tight() {
    let (w, h) = (200usize, 300usize);
    let tol = 1e-4f32;
    let mut failures = Vec::new();
    for &s in &[1.0f32, 2.0, 3.0, 4.0, 5.0, 5.9] {
        let (y, d) = check_sigma(w, h, s);
        eprintln!("FIR sigma={s:>4}: worst_row_diff={d:.6} at row {y}/{h}");
        if d > tol {
            failures.push((s, y, d));
        }
    }
    assert!(
        failures.is_empty(),
        "FIR SIMD diverged from scalar (banding): {failures:?}"
    );
}
