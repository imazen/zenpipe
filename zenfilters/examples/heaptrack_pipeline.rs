//! Memory profiling for pipeline apply at various resolutions.
//!
//! Run: heaptrack cargo run --example heaptrack_pipeline --release -- <width> <height>
//! Analyze: heaptrack_print heaptrack.*.zst | grep "peak heap"

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

fn run(width: u32, height: u32) {
    let src = make_image(width as usize, height as usize);
    let mut dst = vec![0.0f32; src.len()];
    let mut ctx = FilterContext::new();

    let mut pipeline = Pipeline::new(PipelineConfig::default()).unwrap();
    let mut exp = zenfilters::filters::Exposure::default();
    exp.stops = 0.5;
    pipeline.push(Box::new(exp));
    pipeline.push(Box::new(zenfilters::filters::Clarity::default()));
    pipeline.push(Box::new(zenfilters::filters::Sharpen::default()));

    let t0 = Instant::now();
    pipeline
        .apply(&src, &mut dst, width, height, 4, &mut ctx)
        .unwrap();
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let mp = (width as f64 * height as f64) / 1_000_000.0;
    let src_mb = (src.len() * 4) as f64 / (1024.0 * 1024.0);

    eprintln!(
        "{width}x{height}: {ms:.1} ms ({:.0} MP/s), src+dst = {:.1} MB",
        mp / (ms / 1000.0),
        src_mb * 2.0
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 {
        let w: u32 = args[1].parse().expect("width");
        let h: u32 = args[2].parse().expect("height");
        run(w, h);
    } else {
        // Run all sizes for comparison
        for &(w, h) in &[(640, 480), (1920, 1080), (3840, 2160), (7680, 4320)] {
            run(w, h);
        }
    }
}
