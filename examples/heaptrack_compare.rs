//! Compare memory: per-pixel pipeline (strips) vs neighborhood pipeline (full-frame).
//!
//! Run: heaptrack cargo run --example heaptrack_compare --release -- <mode> <width> <height>
//! mode: "pixelonly" or "neighborhood"

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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(|s| s.as_str()).unwrap_or("neighborhood");
    let w: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3840);
    let h: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(2160);

    let src = make_image(w as usize, h as usize);
    let mut dst = vec![0.0f32; src.len()];
    let mut ctx = FilterContext::new();

    let mut pipeline = Pipeline::new(PipelineConfig::default()).unwrap();

    // Per-pixel filters only (strips, no full-frame alloc)
    let mut exp = zenfilters::filters::Exposure::default();
    exp.stops = 0.5;
    pipeline.push(Box::new(exp));
    pipeline.push(Box::new(zenfilters::filters::Contrast::default()));
    pipeline.push(Box::new(zenfilters::filters::Saturation::default()));

    if mode == "neighborhood" {
        // Add neighborhood filters (forces full-frame)
        pipeline.push(Box::new(zenfilters::filters::Clarity::default()));
        pipeline.push(Box::new(zenfilters::filters::Sharpen::default()));
    }

    let has_nh = pipeline.has_neighborhood_filter();
    let halo = pipeline.total_halo(w, h);
    eprintln!("{mode} {w}x{h}: neighborhood={has_nh} halo={halo}");

    pipeline.apply(&src, &mut dst, w, h, 4, &mut ctx).unwrap();
    eprintln!("done");
}
