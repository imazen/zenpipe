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
    assert_eq!(data.len(), 2 * 2 * 4);
    assert_eq!(data[0], 1); // x=1
    assert_eq!(data[1], 1); // y=1
    assert_eq!(data[4], 2); // x=2
    assert_eq!(data[5], 1); // y=1
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
fn orient_flip_h() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let orient = g.add_node(NodeOp::Orient(zenresize::Orientation::FlipH));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, orient, EdgeKind::Input);
    g.add_edge(orient, out, EdgeKind::Input);

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
fn orient_flip_v() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let orient = g.add_node(NodeOp::Orient(zenresize::Orientation::FlipV));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, orient, EdgeKind::Input);
    g.add_edge(orient, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, gradient_source(4, 4));

    let mut pipeline = g.compile(sources).unwrap();
    let data = drain(pipeline.as_mut());

    // First row should now be the last row (y=3)
    assert_eq!(data[1], 3);
    let last_row = 4 * 4 * 3;
    assert_eq!(data[last_row + 1], 0);
}

#[test]
fn orient_rotate_90() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let orient = g.add_node(NodeOp::Orient(zenresize::Orientation::Rotate90));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, orient, EdgeKind::Input);
    g.add_edge(orient, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, gradient_source(4, 2));

    let pipeline = g.compile(sources).unwrap();
    // 4×2 rotated 90° CW → 2×4
    assert_eq!(pipeline.width(), 2);
    assert_eq!(pipeline.height(), 4);
}

#[test]
fn orient_rotate_180() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let orient = g.add_node(NodeOp::Orient(zenresize::Orientation::Rotate180));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, orient, EdgeKind::Input);
    g.add_edge(orient, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, gradient_source(4, 4));

    let mut pipeline = g.compile(sources).unwrap();
    let data = drain(pipeline.as_mut());

    // (0,0) should now be (3,3)
    assert_eq!(data[0], 3);
    assert_eq!(data[1], 3);
    let last = (4 * 4 - 1) * 4;
    assert_eq!(data[last], 0);
    assert_eq!(data[last + 1], 0);
}

#[test]
fn orient_transpose() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let orient = g.add_node(NodeOp::Orient(zenresize::Orientation::Transpose));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, orient, EdgeKind::Input);
    g.add_edge(orient, out, EdgeKind::Input);

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
    let stride = 2 * 4;
    assert_eq!(data[stride], 1);
    assert_eq!(data[stride + 1], 0);
}

#[test]
fn orient_identity_passthrough() {
    // Identity orientation should be a no-op (no materialization)
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let orient = g.add_node(NodeOp::Orient(zenresize::Orientation::Identity));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, orient, EdgeKind::Input);
    g.add_edge(orient, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_source(4, 4, [42, 42, 42, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    let data = drain(pipeline.as_mut());
    for px in data.chunks_exact(4) {
        assert_eq!(px, [42, 42, 42, 255]);
    }
}

#[test]
fn layout_resize_via_zenresize() {
    // Use Layout node to resize via zenresize's full pipeline
    use zenresize::{DecoderOffer, DecoderRequest, Orientation, Size};

    let in_w = 8u32;
    let in_h = 8u32;
    let out_w = 4u32;
    let out_h = 4u32;

    // Build a LayoutPlan for simple resize (no crop, no orient, no canvas padding)
    let request = DecoderRequest::new(Size::new(out_w, out_h), Orientation::Identity);
    let offer = DecoderOffer::full_decode(in_w, in_h);
    let ideal = zenresize::Pipeline::new(in_w, in_h)
        .constrain(zenresize::Constraint::new(
            zenresize::ConstraintMode::Distort,
            out_w,
            out_h,
        ))
        .plan()
        .unwrap();
    let (ideal, _req) = ideal;
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
    sources.insert(src, solid_source(in_w, in_h, [128, 64, 32, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), out_w);
    assert_eq!(pipeline.height(), out_h);

    let data = drain(pipeline.as_mut());
    assert_eq!(data.len(), out_w as usize * out_h as usize * 4);
    for px in data.chunks_exact(4) {
        assert!((px[0] as i16 - 128).unsigned_abs() <= 2, "R: {}", px[0]);
        assert!((px[1] as i16 - 64).unsigned_abs() <= 2, "G: {}", px[1]);
        assert!((px[2] as i16 - 32).unsigned_abs() <= 2, "B: {}", px[2]);
    }
}

#[test]
fn streaming_resize_graph() {
    // Direct streaming resize via ResizeSource (no layout plan)
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
    for px in data.chunks_exact(4) {
        assert!((px[0] as i16 - 128).unsigned_abs() <= 2, "R: {}", px[0]);
        assert!((px[1] as i16 - 64).unsigned_abs() <= 2, "G: {}", px[1]);
        assert!((px[2] as i16 - 32).unsigned_abs() <= 2, "B: {}", px[2]);
    }
}

#[test]
fn streaming_composite_graph() {
    let mut g = PipelineGraph::new();
    let bg_src = g.add_node(NodeOp::Source);
    let fg_src = g.add_node(NodeOp::Source);
    let comp = g.add_node(NodeOp::Composite { fg_x: 0, fg_y: 0 });
    let out = g.add_node(NodeOp::Output);

    g.add_edge(bg_src, comp, EdgeKind::Canvas);
    g.add_edge(fg_src, comp, EdgeKind::Input);
    g.add_edge(comp, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(bg_src, solid_source(4, 4, [0, 0, 0, 255]));
    sources.insert(fg_src, solid_source(4, 4, [255, 0, 0, 255]));

    let mut pipeline = g.compile(sources).unwrap();
    // CompositeSource outputs Rgbaf32LinearPremul
    assert_eq!(pipeline.format(), PixelFormat::Rgbaf32LinearPremul);
    assert_eq!(pipeline.width(), 4);
    assert_eq!(pipeline.height(), 4);

    let data = drain(pipeline.as_mut());
    let f32_data: &[f32] = bytemuck::cast_slice(&data);
    // Opaque red over opaque black = red
    assert!(f32_data[0] > 0.9, "R should be ~1.0, got {}", f32_data[0]);
    assert!(f32_data[1] < 0.01);
    assert!(f32_data[2] < 0.01);
    assert!(f32_data[3] > 0.99);
}

#[test]
fn crop_then_orient_chain() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let crop = g.add_node(NodeOp::Crop {
        x: 0,
        y: 0,
        w: 4,
        h: 2,
    });
    let flip = g.add_node(NodeOp::Orient(zenresize::Orientation::FlipH));
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
    // Cropped to [0..4, 0..2], then flipped H: [3,2,1,0]
    assert_eq!(data[0], 3);
    assert_eq!(data[4], 2);
    assert_eq!(data[8], 1);
    assert_eq!(data[12], 0);
}

#[test]
fn materialize_custom_graph() {
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
fn no_output_node_error() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);

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

    let sources = HashMap::new();
    let result = g.compile(sources);
    assert!(result.is_err());
}

#[test]
fn auto_format_conversion_for_composite() {
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

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.format(), PixelFormat::Rgbaf32LinearPremul);

    let data = drain(pipeline.as_mut());
    let f32_data: &[f32] = bytemuck::cast_slice(&data);
    // Transparent fg over black bg = black bg (0, 0, 0, 1) in premul linear
    for px in f32_data.chunks_exact(4) {
        assert!(px[0].abs() < 0.01);
        assert!(px[1].abs() < 0.01);
        assert!(px[2].abs() < 0.01);
        assert!((px[3] - 1.0).abs() < 0.01);
    }
}

// ==========================================================================
// ensure_format direct path tests
// ==========================================================================

#[test]
fn auto_format_rgba8_to_linear_direct() {
    // Verify Rgba8 → Rgbaf32Linear uses the direct path (not multi-hop via premul)
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    // Composite requires Rgbaf32LinearPremul, but we test direct linear via
    // a PixelTransform that expects Rgbaf32Linear input
    let to_linear = g.add_node(NodeOp::PixelTransform(Box::new(zenpipe::ops::SrgbToLinear)));
    let back = g.add_node(NodeOp::PixelTransform(Box::new(zenpipe::ops::LinearToSrgb)));
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, to_linear, EdgeKind::Input);
    g.add_edge(to_linear, back, EdgeKind::Input);
    g.add_edge(back, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_source(4, 2, [200, 100, 50, 180]));

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.format(), PixelFormat::Rgba8);

    let data = drain(pipeline.as_mut());
    for px in data.chunks_exact(4) {
        assert!((px[0] as i16 - 200).unsigned_abs() <= 1, "R: {}", px[0]);
        assert!((px[1] as i16 - 100).unsigned_abs() <= 1, "G: {}", px[1]);
        assert!((px[2] as i16 - 50).unsigned_abs() <= 1, "B: {}", px[2]);
        assert_eq!(px[3], 180); // alpha preserved exactly
    }
}
