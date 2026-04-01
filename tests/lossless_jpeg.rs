//! Integration test: lossless JPEG fast path for orient-only pipelines.
//!
//! Validates that `try_lossless_jpeg()` correctly short-circuits the full
//! decode->transform->encode path when all operations are pure orientation
//! transforms, and falls through when non-lossless operations are present.
//!
//! Run: `cargo test --features lossless-jpeg --test lossless_jpeg -- --nocapture`

#![cfg(feature = "lossless-jpeg")]

use zenjpeg::encoder::{ChromaSubsampling, EncoderConfig, PixelLayout};
use zenjpeg::lossless::{LosslessTransform, TransformConfig};
use zennode::{
    AlphaHandling, FormatHint, NodeGroup, NodeRole, NodeSchema, ParamMap, ParamValue,
    PixelFormatPreference,
};

use zenpipe::bridge::EncodeConfig;
use zenpipe::lossless::try_lossless_jpeg;

// ============================================================================
// Helper: generate a small test JPEG (MCU-aligned for clean lossless transforms)
// ============================================================================

fn generate_test_jpeg(w: u32, h: u32) -> Vec<u8> {
    let bpp = 3usize; // RGB8
    let stride = w as usize * bpp;
    let config = EncoderConfig::ycbcr(90.0, ChromaSubsampling::None)
        .progressive(false)
        .optimize_huffman(false);
    let mut enc = config
        .request()
        .encode_from_bytes(w, h, PixelLayout::Rgb8Srgb)
        .expect("encoder creation");

    let mut pixels = vec![0u8; stride * h as usize];
    for y in 0..h {
        for x in 0..w {
            let i = (y as usize * stride) + (x as usize * bpp);
            pixels[i] = (x * 255 / w) as u8; // R gradient
            pixels[i + 1] = (y * 255 / h) as u8; // G gradient
            pixels[i + 2] = 128; // B constant
        }
    }

    enc.push_packed(&pixels, enough::Unstoppable)
        .expect("push pixels");
    enc.finish().expect("finish encode")
}

/// Default geometry FormatHint (no format preferences, no alpha, no neighborhood).
const GEOM_FORMAT: FormatHint = FormatHint {
    preferred: PixelFormatPreference::Any,
    alpha: AlphaHandling::Process,
    changes_dimensions: false,
    is_neighborhood: false,
};

// ============================================================================
// Mock orient node
// ============================================================================

static ORIENT_SCHEMA: NodeSchema = NodeSchema {
    id: "zenlayout.orient",
    label: "Orient",
    description: "Auto-orient from EXIF",
    group: NodeGroup::Geometry,
    role: NodeRole::Geometry,
    params: &[],
    tags: &[],
    coalesce: None,
    format: GEOM_FORMAT,
    version: 1,
    compat_version: 1,
    json_key: "",
    deny_unknown_fields: false,
    inputs: &[],
};

#[derive(Clone)]
struct MockOrientNode {
    params: ParamMap,
}

impl MockOrientNode {
    fn with_exif(orientation: u32) -> Box<dyn zennode::NodeInstance> {
        let mut params = ParamMap::new();
        params.insert("orientation".into(), ParamValue::U32(orientation));
        Box::new(Self { params })
    }
}

impl zennode::NodeInstance for MockOrientNode {
    fn schema(&self) -> &'static NodeSchema {
        &ORIENT_SCHEMA
    }
    fn to_params(&self) -> ParamMap {
        self.params.clone()
    }
    fn get_param(&self, name: &str) -> Option<ParamValue> {
        self.params.get(name).cloned()
    }
    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        self.params.insert(name.into(), value);
        true
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn clone_boxed(&self) -> Box<dyn zennode::NodeInstance> {
        Box::new(self.clone())
    }
}

// ============================================================================
// Mock flip node
// ============================================================================

static FLIP_H_SCHEMA: NodeSchema = NodeSchema {
    id: "zenlayout.flip_h",
    label: "Flip H",
    description: "Horizontal flip",
    group: NodeGroup::Geometry,
    role: NodeRole::Geometry,
    params: &[],
    tags: &[],
    coalesce: None,
    format: GEOM_FORMAT,
    version: 1,
    compat_version: 1,
    json_key: "",
    deny_unknown_fields: false,
    inputs: &[],
};

#[derive(Clone)]
struct MockFlipHNode;

impl MockFlipHNode {
    fn boxed() -> Box<dyn zennode::NodeInstance> {
        Box::new(Self)
    }
}

impl zennode::NodeInstance for MockFlipHNode {
    fn schema(&self) -> &'static NodeSchema {
        &FLIP_H_SCHEMA
    }
    fn to_params(&self) -> ParamMap {
        ParamMap::new()
    }
    fn get_param(&self, _name: &str) -> Option<ParamValue> {
        None
    }
    fn set_param(&mut self, _name: &str, _value: ParamValue) -> bool {
        false
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn clone_boxed(&self) -> Box<dyn zennode::NodeInstance> {
        Box::new(Self)
    }
}

// ============================================================================
// Mock rotate 90 node
// ============================================================================

static ROTATE90_SCHEMA: NodeSchema = NodeSchema {
    id: "zenlayout.rotate_90",
    label: "Rotate 90",
    description: "Rotate 90 degrees CW",
    group: NodeGroup::Geometry,
    role: NodeRole::Geometry,
    params: &[],
    tags: &[],
    coalesce: None,
    format: GEOM_FORMAT,
    version: 1,
    compat_version: 1,
    json_key: "",
    deny_unknown_fields: false,
    inputs: &[],
};

#[derive(Clone)]
struct MockRotate90Node;

impl MockRotate90Node {
    fn boxed() -> Box<dyn zennode::NodeInstance> {
        Box::new(Self)
    }
}

impl zennode::NodeInstance for MockRotate90Node {
    fn schema(&self) -> &'static NodeSchema {
        &ROTATE90_SCHEMA
    }
    fn to_params(&self) -> ParamMap {
        ParamMap::new()
    }
    fn get_param(&self, _name: &str) -> Option<ParamValue> {
        None
    }
    fn set_param(&mut self, _name: &str, _value: ParamValue) -> bool {
        false
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn clone_boxed(&self) -> Box<dyn zennode::NodeInstance> {
        Box::new(Self)
    }
}

// ============================================================================
// Mock constrain node (to test fallback to non-lossless)
// ============================================================================

static CONSTRAIN_SCHEMA: NodeSchema = NodeSchema {
    id: "zenresize.constrain",
    label: "Constrain",
    description: "Resize within constraints",
    group: NodeGroup::Geometry,
    role: NodeRole::Geometry,
    params: &[],
    tags: &[],
    coalesce: None,
    format: FormatHint {
        preferred: PixelFormatPreference::Any,
        alpha: AlphaHandling::Process,
        changes_dimensions: true,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    deny_unknown_fields: false,
    inputs: &[],
};

#[derive(Clone)]
struct MockConstrainNode {
    params: ParamMap,
}

impl MockConstrainNode {
    fn boxed() -> Box<dyn zennode::NodeInstance> {
        let mut params = ParamMap::new();
        params.insert("w".into(), ParamValue::U32(200));
        params.insert("h".into(), ParamValue::U32(150));
        params.insert("mode".into(), ParamValue::Str("fit".into()));
        Box::new(Self { params })
    }
}

impl zennode::NodeInstance for MockConstrainNode {
    fn schema(&self) -> &'static NodeSchema {
        &CONSTRAIN_SCHEMA
    }
    fn to_params(&self) -> ParamMap {
        self.params.clone()
    }
    fn get_param(&self, name: &str) -> Option<ParamValue> {
        self.params.get(name).cloned()
    }
    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        self.params.insert(name.into(), value);
        true
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn clone_boxed(&self) -> Box<dyn zennode::NodeInstance> {
        Box::new(self.clone())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[test]
fn lossless_rotate90_produces_valid_jpeg() {
    // Use MCU-aligned dimensions (multiples of 8) for clean transforms.
    let jpeg_data = generate_test_jpeg(64, 48);
    assert!(jpeg_data.len() > 100, "JPEG too small");

    let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![MockRotate90Node::boxed()];
    let encode_config = EncodeConfig::default();

    let result = try_lossless_jpeg(&jpeg_data, &nodes, &encode_config, 1, &enough::Unstoppable)
        .expect("lossless transform should not error");

    let result = result.expect("should take the lossless path");

    // Output should be valid JPEG.
    assert!(result.data.len() > 100);
    assert_eq!(&result.data[..2], &[0xFF, 0xD8], "must start with SOI");

    // Rotate90 swaps dimensions.
    assert_eq!(result.width, 48, "width should be original height");
    assert_eq!(result.height, 64, "height should be original width");
}

#[test]
fn lossless_matches_direct_transform() {
    let jpeg_data = generate_test_jpeg(64, 48);

    // Run through try_lossless_jpeg with Rotate90.
    let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![MockRotate90Node::boxed()];
    let encode_config = EncodeConfig::default();
    let lossless_result =
        try_lossless_jpeg(&jpeg_data, &nodes, &encode_config, 1, &enough::Unstoppable)
            .expect("should not error")
            .expect("should take lossless path");

    // Run the same transform directly via zenjpeg::lossless::transform.
    let config = TransformConfig {
        transform: LosslessTransform::Rotate90,
        edge_handling: zenjpeg::lossless::EdgeHandling::TrimPartialBlocks,
    };
    let direct_result =
        zenjpeg::lossless::transform(&jpeg_data, &config, enough::Unstoppable).expect("direct ok");

    // Results must be byte-identical.
    assert_eq!(
        lossless_result.data, direct_result,
        "lossless path output must match direct zenjpeg::lossless::transform()"
    );
}

#[test]
fn lossless_orient_auto_from_exif() {
    let jpeg_data = generate_test_jpeg(64, 48);

    // Auto-orient with EXIF 6 (Rotate90) -- orient node reads from exif_orientation param.
    let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![MockOrientNode::with_exif(6)];
    let encode_config = EncodeConfig::default();

    let result = try_lossless_jpeg(&jpeg_data, &nodes, &encode_config, 1, &enough::Unstoppable)
        .expect("should not error")
        .expect("should take lossless path (orient with EXIF 6 = Rotate90)");

    // EXIF 6 = Rotate90 -> dimensions swap.
    assert_eq!(result.width, 48);
    assert_eq!(result.height, 64);
}

#[test]
fn lossless_identity_returns_source_unchanged() {
    let jpeg_data = generate_test_jpeg(64, 48);

    // EXIF 1 = Normal (identity transform).
    let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![MockOrientNode::with_exif(1)];
    let encode_config = EncodeConfig::default();

    let result = try_lossless_jpeg(&jpeg_data, &nodes, &encode_config, 1, &enough::Unstoppable)
        .expect("should not error")
        .expect("identity should still take lossless path");

    // Identity returns source data unchanged.
    assert_eq!(result.data, jpeg_data);
    assert_eq!(result.width, 64);
    assert_eq!(result.height, 48);
}

#[test]
fn lossless_composed_transforms() {
    let jpeg_data = generate_test_jpeg(64, 48);

    // Rotate90 then FlipH = Transpose.
    let nodes: Vec<Box<dyn zennode::NodeInstance>> =
        vec![MockRotate90Node::boxed(), MockFlipHNode::boxed()];
    let encode_config = EncodeConfig::default();

    let result = try_lossless_jpeg(&jpeg_data, &nodes, &encode_config, 1, &enough::Unstoppable)
        .expect("should not error")
        .expect("composed transforms should be lossless");

    // Transpose swaps dimensions.
    assert_eq!(result.width, 48);
    assert_eq!(result.height, 64);

    // Should match direct Transpose transform.
    let config = TransformConfig {
        transform: LosslessTransform::Transpose,
        edge_handling: zenjpeg::lossless::EdgeHandling::TrimPartialBlocks,
    };
    let direct =
        zenjpeg::lossless::transform(&jpeg_data, &config, enough::Unstoppable).expect("direct ok");
    assert_eq!(result.data, direct);
}

#[test]
fn lossless_fallback_when_resize_present() {
    let jpeg_data = generate_test_jpeg(64, 48);

    // Rotate90 + Constrain -> can't do losslessly.
    let nodes: Vec<Box<dyn zennode::NodeInstance>> =
        vec![MockRotate90Node::boxed(), MockConstrainNode::boxed()];
    let encode_config = EncodeConfig::default();

    let result = try_lossless_jpeg(&jpeg_data, &nodes, &encode_config, 1, &enough::Unstoppable)
        .expect("should not error");

    assert!(
        result.is_none(),
        "should fall back when non-lossless ops present"
    );
}

#[test]
fn lossless_fallback_for_non_jpeg_source() {
    // PNG magic bytes.
    let png_data = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

    let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![MockRotate90Node::boxed()];
    let encode_config = EncodeConfig::default();

    let result = try_lossless_jpeg(&png_data, &nodes, &encode_config, 1, &enough::Unstoppable)
        .expect("should not error");

    assert!(result.is_none(), "should fall back for non-JPEG source");
}

#[test]
fn lossless_fallback_for_non_jpeg_output() {
    let jpeg_data = generate_test_jpeg(64, 48);

    let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![MockRotate90Node::boxed()];
    let encode_config = EncodeConfig {
        format: Some("webp".to_string()),
        ..Default::default()
    };

    let result = try_lossless_jpeg(&jpeg_data, &nodes, &encode_config, 1, &enough::Unstoppable)
        .expect("should not error");

    assert!(
        result.is_none(),
        "should fall back when output format is not JPEG"
    );
}

#[test]
fn lossless_empty_nodes_returns_identity() {
    let jpeg_data = generate_test_jpeg(64, 48);

    let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![];
    let encode_config = EncodeConfig::default();

    let result = try_lossless_jpeg(&jpeg_data, &nodes, &encode_config, 1, &enough::Unstoppable)
        .expect("should not error")
        .expect("empty pipeline should take lossless path (identity)");

    // Identity: output equals input.
    assert_eq!(result.data, jpeg_data);
}
