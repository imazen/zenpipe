use hashbrown::HashMap;

use zenpipe::graph::{EdgeKind, NodeOp, PipelineGraph};
use zenpipe::ops::{SrgbToLinearPremul, UnpremulLinearToSrgb};
use zenpipe::sources::CallbackSource;
use zenpipe::{PixelFormat, Source};

/// Collect all strips from a source into a flat Vec<u8>.
fn drain(source: &mut dyn Source) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok(Some(strip)) = source.next() {
        out.extend_from_slice(strip.data);
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
        PixelFormat::Rgba8,
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

/// Create a gradient source where pixel[0] = x coord, pixel[1] = y coord.
fn gradient_source(width: u32, height: u32) -> Box<dyn Source> {
    let mut row_idx = 0u32;
    Box::new(CallbackSource::new(
        width,
        height,
        PixelFormat::Rgba8,
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

#[test]
fn passthrough_graph() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_source(4, 4, [255, 0, 0, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 4);
    assert_eq!(pipeline.height(), 4);

    let data = drain(pipeline.as_mut());
    assert_eq!(data.len(), 4 * 4 * 4);
    for px in data.chunks_exact(4) {
        assert_eq!(px, [255, 0, 0, 255]);
    }
}

#[test]
fn crop_graph() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let crop = g.add_node(NodeOp::Crop {
        x: 1,
        y: 1,
        w: 2,
        h: 2,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, crop, EdgeKind::Input);
    g.add_edge(crop, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, gradient_source(4, 4));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 2);
    assert_eq!(pipeline.height(), 2);

    let data = drain(pipeline.as_mut());
    // Verify cropped region: x=[1,2], y=[1,2]
    assert_eq!(data.len(), 2 * 2 * 4);
    assert_eq!(data[0], 1); // x=1
    assert_eq!(data[1], 1); // y=1
    assert_eq!(data[4], 2); // x=2
    assert_eq!(data[5], 1); // y=1
    assert_eq!(data[8], 1); // x=1, row 2
    assert_eq!(data[9], 2); // y=2
}

#[test]
fn pixel_op_fusion() {
    // sRGB → linear premul → unpremul → sRGB roundtrip via graph
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let to_linear = g.add_node(NodeOp::PixelTransform(Box::new(SrgbToLinearPremul)));
    let to_srgb = g.add_node(NodeOp::PixelTransform(Box::new(UnpremulLinearToSrgb)));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, to_linear, EdgeKind::Input);
    g.add_edge(to_linear, to_srgb, EdgeKind::Input);
    g.add_edge(to_srgb, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_source(4, 2, [200, 100, 50, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.format(), PixelFormat::Rgba8);

    let data = drain(pipeline.as_mut());
    for px in data.chunks_exact(4) {
        assert!((px[0] as i16 - 200).unsigned_abs() <= 1);
        assert!((px[1] as i16 - 100).unsigned_abs() <= 1);
        assert!((px[2] as i16 - 50).unsigned_abs() <= 1);
        assert_eq!(px[3], 255);
    }
}

#[test]
fn flip_h_graph() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let flip = g.add_node(NodeOp::FlipH);
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, flip, EdgeKind::Input);
    g.add_edge(flip, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, gradient_source(4, 2));

    let mut pipeline = g.compile(sources).unwrap();
    let data = drain(pipeline.as_mut());

    // Row 0: x should be reversed [3,2,1,0]
    assert_eq!(data[0], 3);
    assert_eq!(data[4], 2);
    assert_eq!(data[8], 1);
    assert_eq!(data[12], 0);
    // y should still be 0
    assert_eq!(data[1], 0);
}

#[test]
fn flip_v_graph() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let flip = g.add_node(NodeOp::FlipV);
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, flip, EdgeKind::Input);
    g.add_edge(flip, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, gradient_source(4, 4));

    let mut pipeline = g.compile(sources).unwrap();
    let data = drain(pipeline.as_mut());

    // First row should now be the last row (y=3)
    assert_eq!(data[1], 3); // y coord
    // Last row should be y=0
    let last_row = 4 * 4 * 3; // row 3, pixel 0
    assert_eq!(data[last_row + 1], 0); // y coord
}

#[test]
fn rotate_90_graph() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let rot = g.add_node(NodeOp::Rotate90);
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, rot, EdgeKind::Input);
    g.add_edge(rot, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, gradient_source(4, 2));

    let pipeline = g.compile(sources).unwrap();
    // 4×2 rotated 90° CW → 2×4
    assert_eq!(pipeline.width(), 2);
    assert_eq!(pipeline.height(), 4);
}

#[test]
fn transpose_graph() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let t = g.add_node(NodeOp::Transpose);
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, t, EdgeKind::Input);
    g.add_edge(t, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, gradient_source(8, 2));

    let mut pipeline = g.compile(sources).unwrap();
    // 8×2 transposed → 2×8
    assert_eq!(pipeline.width(), 2);
    assert_eq!(pipeline.height(), 8);

    let data = drain(pipeline.as_mut());
    // Original (0,0) → transposed (0,0): x=0, y=0
    assert_eq!(data[0], 0);
    assert_eq!(data[1], 0);
    // Original (1,0) → transposed (0,1): x=1, y=0
    let stride = 2 * 4; // new width=2
    assert_eq!(data[stride], 1);
    assert_eq!(data[stride + 1], 0);
}

#[test]
fn expand_canvas_graph() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let exp = g.add_node(NodeOp::ExpandCanvas {
        left: 2,
        top: 1,
        right: 2,
        bottom: 1,
        color: [0, 0, 255, 255], // blue border
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, exp, EdgeKind::Input);
    g.add_edge(exp, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_source(4, 4, [255, 0, 0, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    // 4×4 + 2+2 left/right + 1+1 top/bottom → 8×6
    assert_eq!(pipeline.width(), 8);
    assert_eq!(pipeline.height(), 6);

    let data = drain(pipeline.as_mut());
    // Top-left corner should be blue
    assert_eq!(&data[0..4], [0, 0, 255, 255]);
    // Center should be red (offset: row 1, col 2)
    let stride = 8 * 4;
    let center = stride + 2 * 4; // row 1, col 2
    assert_eq!(&data[center..center + 4], [255, 0, 0, 255]);
}

#[test]
fn composite_graph() {
    // Composite a small foreground over a larger background
    let mut g = PipelineGraph::new();
    let bg_src = g.add_node(NodeOp::Source);
    let fg_src = g.add_node(NodeOp::Source);
    let comp = g.add_node(NodeOp::Composite { fg_x: 0, fg_y: 0 });
    let out = g.add_node(NodeOp::Output);

    g.add_edge(bg_src, comp, EdgeKind::Canvas);
    g.add_edge(fg_src, comp, EdgeKind::Input);
    g.add_edge(comp, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    // Background: 4×4 black
    sources.insert(bg_src, solid_source(4, 4, [0, 0, 0, 255]));
    // Foreground: 4×4 opaque red (same size for simplicity)
    sources.insert(fg_src, solid_source(4, 4, [255, 0, 0, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    // Output should be Rgbaf32LinearPremul (composite format)
    assert_eq!(pipeline.format(), PixelFormat::Rgbaf32LinearPremul);
    assert_eq!(pipeline.width(), 4);
    assert_eq!(pipeline.height(), 4);

    let data = drain(pipeline.as_mut());
    // Opaque red over opaque black = opaque red in linear premul
    // Check that data is non-empty and has reasonable values
    assert_eq!(data.len(), 4 * 4 * 16); // 16 bytes per pixel for f32
    let f32_data: &[f32] = bytemuck::cast_slice(&data);
    // First pixel: should be close to linear red (1.0, 0, 0, 1.0) in premul
    assert!(f32_data[0] > 0.9, "R should be ~1.0, got {}", f32_data[0]);
    assert!(f32_data[1] < 0.01, "G should be ~0, got {}", f32_data[1]);
    assert!(f32_data[2] < 0.01, "B should be ~0, got {}", f32_data[2]);
    assert!(f32_data[3] > 0.99, "A should be ~1.0, got {}", f32_data[3]);
}

#[test]
fn crop_then_flip_chain() {
    // Multi-step: crop → flipH
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let crop = g.add_node(NodeOp::Crop {
        x: 0,
        y: 0,
        w: 4,
        h: 2,
    });
    let flip = g.add_node(NodeOp::FlipH);
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, crop, EdgeKind::Input);
    g.add_edge(crop, flip, EdgeKind::Input);
    g.add_edge(flip, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, gradient_source(8, 4));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 4);
    assert_eq!(pipeline.height(), 2);

    let data = drain(pipeline.as_mut());
    // Cropped to [0..4, 0..2], then flipped H
    // Row 0: originally x=[0,1,2,3], after flip: [3,2,1,0]
    assert_eq!(data[0], 3);
    assert_eq!(data[4], 2);
    assert_eq!(data[8], 1);
    assert_eq!(data[12], 0);
}

#[test]
fn resize_graph() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let resize = g.add_node(NodeOp::Resize { w: 2, h: 2 });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, resize, EdgeKind::Input);
    g.add_edge(resize, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_source(8, 8, [128, 64, 32, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 2);
    assert_eq!(pipeline.height(), 2);
    assert_eq!(pipeline.format(), PixelFormat::Rgba8);

    let data = drain(pipeline.as_mut());
    assert_eq!(data.len(), 2 * 2 * 4);
    // Solid color downscaled should remain approximately the same
    for px in data.chunks_exact(4) {
        assert!((px[0] as i16 - 128).unsigned_abs() <= 2, "R: {}", px[0]);
        assert!((px[1] as i16 - 64).unsigned_abs() <= 2, "G: {}", px[1]);
        assert!((px[2] as i16 - 32).unsigned_abs() <= 2, "B: {}", px[2]);
    }
}

#[test]
fn materialize_custom_graph() {
    // Use custom Materialize to invert all pixel values
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let mat = g.add_node(NodeOp::Materialize(Box::new(
        |data: &mut Vec<u8>, _w: &mut u32, _h: &mut u32, _fmt: &mut PixelFormat| {
            for byte in data.iter_mut() {
                *byte = 255 - *byte;
            }
        },
    )));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, mat, EdgeKind::Input);
    g.add_edge(mat, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_source(2, 2, [100, 150, 200, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    let data = drain(pipeline.as_mut());
    for px in data.chunks_exact(4) {
        assert_eq!(px, [155, 105, 55, 0]);
    }
}

#[test]
fn rotate_180_graph() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let rot = g.add_node(NodeOp::Rotate180);
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, rot, EdgeKind::Input);
    g.add_edge(rot, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, gradient_source(4, 4));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 4);
    assert_eq!(pipeline.height(), 4);

    let data = drain(pipeline.as_mut());
    // (0,0) should now be (3,3)
    assert_eq!(data[0], 3); // x
    assert_eq!(data[1], 3); // y
    // Last pixel (3,3) → (0,0)
    let last = (4 * 4 - 1) * 4;
    assert_eq!(data[last], 0); // x
    assert_eq!(data[last + 1], 0); // y
}

#[test]
fn no_output_node_error() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    // No output node

    let mut sources = HashMap::new();
    sources.insert(src, solid_source(4, 4, [0, 0, 0, 255]));

    let result = g.compile(sources);
    assert!(result.is_err());
}

#[test]
fn missing_source_error() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, out, EdgeKind::Input);

    // Don't provide any source
    let sources = HashMap::new();
    let result = g.compile(sources);
    assert!(result.is_err());
}

#[test]
fn auto_format_conversion_for_composite() {
    // Verify that Rgba8 sources are automatically converted to
    // Rgbaf32LinearPremul for compositing
    let mut g = PipelineGraph::new();
    let bg = g.add_node(NodeOp::Source);
    let fg = g.add_node(NodeOp::Source);
    let comp = g.add_node(NodeOp::Composite { fg_x: 0, fg_y: 0 });
    let out = g.add_node(NodeOp::Output);

    g.add_edge(bg, comp, EdgeKind::Canvas);
    g.add_edge(fg, comp, EdgeKind::Input);
    g.add_edge(comp, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(bg, solid_source(2, 2, [0, 0, 0, 255]));
    sources.insert(fg, solid_source(2, 2, [0, 0, 0, 0])); // fully transparent

    // Should compile without errors — format conversion is automatic
    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.format(), PixelFormat::Rgbaf32LinearPremul);

    let data = drain(pipeline.as_mut());
    let f32_data: &[f32] = bytemuck::cast_slice(&data);
    // Transparent fg over black bg = black bg
    // In premul linear, black opaque = (0, 0, 0, 1)
    for px in f32_data.chunks_exact(4) {
        assert!(px[0].abs() < 0.01);
        assert!(px[1].abs() < 0.01);
        assert!(px[2].abs() < 0.01);
        assert!((px[3] - 1.0).abs() < 0.01);
    }
}
