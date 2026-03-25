//! Coverage-fill tests for low/zero-coverage modules:
//! - sources/expand_canvas.rs
//! - sources/flip.rs
//! - sources/mask_transform.rs
//! - error.rs
//! - graph.rs (additional NodeOp compilation paths)

use hashbrown::HashMap;

use zenpipe::graph::{EdgeKind, NodeOp, PipelineGraph, SourceInfo};
use zenpipe::sources::{CallbackSource, ExpandCanvasSource, FlipHSource, MaterializedSource};
use zenpipe::{PipeError, Source, format};

// =============================================================================
// Test helpers
// =============================================================================

/// Collect all strips from a source into a flat Vec<u8>.
fn drain(source: &mut dyn Source) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok(Some(strip)) = source.next() {
        out.extend_from_slice(strip.as_strided_bytes());
    }
    out
}

/// Create a solid-color RGBA8 source (boxed for graph use).
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

/// Create a gradient source where pixel = [x, y, 0, 255].
fn gradient_source(width: u32, height: u32) -> Box<dyn Source> {
    let mut row_idx = 0u32;
    Box::new(CallbackSource::new(
        width,
        height,
        format::RGBA8_SRGB,
        16,
        move |buf| {
            if row_idx >= height {
                return Ok(false);
            }
            for x in 0..width as usize {
                let i = x * 4;
                buf[i] = x as u8;
                buf[i + 1] = row_idx as u8;
                buf[i + 2] = 0;
                buf[i + 3] = 255;
            }
            row_idx += 1;
            Ok(true)
        },
    ))
}

/// Create a solid-color RGB8 source (boxed for graph use).
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

// =============================================================================
// ExpandCanvasSource tests (sources/expand_canvas.rs)
// =============================================================================

#[test]
fn expand_canvas_padding_all_sides() {
    // 4x4 red image placed at (2, 2) on an 8x8 canvas with blue background
    let src = solid_source(4, 4, [255, 0, 0, 255]);
    let bg = [0, 0, 255, 255]; // blue
    let mut canvas = ExpandCanvasSource::new(src, 8, 8, 2, 2, bg);

    assert_eq!(canvas.width(), 8);
    assert_eq!(canvas.height(), 8);
    assert_eq!(canvas.format(), format::RGBA8_SRGB);

    let data = drain(&mut canvas);
    assert_eq!(data.len(), 8 * 8 * 4);

    // Row 0 (pure padding): should be blue
    assert_eq!(&data[0..4], &[0, 0, 255, 255]);
    assert_eq!(&data[7 * 4..8 * 4], &[0, 0, 255, 255]);

    // Row 1 (pure padding): should be blue
    let row1 = 8 * 4;
    assert_eq!(&data[row1..row1 + 4], &[0, 0, 255, 255]);

    // Row 2, col 0-1 (left padding): blue
    let row2 = 2 * 8 * 4;
    assert_eq!(&data[row2..row2 + 4], &[0, 0, 255, 255]);
    assert_eq!(&data[row2 + 4..row2 + 8], &[0, 0, 255, 255]);

    // Row 2, col 2 (content start): red
    assert_eq!(&data[row2 + 8..row2 + 12], &[255, 0, 0, 255]);

    // Row 2, col 5 (content end): red
    assert_eq!(&data[row2 + 5 * 4..row2 + 6 * 4], &[255, 0, 0, 255]);

    // Row 2, col 6 (right padding): blue
    assert_eq!(&data[row2 + 6 * 4..row2 + 7 * 4], &[0, 0, 255, 255]);

    // Row 6 (below content, pure padding): blue
    let row6 = 6 * 8 * 4;
    assert_eq!(&data[row6..row6 + 4], &[0, 0, 255, 255]);
}

#[test]
fn expand_canvas_no_padding() {
    // Place a 4x4 image at (0, 0) on a 4x4 canvas — should be identity
    let src = solid_source(4, 4, [42, 42, 42, 255]);
    let mut canvas = ExpandCanvasSource::new(src, 4, 4, 0, 0, [0, 0, 0, 0]);

    assert_eq!(canvas.width(), 4);
    assert_eq!(canvas.height(), 4);

    let data = drain(&mut canvas);
    assert_eq!(data.len(), 4 * 4 * 4);
    for px in data.chunks_exact(4) {
        assert_eq!(px, [42, 42, 42, 255]);
    }
}

#[test]
fn expand_canvas_negative_offset_crops_content() {
    // Place at (-2, -1) on a 4x4 canvas: skip 2 source cols, 1 source row
    // Source is 6x6 gradient, canvas is 4x4
    let src = gradient_source(6, 6);
    let mut canvas = ExpandCanvasSource::new(src, 4, 4, -2, -1, [0, 0, 0, 255]);

    assert_eq!(canvas.width(), 4);
    assert_eq!(canvas.height(), 4);

    let data = drain(&mut canvas);
    assert_eq!(data.len(), 4 * 4 * 4);

    // First output row should be source row 1, cols 2..6
    // Source pixel (2, 1) → output (0, 0): x=2, y=1
    assert_eq!(data[0], 2); // x
    assert_eq!(data[1], 1); // y

    // Output pixel (1, 0): source (3, 1)
    assert_eq!(data[4], 3);
    assert_eq!(data[5], 1);
}

#[test]
fn expand_canvas_larger_canvas_than_source() {
    // 2x2 source, 10x10 canvas, placed at (4, 4)
    let src = solid_source(2, 2, [128, 64, 32, 255]);
    let mut canvas = ExpandCanvasSource::new(src, 10, 10, 4, 4, [0, 0, 0, 255]);

    assert_eq!(canvas.width(), 10);
    assert_eq!(canvas.height(), 10);

    let data = drain(&mut canvas);
    assert_eq!(data.len(), 10 * 10 * 4);

    // Check that the content pixel at (4, 4) is correct
    let offset = (4 * 10 + 4) * 4;
    assert_eq!(&data[offset..offset + 4], &[128, 64, 32, 255]);

    // Check that (4, 5) is also content
    let offset2 = (5 * 10 + 4) * 4;
    assert_eq!(&data[offset2..offset2 + 4], &[128, 64, 32, 255]);

    // Check that (3, 4) is padding (black)
    let pad = (4 * 10 + 3) * 4;
    assert_eq!(&data[pad..pad + 4], &[0, 0, 0, 255]);
}

// =============================================================================
// FlipHSource tests (sources/flip.rs)
// =============================================================================

#[test]
fn flip_h_reverses_pixel_order() {
    // 4-pixel wide gradient: [0,y,0,255], [1,y,0,255], [2,y,0,255], [3,y,0,255]
    let src = gradient_source(4, 2);
    let mut flip = FlipHSource::new(src);

    assert_eq!(flip.width(), 4);
    assert_eq!(flip.height(), 2);
    assert_eq!(flip.format(), format::RGBA8_SRGB);

    let data = drain(&mut flip);
    assert_eq!(data.len(), 4 * 2 * 4);

    // Row 0: x values should be reversed [3, 2, 1, 0]
    assert_eq!(data[0], 3);
    assert_eq!(data[4], 2);
    assert_eq!(data[8], 1);
    assert_eq!(data[12], 0);

    // y should still be 0
    assert_eq!(data[1], 0);

    // Row 1: x values reversed, y = 1
    let row1 = 4 * 4;
    assert_eq!(data[row1], 3);
    assert_eq!(data[row1 + 4], 2);
    assert_eq!(data[row1 + 1], 1); // y
}

#[test]
fn flip_h_single_pixel_wide() {
    // 1-pixel wide: flip should be identity
    let src = solid_source(1, 4, [100, 200, 50, 255]);
    let mut flip = FlipHSource::new(src);

    assert_eq!(flip.width(), 1);
    assert_eq!(flip.height(), 4);

    let data = drain(&mut flip);
    for px in data.chunks_exact(4) {
        assert_eq!(px, [100, 200, 50, 255]);
    }
}

#[test]
fn flip_h_double_flip_is_identity() {
    // Flipping twice should return original pixel order
    let src = gradient_source(8, 4);
    let flip1 = FlipHSource::new(src);
    let mut flip2 = FlipHSource::new(Box::new(flip1));

    let data = drain(&mut flip2);

    // Verify original gradient: x coord at pixel position
    for y in 0..4usize {
        for x in 0..8usize {
            let i = (y * 8 + x) * 4;
            assert_eq!(data[i], x as u8, "x at ({x},{y})");
            assert_eq!(data[i + 1], y as u8, "y at ({x},{y})");
        }
    }
}

#[test]
fn flip_h_multi_strip() {
    // Tall image to ensure multiple strips are all flipped
    let src = gradient_source(4, 50);
    let mut flip = FlipHSource::new(src);

    let mut total_rows = 0u32;
    while let Ok(Some(strip)) = flip.next() {
        assert_eq!(strip.width(), 4);
        // Each strip should be flipped
        for r in 0..strip.rows() {
            let row = strip.row(r);
            assert_eq!(row[0], 3, "first pixel x should be 3 (flipped)");
            assert_eq!(row[3 * 4], 0, "last pixel x should be 0 (flipped)");
        }
        total_rows += strip.rows();
    }
    assert_eq!(total_rows, 50);
}

// =============================================================================
// MaskTransformSource tests (sources/mask_transform.rs)
// =============================================================================

/// A simple mask that returns half-alpha (0.5) for all pixels.
struct HalfMask;

impl zenblend::mask::MaskSource for HalfMask {
    fn fill_mask_row(&self, dst: &mut [f32], _y: u32) -> zenblend::mask::MaskFill {
        for v in dst.iter_mut() {
            *v = 0.5;
        }
        zenblend::mask::MaskFill::Partial
    }
}

/// A mask that's fully transparent (0.0) for all pixels.
struct TransparentMask;

impl zenblend::mask::MaskSource for TransparentMask {
    fn fill_mask_row(&self, dst: &mut [f32], _y: u32) -> zenblend::mask::MaskFill {
        for v in dst.iter_mut() {
            *v = 0.0;
        }
        zenblend::mask::MaskFill::AllTransparent
    }
}

/// A mask that's fully opaque (1.0) for all pixels.
struct OpaqueMask;

impl zenblend::mask::MaskSource for OpaqueMask {
    fn fill_mask_row(&self, dst: &mut [f32], _y: u32) -> zenblend::mask::MaskFill {
        for v in dst.iter_mut() {
            *v = 1.0;
        }
        zenblend::mask::MaskFill::AllOpaque
    }
}

/// Create a solid f32 premul linear RGBA source.
fn solid_premul_f32(width: u32, height: u32, pixel: [f32; 4]) -> Box<dyn Source> {
    let row_floats = width as usize * 4;
    let row_bytes = row_floats * 4;
    let mut rows_produced = 0u32;
    Box::new(CallbackSource::new(
        width,
        height,
        format::RGBAF32_LINEAR_PREMUL,
        16,
        move |buf| {
            if rows_produced >= height {
                return Ok(false);
            }
            let f32_buf: &mut [f32] = bytemuck::cast_slice_mut(&mut buf[..row_bytes]);
            for px in f32_buf.chunks_exact_mut(4) {
                px.copy_from_slice(&pixel);
            }
            rows_produced += 1;
            Ok(true)
        },
    ))
}

#[test]
fn mask_transform_half_alpha() {
    // Opaque white premul → half mask → all channels halved
    use zenpipe::sources::MaskTransformSource;

    let src = solid_premul_f32(4, 4, [1.0, 1.0, 1.0, 1.0]);
    let mask = Box::new(HalfMask);
    let mut mt = MaskTransformSource::new(src, mask).unwrap();

    assert_eq!(mt.width(), 4);
    assert_eq!(mt.height(), 4);
    assert_eq!(mt.format(), format::RGBAF32_LINEAR_PREMUL);

    let data = drain(&mut mt);
    let floats: &[f32] = bytemuck::cast_slice(&data);

    for px in floats.chunks_exact(4) {
        assert!(
            (px[0] - 0.5).abs() < 0.01,
            "R should be ~0.5, got {}",
            px[0]
        );
        assert!(
            (px[1] - 0.5).abs() < 0.01,
            "G should be ~0.5, got {}",
            px[1]
        );
        assert!(
            (px[2] - 0.5).abs() < 0.01,
            "B should be ~0.5, got {}",
            px[2]
        );
        assert!(
            (px[3] - 0.5).abs() < 0.01,
            "A should be ~0.5, got {}",
            px[3]
        );
    }
}

#[test]
fn mask_transform_transparent_mask_zeros_output() {
    use zenpipe::sources::MaskTransformSource;

    let src = solid_premul_f32(4, 2, [1.0, 0.5, 0.25, 1.0]);
    let mask = Box::new(TransparentMask);
    let mut mt = MaskTransformSource::new(src, mask).unwrap();

    let data = drain(&mut mt);
    let floats: &[f32] = bytemuck::cast_slice(&data);

    for px in floats.chunks_exact(4) {
        assert!(px[0].abs() < 0.01, "R should be ~0, got {}", px[0]);
        assert!(px[1].abs() < 0.01, "G should be ~0, got {}", px[1]);
        assert!(px[2].abs() < 0.01, "B should be ~0, got {}", px[2]);
        assert!(px[3].abs() < 0.01, "A should be ~0, got {}", px[3]);
    }
}

#[test]
fn mask_transform_opaque_mask_preserves_pixels() {
    use zenpipe::sources::MaskTransformSource;

    let src = solid_premul_f32(4, 2, [0.8, 0.4, 0.2, 1.0]);
    let mask = Box::new(OpaqueMask);
    let mut mt = MaskTransformSource::new(src, mask).unwrap();

    let data = drain(&mut mt);
    let floats: &[f32] = bytemuck::cast_slice(&data);

    for px in floats.chunks_exact(4) {
        assert!(
            (px[0] - 0.8).abs() < 0.01,
            "R should be ~0.8, got {}",
            px[0]
        );
        assert!(
            (px[1] - 0.4).abs() < 0.01,
            "G should be ~0.4, got {}",
            px[1]
        );
        assert!(
            (px[2] - 0.2).abs() < 0.01,
            "B should be ~0.2, got {}",
            px[2]
        );
        assert!(
            (px[3] - 1.0).abs() < 0.01,
            "A should be ~1.0, got {}",
            px[3]
        );
    }
}

#[test]
fn mask_transform_rejects_wrong_format() {
    use zenpipe::sources::MaskTransformSource;

    // RGBA8_SRGB is not RGBAF32_LINEAR_PREMUL — should error
    let src = solid_source(4, 4, [255, 0, 0, 255]);
    let mask = Box::new(OpaqueMask);
    let result = MaskTransformSource::new(src, mask);
    assert!(result.is_err());
}

// =============================================================================
// PipeError Display tests (error.rs)
// =============================================================================

#[test]
fn pipe_error_display_format_mismatch() {
    let err = PipeError::FormatMismatch {
        expected: format::RGBA8_SRGB,
        got: format::RGB8_SRGB,
    };
    let msg = format!("{err}");
    assert!(msg.contains("format mismatch"), "got: {msg}");
    assert!(msg.contains("expected"), "got: {msg}");
}

#[test]
fn pipe_error_display_resize() {
    let err = PipeError::Resize("test resize error".into());
    let msg = format!("{err}");
    assert!(msg.contains("resize"), "got: {msg}");
    assert!(msg.contains("test resize error"), "got: {msg}");
}

#[test]
fn pipe_error_display_dimension_mismatch() {
    let err = PipeError::DimensionMismatch("width differs".into());
    let msg = format!("{err}");
    assert!(msg.contains("dimension mismatch"), "got: {msg}");
    assert!(msg.contains("width differs"), "got: {msg}");
}

#[test]
fn pipe_error_display_limit_exceeded() {
    let err = PipeError::LimitExceeded("too many pixels".into());
    let msg = format!("{err}");
    assert!(msg.contains("limit exceeded"), "got: {msg}");
    assert!(msg.contains("too many pixels"), "got: {msg}");
}

#[test]
fn pipe_error_display_cancelled() {
    let err = PipeError::Cancelled;
    let msg = format!("{err}");
    assert_eq!(msg, "cancelled");
}

#[test]
fn pipe_error_display_op() {
    let err = PipeError::Op("custom operation failed".into());
    let msg = format!("{err}");
    assert!(msg.contains("operation"), "got: {msg}");
    assert!(msg.contains("custom operation failed"), "got: {msg}");
}

#[test]
fn pipe_error_is_std_error() {
    // Verify PipeError implements std::error::Error
    let err: Box<dyn std::error::Error> = Box::new(PipeError::Cancelled);
    assert_eq!(format!("{err}"), "cancelled");
}

#[test]
fn pipe_error_debug_impl() {
    // Verify Debug works (derived)
    let err = PipeError::Resize("x".into());
    let debug = format!("{err:?}");
    assert!(debug.contains("Resize"), "got: {debug}");
}

#[test]
fn pipe_error_from_stop_reason() {
    // Test From<StopReason> → PipeError::Cancelled
    let stop_reason = enough::StopReason::Cancelled;
    let err: PipeError = stop_reason.into();
    assert_eq!(format!("{err}"), "cancelled");
}

// =============================================================================
// Graph compilation: Orient (additional paths)
// =============================================================================

#[test]
fn graph_auto_orient_exif_6_rotate90() {
    // EXIF 6 = Rotate90 CW. 4x2 → 2x4
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let orient = g.add_node(NodeOp::AutoOrient(6));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, orient, EdgeKind::Input);
    g.add_edge(orient, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, gradient_source(4, 2));

    let pipeline = g.compile(sources).unwrap();
    // 4x2 rotated 90° CW → 2x4
    assert_eq!(pipeline.width(), 2);
    assert_eq!(pipeline.height(), 4);
}

#[test]
fn graph_auto_orient_exif_1_identity() {
    // EXIF 1 = Identity (no-op)
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let orient = g.add_node(NodeOp::AutoOrient(1));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, orient, EdgeKind::Input);
    g.add_edge(orient, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_source(4, 4, [42, 42, 42, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 4);
    assert_eq!(pipeline.height(), 4);

    let data = drain(pipeline.as_mut());
    for px in data.chunks_exact(4) {
        assert_eq!(px, [42, 42, 42, 255]);
    }
}

#[test]
fn graph_auto_orient_invalid_exif_is_identity() {
    // EXIF 0 and EXIF 99 should both be identity
    for exif_val in [0u8, 9, 99, 255] {
        let mut g = PipelineGraph::new();
        let src = g.add_node(NodeOp::Source);
        let orient = g.add_node(NodeOp::AutoOrient(exif_val));
        let out = g.add_node(NodeOp::Output);
        g.add_edge(src, orient, EdgeKind::Input);
        g.add_edge(orient, out, EdgeKind::Input);

        let mut sources = HashMap::new();
        sources.insert(src, solid_source(4, 4, [10, 20, 30, 255]));

        let mut pipeline = g.compile(sources).unwrap();
        assert_eq!(pipeline.width(), 4);
        assert_eq!(pipeline.height(), 4);

        let data = drain(pipeline.as_mut());
        for px in data.chunks_exact(4) {
            assert_eq!(px, [10, 20, 30, 255]);
        }
    }
}

// =============================================================================
// Graph compilation: Composite (with offset)
// =============================================================================

#[test]
fn graph_composite_with_offset() {
    let mut g = PipelineGraph::new();
    let bg_src = g.add_node(NodeOp::Source);
    let fg_src = g.add_node(NodeOp::Source);
    let comp = g.add_node(NodeOp::Composite {
        fg_x: 2,
        fg_y: 2,
        blend_mode: None,
    });
    let out = g.add_node(NodeOp::Output);

    g.add_edge(bg_src, comp, EdgeKind::Canvas);
    g.add_edge(fg_src, comp, EdgeKind::Input);
    g.add_edge(comp, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    // 8x8 black background, 4x4 red foreground at (2, 2)
    sources.insert(bg_src, solid_source(8, 8, [0, 0, 0, 255]));
    sources.insert(fg_src, solid_source(4, 4, [255, 0, 0, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.format(), format::RGBAF32_LINEAR_PREMUL);
    assert_eq!(pipeline.width(), 8);
    assert_eq!(pipeline.height(), 8);

    let data = drain(pipeline.as_mut());
    let floats: &[f32] = bytemuck::cast_slice(&data);

    // Pixel (0, 0): should be black (bg only)
    assert!(
        floats[0] < 0.01,
        "R at (0,0) should be ~0, got {}",
        floats[0]
    );

    // Pixel (2, 2): should be red (fg over bg)
    let offset = (2 * 8 + 2) * 4;
    assert!(
        floats[offset] > 0.9,
        "R at (2,2) should be ~1.0, got {}",
        floats[offset]
    );
    assert!(floats[offset + 1] < 0.01, "G at (2,2) should be ~0");
}

// =============================================================================
// Graph compilation: Overlay
// =============================================================================

#[test]
fn graph_overlay_watermark() {
    // 4x4 overlay on top of 8x8 background at position (2, 2)
    let overlay_w = 4u32;
    let overlay_h = 4u32;
    let overlay_fmt = format::RGBA8_SRGB;
    let mut overlay_data = vec![0u8; overlay_w as usize * overlay_h as usize * 4];
    // Fill with semi-transparent red
    for px in overlay_data.chunks_exact_mut(4) {
        px.copy_from_slice(&[255, 0, 0, 128]);
    }

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let overlay = g.add_node(NodeOp::Overlay {
        image_data: overlay_data,
        width: overlay_w,
        height: overlay_h,
        format: overlay_fmt,
        x: 2,
        y: 2,
        opacity: 1.0,
        blend_mode: None,
    });
    let out = g.add_node(NodeOp::Output);

    g.add_edge(src, overlay, EdgeKind::Input);
    g.add_edge(overlay, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_source(8, 8, [0, 0, 0, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 8);
    assert_eq!(pipeline.height(), 8);
    assert_eq!(pipeline.format(), format::RGBAF32_LINEAR_PREMUL);

    let data = drain(pipeline.as_mut());
    let floats: &[f32] = bytemuck::cast_slice(&data);

    // Pixel (0, 0): should be black
    assert!(floats[0] < 0.01, "R at (0,0) should be ~0");

    // Pixel (2, 2): should have some red from overlay blended over black
    let offset = (2 * 8 + 2) * 4;
    assert!(
        floats[offset] > 0.05,
        "R at (2,2) should be > 0, got {}",
        floats[offset]
    );
}

#[test]
fn graph_overlay_with_reduced_opacity() {
    let overlay_w = 2u32;
    let overlay_h = 2u32;
    let mut overlay_data = vec![0u8; overlay_w as usize * overlay_h as usize * 4];
    for px in overlay_data.chunks_exact_mut(4) {
        px.copy_from_slice(&[255, 255, 255, 255]); // opaque white
    }

    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let overlay = g.add_node(NodeOp::Overlay {
        image_data: overlay_data,
        width: overlay_w,
        height: overlay_h,
        format: format::RGBA8_SRGB,
        x: 0,
        y: 0,
        opacity: 0.5,
        blend_mode: None,
    });
    let out = g.add_node(NodeOp::Output);

    g.add_edge(src, overlay, EdgeKind::Input);
    g.add_edge(overlay, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_source(4, 4, [0, 0, 0, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    let data = drain(pipeline.as_mut());
    let floats: &[f32] = bytemuck::cast_slice(&data);

    // Pixel (0, 0): white at 50% over black → ~0.5 per channel
    assert!(
        floats[0] > 0.3 && floats[0] < 0.7,
        "R at (0,0) should be ~0.5, got {}",
        floats[0]
    );
}

// =============================================================================
// Graph compilation: RemoveAlpha
// =============================================================================

#[test]
fn graph_remove_alpha_semi_transparent() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let rm = g.add_node(NodeOp::RemoveAlpha { matte: [0, 0, 0] });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, rm, EdgeKind::Input);
    g.add_edge(rm, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    // Semi-transparent white on black matte
    sources.insert(src, solid_source(4, 4, [255, 255, 255, 128]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.format(), format::RGB8_SRGB);
    assert_eq!(pipeline.width(), 4);
    assert_eq!(pipeline.height(), 4);

    let data = drain(pipeline.as_mut());
    assert_eq!(data.len(), 4 * 4 * 3);
    // Should be blended: some value between 0 and 255
    for px in data.chunks_exact(3) {
        assert!(
            px[0] > 50 && px[0] < 200,
            "R should be mid-range, got {}",
            px[0]
        );
    }
}

// =============================================================================
// Graph compilation: AddAlpha
// =============================================================================

#[test]
fn graph_add_alpha_from_rgb() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let add = g.add_node(NodeOp::AddAlpha);
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, add, EdgeKind::Input);
    g.add_edge(add, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_rgb8(4, 4, [128, 64, 32]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.format(), format::RGBA8_SRGB);

    let data = drain(pipeline.as_mut());
    assert_eq!(data.len(), 4 * 4 * 4);
    for px in data.chunks_exact(4) {
        assert!((px[0] as i16 - 128).unsigned_abs() <= 1, "R: {}", px[0]);
        assert!((px[1] as i16 - 64).unsigned_abs() <= 1, "G: {}", px[1]);
        assert!((px[2] as i16 - 32).unsigned_abs() <= 1, "B: {}", px[2]);
        assert_eq!(px[3], 255, "A should be 255");
    }
}

#[test]
fn graph_add_alpha_already_rgba_noop() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let add = g.add_node(NodeOp::AddAlpha);
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, add, EdgeKind::Input);
    g.add_edge(add, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_source(4, 4, [128, 64, 32, 200]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.format(), format::RGBA8_SRGB);

    let data = drain(pipeline.as_mut());
    for px in data.chunks_exact(4) {
        assert_eq!(px, [128, 64, 32, 200]);
    }
}

// =============================================================================
// Graph compilation: Materialize
// =============================================================================

#[test]
fn graph_materialize_custom_transform() {
    // Materialize and invert all pixel values
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let mat = g.add_node(NodeOp::Materialize(Box::new(
        |data: &mut Vec<u8>, _w: &mut u32, _h: &mut u32, _fmt: &mut zenpipe::PixelFormat| {
            for byte in data.iter_mut() {
                *byte = 255 - *byte;
            }
        },
    )));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, mat, EdgeKind::Input);
    g.add_edge(mat, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_source(4, 4, [200, 100, 50, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    let data = drain(pipeline.as_mut());
    for px in data.chunks_exact(4) {
        assert_eq!(px, [55, 155, 205, 0]);
    }
}

#[test]
fn graph_materialize_resize_dimensions() {
    // Materialize and change dimensions
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let mat = g.add_node(NodeOp::Materialize(Box::new(
        |data: &mut Vec<u8>, w: &mut u32, h: &mut u32, _fmt: &mut zenpipe::PixelFormat| {
            // Crop to 2x2 (take top-left corner)
            let old_w = *w as usize;
            let bpp = 4;
            let mut new_data = Vec::new();
            for y in 0..2usize {
                let row_start = y * old_w * bpp;
                new_data.extend_from_slice(&data[row_start..row_start + 2 * bpp]);
            }
            *data = new_data;
            *w = 2;
            *h = 2;
        },
    )));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, mat, EdgeKind::Input);
    g.add_edge(mat, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, gradient_source(8, 8));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 2);
    assert_eq!(pipeline.height(), 2);

    let data = drain(pipeline.as_mut());
    assert_eq!(data.len(), 2 * 2 * 4);
    // Top-left pixel should be (0, 0)
    assert_eq!(data[0], 0);
    assert_eq!(data[1], 0);
}

// =============================================================================
// Graph compilation: CropWhitespace
// =============================================================================

#[test]
fn graph_crop_whitespace_removes_border() {
    // Create an image with a uniform white border around red center
    let width = 8u32;
    let height = 8u32;
    let bpp = 4usize;
    let mut data = vec![255u8; width as usize * height as usize * bpp];
    // Fill center 4x4 with red
    for y in 2..6usize {
        for x in 2..6usize {
            let i = (y * width as usize + x) * bpp;
            data[i] = 255; // R
            data[i + 1] = 0; // G
            data[i + 2] = 0; // B
            data[i + 3] = 255; // A
        }
    }

    let src = Box::new(CallbackSource::from_data(
        &data,
        width,
        height,
        format::RGBA8_SRGB,
        16,
    ));

    let mut g = PipelineGraph::new();
    let src_node = g.add_node(NodeOp::Source);
    let crop = g.add_node(NodeOp::CropWhitespace {
        threshold: 10,
        percent_padding: 0.0,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src_node, crop, EdgeKind::Input);
    g.add_edge(crop, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src_node, src as Box<dyn Source>);

    let mut pipeline = g.compile(sources).unwrap();
    // Should crop down to 4x4
    assert_eq!(pipeline.width(), 4, "width should be cropped");
    assert_eq!(pipeline.height(), 4, "height should be cropped");

    let out_data = drain(pipeline.as_mut());
    // All pixels should be red
    for px in out_data.chunks_exact(4) {
        assert_eq!(px[0], 255, "R");
        assert_eq!(px[1], 0, "G");
        assert_eq!(px[2], 0, "B");
    }
}

#[test]
fn graph_crop_whitespace_uniform_image_noop() {
    // Fully uniform white image — should be a no-op (return full image)
    let mut g = PipelineGraph::new();
    let src_node = g.add_node(NodeOp::Source);
    let crop = g.add_node(NodeOp::CropWhitespace {
        threshold: 10,
        percent_padding: 0.0,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src_node, crop, EdgeKind::Input);
    g.add_edge(crop, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src_node, solid_source(8, 8, [255, 255, 255, 255]));

    let pipeline = g.compile(sources).unwrap();
    // Uniform image: no content found, should return full dimensions
    assert_eq!(pipeline.width(), 8);
    assert_eq!(pipeline.height(), 8);
}

// =============================================================================
// Graph compilation: Analyze
// =============================================================================

#[test]
fn graph_analyze_custom_analysis() {
    // Analyze: materialize, inspect, return as-is
    let mut g = PipelineGraph::new();
    let src_node = g.add_node(NodeOp::Source);
    let analyze = g.add_node(NodeOp::Analyze(Box::new(|mat: MaterializedSource| {
        // Just verify we got the right dimensions and pass through
        assert_eq!(mat.width(), 4);
        assert_eq!(mat.height(), 4);
        Ok(Box::new(mat) as Box<dyn Source>)
    })));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src_node, analyze, EdgeKind::Input);
    g.add_edge(analyze, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src_node, solid_source(4, 4, [100, 200, 50, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 4);
    assert_eq!(pipeline.height(), 4);

    let data = drain(pipeline.as_mut());
    for px in data.chunks_exact(4) {
        assert_eq!(px, [100, 200, 50, 255]);
    }
}

#[test]
fn graph_analyze_modifies_pipeline() {
    // Analyze: materialize, then crop via returned source
    let mut g = PipelineGraph::new();
    let src_node = g.add_node(NodeOp::Source);
    let analyze = g.add_node(NodeOp::Analyze(Box::new(|mat: MaterializedSource| {
        // Crop to 2x2 from the materialized source
        use zenpipe::sources::CropSource;
        Ok(Box::new(CropSource::new(Box::new(mat), 1, 1, 2, 2)?) as Box<dyn Source>)
    })));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src_node, analyze, EdgeKind::Input);
    g.add_edge(analyze, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src_node, gradient_source(8, 8));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 2);
    assert_eq!(pipeline.height(), 2);

    let data = drain(pipeline.as_mut());
    // Pixel (0,0) should be original (1,1)
    assert_eq!(data[0], 1);
    assert_eq!(data[1], 1);
}

// =============================================================================
// Graph: estimate() tests
// =============================================================================

#[test]
fn estimate_orient_swaps_axes() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let orient = g.add_node(NodeOp::Orient(zenresize::Orientation::Rotate90));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, orient, EdgeKind::Input);
    g.add_edge(orient, out, EdgeKind::Input);

    let mut info = HashMap::new();
    info.insert(
        src,
        SourceInfo {
            width: 100,
            height: 50,
            format: format::RGBA8_SRGB,
        },
    );

    let est = g.estimate(&info).unwrap();
    assert_eq!(est.output_width, 50);
    assert_eq!(est.output_height, 100);
    assert!(est.materializes, "orient should require materialization");
    assert!(est.materialization_bytes > 0);
}

#[test]
fn estimate_auto_orient_identity_no_materialization() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let orient = g.add_node(NodeOp::AutoOrient(1)); // identity
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, orient, EdgeKind::Input);
    g.add_edge(orient, out, EdgeKind::Input);

    let mut info = HashMap::new();
    info.insert(
        src,
        SourceInfo {
            width: 100,
            height: 50,
            format: format::RGBA8_SRGB,
        },
    );

    let est = g.estimate(&info).unwrap();
    assert_eq!(est.output_width, 100);
    assert_eq!(est.output_height, 50);
    assert!(!est.materializes, "identity orient should not materialize");
}

#[test]
fn estimate_composite() {
    let mut g = PipelineGraph::new();
    let bg = g.add_node(NodeOp::Source);
    let fg = g.add_node(NodeOp::Source);
    let comp = g.add_node(NodeOp::Composite {
        fg_x: 0,
        fg_y: 0,
        blend_mode: None,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(bg, comp, EdgeKind::Canvas);
    g.add_edge(fg, comp, EdgeKind::Input);
    g.add_edge(comp, out, EdgeKind::Input);

    let mut info = HashMap::new();
    info.insert(
        bg,
        SourceInfo {
            width: 100,
            height: 100,
            format: format::RGBA8_SRGB,
        },
    );
    info.insert(
        fg,
        SourceInfo {
            width: 50,
            height: 50,
            format: format::RGBA8_SRGB,
        },
    );

    let est = g.estimate(&info).unwrap();
    // Output should be bg dimensions
    assert_eq!(est.output_width, 100);
    assert_eq!(est.output_height, 100);
    assert_eq!(est.output_format, format::RGBAF32_LINEAR_PREMUL);
    assert!(est.streaming_bytes > 0);
}

#[test]
fn estimate_overlay() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let overlay = g.add_node(NodeOp::Overlay {
        image_data: vec![0u8; 16],
        width: 2,
        height: 2,
        format: format::RGBA8_SRGB,
        x: 0,
        y: 0,
        opacity: 1.0,
        blend_mode: None,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, overlay, EdgeKind::Input);
    g.add_edge(overlay, out, EdgeKind::Input);

    let mut info = HashMap::new();
    info.insert(
        src,
        SourceInfo {
            width: 100,
            height: 100,
            format: format::RGBA8_SRGB,
        },
    );

    let est = g.estimate(&info).unwrap();
    assert_eq!(est.output_width, 100);
    assert_eq!(est.output_height, 100);
    assert_eq!(est.output_format, format::RGBAF32_LINEAR_PREMUL);
}

#[test]
fn estimate_remove_alpha() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let rm = g.add_node(NodeOp::RemoveAlpha {
        matte: [255, 255, 255],
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, rm, EdgeKind::Input);
    g.add_edge(rm, out, EdgeKind::Input);

    let mut info = HashMap::new();
    info.insert(
        src,
        SourceInfo {
            width: 100,
            height: 100,
            format: format::RGBA8_SRGB,
        },
    );

    let est = g.estimate(&info).unwrap();
    assert_eq!(est.output_format, format::RGB8_SRGB);
    assert_eq!(est.output_width, 100);
}

#[test]
fn estimate_add_alpha() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let add = g.add_node(NodeOp::AddAlpha);
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, add, EdgeKind::Input);
    g.add_edge(add, out, EdgeKind::Input);

    let mut info = HashMap::new();
    info.insert(
        src,
        SourceInfo {
            width: 100,
            height: 100,
            format: format::RGB8_SRGB,
        },
    );

    let est = g.estimate(&info).unwrap();
    assert_eq!(est.output_format, format::RGBA8_SRGB);
}

#[test]
fn estimate_materialize() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let mat = g.add_node(NodeOp::Materialize(Box::new(
        |_data: &mut Vec<u8>, _w: &mut u32, _h: &mut u32, _fmt: &mut zenpipe::PixelFormat| {},
    )));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, mat, EdgeKind::Input);
    g.add_edge(mat, out, EdgeKind::Input);

    let mut info = HashMap::new();
    info.insert(
        src,
        SourceInfo {
            width: 100,
            height: 100,
            format: format::RGBA8_SRGB,
        },
    );

    let est = g.estimate(&info).unwrap();
    assert!(est.materializes);
    assert!(est.materialization_bytes >= 100 * 100 * 4);
}

#[test]
fn estimate_crop_whitespace() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let crop = g.add_node(NodeOp::CropWhitespace {
        threshold: 10,
        percent_padding: 0.0,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, crop, EdgeKind::Input);
    g.add_edge(crop, out, EdgeKind::Input);

    let mut info = HashMap::new();
    info.insert(
        src,
        SourceInfo {
            width: 100,
            height: 100,
            format: format::RGBA8_SRGB,
        },
    );

    let est = g.estimate(&info).unwrap();
    // Worst case: no crop, pass through full dimensions
    assert_eq!(est.output_width, 100);
    assert_eq!(est.output_height, 100);
    assert!(est.materializes);
}

#[test]
fn estimate_analyze() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let analyze = g.add_node(NodeOp::Analyze(Box::new(|mat: MaterializedSource| {
        Ok(Box::new(mat) as Box<dyn Source>)
    })));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, analyze, EdgeKind::Input);
    g.add_edge(analyze, out, EdgeKind::Input);

    let mut info = HashMap::new();
    info.insert(
        src,
        SourceInfo {
            width: 80,
            height: 60,
            format: format::RGBA8_SRGB,
        },
    );

    let est = g.estimate(&info).unwrap();
    assert!(est.materializes);
    assert_eq!(est.output_width, 80);
    assert_eq!(est.output_height, 60);
}

// =============================================================================
// Graph validation edge cases
// =============================================================================

#[test]
fn graph_validate_cycle_detected() {
    let mut g = PipelineGraph::new();
    let a = g.add_node(NodeOp::Source);
    let b = g.add_node(NodeOp::Crop {
        x: 0,
        y: 0,
        w: 4,
        h: 4,
    });
    let c = g.add_node(NodeOp::Output);
    // Create cycle: a→b, b→a
    g.add_edge(a, b, EdgeKind::Input);
    g.add_edge(b, a, EdgeKind::Input);
    g.add_edge(b, c, EdgeKind::Input);

    let result = g.validate();
    assert!(result.is_err());
    let msg = format!("{}", result.unwrap_err());
    assert!(msg.contains("cycle"), "expected cycle error, got: {msg}");
}

#[test]
fn graph_validate_self_loop_rejected() {
    let mut g = PipelineGraph::new();
    let a = g.add_node(NodeOp::Source);
    let out = g.add_node(NodeOp::Output);
    g.add_edge(a, a, EdgeKind::Input); // self-loop
    g.add_edge(a, out, EdgeKind::Input);

    let result = g.validate();
    assert!(result.is_err());
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("self-loop"),
        "expected self-loop error, got: {msg}"
    );
}

#[test]
fn graph_validate_multiple_outputs_rejected() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let _out1 = g.add_node(NodeOp::Output);
    let _out2 = g.add_node(NodeOp::Output);
    g.add_edge(src, _out1, EdgeKind::Input);
    g.add_edge(src, _out2, EdgeKind::Input);

    let result = g.validate();
    assert!(result.is_err());
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("Output nodes"),
        "expected multiple outputs error, got: {msg}"
    );
}

#[test]
fn graph_validate_empty_graph_rejected() {
    let g = PipelineGraph::new();
    let result = g.validate();
    assert!(result.is_err());
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("no nodes"),
        "expected no nodes error, got: {msg}"
    );
}

#[test]
fn graph_validate_out_of_range_edge_rejected() {
    let mut g = PipelineGraph::new();
    let _src = g.add_node(NodeOp::Source);
    let _out = g.add_node(NodeOp::Output);
    // Edge pointing to non-existent node index 99
    g.add_edge(0, 99, EdgeKind::Input);

    let result = g.validate();
    assert!(result.is_err());
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("out of range"),
        "expected out of range error, got: {msg}"
    );
}

// =============================================================================
// ResourceEstimate methods
// =============================================================================

#[test]
fn resource_estimate_peak_memory() {
    let est = zenpipe::ResourceEstimate {
        streaming_bytes: 1000,
        materialization_bytes: 2000,
        materializes: true,
        output_width: 10,
        output_height: 10,
        output_format: format::RGBA8_SRGB,
    };
    assert_eq!(est.peak_memory_bytes(), 3000);
}

#[test]
fn resource_estimate_check_within_limits() {
    let est = zenpipe::ResourceEstimate {
        streaming_bytes: 1000,
        materialization_bytes: 2000,
        materializes: true,
        output_width: 10,
        output_height: 10,
        output_format: format::RGBA8_SRGB,
    };
    let limits = zenpipe::Limits::default();
    assert!(est.check(&limits).is_ok());
}
