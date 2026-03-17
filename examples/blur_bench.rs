//! Gaussian blur benchmark harness — A/B comparison at realistic sigma values.
//!
//! Tests blur candidates at the sigma values that matter for zenfilters:
//! - σ=4: sharpen/clarity fine detail
//! - σ=16: clarity coarse, highest-traffic path
//! - σ=30: local tone map, brilliance
//!
//! Run: `just blur-bench`

use std::sync::Arc;
use zenbench::{Suite, Throughput};
use zenfilters::FilterContext;
use zenfilters::blur_internals::*;

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
    let group_name = format!("blur_{sigma_label}_{size_label}");

    suite.compare(&group_name, |group| {
        group.throughput(Throughput::Elements(n as u64));

        // Dispatch path (FIR for small σ, stackblur for large σ)
        {
            let src = Arc::clone(&src);
            let kernel = Arc::clone(&kernel);
            group.bench("dispatch", move |b| {
                let mut ctx = FilterContext::new();
                let mut dst = vec![0.0f32; n];
                gaussian_blur_plane(&src, &mut dst, w, h, &kernel, &mut ctx);
                b.iter(|| {
                    gaussian_blur_plane(&src, &mut dst, w, h, &kernel, &mut ctx);
                });
            });
        }

        // Our stackblur (scalar, transpose-based vertical)
        {
            let src = Arc::clone(&src);
            let sb_radius = sigma_to_stackblur_radius(sigma);
            group.bench("zen_stackblur", move |b| {
                let mut ctx = FilterContext::new();
                let mut dst = vec![0.0f32; n];
                stackblur_plane(&src, &mut dst, w, h, sb_radius, &mut ctx);
                b.iter(|| {
                    stackblur_plane(&src, &mut dst, w, h, sb_radius, &mut ctx);
                });
            });
        }

        // Dispatch (now routes to SIMD stackblur for σ≥6, FIR below)
        // This is what the pipeline actually uses — includes the SIMD vertical pass.
        // zen_stackblur above is the old scalar transpose-based version for comparison.

        // libblur stackblur (SSE SIMD, in-place, direct column scan for vertical)
        {
            let src = Arc::clone(&src);
            let sb_radius = sigma_to_stackblur_radius(sigma);
            group.bench("libblur_stackblur", move |b| {
                let mut buf = src.to_vec();
                {
                    let mut img = libblur::BlurImageMut::borrow(
                        &mut buf,
                        w,
                        h,
                        libblur::FastBlurChannels::Plane,
                    );
                    let _ = libblur::stack_blur_f32(
                        &mut img,
                        libblur::AnisotropicRadius::new(sb_radius),
                        libblur::ThreadingPolicy::Single,
                    );
                }
                b.iter(|| {
                    buf.copy_from_slice(&src);
                    let mut img = libblur::BlurImageMut::borrow(
                        &mut buf,
                        w,
                        h,
                        libblur::FastBlurChannels::Plane,
                    );
                    let _ = libblur::stack_blur_f32(
                        &mut img,
                        libblur::AnisotropicRadius::new(sb_radius),
                        libblur::ThreadingPolicy::Single,
                    );
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
