#![cfg(feature = "imageflow-compat")]
//! Parity tests: verify that imageflow_types v2 Node variants translate
//! correctly to zennode instances via `translate_nodes()`.
//!
//! Each test constructs a concrete Node value, calls translate_nodes(), and
//! asserts on the result — checking schema IDs, param values, and
//! config fields (preset, decode_io_id, encode_io_id, create_canvas).

use std::collections::HashMap;

use imageflow_types::{
    Color, ColorFilterSrgb, ColorSrgb, CommandStringKind, CompositingMode, Constraint,
    ConstraintGravity, ConstraintMode, EncoderPreset, Node, PixelFormat, RoundCornersMode,
};
use zennode::{NodeInstance, ParamValue};
use zenpipe::imageflow_compat::translate;

/// Helper: translate a single node with empty io_buffers.
fn translate_one(node: Node) -> Result<translate::TranslatedPipeline, translate::TranslateError> {
    let io_buffers: HashMap<i32, Vec<u8>> = HashMap::new();
    let nodes = vec![node];
    translate::translate_nodes(&nodes, &io_buffers)
}

/// Helper: translate a single node, assert Ok, return the pipeline.
fn translate_ok(node: Node) -> translate::TranslatedPipeline {
    translate_one(node).expect("translation should succeed")
}

/// Helper: get schema ID from the first node in the pipeline.
fn first_schema_id(pipeline: &translate::TranslatedPipeline) -> &str {
    assert!(!pipeline.nodes.is_empty(), "expected at least one node");
    pipeline.nodes[0].schema().id
}

fn assert_param_u32(node: &Box<dyn NodeInstance>, name: &str, expected: u32) {
    let val = node.get_param(name).unwrap_or_else(|| panic!("param '{name}' not found"));
    let actual = val.as_u32().unwrap_or_else(|| panic!("param '{name}' is not u32: {val:?}"));
    assert_eq!(actual, expected, "param '{name}': expected {expected}, got {actual}");
}

fn assert_param_i32(node: &Box<dyn NodeInstance>, name: &str, expected: i32) {
    let val = node.get_param(name).unwrap_or_else(|| panic!("param '{name}' not found"));
    let actual = val.as_i32().unwrap_or_else(|| panic!("param '{name}' is not i32: {val:?}"));
    assert_eq!(actual, expected, "param '{name}': expected {expected}, got {actual}");
}

fn assert_param_f32_approx(node: &Box<dyn NodeInstance>, name: &str, expected: f32) {
    let val = node.get_param(name).unwrap_or_else(|| panic!("param '{name}' not found"));
    let actual = val.as_f32().unwrap_or_else(|| panic!("param '{name}' is not f32: {val:?}"));
    assert!(
        (actual - expected).abs() < 0.001,
        "param '{name}': expected {expected}, got {actual}"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Unit variants (no params)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn flip_v_produces_correct_schema() {
    let p = translate_ok(Node::FlipV);
    assert_eq!(first_schema_id(&p), "zenlayout.flip_v");
    assert_eq!(p.nodes.len(), 1);
}

#[test]
fn flip_h_produces_correct_schema() {
    let p = translate_ok(Node::FlipH);
    assert_eq!(first_schema_id(&p), "zenlayout.flip_h");
    assert_eq!(p.nodes.len(), 1);
}

#[test]
fn rotate_90_produces_correct_schema() {
    let p = translate_ok(Node::Rotate90);
    assert_eq!(first_schema_id(&p), "zenlayout.rotate_90");
    assert_eq!(p.nodes.len(), 1);
}

#[test]
fn rotate_180_produces_correct_schema() {
    let p = translate_ok(Node::Rotate180);
    assert_eq!(first_schema_id(&p), "zenlayout.rotate_180");
    assert_eq!(p.nodes.len(), 1);
}

#[test]
fn rotate_270_produces_correct_schema() {
    let p = translate_ok(Node::Rotate270);
    assert_eq!(first_schema_id(&p), "zenlayout.rotate_270");
    assert_eq!(p.nodes.len(), 1);
}

#[test]
fn watermark_red_dot_produces_correct_schema() {
    let p = translate_ok(Node::WatermarkRedDot);
    assert_eq!(p.nodes.len(), 1);
    let id = p.nodes[0].schema().id;
    assert!(
        id.contains("watermark") || id.contains("red_dot"),
        "expected schema ID containing 'watermark' or 'red_dot', got: {id}"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Decomposed: Transpose → 2 nodes (rotate_90 + flip_h)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn transpose_produces_two_nodes() {
    let p = translate_ok(Node::Transpose);
    assert_eq!(p.nodes.len(), 2, "Transpose should decompose into 2 nodes");
    assert_eq!(p.nodes[0].schema().id, "zenlayout.rotate_90");
    assert_eq!(p.nodes[1].schema().id, "zenlayout.flip_h");
}

// ═══════════════════════════════════════════════════════════════════
// Simple params
// ═══════════════════════════════════════════════════════════════════

#[test]
fn apply_orientation_sets_flag() {
    let p = translate_ok(Node::ApplyOrientation { flag: 6 });
    assert_eq!(first_schema_id(&p), "zenlayout.orient");
    assert_eq!(p.nodes[0].get_param("orientation"), Some(ParamValue::I32(6)));
}

#[test]
fn crop_converts_corners_to_xywh() {
    let p = translate_ok(Node::Crop { x1: 10, y1: 20, x2: 110, y2: 220 });
    assert_eq!(first_schema_id(&p), "zenlayout.crop");
    assert_eq!(p.nodes[0].get_param("x"), Some(ParamValue::U32(10)));
    assert_eq!(p.nodes[0].get_param("y"), Some(ParamValue::U32(20)));
    assert_eq!(p.nodes[0].get_param("w"), Some(ParamValue::U32(100)));
    assert_eq!(p.nodes[0].get_param("h"), Some(ParamValue::U32(200)));
}

#[test]
fn crop_whitespace_sets_params() {
    let p = translate_ok(Node::CropWhitespace { threshold: 80, percent_padding: 0.5 });
    assert_eq!(first_schema_id(&p), "zenpipe.crop_whitespace");
    assert_eq!(p.nodes[0].get_param("threshold"), Some(ParamValue::U32(80)));
    assert_eq!(p.nodes[0].get_param("percent_padding"), Some(ParamValue::F32(0.5)));
}

// ═══════════════════════════════════════════════════════════════════
// Complex params: Constrain
// ═══════════════════════════════════════════════════════════════════

#[test]
fn constrain_within_basic() {
    let c = Constraint {
        mode: ConstraintMode::Within,
        w: Some(800),
        h: Some(600),
        hints: None,
        gravity: None,
        canvas_color: None,
    };
    let p = translate_ok(Node::Constrain(c));
    assert!(!p.nodes.is_empty());
    let node = &p.nodes[0];
    assert_eq!(node.schema().id, "zenresize.constrain");
    assert_eq!(node.get_param("w"), Some(ParamValue::U32(800)));
    assert_eq!(node.get_param("h"), Some(ParamValue::U32(600)));
    assert_eq!(node.get_param("mode"), Some(ParamValue::Str("within".into())));
}

#[test]
fn constrain_fit_crop_with_gravity() {
    let c = Constraint {
        mode: ConstraintMode::FitCrop,
        w: Some(400),
        h: Some(300),
        hints: None,
        gravity: Some(ConstraintGravity::Percentage { x: 0.25, y: 0.75 }),
        canvas_color: None,
    };
    let p = translate_ok(Node::Constrain(c));
    assert!(!p.nodes.is_empty());
    let node = &p.nodes[0];
    assert_eq!(node.get_param("mode"), Some(ParamValue::Str("fit_crop".into())));
    assert_eq!(node.get_param("w"), Some(ParamValue::U32(400)));
    assert_eq!(node.get_param("h"), Some(ParamValue::U32(300)));
}

#[test]
fn constrain_all_modes_produce_correct_mode_string() {
    let modes = [
        (ConstraintMode::Distort, "distort"),
        (ConstraintMode::Within, "within"),
        (ConstraintMode::Fit, "fit"),
        (ConstraintMode::FitCrop, "fit_crop"),
        (ConstraintMode::WithinCrop, "within_crop"),
        (ConstraintMode::FitPad, "fit_pad"),
        (ConstraintMode::WithinPad, "within_pad"),
        (ConstraintMode::AspectCrop, "aspect_crop"),
        (ConstraintMode::LargerThan, "larger_than"),
    ];
    for (mode, expected_str) in modes {
        let c = Constraint {
            mode,
            w: Some(100),
            h: Some(100),
            hints: None,
            gravity: None,
            canvas_color: None,
        };
        let p = translate_ok(Node::Constrain(c));
        let node = &p.nodes[0];
        assert_eq!(
            node.get_param("mode"),
            Some(ParamValue::Str(expected_str.into())),
            "ConstraintMode::{mode:?} should produce mode string '{expected_str}'"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════
// Resample2D
// ═══════════════════════════════════════════════════════════════════

#[test]
fn resample_2d_basic() {
    let p = translate_ok(Node::Resample2D { w: 400, h: 300, hints: None });
    assert!(!p.nodes.is_empty());
    let node = &p.nodes[0];
    assert_eq!(node.schema().id, "zenresize.resize");
    assert_eq!(node.get_param("w"), Some(ParamValue::U32(400)));
    assert_eq!(node.get_param("h"), Some(ParamValue::U32(300)));
}

// ═══════════════════════════════════════════════════════════════════
// Color handling
// ═══════════════════════════════════════════════════════════════════

#[test]
fn expand_canvas_transparent() {
    let p = translate_ok(Node::ExpandCanvas {
        left: 10,
        top: 10,
        right: 10,
        bottom: 10,
        color: Color::Transparent,
    });
    assert_eq!(first_schema_id(&p), "zenlayout.expand_canvas");
    assert_eq!(p.nodes[0].get_param("left"), Some(ParamValue::U32(10)));
    assert_eq!(p.nodes[0].get_param("top"), Some(ParamValue::U32(10)));
    assert_eq!(p.nodes[0].get_param("right"), Some(ParamValue::U32(10)));
    assert_eq!(p.nodes[0].get_param("bottom"), Some(ParamValue::U32(10)));
    assert_eq!(
        p.nodes[0].get_param("color"),
        Some(ParamValue::Str("transparent".into()))
    );
}

#[test]
fn fill_rect_with_hex_color() {
    let p = translate_ok(Node::FillRect {
        x1: 0,
        y1: 0,
        x2: 100,
        y2: 100,
        color: Color::Srgb(ColorSrgb::Hex("FF0000".into())),
    });
    assert_eq!(first_schema_id(&p), "zenpipe.fill_rect");
    assert_eq!(p.nodes[0].get_param("color_r"), Some(ParamValue::U32(255)));
    assert_eq!(p.nodes[0].get_param("color_g"), Some(ParamValue::U32(0)));
    assert_eq!(p.nodes[0].get_param("color_b"), Some(ParamValue::U32(0)));
    assert_eq!(p.nodes[0].get_param("color_a"), Some(ParamValue::U32(255)));
}

#[test]
fn fill_rect_black() {
    let p = translate_ok(Node::FillRect {
        x1: 5,
        y1: 5,
        x2: 50,
        y2: 50,
        color: Color::Black,
    });
    assert_eq!(p.nodes[0].get_param("color_r"), Some(ParamValue::U32(0)));
    assert_eq!(p.nodes[0].get_param("color_g"), Some(ParamValue::U32(0)));
    assert_eq!(p.nodes[0].get_param("color_b"), Some(ParamValue::U32(0)));
    assert_eq!(p.nodes[0].get_param("color_a"), Some(ParamValue::U32(255)));
}

// ═══════════════════════════════════════════════════════════════════
// Region and RegionPercent
// ═══════════════════════════════════════════════════════════════════

#[test]
fn region_maps_pixel_coordinates() {
    // v2 Region uses pixel edge coordinates (i32).
    // Mapped to zenlayout.region's left_px/top_px/right_px/bottom_px.
    let p = translate_ok(Node::Region {
        x1: -10,
        y1: -20,
        x2: 100,
        y2: 200,
        background_color: Color::Transparent,
    });
    assert_eq!(first_schema_id(&p), "zenlayout.region");
    assert_eq!(p.nodes.len(), 1);

    assert_param_i32(&p.nodes[0], "left_px", -10);
    assert_param_i32(&p.nodes[0], "top_px", -20);
    assert_param_i32(&p.nodes[0], "right_px", 100);
    assert_param_i32(&p.nodes[0], "bottom_px", 200);
}

#[test]
fn region_percent_converts_edges_to_origin_size() {
    // v2 RegionPercent uses edge percentages (0-100 scale).
    // Mapped to zenlayout.crop_percent's x/y/w/h (0.0-1.0 fractions).
    // x = x1/100, y = y1/100, w = (x2-x1)/100, h = (y2-y1)/100
    let p = translate_ok(Node::RegionPercent {
        x1: 10.0,
        y1: 20.0,
        x2: 60.0,
        y2: 70.0,
        background_color: Color::Black,
    });
    assert_eq!(first_schema_id(&p), "zenlayout.crop_percent");
    assert_eq!(p.nodes.len(), 1);

    assert_param_f32_approx(&p.nodes[0], "x", 0.1);
    assert_param_f32_approx(&p.nodes[0], "y", 0.2);
    assert_param_f32_approx(&p.nodes[0], "w", 0.5);
    assert_param_f32_approx(&p.nodes[0], "h", 0.5);
}

// ═══════════════════════════════════════════════════════════════════
// RoundImageCorners
// ═══════════════════════════════════════════════════════════════════

#[test]
fn round_corners_percentage() {
    let p = translate_ok(Node::RoundImageCorners {
        radius: RoundCornersMode::Percentage(15.0),
        background_color: Color::Transparent,
    });
    assert_eq!(first_schema_id(&p), "zenpipe.round_corners");
    assert_eq!(p.nodes[0].get_param("radius"), Some(ParamValue::F32(15.0)));
    assert_eq!(p.nodes[0].get_param("mode"), Some(ParamValue::Str("percentage".into())));
}

#[test]
fn round_corners_circle() {
    let p = translate_ok(Node::RoundImageCorners {
        radius: RoundCornersMode::Circle,
        background_color: Color::Black,
    });
    assert_eq!(first_schema_id(&p), "zenpipe.round_corners");
    assert_eq!(p.nodes[0].get_param("mode"), Some(ParamValue::Str("circle".into())));
    // Circle uses radius=50.0 internally
    assert_eq!(p.nodes[0].get_param("radius"), Some(ParamValue::F32(50.0)));
}

// ═══════════════════════════════════════════════════════════════════
// WhiteBalanceHistogramAreaThresholdSrgb
// ═══════════════════════════════════════════════════════════════════

#[test]
fn white_balance_default_threshold() {
    let p = translate_ok(Node::WhiteBalanceHistogramAreaThresholdSrgb { threshold: None });
    assert_eq!(first_schema_id(&p), "imageflow.white_balance_srgb");
    assert_eq!(p.nodes[0].get_param("threshold"), Some(ParamValue::F32(0.006)));
}

#[test]
fn white_balance_custom_threshold() {
    let p = translate_ok(Node::WhiteBalanceHistogramAreaThresholdSrgb {
        threshold: Some(0.01),
    });
    assert_eq!(first_schema_id(&p), "imageflow.white_balance_srgb");
    assert_eq!(p.nodes[0].get_param("threshold"), Some(ParamValue::F32(0.01)));
}

// ═══════════════════════════════════════════════════════════════════
// ColorMatrixSrgb
// ═══════════════════════════════════════════════════════════════════

#[test]
fn color_matrix_srgb_sets_matrix() {
    let matrix = [
        [1.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 0.0, 1.0],
    ];
    let p = translate_ok(Node::ColorMatrixSrgb { matrix });
    assert_eq!(p.nodes.len(), 1);
    assert_eq!(p.nodes[0].schema().id, "imageflow.color_matrix_srgb");
    let param = p.nodes[0].get_param("matrix");
    assert!(param.is_some(), "matrix param should exist");
    if let Some(ParamValue::F32Array(v)) = param {
        assert_eq!(v.len(), 25);
    } else {
        panic!("expected F32Array param for matrix");
    }
}

// ═══════════════════════════════════════════════════════════════════
// Config nodes (Decode, Encode, CreateCanvas)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn decode_sets_io_id() {
    let p = translate_ok(Node::Decode { io_id: 0, commands: None });
    assert_eq!(p.decode_io_id, Some(0));
    assert!(p.nodes.is_empty(), "Decode should not produce pixel nodes");
}

#[test]
fn decode_with_nonzero_io_id() {
    let p = translate_ok(Node::Decode { io_id: 42, commands: None });
    assert_eq!(p.decode_io_id, Some(42));
}

#[test]
fn encode_mozjpeg_sets_preset_and_io_id() {
    let p = translate_ok(Node::Encode {
        io_id: 1,
        preset: EncoderPreset::Mozjpeg { quality: Some(85), progressive: None, matte: None },
    });
    assert_eq!(p.encode_io_id, Some(1));
    assert!(p.preset.is_some(), "Encode should produce a preset mapping");
    assert!(p.nodes.is_empty(), "Encode should not produce pixel nodes");
}

#[test]
fn create_canvas_sets_canvas_params() {
    let p = translate_ok(Node::CreateCanvas {
        w: 200,
        h: 100,
        color: Color::Black,
        format: PixelFormat::Bgra32,
    });
    assert!(p.create_canvas.is_some(), "CreateCanvas should set create_canvas");
    let canvas = p.create_canvas.as_ref().unwrap();
    assert_eq!(canvas.w, 200);
    assert_eq!(canvas.h, 100);
    // decode_io_id is set to -1 as a sentinel
    assert_eq!(p.decode_io_id, Some(-1));
}

// ═══════════════════════════════════════════════════════════════════
// ColorFilterSrgb variants → color_matrix_srgb nodes
// ═══════════════════════════════════════════════════════════════════

#[test]
fn color_filter_sepia_produces_color_matrix() {
    let p = translate_ok(Node::ColorFilterSrgb(ColorFilterSrgb::Sepia));
    assert_eq!(p.nodes.len(), 1);
    assert_eq!(p.nodes[0].schema().id, "imageflow.color_matrix_srgb");
}

#[test]
fn color_filter_grayscale_ntsc_produces_color_matrix() {
    let p = translate_ok(Node::ColorFilterSrgb(ColorFilterSrgb::GrayscaleNtsc));
    assert_eq!(p.nodes.len(), 1);
    assert_eq!(p.nodes[0].schema().id, "imageflow.color_matrix_srgb");
}

#[test]
fn color_filter_invert_produces_color_matrix() {
    let p = translate_ok(Node::ColorFilterSrgb(ColorFilterSrgb::Invert));
    assert_eq!(p.nodes.len(), 1);
    assert_eq!(p.nodes[0].schema().id, "imageflow.color_matrix_srgb");
}

#[test]
fn color_filter_contrast_produces_color_matrix() {
    let p = translate_ok(Node::ColorFilterSrgb(ColorFilterSrgb::Contrast(0.5)));
    assert_eq!(p.nodes.len(), 1);
    assert_eq!(p.nodes[0].schema().id, "imageflow.color_matrix_srgb");
}

#[test]
fn color_filter_brightness_produces_color_matrix() {
    let p = translate_ok(Node::ColorFilterSrgb(ColorFilterSrgb::Brightness(0.2)));
    assert_eq!(p.nodes.len(), 1);
    assert_eq!(p.nodes[0].schema().id, "imageflow.color_matrix_srgb");
}

#[test]
fn color_filter_saturation_produces_color_matrix() {
    let p = translate_ok(Node::ColorFilterSrgb(ColorFilterSrgb::Saturation(-0.3)));
    assert_eq!(p.nodes.len(), 1);
    assert_eq!(p.nodes[0].schema().id, "imageflow.color_matrix_srgb");
}

#[test]
fn color_filter_alpha_produces_color_matrix() {
    let p = translate_ok(Node::ColorFilterSrgb(ColorFilterSrgb::Alpha(0.5)));
    assert_eq!(p.nodes.len(), 1);
    assert_eq!(p.nodes[0].schema().id, "imageflow.color_matrix_srgb");
}

#[test]
fn color_filter_all_grayscale_variants() {
    let variants = [
        ColorFilterSrgb::GrayscaleNtsc,
        ColorFilterSrgb::GrayscaleFlat,
        ColorFilterSrgb::GrayscaleBt709,
        ColorFilterSrgb::GrayscaleRy,
    ];
    for filter in &variants {
        let p = translate_ok(Node::ColorFilterSrgb(*filter));
        assert_eq!(p.nodes.len(), 1, "filter {filter:?} should produce exactly 1 node");
        assert_eq!(
            p.nodes[0].schema().id,
            "imageflow.color_matrix_srgb",
            "filter {filter:?} should produce color_matrix_srgb"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════
// Unsupported nodes → TranslateError
// ═══════════════════════════════════════════════════════════════════

#[test]
fn command_string_without_expansion_is_unsupported() {
    let result = translate_one(Node::CommandString {
        kind: CommandStringKind::ImageResizer4,
        value: "w=100&h=100".into(),
        decode: Some(0),
        encode: Some(1),
        watermarks: None,
    });
    assert!(result.is_err(), "CommandString should be unsupported without expansion");
}

#[test]
fn draw_image_exact_produces_composite() {
    let p = translate_ok(Node::DrawImageExact {
        x: 10,
        y: 20,
        w: 100,
        h: 100,
        blend: Some(CompositingMode::Compose),
        hints: None,
    });
    assert_eq!(first_schema_id(&p), "zenpipe.composite");
    assert_param_u32(&p.nodes[0], "fg_x", 10);
    assert_param_u32(&p.nodes[0], "fg_y", 20);
}

#[test]
fn draw_image_exact_overwrite_blend() {
    let p = translate_ok(Node::DrawImageExact {
        x: 0,
        y: 0,
        w: 50,
        h: 50,
        blend: Some(CompositingMode::Overwrite),
        hints: None,
    });
    let mode = p.nodes[0].get_param("blend_mode")
        .and_then(|v| v.as_str().map(|s| s.to_string()));
    assert_eq!(mode.as_deref(), Some("source"));
}

#[test]
fn copy_rect_to_canvas_produces_crop_and_composite() {
    let p = translate_ok(Node::CopyRectToCanvas {
        from_x: 10,
        from_y: 20,
        w: 100,
        h: 80,
        x: 50,
        y: 60,
    });
    // Should produce 2 nodes: Crop then Composite.
    assert_eq!(p.nodes.len(), 2);
    assert_eq!(p.nodes[0].schema().id, "zenlayout.crop");
    assert_eq!(p.nodes[1].schema().id, "zenpipe.composite");

    // Crop params: from_x/from_y as origin, w/h as size.
    assert_param_u32(&p.nodes[0], "x", 10);
    assert_param_u32(&p.nodes[0], "y", 20);
    assert_param_u32(&p.nodes[0], "w", 100);
    assert_param_u32(&p.nodes[0], "h", 80);

    // Composite params: target position on canvas.
    assert_param_u32(&p.nodes[1], "fg_x", 50);
    assert_param_u32(&p.nodes[1], "fg_y", 60);
}

// ═══════════════════════════════════════════════════════════════════
// CaptureBitmapKey (internal no-op)
// ═══════════════════════════════════════════════════════════════════

#[test]
fn capture_bitmap_key_is_noop() {
    let p = translate_ok(Node::CaptureBitmapKey { capture_id: 42 });
    assert!(p.nodes.is_empty(), "CaptureBitmapKey should produce no pixel nodes");
    assert!(p.preset.is_none());
    assert!(p.decode_io_id.is_none());
}

// ═══════════════════════════════════════════════════════════════════
// Multi-node pipelines
// ═══════════════════════════════════════════════════════════════════

#[test]
fn multi_node_pipeline_preserves_order() {
    let io_buffers: HashMap<i32, Vec<u8>> = HashMap::new();
    let nodes = vec![
        Node::Decode { io_id: 0, commands: None },
        Node::FlipV,
        Node::Crop { x1: 0, y1: 0, x2: 50, y2: 50 },
        Node::Rotate90,
        Node::Encode {
            io_id: 1,
            preset: EncoderPreset::Libpng {
                depth: None,
                matte: None,
                zlib_compression: None,
            },
        },
    ];
    let p = translate::translate_nodes(&nodes, &io_buffers).expect("should succeed");
    assert_eq!(p.decode_io_id, Some(0));
    assert_eq!(p.encode_io_id, Some(1));
    assert!(p.preset.is_some());
    // 3 pixel-processing nodes: FlipV, Crop, Rotate90
    assert_eq!(p.nodes.len(), 3);
    assert_eq!(p.nodes[0].schema().id, "zenlayout.flip_v");
    assert_eq!(p.nodes[1].schema().id, "zenlayout.crop");
    assert_eq!(p.nodes[2].schema().id, "zenlayout.rotate_90");
}

// ═══════════════════════════════════════════════════════════════════
// Structural parity: all Node variants are handled
// ═══════════════════════════════════════════════════════════════════

/// Construct every Node variant and verify that each either translates
/// successfully or returns a known TranslateError. No variant should
/// silently fail or panic.
#[test]
fn all_v2_variants_handled() {
    // Translatable variants
    let translatable: Vec<(&str, Node)> = vec![
        ("FlipV", Node::FlipV),
        ("FlipH", Node::FlipH),
        ("Rotate90", Node::Rotate90),
        ("Rotate180", Node::Rotate180),
        ("Rotate270", Node::Rotate270),
        ("Transpose", Node::Transpose),
        ("WatermarkRedDot", Node::WatermarkRedDot),
        ("ApplyOrientation", Node::ApplyOrientation { flag: 1 }),
        ("Crop", Node::Crop { x1: 0, y1: 0, x2: 10, y2: 10 }),
        ("CropWhitespace", Node::CropWhitespace { threshold: 50, percent_padding: 0.0 }),
        (
            "Constrain",
            Node::Constrain(Constraint {
                mode: ConstraintMode::Within,
                w: Some(100),
                h: Some(100),
                hints: None,
                gravity: None,
                canvas_color: None,
            }),
        ),
        ("Resample2D", Node::Resample2D { w: 100, h: 100, hints: None }),
        (
            "ExpandCanvas",
            Node::ExpandCanvas {
                left: 1,
                top: 1,
                right: 1,
                bottom: 1,
                color: Color::Black,
            },
        ),
        (
            "FillRect",
            Node::FillRect { x1: 0, y1: 0, x2: 10, y2: 10, color: Color::Black },
        ),
        (
            "Region",
            Node::Region { x1: 0, y1: 0, x2: 10, y2: 10, background_color: Color::Black },
        ),
        (
            "RegionPercent",
            Node::RegionPercent {
                x1: 0.0,
                y1: 0.0,
                x2: 100.0,
                y2: 100.0,
                background_color: Color::Black,
            },
        ),
        (
            "RoundImageCorners",
            Node::RoundImageCorners {
                radius: RoundCornersMode::Pixels(10.0),
                background_color: Color::Transparent,
            },
        ),
        (
            "WhiteBalanceHistogramAreaThresholdSrgb",
            Node::WhiteBalanceHistogramAreaThresholdSrgb { threshold: Some(0.006) },
        ),
        (
            "ColorMatrixSrgb",
            Node::ColorMatrixSrgb {
                matrix: [[1.0, 0.0, 0.0, 0.0, 0.0]; 5],
            },
        ),
        ("ColorFilterSrgb_Sepia", Node::ColorFilterSrgb(ColorFilterSrgb::Sepia)),
        (
            "ColorFilterSrgb_GrayscaleNtsc",
            Node::ColorFilterSrgb(ColorFilterSrgb::GrayscaleNtsc),
        ),
        (
            "ColorFilterSrgb_GrayscaleFlat",
            Node::ColorFilterSrgb(ColorFilterSrgb::GrayscaleFlat),
        ),
        (
            "ColorFilterSrgb_GrayscaleBt709",
            Node::ColorFilterSrgb(ColorFilterSrgb::GrayscaleBt709),
        ),
        ("ColorFilterSrgb_GrayscaleRy", Node::ColorFilterSrgb(ColorFilterSrgb::GrayscaleRy)),
        ("ColorFilterSrgb_Invert", Node::ColorFilterSrgb(ColorFilterSrgb::Invert)),
        ("ColorFilterSrgb_Alpha", Node::ColorFilterSrgb(ColorFilterSrgb::Alpha(0.5))),
        ("ColorFilterSrgb_Contrast", Node::ColorFilterSrgb(ColorFilterSrgb::Contrast(0.5))),
        (
            "ColorFilterSrgb_Brightness",
            Node::ColorFilterSrgb(ColorFilterSrgb::Brightness(0.2)),
        ),
        (
            "ColorFilterSrgb_Saturation",
            Node::ColorFilterSrgb(ColorFilterSrgb::Saturation(0.3)),
        ),
        ("Decode", Node::Decode { io_id: 0, commands: None }),
        (
            "Encode",
            Node::Encode {
                io_id: 1,
                preset: EncoderPreset::Libpng {
                    depth: None,
                    matte: None,
                    zlib_compression: None,
                },
            },
        ),
        (
            "CreateCanvas",
            Node::CreateCanvas {
                w: 10,
                h: 10,
                color: Color::Black,
                format: PixelFormat::Bgra32,
            },
        ),
        ("CaptureBitmapKey", Node::CaptureBitmapKey { capture_id: 0 }),
        // Composition nodes (now supported)
        (
            "DrawImageExact",
            Node::DrawImageExact {
                x: 0,
                y: 0,
                w: 100,
                h: 100,
                blend: Some(CompositingMode::Compose),
                hints: None,
            },
        ),
        (
            "CopyRectToCanvas",
            Node::CopyRectToCanvas { from_x: 0, from_y: 0, w: 10, h: 10, x: 0, y: 0 },
        ),
    ];

    for (name, node) in &translatable {
        let result = translate_one(node.clone());
        assert!(
            result.is_ok(),
            "Node variant '{name}' should translate successfully, got: {:?}",
            result.err()
        );
    }

    // Known-unsupported variants (must be expanded before translation)
    let unsupported: Vec<(&str, Node)> = vec![
        (
            "CommandString",
            Node::CommandString {
                kind: CommandStringKind::ImageResizer4,
                value: "w=100".into(),
                decode: Some(0),
                encode: Some(1),
                watermarks: None,
            },
        ),
    ];

    for (name, node) in &unsupported {
        let result = translate_one(node.clone());
        assert!(
            result.is_err(),
            "Node variant '{name}' should return an error (known unsupported)"
        );
    }

    // Watermark is tested separately (requires io_buffers with actual image data).
    // It should translate OK with valid data, but we cannot construct one without
    // an actual image buffer. We just verify it does not panic with empty buffers.
    let wm = Node::Watermark(imageflow_types::Watermark {
        io_id: 99,
        fit_box: None,
        fit_mode: None,
        gravity: None,
        min_canvas_width: None,
        min_canvas_height: None,
        opacity: None,
        hints: None,
    });
    let wm_result = translate_one(wm);
    // Watermark will fail because io_id 99 has no buffer — that is expected.
    // The important thing is it returns Err, not panics.
    assert!(
        wm_result.is_err(),
        "Watermark with missing io_buffer should error, not panic"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Encoder preset parity: all EncoderPreset variants handled
// ═══════════════════════════════════════════════════════════════════

#[test]
fn encoder_presets_all_handled() {
    use imageflow_types::QualityProfile;
    use zenpipe::imageflow_compat::preset_map;

    let presets: Vec<(&str, EncoderPreset)> = vec![
        (
            "Auto",
            EncoderPreset::Auto {
                quality_profile: QualityProfile::Good,
                quality_profile_dpr: None,
                matte: None,
                lossless: None,
                allow: None,
            },
        ),
        (
            "Format_Jpeg",
            EncoderPreset::Format {
                format: imageflow_types::OutputImageFormat::Jpeg,
                quality_profile: Some(QualityProfile::High),
                quality_profile_dpr: None,
                matte: None,
                lossless: None,
                allow: None,
                encoder_hints: None,
            },
        ),
        (
            "Format_Png",
            EncoderPreset::Format {
                format: imageflow_types::OutputImageFormat::Png,
                quality_profile: None,
                quality_profile_dpr: None,
                matte: None,
                lossless: None,
                allow: None,
                encoder_hints: None,
            },
        ),
        (
            "Format_WebP",
            EncoderPreset::Format {
                format: imageflow_types::OutputImageFormat::Webp,
                quality_profile: None,
                quality_profile_dpr: None,
                matte: None,
                lossless: None,
                allow: None,
                encoder_hints: None,
            },
        ),
        (
            "Format_Keep",
            EncoderPreset::Format {
                format: imageflow_types::OutputImageFormat::Keep,
                quality_profile: None,
                quality_profile_dpr: None,
                matte: None,
                lossless: None,
                allow: None,
                encoder_hints: None,
            },
        ),
        (
            "Mozjpeg",
            EncoderPreset::Mozjpeg { quality: Some(85), progressive: Some(true), matte: None },
        ),
        (
            "Mozjpeg_baseline",
            EncoderPreset::Mozjpeg {
                quality: Some(75),
                progressive: Some(false),
                matte: Some(Color::Black),
            },
        ),
        (
            "LibjpegTurbo",
            EncoderPreset::LibjpegTurbo {
                quality: Some(90),
                progressive: Some(false),
                optimize_huffman_coding: None,
                matte: None,
            },
        ),
        (
            "Libpng",
            EncoderPreset::Libpng {
                depth: None,
                matte: None,
                zlib_compression: None,
            },
        ),
        (
            "Pngquant",
            EncoderPreset::Pngquant {
                quality: Some(80),
                minimum_quality: Some(60),
                speed: Some(3),
                maximum_deflate: None,
            },
        ),
        ("Lodepng", EncoderPreset::Lodepng { maximum_deflate: Some(true) }),
        ("WebPLossy", EncoderPreset::WebPLossy { quality: 75.0 }),
        ("WebPLossless", EncoderPreset::WebPLossless),
        ("Gif", EncoderPreset::Gif),
    ];

    for (name, preset) in &presets {
        let result = preset_map::map_preset(preset);
        assert!(
            result.is_ok(),
            "EncoderPreset '{name}' should map successfully, got: {:?}",
            result.err()
        );
    }
}

// ═══════════════════════════════════════════════════════════════════
// Edge cases
// ═══════════════════════════════════════════════════════════════════

#[test]
fn crop_zero_size_does_not_panic() {
    // x2 == x1 → w=0, h=0
    let p = translate_ok(Node::Crop { x1: 50, y1: 50, x2: 50, y2: 50 });
    assert_eq!(p.nodes[0].get_param("w"), Some(ParamValue::U32(0)));
    assert_eq!(p.nodes[0].get_param("h"), Some(ParamValue::U32(0)));
}

#[test]
fn constrain_with_only_width() {
    let c = Constraint {
        mode: ConstraintMode::Fit,
        w: Some(500),
        h: None,
        hints: None,
        gravity: None,
        canvas_color: None,
    };
    let p = translate_ok(Node::Constrain(c));
    let node = &p.nodes[0];
    assert_eq!(node.get_param("w"), Some(ParamValue::U32(500)));
}

#[test]
fn expand_canvas_with_black() {
    let p = translate_ok(Node::ExpandCanvas {
        left: 5,
        top: 5,
        right: 5,
        bottom: 5,
        color: Color::Black,
    });
    assert_eq!(
        p.nodes[0].get_param("color"),
        Some(ParamValue::Str("#000000FF".into()))
    );
}

#[test]
fn expand_canvas_with_hex_color() {
    let p = translate_ok(Node::ExpandCanvas {
        left: 0,
        top: 0,
        right: 20,
        bottom: 20,
        color: Color::Srgb(ColorSrgb::Hex("AABBCC".into())),
    });
    assert_eq!(
        p.nodes[0].get_param("color"),
        Some(ParamValue::Str("#AABBCC".into()))
    );
}

#[test]
fn empty_pipeline_succeeds() {
    let io_buffers: HashMap<i32, Vec<u8>> = HashMap::new();
    let nodes: Vec<Node> = vec![];
    let p = translate::translate_nodes(&nodes, &io_buffers).expect("empty pipeline should succeed");
    assert!(p.nodes.is_empty());
    assert!(p.preset.is_none());
    assert!(p.decode_io_id.is_none());
    assert!(p.encode_io_id.is_none());
}

#[test]
fn apply_orientation_identity() {
    // flag=1 is the identity orientation (no change needed)
    let p = translate_ok(Node::ApplyOrientation { flag: 1 });
    assert_eq!(p.nodes[0].get_param("orientation"), Some(ParamValue::I32(1)));
}

#[test]
fn round_corners_pixels_custom() {
    let p = translate_ok(Node::RoundImageCorners {
        radius: RoundCornersMode::PixelsCustom {
            top_left: 5.0,
            top_right: 10.0,
            bottom_right: 15.0,
            bottom_left: 20.0,
        },
        background_color: Color::Transparent,
    });
    assert_eq!(p.nodes[0].get_param("mode"), Some(ParamValue::Str("pixels_custom".into())));
    assert_eq!(p.nodes[0].get_param("radius_tl"), Some(ParamValue::F32(5.0)));
    assert_eq!(p.nodes[0].get_param("radius_tr"), Some(ParamValue::F32(10.0)));
    assert_eq!(p.nodes[0].get_param("radius_bl"), Some(ParamValue::F32(20.0)));
    assert_eq!(p.nodes[0].get_param("radius_br"), Some(ParamValue::F32(15.0)));
}

// ═══════════════════════════════════════════════════════════════════════
// Execute dimension tests — translate + pipeline through bridge
// (imazen/zenpipe#14: rotate-after-constrain dimension regression)
// ═══════════════════════════════════════════════════════════════════════

fn translate_and_execute(nodes: Vec<Node>, src_w: u32, src_h: u32) -> (u32, u32) {
    let io_buffers: HashMap<i32, Vec<u8>> = HashMap::new();
    let pipeline = translate::translate_nodes(&nodes, &io_buffers)
        .expect("translation should succeed");

    let bpp = zenpipe::format::RGBA8_SRGB.bytes_per_pixel() as usize;
    let data = vec![128u8; src_w as usize * src_h as usize * bpp];
    let source: Box<dyn zenpipe::Source> = Box::new(
        zenpipe::sources::MaterializedSource::from_data(data, src_w, src_h, zenpipe::format::RGBA8_SRGB),
    );

    let result = zenpipe::bridge::build_pipeline(source, &pipeline.nodes, &[])
        .expect("pipeline should build");
    (result.source.width(), result.source.height())
}

#[test]
fn execute_constrain_then_rotate90_dimensions() {
    // 600x450 → constrain within 70x70 → 70x53 → rotate90 → 53x70
    let (w, h) = translate_and_execute(
        vec![
            Node::Constrain(Constraint {
                mode: ConstraintMode::Within,
                w: Some(70), h: Some(70),
                hints: None, gravity: None, canvas_color: None,
            }),
            Node::Rotate90,
        ],
        600, 450,
    );
    assert_eq!((w, h), (53, 70), "constrain+rot90: expected 53x70, got {w}x{h}");
}

#[test]
fn execute_constrain_then_rotate270_dimensions() {
    let (w, h) = translate_and_execute(
        vec![
            Node::Constrain(Constraint {
                mode: ConstraintMode::Within,
                w: Some(70), h: Some(70),
                hints: None, gravity: None, canvas_color: None,
            }),
            Node::Rotate270,
        ],
        600, 450,
    );
    assert_eq!((w, h), (53, 70), "constrain+rot270: expected 53x70, got {w}x{h}");
}

#[test]
fn execute_rotate90_standalone_dimensions() {
    let (w, h) = translate_and_execute(vec![Node::Rotate90], 100, 60);
    assert_eq!((w, h), (60, 100), "standalone rot90: expected 60x100, got {w}x{h}");
}

#[test]
fn execute_constrain_then_flip_h_dimensions() {
    let (w, h) = translate_and_execute(
        vec![
            Node::Constrain(Constraint {
                mode: ConstraintMode::Within,
                w: Some(70), h: Some(70),
                hints: None, gravity: None, canvas_color: None,
            }),
            Node::FlipH,
        ],
        600, 450,
    );
    assert_eq!((w, h), (70, 53), "constrain+flip_h: expected 70x53, got {w}x{h}");
}
