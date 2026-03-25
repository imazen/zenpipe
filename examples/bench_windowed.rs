//! Benchmark: WindowedFilterSource vs Pipeline::apply() (full-frame).
//!
//! Run: cargo run --example bench_windowed --release

use std::time::Instant;

use zenpipe::format;
use zenpipe::sources::{MaterializedSource, WindowedFilterSource};
use zenpipe::{PipeError, PixelFormat, Source, Strip};

struct SyntheticSource {
    width: u32,
    height: u32,
    y: u32,
    strip_h: u32,
    buf: Vec<u8>,
}

impl SyntheticSource {
    fn new(width: u32, height: u32) -> Self {
        let strip_h = 16u32;
        let stride = width as usize * 16; // RGBA f32 = 16 bytes
        let buf_size = stride * strip_h as usize;
        // Fill with ~0.5 in f32
        let val: f32 = 0.5;
        let bytes = val.to_ne_bytes();
        let mut buf = vec![0u8; buf_size];
        for chunk in buf.chunks_exact_mut(4) {
            chunk.copy_from_slice(&bytes);
        }
        Self {
            width,
            height,
            y: 0,
            strip_h,
            buf,
        }
    }
}

impl Source for SyntheticSource {
    fn next(&mut self) -> Result<Option<Strip<'_>>, PipeError> {
        if self.y >= self.height {
            return Ok(None);
        }
        let rows = self.strip_h.min(self.height - self.y);
        let stride = self.width as usize * 16;
        let len = rows as usize * stride;
        self.y += rows;
        Ok(Some(Strip::new(
            &self.buf[..len],
            self.width,
            rows,
            stride,
            format::RGBAF32_LINEAR,
        )?))
    }
    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
    fn format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR
    }
}

fn drain(source: &mut dyn Source) -> Result<u32, PipeError> {
    let mut total = 0u32;
    while let Some(strip) = source.next()? {
        total += strip.rows();
    }
    Ok(total)
}

fn build_pipeline() -> zenfilters::Pipeline {
    let mut pipeline = zenfilters::Pipeline::new(zenfilters::PipelineConfig::default()).unwrap();
    let mut exp = zenfilters::filters::Exposure::default();
    exp.stops = 0.5;
    pipeline.push(Box::new(exp));
    pipeline.push(Box::new(zenfilters::filters::Clarity::default()));
    pipeline.push(Box::new(zenfilters::filters::Sharpen::default()));
    pipeline
}

fn bench_windowed(width: u32, height: u32, iters: u32) -> f64 {
    // Warm up
    {
        let pipeline = build_pipeline();
        let overlap = pipeline.max_neighborhood_radius(width, height);
        let src = SyntheticSource::new(width, height);
        let mut wf = WindowedFilterSource::new(Box::new(src), pipeline, overlap).unwrap();
        drain(&mut wf).unwrap();
    }

    let t0 = Instant::now();
    for _ in 0..iters {
        let pipeline = build_pipeline();
        let overlap = pipeline.max_neighborhood_radius(width, height);
        let src = SyntheticSource::new(width, height);
        let mut wf = WindowedFilterSource::new(Box::new(src), pipeline, overlap).unwrap();
        drain(&mut wf).unwrap();
    }
    t0.elapsed().as_secs_f64() * 1000.0 / iters as f64
}

fn bench_full_frame(width: u32, height: u32, iters: u32) -> f64 {
    let pipeline = build_pipeline();
    let mut ctx = zenfilters::FilterContext::new();
    let n = width as usize * height as usize * 4;
    let src = vec![0.5f32; n];
    let mut dst = vec![0.0f32; n];

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
    t0.elapsed().as_secs_f64() * 1000.0 / iters as f64
}

fn main() {
    eprintln!("Pipeline: exposure(0.5) + clarity + sharpen");
    eprintln!(
        "{:<12} {:>12} {:>12} {:>8}",
        "Resolution", "Windowed", "FullFrame", "Ratio"
    );
    eprintln!("{}", "-".repeat(48));

    for &(w, h, iters) in &[(640, 480, 20), (1920, 1080, 5), (3840, 2160, 3)] {
        let wms = bench_windowed(w, h, iters);
        let fms = bench_full_frame(w, h, iters);
        eprintln!(
            "{:>4}x{:<6} {:>8.1} ms {:>8.1} ms {:>7.2}x",
            w,
            h,
            wms,
            fms,
            wms / fms
        );
    }
}
