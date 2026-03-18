//! Tests for alpha handling, overlay/watermark, auto-orient, and enhanced resize.

use hashbrown::HashMap;

use zenpipe::Source;
use zenpipe::format;
use zenpipe::graph::{EdgeKind, NodeOp, PipelineGraph};
use zenpipe::sources::CallbackSource;

fn drain(source: &mut dyn Source) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok(Some(strip)) = source.next() {
        out.extend_from_slice(strip.as_strided_bytes());
    }
    out
}

fn solid_rgba8(width: u32, height: u32, pixel: [u8; 4]) -> Box<dyn Source> {
    let row_bytes = width as usize * 4;
    let mut rows = 0u32;
    Box::new(CallbackSource::new(
        width,
        height,
        format::RGBA8_SRGB,
        16,
        move |buf| {
            if rows >= height {
                return Ok(false);
            }
            for px in buf[..row_bytes].chunks_exact_mut(4) {
                px.copy_from_slice(&pixel);
            }
            rows += 1;
            Ok(true)
        },
    ))
}

fn solid_rgb8(width: u32, height: u32, pixel: [u8; 3]) -> Box<dyn Source> {
    let row_bytes = width as usize * 3;
    let mut rows = 0u32;
    Box::new(CallbackSource::new(
        width,
        height,
        format::RGB8_SRGB,
        16,
        move |buf| {
            if rows >= height {
                return Ok(false);
            }
            for px in buf[..row_bytes].chunks_exact_mut(3) {
                px.copy_from_slice(&pixel);
            }
            rows += 1;
            Ok(true)
        },
    ))
}

// =========================================================================
// RemoveAlpha tests
// =========================================================================

#[test]
fn remove_alpha_opaque_pixels() {
    // Fully opaque red → RGB should be [255, 0, 0]
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let rm = g.add_node(NodeOp::RemoveAlpha {
        matte: [255, 255, 255],
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, rm, EdgeKind::Input);
    g.add_edge(rm, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_rgba8(4, 4, [255, 0, 0, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.format(), format::RGB8_SRGB);

    let data = drain(pipeline.as_mut());
    assert_eq!(data.len(), 4 * 4 * 3); // RGB = 3 bytes/pixel
    for px in data.chunks_exact(3) {
        assert_eq!(px, [255, 0, 0]);
    }
}

#[test]
fn remove_alpha_transparent_white_matte() {
    // Fully transparent on white matte → white
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let rm = g.add_node(NodeOp::RemoveAlpha {
        matte: [255, 255, 255],
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, rm, EdgeKind::Input);
    g.add_edge(rm, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_rgba8(4, 4, [255, 0, 0, 0])); // transparent red

    let mut pipeline = g.compile(sources).unwrap();
    let data = drain(pipeline.as_mut());
    for px in data.chunks_exact(3) {
        assert_eq!(px, [255, 255, 255]); // white matte shows through
    }
}

#[test]
fn remove_alpha_50_percent() {
    // 50% red on white → ~(255, 127, 127)
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let rm = g.add_node(NodeOp::RemoveAlpha {
        matte: [255, 255, 255],
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, rm, EdgeKind::Input);
    g.add_edge(rm, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_rgba8(4, 4, [255, 0, 0, 128]));

    let mut pipeline = g.compile(sources).unwrap();
    let data = drain(pipeline.as_mut());
    for px in data.chunks_exact(3) {
        // 255*128/255 + 255*127/255 ≈ 128 + 127 = 255 for R
        assert!(px[0] > 250, "R: {}", px[0]);
        // 0*128/255 + 255*127/255 ≈ 127 for G
        assert!((px[1] as i16 - 127).unsigned_abs() <= 1, "G: {}", px[1]);
        assert!((px[2] as i16 - 127).unsigned_abs() <= 1, "B: {}", px[2]);
    }
}

// =========================================================================
// AddAlpha tests
// =========================================================================

#[test]
fn add_alpha_to_rgb() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let add = g.add_node(NodeOp::AddAlpha);
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, add, EdgeKind::Input);
    g.add_edge(add, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_rgb8(4, 4, [100, 200, 50]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.format(), format::RGBA8_SRGB);

    let data = drain(pipeline.as_mut());
    assert_eq!(data.len(), 4 * 4 * 4); // RGBA = 4 bytes/pixel
    for px in data.chunks_exact(4) {
        assert_eq!(px[0], 100);
        assert_eq!(px[1], 200);
        assert_eq!(px[2], 50);
        assert_eq!(px[3], 255); // opaque
    }
}

#[test]
fn add_alpha_noop_when_already_rgba() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let add = g.add_node(NodeOp::AddAlpha);
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, add, EdgeKind::Input);
    g.add_edge(add, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_rgba8(4, 4, [100, 200, 50, 200]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.format(), format::RGBA8_SRGB);

    let data = drain(pipeline.as_mut());
    for px in data.chunks_exact(4) {
        assert_eq!(px, [100, 200, 50, 200]); // alpha preserved
    }
}

// =========================================================================
// Overlay tests
// =========================================================================

#[test]
fn overlay_opaque_covers_background() {
    // 4×4 background + 2×2 opaque overlay at (1,1)
    let overlay_data = [255u8, 0, 0, 255].repeat(2 * 2); // red RGBA8

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let ov = g.add_node(NodeOp::Overlay {
        image_data: overlay_data,
        width: 2,
        height: 2,
        format: format::RGBA8_SRGB,
        x: 1,
        y: 1,
        opacity: 1.0,
        blend_mode: None,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, ov, EdgeKind::Input);
    g.add_edge(ov, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_rgba8(4, 4, [0, 0, 0, 255])); // black background

    let mut pipeline = g.compile(sources).unwrap();
    // Output is premul linear f32
    assert_eq!(pipeline.width(), 4);
    assert_eq!(pipeline.height(), 4);

    let data = drain(pipeline.as_mut());
    assert!(!data.is_empty());
}

#[test]
fn overlay_with_opacity() {
    let overlay_data = [255u8, 0, 0, 255].repeat(4 * 4);

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let ov = g.add_node(NodeOp::Overlay {
        image_data: overlay_data,
        width: 4,
        height: 4,
        format: format::RGBA8_SRGB,
        x: 0,
        y: 0,
        opacity: 0.5,
        blend_mode: None,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, ov, EdgeKind::Input);
    g.add_edge(ov, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_rgba8(4, 4, [0, 0, 255, 255])); // blue background

    let mut pipeline = g.compile(sources).unwrap();
    let data = drain(pipeline.as_mut());
    let f32_data: &[f32] = bytemuck::cast_slice(&data);
    // Should be a blend of red and blue
    for px in f32_data.chunks_exact(4) {
        assert!(px[0] > 0.1, "R should have red contribution, got {}", px[0]);
        assert!(
            px[2] > 0.1,
            "B should have blue contribution, got {}",
            px[2]
        );
    }
}

// =========================================================================
// AutoOrient tests
// =========================================================================

#[test]
fn auto_orient_rotate90() {
    // 4×2 image with AutoOrient(6) (EXIF Rotate90) → should become 2×4
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let orient = g.add_node(NodeOp::AutoOrient(6)); // EXIF 6 = Rotate90
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, orient, EdgeKind::Input);
    g.add_edge(orient, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_rgba8(4, 2, [42, 42, 42, 255]));

    let pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 2);
    assert_eq!(pipeline.height(), 4);
}

#[test]
fn auto_orient_identity() {
    // EXIF 1 = Identity → no-op
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let orient = g.add_node(NodeOp::AutoOrient(1));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, orient, EdgeKind::Input);
    g.add_edge(orient, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_rgba8(4, 4, [42, 42, 42, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 4);
    assert_eq!(pipeline.height(), 4);

    let data = drain(pipeline.as_mut());
    for px in data.chunks_exact(4) {
        assert_eq!(px, [42, 42, 42, 255]);
    }
}

#[test]
fn auto_orient_invalid_value() {
    // EXIF 0 or 99 → treated as identity
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let orient = g.add_node(NodeOp::AutoOrient(99));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, orient, EdgeKind::Input);
    g.add_edge(orient, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_rgba8(4, 4, [10, 20, 30, 255]));

    let pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 4);
    assert_eq!(pipeline.height(), 4);
}

// =========================================================================
// Enhanced resize tests
// =========================================================================

#[test]
fn resize_with_filter() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let resize = g.add_node(NodeOp::Resize {
        w: 4,
        h: 4,
        filter: Some(zenresize::Filter::Mitchell),
        sharpen_percent: None,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, resize, EdgeKind::Input);
    g.add_edge(resize, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_rgba8(8, 8, [128, 64, 32, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 4);
    assert_eq!(pipeline.height(), 4);

    let data = drain(pipeline.as_mut());
    for px in data.chunks_exact(4) {
        assert!((px[0] as i16 - 128).unsigned_abs() <= 2);
    }
}

#[test]
fn resize_with_sharpen() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let resize = g.add_node(NodeOp::Resize {
        w: 4,
        h: 4,
        filter: Some(zenresize::Filter::Lanczos),
        sharpen_percent: Some(15.0),
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, resize, EdgeKind::Input);
    g.add_edge(resize, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_rgba8(8, 8, [128, 64, 32, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 4);
    assert_eq!(pipeline.height(), 4);

    let data = drain(pipeline.as_mut());
    assert_eq!(data.len(), 4 * 4 * 4);
}
