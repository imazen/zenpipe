//! Diagnostic: dump the per-row SIMD-vs-scalar blur divergence profile so we can
//! tell whether stackblur banding is boundary-only or distributed mid-image
//! strips. Not an assertion test — prints and (intentionally) does not fail.

#![cfg(feature = "experimental")]

use zenfilters::FilterContext;
use zenfilters::blur_internals::{
    GaussianKernel, gaussian_blur_plane, gaussian_blur_plane_scalar,
};

fn make_plane(w: usize, h: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let g = 0.80 + 0.18 * (y as f32 / (h as f32 - 1.0));
            let ripple = (((x * 7 + y * 13) % 11) as f32 / 11.0 - 0.5) * 0.01;
            v[y * w + x] = g + ripple;
        }
    }
    v
}

#[test]
fn dump_row_divergence_profile() {
    let (w, h) = (200usize, 300usize);
    let sigma = 8.0f32; // stackblur regime
    let src = make_plane(w, h);
    let kernel = GaussianKernel::new(sigma);
    let mut ctx = FilterContext::new();

    let mut simd = vec![0.0f32; w * h];
    gaussian_blur_plane(&src, &mut simd, w as u32, h as u32, &kernel, &mut ctx);
    let mut scalar = vec![0.0f32; w * h];
    gaussian_blur_plane_scalar(&src, &mut scalar, w as u32, h as u32, &kernel, &mut ctx);

    let mut rows_over = 0;
    let mut first = None;
    let mut last = None;
    eprintln!("rows with max diff > 1e-3 (sigma={sigma}, h={h}):");
    for y in 0..h {
        let mut m = 0.0f32;
        for x in 0..w {
            m = m.max((simd[y * w + x] - scalar[y * w + x]).abs());
        }
        if m > 1e-3 {
            rows_over += 1;
            if first.is_none() {
                first = Some(y);
            }
            last = Some(y);
            // print only the first/last handful to keep output readable
            if rows_over <= 6 || y >= h - 6 {
                eprintln!("  row {y:>4}: {m:.5}");
            }
        }
    }
    eprintln!(
        "TOTAL diverging rows: {rows_over}/{h}  span: {:?}..={:?}",
        first, last
    );
    // Also: is the column profile flat (whole row shifted = banding) or spiky?
    if let Some(y) = last {
        let mut mn = f32::INFINITY;
        let mut mx = f32::NEG_INFINITY;
        for x in 0..w {
            let d = simd[y * w + x] - scalar[y * w + x];
            mn = mn.min(d);
            mx = mx.max(d);
        }
        eprintln!("worst row {y}: signed diff range [{mn:.5}, {mx:.5}] (flat band if both same sign & similar)");
    }
}
