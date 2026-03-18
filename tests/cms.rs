//! Tests for ICC color management integration (requires `cms` feature).
//!
//! Validates streaming ICC profile transforms via moxcms:
//! - sRGB ↔ Display P3 transforms
//! - Graph integration with IccTransform node
//! - Format preservation through transforms
//! - Neutral gray stability across gamuts

#![cfg(feature = "std")]
use std::sync::Arc;

use hashbrown::HashMap;
use moxcms::ColorProfile;

use zenpipe::Source;
use zenpipe::format;
use zenpipe::graph::{EdgeKind, NodeOp, PipelineGraph};
use zenpipe::sources::{CallbackSource, IccTransformSource};

/// Collect all strips from a source into a flat Vec<u8>.
fn drain(source: &mut dyn Source) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok(Some(strip)) = source.next() {
        out.extend_from_slice(strip.data());
    }
    out
}

/// Get sRGB ICC profile bytes.
fn srgb_icc() -> Vec<u8> {
    ColorProfile::new_srgb().encode().unwrap()
}

/// Get Display P3 ICC profile bytes.
fn p3_icc() -> Vec<u8> {
    ColorProfile::new_display_p3().encode().unwrap()
}

/// Create a solid RGBA8 sRGB source.
fn solid_rgba8(width: u32, height: u32, pixel: [u8; 4]) -> Box<dyn Source> {
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

// =========================================================================
// IccTransformSource direct tests
// =========================================================================

#[test]
fn icc_srgb_to_p3_basic() {
    let src = solid_rgba8(4, 4, [128, 128, 128, 255]);
    let srgb = srgb_icc();
    let p3 = p3_icc();

    let mut transform = IccTransformSource::new(src, &srgb, &p3).unwrap();

    assert_eq!(transform.width(), 4);
    assert_eq!(transform.height(), 4);
    // Format (layout/depth) is preserved — only color values change.
    assert_eq!(transform.format(), format::RGBA8_SRGB);

    let data = drain(&mut transform);
    assert_eq!(data.len(), 4 * 4 * 4);

    // Neutral gray (128,128,128) should remain near-neutral under gamut mapping.
    // sRGB and P3 share the D65 white point, so neutrals are stable.
    for px in data.chunks_exact(4) {
        assert!(
            (px[0] as i16 - 128).unsigned_abs() <= 2,
            "R: {} (expected ~128)",
            px[0]
        );
        assert!(
            (px[1] as i16 - 128).unsigned_abs() <= 2,
            "G: {} (expected ~128)",
            px[1]
        );
        assert!(
            (px[2] as i16 - 128).unsigned_abs() <= 2,
            "B: {} (expected ~128)",
            px[2]
        );
        assert_eq!(px[3], 255);
    }
}

#[test]
fn icc_p3_to_srgb_basic() {
    let src = solid_rgba8(4, 4, [200, 100, 50, 255]);
    let srgb = srgb_icc();
    let p3 = p3_icc();

    // Interpret as P3, convert to sRGB.
    let mut transform = IccTransformSource::new(src, &p3, &srgb).unwrap();

    let data = drain(&mut transform);
    assert_eq!(data.len(), 4 * 4 * 4);

    // P3 colors interpreted as sRGB should shift (P3 has wider gamut).
    // The exact values depend on moxcms's gamut mapping, but the transform
    // should produce valid u8 output without panicking.
    for px in data.chunks_exact(4) {
        // Just verify values are in valid range and alpha preserved
        assert_eq!(px[3], 255);
    }
}

#[test]
fn icc_srgb_roundtrip() {
    // sRGB → P3 → sRGB should be near-identity for in-gamut colors.
    let srgb = srgb_icc();
    let p3 = p3_icc();

    let src = solid_rgba8(4, 2, [200, 100, 50, 200]);

    let step1 = IccTransformSource::new(src, &srgb, &p3).unwrap();
    let mut step2 = IccTransformSource::new(Box::new(step1), &p3, &srgb).unwrap();

    let data = drain(&mut step2);
    for px in data.chunks_exact(4) {
        assert!(
            (px[0] as i16 - 200).unsigned_abs() <= 3,
            "R roundtrip: {} vs 200",
            px[0]
        );
        assert!(
            (px[1] as i16 - 100).unsigned_abs() <= 3,
            "G roundtrip: {} vs 100",
            px[1]
        );
        assert!(
            (px[2] as i16 - 50).unsigned_abs() <= 3,
            "B roundtrip: {} vs 50",
            px[2]
        );
        assert!(
            (px[3] as i16 - 200).unsigned_abs() <= 1,
            "A roundtrip: {} vs 200",
            px[3]
        );
    }
}

#[test]
fn icc_streaming_strips() {
    // Verify streaming works with multiple strips (not just one big strip).
    let srgb = srgb_icc();
    let p3 = p3_icc();

    let src = solid_rgba8(4, 50, [180, 90, 45, 255]);
    let mut transform = IccTransformSource::new(src, &srgb, &p3).unwrap();

    let mut total_rows = 0u32;
    let mut strip_count = 0u32;
    while let Ok(Some(strip)) = transform.next() {
        total_rows += strip.height();
        strip_count += 1;
    }
    assert_eq!(total_rows, 50);
    // strip_height=16: 16+16+16+2 = 4 strips
    assert_eq!(strip_count, 4);
}

// =========================================================================
// Graph integration
// =========================================================================

#[test]
fn graph_icc_transform_node() {
    let srgb = srgb_icc();
    let p3 = p3_icc();

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let icc = g.add_node(NodeOp::IccTransform {
        src_icc: Arc::from(srgb.as_slice()),
        dst_icc: Arc::from(p3.as_slice()),
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, icc, EdgeKind::Input);
    g.add_edge(icc, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_rgba8(4, 4, [128, 128, 128, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 4);
    assert_eq!(pipeline.height(), 4);

    let data = drain(pipeline.as_mut());
    assert_eq!(data.len(), 4 * 4 * 4);

    // Neutral gray should be stable.
    for px in data.chunks_exact(4) {
        assert!((px[0] as i16 - 128).unsigned_abs() <= 2, "R: {}", px[0]);
    }
}

#[test]
fn graph_icc_then_crop() {
    // ICC transform → crop — both streaming, no materialization.
    let srgb = srgb_icc();
    let p3 = p3_icc();

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let icc = g.add_node(NodeOp::IccTransform {
        src_icc: Arc::from(srgb.as_slice()),
        dst_icc: Arc::from(p3.as_slice()),
    });
    let crop = g.add_node(NodeOp::Crop {
        x: 1,
        y: 1,
        w: 2,
        h: 2,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, icc, EdgeKind::Input);
    g.add_edge(icc, crop, EdgeKind::Input);
    g.add_edge(crop, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_rgba8(8, 8, [255, 0, 0, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 2);
    assert_eq!(pipeline.height(), 2);

    let data = drain(pipeline.as_mut());
    assert_eq!(data.len(), 2 * 2 * 4);
}

#[test]
fn graph_crop_then_icc() {
    // crop → ICC transform.
    let srgb = srgb_icc();
    let p3 = p3_icc();

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let crop = g.add_node(NodeOp::Crop {
        x: 0,
        y: 0,
        w: 4,
        h: 4,
    });
    let icc = g.add_node(NodeOp::IccTransform {
        src_icc: Arc::from(srgb.as_slice()),
        dst_icc: Arc::from(p3.as_slice()),
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, crop, EdgeKind::Input);
    g.add_edge(crop, icc, EdgeKind::Input);
    g.add_edge(icc, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_rgba8(8, 8, [100, 200, 50, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 4);
    assert_eq!(pipeline.height(), 4);

    let data = drain(pipeline.as_mut());
    assert_eq!(data.len(), 4 * 4 * 4);
    // Alpha should be preserved.
    for px in data.chunks_exact(4) {
        assert_eq!(px[3], 255);
    }
}
