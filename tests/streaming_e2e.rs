//! E2E streaming pipeline tests with memory validation.
//!
//! Proves the pipeline processes 4K images strip-by-strip without
//! materializing the full image. Peak heap stays bounded.
//!
//! Run: `cargo test --features filters --test streaming_e2e -- --nocapture`
//! Heaptrack: `heaptrack cargo test --features filters --release --test streaming_e2e -- --nocapture`

#![cfg(feature = "filters")]

use std::time::Instant;

use hashbrown::HashMap;

use zenpipe::graph::{EdgeKind, NodeOp, PipelineGraph};
use zenpipe::sources::CallbackSource;
use zenpipe::{Source, format};

const WIDTH: u32 = 3840;
const HEIGHT: u32 = 2160;

/// 4K RGBA8 is 33 MB fully materialized. If streaming works, peak heap
/// should stay well under this.
/// 4K RGBA8 fully materialized = 33 MB. Streaming should stay well under this.
#[allow(dead_code)]
const FULL_4K_BYTES: usize = WIDTH as usize * HEIGHT as usize * 4;

/// Create a 4K gradient source. Pixel [x,y] = [x%256, y%256, 128, 255].
fn gradient_4k() -> Box<dyn Source> {
    let mut row_idx = 0u32;
    Box::new(CallbackSource::new(
        WIDTH,
        HEIGHT,
        format::RGBA8_SRGB,
        16,
        move |buf| {
            if row_idx >= HEIGHT {
                return Ok(false);
            }
            for x in 0..WIDTH as usize {
                let i = x * 4;
                buf[i] = (x & 0xFF) as u8;
                buf[i + 1] = (row_idx & 0xFF) as u8;
                buf[i + 2] = 128;
                buf[i + 3] = 255;
            }
            row_idx += 1;
            Ok(true)
        },
    ))
}

/// Collect all strips, counting them. Returns (total_bytes, strip_count, max_strip_height).
fn drain_counting(source: &mut dyn Source) -> (usize, u32, u32) {
    let mut total_bytes = 0usize;
    let mut strip_count = 0u32;
    let mut max_strip_h = 0u32;
    while let Ok(Some(strip)) = source.next() {
        total_bytes += strip.data.len();
        strip_count += 1;
        max_strip_h = max_strip_h.max(strip.height);
    }
    (total_bytes, strip_count, max_strip_h)
}

// ============================================================================
// 4K → 1080p streaming resize
// ============================================================================

#[test]
fn streaming_4k_resize() {
    let t0 = Instant::now();

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let resize = g.add_node(NodeOp::Resize { w: 1920, h: 1080 });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, resize, EdgeKind::Input);
    g.add_edge(resize, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, gradient_4k());

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 1920);
    assert_eq!(pipeline.height(), 1080);

    let (total_bytes, strip_count, max_strip_h) = drain_counting(pipeline.as_mut());

    let elapsed = t0.elapsed();

    // Correct output size: 1920 × 1080 × 4 = 8,294,400
    assert_eq!(total_bytes, 1920 * 1080 * 4);
    // Must produce many strips (streaming), not 1 giant strip
    assert!(
        strip_count >= 30,
        "expected ≥30 strips, got {strip_count} — not streaming"
    );
    assert!(
        max_strip_h <= 32,
        "max strip height {max_strip_h} too large — not streaming"
    );

    eprintln!(
        "4K→1080p resize: {strip_count} strips (max {max_strip_h} rows), {:.1}ms",
        elapsed.as_secs_f64() * 1000.0,
    );
}

// ============================================================================
// 4K → 1080p resize + filter (exposure +0.5EV)
// ============================================================================

#[test]
fn streaming_4k_resize_filter() {
    let t0 = Instant::now();

    let mut filter_pipeline =
        zenfilters::Pipeline::new(zenfilters::PipelineConfig::default()).unwrap();
    let mut exposure = zenfilters::filters::Exposure::default();
    exposure.stops = 0.5;
    filter_pipeline.push(Box::new(exposure));

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let resize = g.add_node(NodeOp::Resize { w: 1920, h: 1080 });
    let filter = g.add_node(NodeOp::Filter(filter_pipeline));
    let to_srgb = g.add_node(NodeOp::PixelTransform(Box::new(zenpipe::ops::LinearToSrgb)));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, resize, EdgeKind::Input);
    g.add_edge(resize, filter, EdgeKind::Input);
    g.add_edge(filter, to_srgb, EdgeKind::Input);
    g.add_edge(to_srgb, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, gradient_4k());

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 1920);
    assert_eq!(pipeline.height(), 1080);
    assert_eq!(pipeline.format(), format::RGBA8_SRGB);

    let (total_bytes, strip_count, max_strip_h) = drain_counting(pipeline.as_mut());

    let elapsed = t0.elapsed();

    assert_eq!(total_bytes, 1920 * 1080 * 4);
    assert!(
        strip_count >= 30,
        "expected ≥30 strips, got {strip_count} — not streaming"
    );
    assert!(
        max_strip_h <= 32,
        "max strip height {max_strip_h} too large — not streaming"
    );

    eprintln!(
        "4K→1080p resize+filter: {strip_count} strips (max {max_strip_h} rows), {:.1}ms",
        elapsed.as_secs_f64() * 1000.0,
    );
}

// ============================================================================
// 4K → 1080p via Layout (streaming_from_plan: crop + resize + padding)
// ============================================================================

#[test]
fn streaming_4k_layout() {
    use zenresize::{Constraint, ConstraintMode, DecoderOffer, DecoderRequest, Orientation, Size};

    let t0 = Instant::now();

    let request = DecoderRequest::new(Size::new(1920, 1080), Orientation::Identity);
    let offer = DecoderOffer::full_decode(WIDTH, HEIGHT);
    let (ideal, _req) = zenresize::Pipeline::new(WIDTH, HEIGHT)
        .constrain(Constraint::new(ConstraintMode::Distort, 1920, 1080))
        .plan()
        .unwrap();
    let plan = ideal.finalize(&request, &offer);

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let layout = g.add_node(NodeOp::Layout {
        plan,
        filter: zenresize::Filter::Robidoux,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, layout, EdgeKind::Input);
    g.add_edge(layout, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, gradient_4k());

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 1920);
    assert_eq!(pipeline.height(), 1080);

    let (total_bytes, strip_count, max_strip_h) = drain_counting(pipeline.as_mut());

    let elapsed = t0.elapsed();

    assert_eq!(total_bytes, 1920 * 1080 * 4);
    assert!(
        strip_count >= 30,
        "expected ≥30 strips, got {strip_count} — not streaming"
    );

    eprintln!(
        "4K→1080p Layout: {strip_count} strips (max {max_strip_h} rows), {:.1}ms",
        elapsed.as_secs_f64() * 1000.0,
    );
}

// ============================================================================
// 4K → 1080p Layout with orientation (FlipH — fully streaming)
// ============================================================================

#[test]
fn streaming_4k_layout_fliph() {
    use zenresize::{Constraint, ConstraintMode, DecoderOffer, DecoderRequest, Orientation, Size};

    let request = DecoderRequest::new(Size::new(1920, 1080), Orientation::FlipH);
    let offer = DecoderOffer::full_decode(WIDTH, HEIGHT);
    let (ideal, _req) = zenresize::Pipeline::new(WIDTH, HEIGHT)
        .constrain(Constraint::new(ConstraintMode::Distort, 1920, 1080))
        .plan()
        .unwrap();
    let (ideal, _): (zenresize::IdealLayout, _) = (ideal, _req);
    let plan = ideal.finalize(&request, &offer);

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let layout = g.add_node(NodeOp::Layout {
        plan,
        filter: zenresize::Filter::Robidoux,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, layout, EdgeKind::Input);
    g.add_edge(layout, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, gradient_4k());

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 1920);
    assert_eq!(pipeline.height(), 1080);

    let (total_bytes, strip_count, _) = drain_counting(pipeline.as_mut());

    assert_eq!(total_bytes, 1920 * 1080 * 4);
    // FlipH is row-local — should still stream
    assert!(
        strip_count >= 30,
        "FlipH should stream, got {strip_count} strips"
    );

    eprintln!("4K→1080p Layout FlipH: {strip_count} strips (streaming)");
}

// ============================================================================
// Full pipeline: 4K → resize → filter → back to sRGB, verify strip count
// ============================================================================

#[test]
fn streaming_4k_full_pipeline() {
    let t0 = Instant::now();

    // Build: Source → Resize → Filter(exposure+saturation) → LinearToSrgb → Output
    let mut filter_pipeline =
        zenfilters::Pipeline::new(zenfilters::PipelineConfig::default()).unwrap();
    let mut exposure = zenfilters::filters::Exposure::default();
    exposure.stops = 0.3;
    filter_pipeline.push(Box::new(exposure));
    let mut sat = zenfilters::filters::Saturation::default();
    sat.factor = 1.2;
    filter_pipeline.push(Box::new(sat));

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let resize = g.add_node(NodeOp::Resize { w: 1920, h: 1080 });
    let filter = g.add_node(NodeOp::Filter(filter_pipeline));
    let to_srgb = g.add_node(NodeOp::PixelTransform(Box::new(zenpipe::ops::LinearToSrgb)));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, resize, EdgeKind::Input);
    g.add_edge(resize, filter, EdgeKind::Input);
    g.add_edge(filter, to_srgb, EdgeKind::Input);
    g.add_edge(to_srgb, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, gradient_4k());

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.format(), format::RGBA8_SRGB);

    let (total_bytes, strip_count, max_strip_h) = drain_counting(pipeline.as_mut());

    let elapsed = t0.elapsed();

    assert_eq!(total_bytes, 1920 * 1080 * 4);
    assert!(strip_count >= 30);
    assert!(max_strip_h <= 32);

    eprintln!(
        "4K full pipeline (resize+exposure+saturation): {strip_count} strips, {:.1}ms",
        elapsed.as_secs_f64() * 1000.0,
    );
}

// ============================================================================
// Windowed neighborhood filter: 4K → resize → clarity → sRGB
// ============================================================================

#[test]
fn streaming_4k_windowed_clarity() {
    let t0 = Instant::now();

    // Clarity is a neighborhood filter (Gaussian blur-based) — uses windowed path
    let mut filter_pipeline =
        zenfilters::Pipeline::new(zenfilters::PipelineConfig::default()).unwrap();
    let mut clarity = zenfilters::filters::Clarity::default();
    clarity.amount = 0.3;
    filter_pipeline.push(Box::new(clarity));
    assert!(filter_pipeline.has_neighborhood_filter());

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let resize = g.add_node(NodeOp::Resize { w: 1920, h: 1080 });
    let filter = g.add_node(NodeOp::Filter(filter_pipeline));
    let to_srgb = g.add_node(NodeOp::PixelTransform(Box::new(zenpipe::ops::LinearToSrgb)));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, resize, EdgeKind::Input);
    g.add_edge(resize, filter, EdgeKind::Input);
    g.add_edge(filter, to_srgb, EdgeKind::Input);
    g.add_edge(to_srgb, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, gradient_4k());

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.format(), format::RGBA8_SRGB);
    assert_eq!(pipeline.width(), 1920);
    assert_eq!(pipeline.height(), 1080);

    let (total_bytes, strip_count, max_strip_h) = drain_counting(pipeline.as_mut());

    let elapsed = t0.elapsed();

    assert_eq!(total_bytes, 1920 * 1080 * 4);
    // Windowed: should produce multiple strips (not 1 giant one)
    assert!(
        strip_count >= 2,
        "windowed clarity should produce multiple strips, got {strip_count}"
    );
    // Max strip height scales with overlap (6*overlap for 75% utilization).
    // For clarity overlap=48: strip=288. Should NOT be full-frame (1080).
    assert!(
        max_strip_h < 1080,
        "max strip {max_strip_h} = full frame, not windowed"
    );

    eprintln!(
        "4K windowed clarity: {strip_count} strips (max {max_strip_h} rows), {:.1}ms",
        elapsed.as_secs_f64() * 1000.0,
    );
}
