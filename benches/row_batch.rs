use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use zenpixels::buffer::PixelBuffer;
use zenpixels::{ChannelLayout, ChannelType, PixelDescriptor, TransferFunction};
use zenpixels_convert::RowConverter;

use zenfilters::srgb_filters::{self, SrgbColorFilter};
use zenfilters::{FilterContext, Pipeline, PipelineBufferExt, PipelineConfig};

// ─── Test data ──────────────────────────────────────────────────────

fn make_rgba8_buffer(width: u32, height: u32) -> PixelBuffer {
    let n = (width as usize) * (height as usize);
    let mut data = Vec::with_capacity(n * 4);
    for i in 0..n {
        let t = i as f32 / n as f32;
        data.push((t * 200.0 + 30.0) as u8);
        data.push(((1.0 - t) * 180.0 + 40.0) as u8);
        data.push((t * 100.0 + 80.0) as u8);
        data.push(255u8);
    }
    PixelBuffer::from_vec(data, width, height, PixelDescriptor::RGBA8_SRGB).unwrap()
}

fn make_rgb16_buffer(width: u32, height: u32) -> PixelBuffer {
    let n = (width as usize) * (height as usize);
    let mut data: Vec<u8> = Vec::with_capacity(n * 6);
    for i in 0..n {
        let t = i as f32 / n as f32;
        data.extend_from_slice(&((t * 50000.0 + 5000.0) as u16).to_le_bytes());
        data.extend_from_slice(&(((1.0 - t) * 40000.0 + 8000.0) as u16).to_le_bytes());
        data.extend_from_slice(&((t * 30000.0 + 10000.0) as u16).to_le_bytes());
    }
    let desc = PixelDescriptor::new(
        ChannelType::U16,
        ChannelLayout::Rgb,
        None,
        TransferFunction::Srgb,
    );
    PixelBuffer::from_vec(data, width, height, desc).unwrap()
}

// ─── sRGB per-pixel filter benchmarks ───────────────────────────────

fn bench_srgb_color_adjust(c: &mut Criterion) {
    let mut group = c.benchmark_group("srgb_color_adjust");

    for &(w, h) in &[(256, 256), (512, 512), (1024, 1024), (2048, 2048)] {
        // Fast path: already RGBA8
        let mut buf = make_rgba8_buffer(w, h);
        group.bench_with_input(
            BenchmarkId::new("rgba8_fast", format!("{w}x{h}")),
            &(w, h),
            |b, _| {
                b.iter(|| {
                    srgb_filters::color_adjust(&mut buf.as_slice_mut(), 0.1, 0.2, 0.15);
                });
            },
        );

        // Slow path: needs RowConverter (RGB16 -> RGBA8 -> process -> RGBA8 -> RGB16)
        let mut buf16 = make_rgb16_buffer(w, h);
        group.bench_with_input(
            BenchmarkId::new("rgb16_convert", format!("{w}x{h}")),
            &(w, h),
            |b, _| {
                b.iter(|| {
                    srgb_filters::color_adjust(&mut buf16.as_slice_mut(), 0.1, 0.2, 0.15);
                });
            },
        );
    }
    group.finish();
}

fn bench_srgb_color_filter(c: &mut Criterion) {
    let mut group = c.benchmark_group("srgb_color_filter");

    for &(w, h) in &[(512, 512), (1024, 1024), (2048, 2048)] {
        let mut buf = make_rgba8_buffer(w, h);
        group.bench_with_input(
            BenchmarkId::new("grayscale_bt709", format!("{w}x{h}")),
            &(w, h),
            |b, _| {
                b.iter(|| {
                    srgb_filters::color_filter(
                        &mut buf.as_slice_mut(),
                        &SrgbColorFilter::GrayscaleBt709,
                    );
                });
            },
        );

        let mut buf2 = make_rgba8_buffer(w, h);
        group.bench_with_input(
            BenchmarkId::new("invert", format!("{w}x{h}")),
            &(w, h),
            |b, _| {
                b.iter(|| {
                    srgb_filters::color_filter(&mut buf2.as_slice_mut(), &SrgbColorFilter::Invert);
                });
            },
        );

        let mut buf3 = make_rgba8_buffer(w, h);
        group.bench_with_input(
            BenchmarkId::new("sepia", format!("{w}x{h}")),
            &(w, h),
            |b, _| {
                b.iter(|| {
                    srgb_filters::color_filter(&mut buf3.as_slice_mut(), &SrgbColorFilter::Sepia);
                });
            },
        );
    }
    group.finish();
}

fn bench_srgb_color_matrix(c: &mut Criterion) {
    let mut group = c.benchmark_group("srgb_color_matrix");

    #[rustfmt::skip]
    let sepia: [f32; 25] = [
        0.393, 0.769, 0.189, 0.0, 0.0,
        0.349, 0.686, 0.168, 0.0, 0.0,
        0.272, 0.534, 0.131, 0.0, 0.0,
        0.0,   0.0,   0.0,   1.0, 0.0,
        0.0,   0.0,   0.0,   0.0, 1.0,
    ];

    for &(w, h) in &[(512, 512), (1024, 1024), (2048, 2048)] {
        let mut buf = make_rgba8_buffer(w, h);
        group.bench_with_input(
            BenchmarkId::new("sepia_matrix", format!("{w}x{h}")),
            &(w, h),
            |b, _| {
                b.iter(|| {
                    srgb_filters::color_matrix(&mut buf.as_slice_mut(), &sepia);
                });
            },
        );
    }
    group.finish();
}

// ─── sRGB neighborhood filter benchmarks ────────────────────────────

fn bench_srgb_sharpen(c: &mut Criterion) {
    let mut group = c.benchmark_group("srgb_sharpen");

    for &(w, h) in &[(256, 256), (512, 512), (1024, 1024)] {
        group.bench_with_input(
            BenchmarkId::new("amount_50", format!("{w}x{h}")),
            &(w, h),
            |b, &(w, h)| {
                b.iter(|| {
                    let mut work = make_rgba8_buffer(w, h);
                    srgb_filters::sharpen(&mut work, 50.0);
                });
            },
        );
    }
    group.finish();
}

fn bench_srgb_blur(c: &mut Criterion) {
    let mut group = c.benchmark_group("srgb_blur");

    for &(w, h) in &[(256, 256), (512, 512), (1024, 1024)] {
        group.bench_with_input(
            BenchmarkId::new("sigma_3", format!("{w}x{h}")),
            &(w, h),
            |b, &(w, h)| {
                b.iter(|| {
                    let mut work = make_rgba8_buffer(w, h);
                    srgb_filters::blur(&mut work, 3.0);
                });
            },
        );
    }
    group.finish();
}

// ─── Oklab pipeline benchmarks ──────────────────────────────────────

fn bench_oklab_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("oklab_pipeline");

    for &(w, h) in &[(256, 256), (512, 512), (1024, 1024), (2048, 2048)] {
        let buf = make_rgba8_buffer(w, h);

        // Empty pipeline (scatter + gather only) — fresh context each time
        let empty_pipe = Pipeline::new(PipelineConfig::default()).unwrap();
        group.bench_with_input(
            BenchmarkId::new("empty_roundtrip", format!("{w}x{h}")),
            &(w, h),
            |b, _| {
                b.iter(|| {
                    let mut ctx = FilterContext::new();
                    empty_pipe.apply_buffer(&buf, &mut ctx).unwrap();
                });
            },
        );

        // Empty pipeline — reused context (measures allocation savings)
        group.bench_with_input(
            BenchmarkId::new("empty_roundtrip_reuse_ctx", format!("{w}x{h}")),
            &(w, h),
            |b, _| {
                let mut ctx = FilterContext::new();
                b.iter(|| {
                    empty_pipe.apply_buffer(&buf, &mut ctx).unwrap();
                });
            },
        );

        // Typical filter stack — fresh context
        let mut full_pipe = Pipeline::new(PipelineConfig::default()).unwrap();
        full_pipe.push(Box::new(zenfilters::filters::Exposure { stops: 0.3 }));
        full_pipe.push(Box::new(zenfilters::filters::Contrast { amount: 0.2 }));
        full_pipe.push(Box::new(zenfilters::filters::Saturation { factor: 1.1 }));
        group.bench_with_input(
            BenchmarkId::new("exposure_contrast_sat", format!("{w}x{h}")),
            &(w, h),
            |b, _| {
                b.iter(|| {
                    let mut ctx = FilterContext::new();
                    full_pipe.apply_buffer(&buf, &mut ctx).unwrap();
                });
            },
        );

        // Typical filter stack — reused context
        group.bench_with_input(
            BenchmarkId::new("exposure_contrast_sat_reuse_ctx", format!("{w}x{h}")),
            &(w, h),
            |b, _| {
                let mut ctx = FilterContext::new();
                b.iter(|| {
                    full_pipe.apply_buffer(&buf, &mut ctx).unwrap();
                });
            },
        );

        // Neighborhood filters (clarity) — fresh vs reused context
        let mut clarity_pipe = Pipeline::new(PipelineConfig::default()).unwrap();
        clarity_pipe.push(Box::new(zenfilters::filters::Clarity {
            sigma: 10.0,
            amount: 0.3,
        }));
        group.bench_with_input(
            BenchmarkId::new("clarity_fresh_ctx", format!("{w}x{h}")),
            &(w, h),
            |b, _| {
                b.iter(|| {
                    let mut ctx = FilterContext::new();
                    clarity_pipe.apply_buffer(&buf, &mut ctx).unwrap();
                });
            },
        );
        group.bench_with_input(
            BenchmarkId::new("clarity_reuse_ctx", format!("{w}x{h}")),
            &(w, h),
            |b, _| {
                let mut ctx = FilterContext::new();
                b.iter(|| {
                    clarity_pipe.apply_buffer(&buf, &mut ctx).unwrap();
                });
            },
        );
    }
    group.finish();
}

// ─── Row conversion overhead isolation ──────────────────────────────

fn bench_row_converter_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("row_converter_overhead");

    for &w in &[256, 512, 1024, 2048] {
        // Measure RowConverter construction
        let desc_src = PixelDescriptor::new(
            ChannelType::U16,
            ChannelLayout::Rgb,
            None,
            TransferFunction::Srgb,
        );
        let desc_dst = PixelDescriptor::RGBA8_SRGB;

        group.bench_with_input(
            BenchmarkId::new("constructor", format!("w{w}")),
            &w,
            |b, _| {
                b.iter(|| {
                    let _ = RowConverter::new(desc_src, desc_dst).unwrap();
                });
            },
        );

        // Measure conversion of a single row
        let converter = RowConverter::new(desc_src, desc_dst).unwrap();
        let src_row = vec![128u8; w * 6]; // RGB16
        let mut dst_row = vec![0u8; w * 4]; // RGBA8

        group.bench_with_input(
            BenchmarkId::new("convert_1_row", format!("w{w}")),
            &w,
            |b, _| {
                b.iter(|| {
                    converter.convert_row(&src_row, &mut dst_row, w as u32);
                });
            },
        );
    }
    group.finish();
}

// ─── Scatter/gather isolation ────────────────────────────────────────

fn bench_scatter_gather(c: &mut Criterion) {
    use zenpixels::ColorPrimaries;
    use zenpixels_convert::oklab;

    let mut group = c.benchmark_group("scatter_gather");

    for &(w, h) in &[(256, 256), (512, 512), (1024, 1024), (2048, 2048)] {
        let n = (w as usize) * (h as usize);
        let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
        let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();

        // Create linear f32 RGB test data
        let mut src = vec![0.0f32; n * 3];
        for i in 0..n {
            let t = i as f32 / n as f32;
            src[i * 3] = t * 0.8 + 0.1;
            src[i * 3 + 1] = (1.0 - t) * 0.7 + 0.15;
            src[i * 3 + 2] = t * 0.5 + 0.2;
        }

        // Scatter only
        group.bench_with_input(
            BenchmarkId::new("scatter", format!("{w}x{h}")),
            &(w, h),
            |b, &(w, h)| {
                let mut planes = zenfilters::OklabPlanes::new(w, h);
                b.iter(|| {
                    zenfilters::scatter_to_oklab(&src, &mut planes, 3, &m1, 1.0);
                });
            },
        );

        // Gather only
        group.bench_with_input(
            BenchmarkId::new("gather", format!("{w}x{h}")),
            &(w, h),
            |b, &(w, h)| {
                let mut planes = zenfilters::OklabPlanes::new(w, h);
                zenfilters::scatter_to_oklab(&src, &mut planes, 3, &m1, 1.0);
                let mut dst = vec![0.0f32; n * 3];
                b.iter(|| {
                    zenfilters::gather_from_oklab(&planes, &mut dst, 3, &m1_inv, 1.0);
                });
            },
        );

        // Full roundtrip (scatter + gather, no pipeline)
        group.bench_with_input(
            BenchmarkId::new("roundtrip", format!("{w}x{h}")),
            &(w, h),
            |b, &(w, h)| {
                let mut planes = zenfilters::OklabPlanes::new(w, h);
                let mut dst = vec![0.0f32; n * 3];
                b.iter(|| {
                    zenfilters::scatter_to_oklab(&src, &mut planes, 3, &m1, 1.0);
                    zenfilters::gather_from_oklab(&planes, &mut dst, 3, &m1_inv, 1.0);
                });
            },
        );

        // Fused sRGB u8 → Oklab scatter
        let mut src_u8 = vec![0u8; n * 3];
        for i in 0..n {
            let t = i as f32 / n as f32;
            src_u8[i * 3] = (t * 200.0 + 30.0) as u8;
            src_u8[i * 3 + 1] = ((1.0 - t) * 180.0 + 40.0) as u8;
            src_u8[i * 3 + 2] = (t * 100.0 + 80.0) as u8;
        }

        group.bench_with_input(
            BenchmarkId::new("fused_scatter_u8", format!("{w}x{h}")),
            &(w, h),
            |b, &(w, h)| {
                let mut planes = zenfilters::OklabPlanes::new(w, h);
                b.iter(|| {
                    zenfilters::scatter_srgb_u8_to_oklab(&src_u8, &mut planes, 3, &m1);
                });
            },
        );

        // Fused Oklab → sRGB u8 gather
        group.bench_with_input(
            BenchmarkId::new("fused_gather_u8", format!("{w}x{h}")),
            &(w, h),
            |b, &(w, h)| {
                let mut planes = zenfilters::OklabPlanes::new(w, h);
                zenfilters::scatter_srgb_u8_to_oklab(&src_u8, &mut planes, 3, &m1);
                let mut dst_u8 = vec![0u8; n * 3];
                b.iter(|| {
                    zenfilters::gather_oklab_to_srgb_u8(&planes, &mut dst_u8, 3, &m1_inv);
                });
            },
        );

        // Fused u8 roundtrip
        group.bench_with_input(
            BenchmarkId::new("fused_roundtrip_u8", format!("{w}x{h}")),
            &(w, h),
            |b, &(w, h)| {
                let mut planes = zenfilters::OklabPlanes::new(w, h);
                let mut dst_u8 = vec![0u8; n * 3];
                b.iter(|| {
                    zenfilters::scatter_srgb_u8_to_oklab(&src_u8, &mut planes, 3, &m1);
                    zenfilters::gather_oklab_to_srgb_u8(&planes, &mut dst_u8, 3, &m1_inv);
                });
            },
        );
    }
    group.finish();
}

// ─── Allocation overhead measurement ────────────────────────────────

fn bench_allocation_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("allocation_overhead");

    for &(w, h) in &[(512, 512), (1024, 1024), (2048, 2048)] {
        let n = w * h;

        // OklabPlanes allocation
        group.bench_with_input(
            BenchmarkId::new("oklab_planes", format!("{w}x{h}")),
            &(w, h),
            |b, _| {
                b.iter(|| {
                    let _ = zenfilters::OklabPlanes::with_alpha(w as u32, h as u32);
                });
            },
        );

        // Linear f32 buffer allocation
        group.bench_with_input(
            BenchmarkId::new("f32_buf_4ch", format!("{w}x{h}")),
            &(w, h),
            |b, _| {
                b.iter(|| {
                    let _ = vec![0.0f32; n * 4];
                });
            },
        );

        // Per-row u8 buffer (what with_rows_rgba allocates each call)
        group.bench_with_input(
            BenchmarkId::new("row_buf_rgba8", format!("w{w}")),
            &w,
            |b, _| {
                b.iter(|| {
                    let _ = vec![0u8; w * 4];
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_srgb_color_adjust,
    bench_srgb_color_filter,
    bench_srgb_color_matrix,
    bench_srgb_sharpen,
    bench_srgb_blur,
    bench_scatter_gather,
    bench_oklab_pipeline,
    bench_row_converter_overhead,
    bench_allocation_overhead,
);
criterion_main!(benches);
