//! Gaussian blur benchmark harness — A/B comparison at realistic sigma values.
//!
//! Tests blur candidates at the sigma values that matter for zenfilters:
//! - σ=4: sharpen/clarity fine detail
//! - σ=16: clarity coarse, highest-traffic path
//! - σ=30: local tone map, brilliance
//!
//! Run: `just blur-bench`

use std::sync::Arc;
use zenfilters::blur_internals::*;
use zenfilters::FilterContext;
use zenbench::{Suite, Throughput};

fn make_gradient_plane(w: usize, h: usize) -> Vec<f32> {
    let n = w * h;
    let mut data = vec![0.0f32; n];
    for y in 0..h {
        for x in 0..w {
            data[y * w + x] = 0.05 + 0.9 * (x as f32 / w as f32) * (y as f32 / h as f32);
        }
    }
    data
}

fn bench_blur_group(
    suite: &mut Suite,
    w: u32,
    h: u32,
    sigma: f32,
    size_label: &str,
    sigma_label: &str,
) {
    let n = (w as usize) * (h as usize);
    let src = Arc::new(make_gradient_plane(w as usize, h as usize));
    let kernel = Arc::new(GaussianKernel::new(sigma));
    let box_blur = Arc::new(ExtendedBoxBlur::from_sigma(sigma));
    let group_name = format!("blur_{sigma_label}_{size_label}");

    suite.compare(&group_name, |group| {
        group.throughput(Throughput::Elements(n as u64));

        // Baseline: current SIMD FIR (dispatch path)
        {
            let src = Arc::clone(&src);
            let kernel = Arc::clone(&kernel);
            group.bench("fir_dispatch", move |b| {
                let mut ctx = FilterContext::new();
                let mut dst = vec![0.0f32; n];
                gaussian_blur_plane(&src, &mut dst, w, h, &kernel, &mut ctx);
                b.iter(|| {
                    gaussian_blur_plane(&src, &mut dst, w, h, &kernel, &mut ctx);
                });
            });
        }

        // Scalar FIR baseline (no SIMD)
        {
            let src = Arc::clone(&src);
            let kernel = Arc::clone(&kernel);
            group.bench("fir_scalar", move |b| {
                let mut ctx = FilterContext::new();
                let mut dst = vec![0.0f32; n];
                gaussian_blur_plane_scalar(&src, &mut dst, w, h, &kernel, &mut ctx);
                b.iter(|| {
                    gaussian_blur_plane_scalar(&src, &mut dst, w, h, &kernel, &mut ctx);
                });
            });
        }

        // Extended box blur (transpose-based, O(1)/pixel)
        {
            let src = Arc::clone(&src);
            let box_blur = Arc::clone(&box_blur);
            group.bench("box_blur", move |b| {
                let mut ctx = FilterContext::new();
                let mut dst = vec![0.0f32; n];
                extended_box_blur_plane(&src, &mut dst, w, h, &box_blur, &mut ctx);
                b.iter(|| {
                    extended_box_blur_plane(&src, &mut dst, w, h, &box_blur, &mut ctx);
                });
            });
        }

        // Deriche IIR (O(1)/pixel, high accuracy)
        {
            let src = Arc::clone(&src);
            let deriche = Arc::new(DericheCoefficients::new(sigma));
            group.bench("deriche_iir", move |b| {
                let mut ctx = FilterContext::new();
                let mut dst = vec![0.0f32; n];
                deriche_blur_plane(&src, &mut dst, w, h, &deriche, &mut ctx);
                b.iter(|| {
                    deriche_blur_plane(&src, &mut dst, w, h, &deriche, &mut ctx);
                });
            });
        }
    });
}

const SIZES: &[(u32, u32, &str)] = &[(1920, 1080, "1080p"), (3840, 2160, "4k")];
const SIGMAS: &[(f32, &str)] = &[(4.0, "sigma4"), (16.0, "sigma16"), (30.0, "sigma30")];

zenbench::main!(|suite| {
    for &(w, h, size_label) in SIZES {
        for &(sigma, sigma_label) in SIGMAS {
            bench_blur_group(suite, w, h, sigma, size_label, sigma_label);
        }
    }
});
