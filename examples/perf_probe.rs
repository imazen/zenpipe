//! Comprehensive SIMD performance audit at 2K, 4K, 8K.
//!
//! Tests every major operation in isolation and in pipeline combinations.
//! Run: `cargo run --release --features experimental --example perf_probe`

use std::sync::Arc;
use zenfilters::filters::*;
use zenfilters::{FusedAdjustParams, FilterContext, OklabPlanes, Pipeline, PipelineConfig};
use zenbench::{Suite, Throughput};
use zenpixels::ColorPrimaries;
use zenpixels_convert::oklab;

fn make_linear_rgb(w: usize, h: usize) -> Vec<f32> {
    let n = w * h;
    let mut data = Vec::with_capacity(n * 3);
    for y in 0..h {
        for x in 0..w {
            let t = (y * w + x) as f32 / n as f32;
            data.push((t * 0.6 + 0.2).clamp(0.01, 0.99));
            data.push(((1.0 - t) * 0.5 + 0.25).clamp(0.01, 0.99));
            data.push(((x as f32 / w as f32) * 0.4 + 0.3).clamp(0.01, 0.99));
        }
    }
    data
}

// ─── Isolated operation benchmarks ───────────────────────────────────

fn bench_scatter_gather(suite: &mut Suite, w: u32, h: u32, label: &str) {
    let n = (w as usize) * (h as usize);
    let src = Arc::new(make_linear_rgb(w as usize, h as usize));
    let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
    let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();

    suite.compare(&format!("scatter_gather_{label}"), |group| {
        group.throughput(Throughput::Elements(n as u64));
        {
            let src = Arc::clone(&src);
            group.bench("scatter", move |b| {
                let mut planes = OklabPlanes::new(w, h);
                b.iter(|| zenfilters::scatter_to_oklab(&src, &mut planes, 3, &m1, 1.0));
            });
        }
        {
            let src = Arc::clone(&src);
            group.bench("gather", move |b| {
                let mut planes = OklabPlanes::new(w, h);
                zenfilters::scatter_to_oklab(&src, &mut planes, 3, &m1, 1.0);
                let mut dst = vec![0.0f32; n * 3];
                b.iter(|| zenfilters::gather_from_oklab(&planes, &mut dst, 3, &m1_inv, 1.0));
            });
        }
        {
            let src = Arc::clone(&src);
            group.bench("roundtrip", move |b| {
                let mut planes = OklabPlanes::new(w, h);
                let mut dst = vec![0.0f32; n * 3];
                b.iter(|| {
                    zenfilters::scatter_to_oklab(&src, &mut planes, 3, &m1, 1.0);
                    zenfilters::gather_from_oklab(&planes, &mut dst, 3, &m1_inv, 1.0);
                });
            });
        }
    });
}

fn bench_blur(suite: &mut Suite, w: u32, h: u32, label: &str) {
    use zenfilters::blur_internals::*;
    let n = (w as usize) * (h as usize);
    let src: Arc<Vec<f32>> = Arc::new(vec![0.5f32; n]);

    for &(sigma, slabel) in &[(4.0f32, "s4"), (16.0, "s16"), (30.0, "s30")] {
        let gname = format!("blur_{slabel}_{label}");
        let src = Arc::clone(&src);
        let kernel = Arc::new(GaussianKernel::new(sigma));
        suite.compare(&gname, |group| {
            group.throughput(Throughput::Elements(n as u64));
            // Dispatch (FIR or SIMD stackblur depending on sigma)
            {
                let src = Arc::clone(&src);
                let kernel = Arc::clone(&kernel);
                group.bench("dispatch", move |b| {
                    let mut ctx = FilterContext::new();
                    let mut dst = vec![0.0f32; n];
                    gaussian_blur_plane(&src, &mut dst, w, h, &kernel, &mut ctx);
                    b.iter(|| gaussian_blur_plane(&src, &mut dst, w, h, &kernel, &mut ctx));
                });
            }
        });
    }
}

fn bench_fused_adjust(suite: &mut Suite, w: u32, h: u32, label: &str) {
    let n = (w as usize) * (h as usize);
    let src = Arc::new(make_linear_rgb(w as usize, h as usize));
    let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
    let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();

    let mut adj = FusedAdjust::new();
    adj.exposure = 0.3;
    adj.contrast = 0.2;
    adj.highlights = 0.4;
    adj.shadows = 0.3;
    adj.saturation = 1.1;
    adj.vibrance = 0.3;
    adj.temperature = 0.02;
    adj.tint = -0.01;
    adj.black_point = 0.01;
    adj.white_point = 0.98;
    let params = FusedAdjustParams::from_adjust(&adj);

    suite.compare(&format!("fused_adjust_{label}"), |group| {
        group.throughput(Throughput::Elements(n as u64));
        // Planar pipeline: scatter → fused_adjust → gather
        {
            let src = Arc::clone(&src);
            group.bench("planar_pipeline", move |b| {
                let mut pipe = Pipeline::new(PipelineConfig::default()).unwrap();
                pipe.push(Box::new(adj.clone()));
                let mut ctx = FilterContext::new();
                let mut dst = vec![0.0f32; n * 3];
                b.iter(|| pipe.apply(&src, &mut dst, w, h, 3, &mut ctx).unwrap());
            });
        }
        // Fused interleaved: single SIMD pass
        {
            let src = Arc::clone(&src);
            let params = params.clone();
            group.bench("fused_interleaved", move |b| {
                let mut dst = vec![0.0f32; n * 3];
                b.iter(|| {
                    zenfilters::fused_interleaved_adjust(
                        &src, &mut dst, 3, &m1, &m1_inv, 1.0, 1.0, &params,
                    );
                });
            });
        }
    });
}

// ─── Pipeline benchmarks ─────────────────────────────────────────────

fn bench_pipeline(suite: &mut Suite, name: &str, w: u32, h: u32, make_pipe: fn() -> Pipeline) {
    let n = (w as usize) * (h as usize);
    let src: Arc<[f32]> = make_linear_rgb(w as usize, h as usize).into();

    suite.compare(name, |group| {
        group.throughput(Throughput::Elements(n as u64));
        let src = Arc::clone(&src);
        group.bench("run", move |b| {
            let pipe = make_pipe();
            let mut ctx = FilterContext::new();
            let mut dst = vec![0.0f32; n * 3];
            pipe.apply(&src, &mut dst, w, h, 3, &mut ctx).unwrap();
            b.iter(|| pipe.apply(&src, &mut dst, w, h, 3, &mut ctx).unwrap());
        });
    });
}

fn pipe_perpixel() -> Pipeline {
    let mut pipe = Pipeline::new(PipelineConfig::default()).unwrap();
    let mut fa = FusedAdjust::new();
    fa.exposure = 0.5;
    fa.contrast = 0.3;
    fa.saturation = 1.2;
    fa.vibrance = 0.4;
    pipe.push(Box::new(fa));
    pipe
}

fn pipe_clarity() -> Pipeline {
    let mut pipe = Pipeline::new(PipelineConfig::default()).unwrap();
    let mut c = Clarity::default();
    c.sigma = 4.0;
    c.amount = 0.3;
    pipe.push(Box::new(c));
    pipe
}

fn pipe_realistic() -> Pipeline {
    let mut pipe = Pipeline::new(PipelineConfig::default()).unwrap();
    let mut fa = FusedAdjust::new();
    fa.exposure = 0.3;
    fa.contrast = 0.2;
    fa.highlights = 0.4;
    fa.shadows = 0.3;
    fa.saturation = 1.1;
    fa.vibrance = 0.3;
    pipe.push(Box::new(fa));
    let mut cl = Clarity::default();
    cl.sigma = 4.0;
    cl.amount = 0.2;
    pipe.push(Box::new(cl));
    let mut sh = AdaptiveSharpen::default();
    sh.amount = 0.3;
    sh.sigma = 1.2;
    sh.noise_floor = 0.004;
    sh.detail = 0.5;
    sh.masking = 0.3;
    pipe.push(Box::new(sh));
    pipe
}

fn pipe_heavy() -> Pipeline {
    let mut pipe = Pipeline::new(PipelineConfig::default()).unwrap();
    let mut cl = Clarity::default();
    cl.sigma = 4.0;
    cl.amount = 0.3;
    pipe.push(Box::new(cl));
    let mut tx = Texture::default();
    tx.sigma = 1.5;
    tx.amount = 0.3;
    pipe.push(Box::new(tx));
    let mut nr = NoiseReduction::default();
    nr.luminance = 0.5;
    nr.chroma = 0.3;
    nr.scales = 4;
    pipe.push(Box::new(nr));
    pipe
}

const SIZES: &[(u32, u32, &str)] = &[
    (1920, 1080, "2k"),
    (3840, 2160, "4k"),
    (7680, 4320, "8k"),
];

fn main() {
    zenbench::run(|suite| {
        for &(w, h, label) in SIZES {
            // Isolated operations
            bench_scatter_gather(suite, w, h, label);
            bench_blur(suite, w, h, label);
            bench_fused_adjust(suite, w, h, label);

            // Full pipelines
            bench_pipeline(suite, &format!("pipe_perpixel_{label}"), w, h, pipe_perpixel);
            bench_pipeline(suite, &format!("pipe_clarity_{label}"), w, h, pipe_clarity);
            bench_pipeline(suite, &format!("pipe_realistic_{label}"), w, h, pipe_realistic);
            bench_pipeline(suite, &format!("pipe_heavy_{label}"), w, h, pipe_heavy);
        }
    });
}
