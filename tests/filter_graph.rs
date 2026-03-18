//! Tests for zenfilters integration via the `filters` feature.

#![cfg(feature = "std")]
use hashbrown::HashMap;

use zenpipe::graph::{EdgeKind, NodeOp, PipelineGraph};
use zenpipe::sources::{CallbackSource, FilterSource};
use zenpipe::{Source, format};

/// Collect all strips from a source into a flat Vec<u8>.
fn drain(source: &mut dyn Source) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok(Some(strip)) = source.next() {
        out.extend_from_slice(strip.as_strided_bytes());
    }
    out
}

/// Create a solid-color RGBA8 source.
fn solid_source(width: u32, height: u32, pixel: [u8; 4]) -> Box<dyn Source> {
    let row_bytes = width as usize * 4;
    let mut rows_produced = 0u32;
    Box::new(CallbackSource::new(
        width,
        height,
        format::RGBA8_SRGB,
        16,
        move |buf| {
            if rows_produced >= height {
                return Ok(false);
            }
            for px in buf[..row_bytes].chunks_exact_mut(4) {
                px.copy_from_slice(&pixel);
            }
            rows_produced += 1;
            Ok(true)
        },
    ))
}

/// Create a linear f32 RGBA source (for direct FilterSource testing).
fn solid_linear_source(width: u32, height: u32, pixel: [f32; 4]) -> Box<dyn Source> {
    let row_bytes = width as usize * 16; // 4 × f32
    let mut rows_produced = 0u32;
    Box::new(CallbackSource::new(
        width,
        height,
        format::RGBAF32_LINEAR,
        16,
        move |buf| {
            if rows_produced >= height {
                return Ok(false);
            }
            let f32_row: &mut [f32] = bytemuck::cast_slice_mut(&mut buf[..row_bytes]);
            for px in f32_row.chunks_exact_mut(4) {
                px.copy_from_slice(&pixel);
            }
            rows_produced += 1;
            Ok(true)
        },
    ))
}

// ==========================================================================
// FilterSource (direct) tests
// ==========================================================================

#[test]
fn filter_source_identity_pipeline() {
    // Empty pipeline should pass data through unchanged
    let pipeline = zenfilters::Pipeline::new(zenfilters::PipelineConfig::default()).unwrap();

    let upstream = solid_linear_source(4, 4, [0.5, 0.3, 0.1, 1.0]);
    let mut src = FilterSource::new(upstream, pipeline).unwrap();

    assert_eq!(src.width(), 4);
    assert_eq!(src.height(), 4);
    assert_eq!(src.format(), format::RGBAF32_LINEAR);

    let data = drain(&mut src);
    let f32_data: &[f32] = bytemuck::cast_slice(&data);
    for px in f32_data.chunks_exact(4) {
        assert!((px[0] - 0.5).abs() < 0.01, "R: {}", px[0]);
        assert!((px[1] - 0.3).abs() < 0.01, "G: {}", px[1]);
        assert!((px[2] - 0.1).abs() < 0.01, "B: {}", px[2]);
        assert!((px[3] - 1.0).abs() < 0.01, "A: {}", px[3]);
    }
}

#[test]
fn filter_source_exposure_per_pixel() {
    // Exposure filter is per-pixel (not neighborhood) — should stream
    let mut pipeline = zenfilters::Pipeline::new(zenfilters::PipelineConfig::default()).unwrap();
    let mut exposure = zenfilters::filters::Exposure::default();
    exposure.stops = 1.0; // +1 stop = ~2× brightness
    pipeline.push(Box::new(exposure));

    assert!(!pipeline.has_neighborhood_filter());

    let upstream = solid_linear_source(4, 4, [0.2, 0.1, 0.05, 1.0]);
    let mut src = FilterSource::new(upstream, pipeline).unwrap();

    let data = drain(&mut src);
    let f32_data: &[f32] = bytemuck::cast_slice(&data);
    // Exposure +1 stop should roughly double values
    for px in f32_data.chunks_exact(4) {
        assert!(px[0] > 0.35, "R should be ~0.4, got {}", px[0]);
        assert!(px[1] > 0.17, "G should be ~0.2, got {}", px[1]);
        assert!(px[2] > 0.08, "B should be ~0.1, got {}", px[2]);
        assert!(
            (px[3] - 1.0).abs() < 0.01,
            "A should be ~1.0, got {}",
            px[3]
        );
    }
}

#[test]
fn filter_source_wrong_format_error() {
    let pipeline = zenfilters::Pipeline::new(zenfilters::PipelineConfig::default()).unwrap();
    // Upstream is Rgba8, FilterSource requires Rgbaf32Linear
    let upstream = solid_source(4, 4, [128, 64, 32, 255]);
    let result = FilterSource::new(upstream, pipeline);
    assert!(result.is_err());
}

#[test]
fn filter_source_small_strips() {
    // Verify streaming works with strip_height=1
    let mut pipeline = zenfilters::Pipeline::new(zenfilters::PipelineConfig::default()).unwrap();
    let mut saturation = zenfilters::filters::Saturation::default();
    saturation.factor = 0.0; // Desaturate fully
    pipeline.push(Box::new(saturation));

    let row_bytes = 4usize * 16; // 4 pixels × 16 bytes/px
    let mut rows_produced = 0u32;
    let upstream: Box<dyn Source> = Box::new(CallbackSource::new(
        4,
        4,
        format::RGBAF32_LINEAR,
        1, // strip height = 1
        move |buf| {
            if rows_produced >= 4 {
                return Ok(false);
            }
            let f32_row: &mut [f32] = bytemuck::cast_slice_mut(&mut buf[..row_bytes]);
            for px in f32_row.chunks_exact_mut(4) {
                px.copy_from_slice(&[0.5, 0.2, 0.1, 1.0]);
            }
            rows_produced += 1;
            Ok(true)
        },
    ));

    let mut src = FilterSource::new(upstream, pipeline).unwrap();
    let mut strip_count = 0;
    while let Ok(Some(strip)) = src.next() {
        assert_eq!(strip.rows(), 1);
        strip_count += 1;
    }
    assert_eq!(strip_count, 4);
}

// ==========================================================================
// Graph integration tests
// ==========================================================================

#[test]
fn filter_graph_exposure_with_auto_convert() {
    // Graph should auto-convert Rgba8 → Rgbaf32Linear for the filter node
    let mut pipeline = zenfilters::Pipeline::new(zenfilters::PipelineConfig::default()).unwrap();
    let mut exposure = zenfilters::filters::Exposure::default();
    exposure.stops = 0.0; // No change — identity
    pipeline.push(Box::new(exposure));

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let filter = g.add_node(NodeOp::Filter(pipeline));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, filter, EdgeKind::Input);
    g.add_edge(filter, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_source(4, 4, [200, 100, 50, 255]));

    let mut compiled = g.compile(sources).unwrap();
    // Output is Rgbaf32Linear (filter's output format)
    assert_eq!(compiled.format(), format::RGBAF32_LINEAR);
    assert_eq!(compiled.width(), 4);
    assert_eq!(compiled.height(), 4);

    let data = drain(compiled.as_mut());
    assert_eq!(data.len(), 4 * 4 * 16); // 16 bytes/pixel
}

#[test]
fn filter_graph_saturation_roundtrip() {
    // Source → Filter(saturation=0) → convert back to Rgba8
    // Full desaturation + reconvert: check dims and format are correct
    let mut pipeline = zenfilters::Pipeline::new(zenfilters::PipelineConfig::default()).unwrap();
    let mut sat = zenfilters::filters::Saturation::default();
    sat.factor = 1.0; // No change (1.0 = identity for saturation scale)
    pipeline.push(Box::new(sat));

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let filter = g.add_node(NodeOp::Filter(pipeline));
    let to_srgb = g.add_node(NodeOp::PixelTransform(Box::new(zenpipe::ops::LinearToSrgb)));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, filter, EdgeKind::Input);
    g.add_edge(filter, to_srgb, EdgeKind::Input);
    g.add_edge(to_srgb, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_source(8, 4, [128, 64, 32, 255]));

    let mut compiled = g.compile(sources).unwrap();
    assert_eq!(compiled.format(), format::RGBA8_SRGB);

    let data = drain(compiled.as_mut());
    assert_eq!(data.len(), 8 * 4 * 4);
    for px in data.chunks_exact(4) {
        // Saturation 1.0 should be near-identity, allow ±3 for oklab roundtrip
        assert!((px[0] as i16 - 128).unsigned_abs() <= 3, "R: {}", px[0]);
        assert!((px[1] as i16 - 64).unsigned_abs() <= 3, "G: {}", px[1]);
        assert!((px[2] as i16 - 32).unsigned_abs() <= 3, "B: {}", px[2]);
        assert_eq!(px[3], 255);
    }
}

#[test]
fn filter_graph_neighborhood_windowed() {
    // Clarity is a neighborhood filter — uses windowed materialization.
    // 16×16 is small enough to fit in one window (strip=min(6*48,16)=16).
    let mut pipeline = zenfilters::Pipeline::new(zenfilters::PipelineConfig::default()).unwrap();
    let clarity = zenfilters::filters::Clarity::default();
    pipeline.push(Box::new(clarity));

    assert!(pipeline.has_neighborhood_filter());

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let filter = g.add_node(NodeOp::Filter(pipeline));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, filter, EdgeKind::Input);
    g.add_edge(filter, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_source(16, 16, [128, 128, 128, 255]));

    let mut compiled = g.compile(sources).unwrap();
    assert_eq!(compiled.format(), format::RGBAF32_LINEAR);
    assert_eq!(compiled.width(), 16);
    assert_eq!(compiled.height(), 16);

    let data = drain(compiled.as_mut());
    assert_eq!(data.len(), 16 * 16 * 16);
}

#[test]
fn filter_graph_neighborhood_large_image() {
    // Test windowed clarity on a larger image to verify strip sliding works
    let mut pipeline = zenfilters::Pipeline::new(zenfilters::PipelineConfig::default()).unwrap();
    let mut clarity = zenfilters::filters::Clarity::default();
    clarity.amount = 0.3;
    pipeline.push(Box::new(clarity));

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let filter = g.add_node(NodeOp::Filter(pipeline));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, filter, EdgeKind::Input);
    g.add_edge(filter, out, EdgeKind::Input);

    // 512×512 image — window slides multiple times with overlap=128, strip=64
    let width = 512u32;
    let height = 512u32;
    let mut rows_produced = 0u32;
    let source: Box<dyn Source> = Box::new(CallbackSource::new(
        width,
        height,
        format::RGBA8_SRGB,
        16,
        move |buf| {
            if rows_produced >= height {
                return Ok(false);
            }
            for x in 0..width as usize {
                let i = x * 4;
                buf[i] = (x & 0xFF) as u8;
                buf[i + 1] = (rows_produced & 0xFF) as u8;
                buf[i + 2] = 128;
                buf[i + 3] = 255;
            }
            rows_produced += 1;
            Ok(true)
        },
    ));

    let mut sources = HashMap::new();
    sources.insert(src, source);

    let mut compiled = g.compile(sources).unwrap();
    assert_eq!(compiled.width(), width);
    assert_eq!(compiled.height(), height);

    let mut total_rows = 0u32;
    let mut strip_count = 0u32;
    while let Ok(Some(strip)) = compiled.next() {
        total_rows += strip.rows();
        strip_count += 1;
    }
    assert_eq!(total_rows, height);
    // For 512×512 with clarity overlap=48, strip=288: ~2 strips.
    // Just verify it's not a single full-frame dump.
    assert!(
        strip_count >= 2,
        "expected multiple strips from windowed filter, got {strip_count}"
    );
}

#[test]
fn filter_graph_chained_with_resize() {
    // Source → Resize → Filter → Output
    // Tests that format conversion chain works: resize outputs Rgba8,
    // filter needs Rgbaf32Linear, graph auto-inserts conversion
    let mut pipeline = zenfilters::Pipeline::new(zenfilters::PipelineConfig::default()).unwrap();
    let exposure = zenfilters::filters::Exposure::default();
    pipeline.push(Box::new(exposure));

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let resize = g.add_node(NodeOp::Resize {
        w: 4,
        h: 4,
        filter: None,
        sharpen_percent: None,
    });
    let filter = g.add_node(NodeOp::Filter(pipeline));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, resize, EdgeKind::Input);
    g.add_edge(resize, filter, EdgeKind::Input);
    g.add_edge(filter, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_source(8, 8, [128, 64, 32, 255]));

    let mut compiled = g.compile(sources).unwrap();
    assert_eq!(compiled.width(), 4);
    assert_eq!(compiled.height(), 4);

    let data = drain(compiled.as_mut());
    assert!(!data.is_empty());
}
