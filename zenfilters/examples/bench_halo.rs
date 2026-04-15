//! Benchmark: windowed strip vs full-frame for neighborhood filters.
//!
//! Run against current code:  cargo run --example bench_halo --release
//! Run against pre-halo code: cd /tmp/zenfilters-before-halo && cargo run --example bench_halo --release

use std::time::Instant;
use zenfilters::{FilterContext, Pipeline, PipelineConfig};

fn make_image(width: usize, height: usize) -> Vec<f32> {
    let n = width * height;
    let mut data = Vec::with_capacity(n * 4);
    for i in 0..n {
        let t = i as f32 / n as f32;
        data.push((t * 0.8 + 0.1).clamp(0.001, 1.0));
        data.push(((1.0 - t) * 0.7 + 0.15).clamp(0.001, 1.0));
        data.push((t * 0.5 + 0.25).clamp(0.001, 1.0));
        data.push(1.0);
    }
    data
}

fn bench(label: &str, width: u32, height: u32, iters: u32) {
    let src = make_image(width as usize, height as usize);
    let mut dst = vec![0.0f32; src.len()];
    let mut ctx = FilterContext::new();

    let mut pipeline = Pipeline::new(PipelineConfig::default()).unwrap();
    let mut exp = zenfilters::filters::Exposure::default();
    exp.stops = 0.5;
    pipeline.push(Box::new(exp));
    pipeline.push(Box::new(zenfilters::filters::Clarity::default()));
    pipeline.push(Box::new(zenfilters::filters::Sharpen::default()));

    // Warm up
    pipeline
        .apply(&src, &mut dst, width, height, 4, &mut ctx)
        .unwrap();

    let t0 = Instant::now();
    for _ in 0..iters {
        pipeline
            .apply(&src, &mut dst, width, height, 4, &mut ctx)
            .unwrap();
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0 / iters as f64;
    let mp = (width as f64 * height as f64) / 1_000_000.0;

    eprintln!("{label}: {ms:.1} ms ({:.0} MP/s)", mp / (ms / 1000.0));
}

fn main() {
    eprintln!("Pipeline: exposure(0.5) + clarity + sharpen");
    eprintln!("---");
    bench("  640x480 ", 640, 480, 20);
    bench(" 1920x1080", 1920, 1080, 5);
    bench(" 3840x2160", 3840, 2160, 3);
}
