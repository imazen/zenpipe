//! Tests for geometry fusion, filter fusion, alpha elision, and streaming format conversions.
//!
//! Validates that:
//! - Adjacent geometry nodes fuse into a single Layout NodeOp
//! - Filter nodes fuse via NodeConverter::fuse_group()
//! - Format conversions stream (no full-frame materialization)
//! - The pipeline output dimensions and pixel data are correct

#![cfg(feature = "zennode")]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;

use zennode::*;
use zenpipe::bridge::{self, NodeConverter};
use zenpipe::graph::NodeOp;
use zenpipe::sources::MaterializedSource;
use zenpipe::{PipeError, Source, format};

// ═══════════════════════════════════════════════════════════════════════
// Test infrastructure
// ═══════════════════════════════════════════════════════════════════════

fn solid_source(w: u32, h: u32) -> Box<dyn Source> {
    let bpp = format::RGBA8_SRGB.bytes_per_pixel() as usize;
    let data = vec![128u8; w as usize * h as usize * bpp];
    Box::new(MaterializedSource::from_data(
        data,
        w,
        h,
        format::RGBA8_SRGB,
    ))
}

fn make_node(schema: &'static NodeSchema, params: ParamMap) -> Box<dyn NodeInstance> {
    Box::new(TestNode { schema, params })
}

struct TestNode {
    schema: &'static NodeSchema,
    params: ParamMap,
}

impl NodeInstance for TestNode {
    fn schema(&self) -> &'static NodeSchema {
        self.schema
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
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
    fn clone_boxed(&self) -> Box<dyn NodeInstance> {
        Box::new(TestNode {
            schema: self.schema,
            params: self.params.clone(),
        })
    }
}

// ─── Schemas ───

static CROP_SCHEMA: NodeSchema = NodeSchema {
    id: "zenlayout.crop",
    label: "Crop",
    description: "",
    group: NodeGroup::Geometry,
    role: NodeRole::Geometry,
    params: &[],
    tags: &[],
    coalesce: Some(CoalesceInfo {
        group: "layout_plan",
        fusable: true,
        is_target: false,
    }),
    format: FormatHint {
        preferred: PixelFormatPreference::Any,
        alpha: AlphaHandling::Process,
        changes_dimensions: true,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    inputs: &[],
    deny_unknown_fields: false,
};

static ORIENT_SCHEMA: NodeSchema = NodeSchema {
    id: "zenlayout.orient",
    label: "Orient",
    description: "",
    group: NodeGroup::Geometry,
    role: NodeRole::Orient,
    params: &[],
    tags: &[],
    coalesce: Some(CoalesceInfo {
        group: "layout_plan",
        fusable: true,
        is_target: false,
    }),
    format: FormatHint {
        preferred: PixelFormatPreference::Any,
        alpha: AlphaHandling::Process,
        changes_dimensions: false,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    inputs: &[],
    deny_unknown_fields: false,
};

static FLIP_H_SCHEMA: NodeSchema = NodeSchema {
    id: "zenlayout.flip_h",
    label: "Flip H",
    description: "",
    group: NodeGroup::Geometry,
    role: NodeRole::Orient,
    params: &[],
    tags: &[],
    coalesce: Some(CoalesceInfo {
        group: "layout_plan",
        fusable: true,
        is_target: false,
    }),
    format: FormatHint {
        preferred: PixelFormatPreference::Any,
        alpha: AlphaHandling::Process,
        changes_dimensions: false,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    inputs: &[],
    deny_unknown_fields: false,
};

static FLIP_V_SCHEMA: NodeSchema = NodeSchema {
    id: "zenlayout.flip_v",
    label: "Flip V",
    description: "",
    group: NodeGroup::Geometry,
    role: NodeRole::Orient,
    params: &[],
    tags: &[],
    coalesce: Some(CoalesceInfo {
        group: "layout_plan",
        fusable: true,
        is_target: false,
    }),
    format: FormatHint {
        preferred: PixelFormatPreference::Any,
        alpha: AlphaHandling::Process,
        changes_dimensions: false,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    inputs: &[],
    deny_unknown_fields: false,
};

static ROT90_SCHEMA: NodeSchema = NodeSchema {
    id: "zenlayout.rotate_90",
    label: "Rotate 90",
    description: "",
    group: NodeGroup::Geometry,
    role: NodeRole::Orient,
    params: &[],
    tags: &[],
    coalesce: Some(CoalesceInfo {
        group: "layout_plan",
        fusable: true,
        is_target: false,
    }),
    format: FormatHint {
        preferred: PixelFormatPreference::Any,
        alpha: AlphaHandling::Process,
        changes_dimensions: true,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    inputs: &[],
    deny_unknown_fields: false,
};

static ROT180_SCHEMA: NodeSchema = NodeSchema {
    id: "zenlayout.rotate_180",
    label: "Rotate 180",
    description: "",
    group: NodeGroup::Geometry,
    role: NodeRole::Orient,
    params: &[],
    tags: &[],
    coalesce: Some(CoalesceInfo {
        group: "layout_plan",
        fusable: true,
        is_target: false,
    }),
    format: FormatHint {
        preferred: PixelFormatPreference::Any,
        alpha: AlphaHandling::Process,
        changes_dimensions: false,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    inputs: &[],
    deny_unknown_fields: false,
};

static ROT270_SCHEMA: NodeSchema = NodeSchema {
    id: "zenlayout.rotate_270",
    label: "Rotate 270",
    description: "",
    group: NodeGroup::Geometry,
    role: NodeRole::Orient,
    params: &[],
    tags: &[],
    coalesce: Some(CoalesceInfo {
        group: "layout_plan",
        fusable: true,
        is_target: false,
    }),
    format: FormatHint {
        preferred: PixelFormatPreference::Any,
        alpha: AlphaHandling::Process,
        changes_dimensions: true,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    inputs: &[],
    deny_unknown_fields: false,
};

static CONSTRAIN_SCHEMA: NodeSchema = NodeSchema {
    id: "zenresize.constrain",
    label: "Constrain",
    description: "",
    group: NodeGroup::Geometry,
    role: NodeRole::Resize,
    params: &[],
    tags: &[],
    coalesce: Some(CoalesceInfo {
        group: "layout_plan",
        fusable: true,
        is_target: false,
    }),
    format: FormatHint {
        preferred: PixelFormatPreference::LinearF32,
        alpha: AlphaHandling::Process,
        changes_dimensions: true,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    inputs: &[],
    deny_unknown_fields: false,
};

static FILTER_SCHEMA: NodeSchema = NodeSchema {
    id: "zenfilters.exposure",
    label: "Exposure",
    description: "",
    group: NodeGroup::Tone,
    role: NodeRole::Filter,
    params: &[],
    tags: &[],
    coalesce: Some(CoalesceInfo {
        group: "fused_adjust",
        fusable: true,
        is_target: false,
    }),
    format: FormatHint {
        preferred: PixelFormatPreference::OklabF32,
        alpha: AlphaHandling::Skip,
        changes_dimensions: false,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    inputs: &[],
    deny_unknown_fields: false,
};

static FILTER2_SCHEMA: NodeSchema = NodeSchema {
    id: "zenfilters.contrast",
    label: "Contrast",
    description: "",
    group: NodeGroup::Tone,
    role: NodeRole::Filter,
    params: &[],
    tags: &[],
    coalesce: Some(CoalesceInfo {
        group: "fused_adjust",
        fusable: true,
        is_target: false,
    }),
    format: FormatHint {
        preferred: PixelFormatPreference::OklabF32,
        alpha: AlphaHandling::Skip,
        changes_dimensions: false,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    inputs: &[],
    deny_unknown_fields: false,
};

fn constrain_params(w: u32, h: u32, mode: &str) -> ParamMap {
    let mut p = ParamMap::new();
    p.insert("w".into(), ParamValue::U32(w));
    p.insert("h".into(), ParamValue::U32(h));
    p.insert("mode".into(), ParamValue::Str(mode.into()));
    p.insert("filter".into(), ParamValue::Str(String::new()));
    p.insert("gravity".into(), ParamValue::Str(String::new()));
    p.insert("sharpen".into(), ParamValue::F32(0.0));
    p.insert("bgcolor".into(), ParamValue::Str(String::new()));
    p
}

fn crop_params(x: u32, y: u32, w: u32, h: u32) -> ParamMap {
    let mut p = ParamMap::new();
    p.insert("x".into(), ParamValue::U32(x));
    p.insert("y".into(), ParamValue::U32(y));
    p.insert("w".into(), ParamValue::U32(w));
    p.insert("h".into(), ParamValue::U32(h));
    p
}

fn orient_params(exif: i32) -> ParamMap {
    let mut p = ParamMap::new();
    p.insert("orientation".into(), ParamValue::I32(exif));
    p
}

// ═══════════════════════════════════════════════════════════════════════
// Geometry fusion tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn fuse_crop_then_constrain() {
    let source = solid_source(800, 600);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![
        make_node(&CROP_SCHEMA, crop_params(100, 50, 600, 500)),
        make_node(&CONSTRAIN_SCHEMA, constrain_params(300, 250, "fit")),
    ];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();
    assert_eq!(result.source.width(), 300);
    assert_eq!(result.source.height(), 250);
}

#[test]
fn fuse_orient_then_constrain() {
    let source = solid_source(800, 600);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![
        make_node(&ORIENT_SCHEMA, orient_params(6)), // rotate 90
        make_node(&CONSTRAIN_SCHEMA, constrain_params(200, 200, "fit")),
    ];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();
    // After rotate 90, source is 600x800 → fit 200x200 → 150x200
    let w = result.source.width();
    let h = result.source.height();
    assert!(w <= 200 && h <= 200, "expected within 200x200, got {w}x{h}");
}

#[test]
fn fuse_flip_h_flip_v_constrain() {
    let source = solid_source(400, 300);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![
        make_node(&FLIP_H_SCHEMA, ParamMap::new()),
        make_node(&FLIP_V_SCHEMA, ParamMap::new()),
        make_node(&CONSTRAIN_SCHEMA, constrain_params(200, 150, "fit")),
    ];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();
    assert_eq!(result.source.width(), 200);
    assert_eq!(result.source.height(), 150);
}

#[test]
fn fuse_rotate90_with_constrain() {
    let source = solid_source(400, 300);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![
        make_node(&ROT90_SCHEMA, ParamMap::new()),
        make_node(&CONSTRAIN_SCHEMA, constrain_params(100, 100, "fit")),
    ];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();
    let w = result.source.width();
    let h = result.source.height();
    assert!(w <= 100 && h <= 100, "got {w}x{h}");
}

#[test]
fn fuse_rotate180_with_crop() {
    let source = solid_source(400, 300);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![
        make_node(&ROT180_SCHEMA, ParamMap::new()),
        make_node(&CROP_SCHEMA, crop_params(50, 50, 200, 100)),
    ];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();
    assert_eq!(result.source.width(), 200);
    assert_eq!(result.source.height(), 100);
}

#[test]
fn fuse_rotate270_with_constrain() {
    let source = solid_source(400, 300);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![
        make_node(&ROT270_SCHEMA, ParamMap::new()),
        make_node(&CONSTRAIN_SCHEMA, constrain_params(150, 150, "within")),
    ];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();
    let w = result.source.width();
    let h = result.source.height();
    assert!(w <= 150 && h <= 150, "got {w}x{h}");
}

#[test]
fn fuse_crop_orient_constrain_three_way() {
    let source = solid_source(1000, 800);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![
        make_node(&CROP_SCHEMA, crop_params(100, 100, 800, 600)),
        make_node(&ORIENT_SCHEMA, orient_params(3)), // rotate 180
        make_node(&CONSTRAIN_SCHEMA, constrain_params(400, 300, "fit")),
    ];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();
    assert_eq!(result.source.width(), 400);
    assert_eq!(result.source.height(), 300);
}

#[test]
fn single_constrain_fuses() {
    let source = solid_source(1000, 800);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![make_node(
        &CONSTRAIN_SCHEMA,
        constrain_params(500, 400, "fit"),
    )];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();
    assert_eq!(result.source.width(), 500);
    assert_eq!(result.source.height(), 400);
}

// ═══════════════════════════════════════════════════════════════════════
// Filter fusion tests
// ═══════════════════════════════════════════════════════════════════════

/// Test converter that claims filter nodes and fuses groups into a single PixelTransform.
struct TestFilterConverter;

struct IdentityOp;
impl zenpipe::ops::PixelOp for IdentityOp {
    fn apply(&mut self, input: &[u8], output: &mut [u8], _width: u32, _height: u32) {
        output[..input.len()].copy_from_slice(input);
    }
    fn input_format(&self) -> zenpipe::PixelFormat {
        format::RGBA8_SRGB
    }
    fn output_format(&self) -> zenpipe::PixelFormat {
        format::RGBA8_SRGB
    }
}

impl NodeConverter for TestFilterConverter {
    fn can_convert(&self, schema_id: &str) -> bool {
        schema_id.starts_with("zenfilters.")
    }
    fn convert(&self, _node: &dyn NodeInstance) -> Result<NodeOp, PipeError> {
        Ok(NodeOp::PixelTransform(Box::new(IdentityOp)))
    }
    fn convert_group(&self, _nodes: &[&dyn NodeInstance]) -> Result<NodeOp, PipeError> {
        Ok(NodeOp::PixelTransform(Box::new(IdentityOp)))
    }
    fn fuse_group(&self, nodes: &[&dyn NodeInstance]) -> Result<Option<NodeOp>, PipeError> {
        if nodes.len() >= 2 {
            // Simulate FusedAdjust: multiple filter nodes → single PixelTransform
            Ok(Some(NodeOp::PixelTransform(Box::new(IdentityOp))))
        } else {
            Ok(None) // single node, don't fuse
        }
    }
}

#[test]
fn adjacent_filters_fuse_via_converter() {
    let source = solid_source(100, 100);
    let converters: Vec<&dyn NodeConverter> = vec![&TestFilterConverter];
    let nodes: Vec<Box<dyn NodeInstance>> = vec![
        make_node(&FILTER_SCHEMA, ParamMap::new()),
        make_node(&FILTER2_SCHEMA, ParamMap::new()),
    ];
    let result = bridge::build_pipeline(source, &nodes, &converters).unwrap();
    // Both filters fused into one op — output should still be 100x100
    assert_eq!(result.source.width(), 100);
    assert_eq!(result.source.height(), 100);
}

#[test]
fn single_filter_not_fused_falls_back_to_convert() {
    let source = solid_source(100, 100);
    let converters: Vec<&dyn NodeConverter> = vec![&TestFilterConverter];
    let nodes: Vec<Box<dyn NodeInstance>> = vec![make_node(&FILTER_SCHEMA, ParamMap::new())];
    let result = bridge::build_pipeline(source, &nodes, &converters).unwrap();
    assert_eq!(result.source.width(), 100);
}

#[test]
fn geometry_then_filters_both_fuse() {
    let source = solid_source(800, 600);
    let converters: Vec<&dyn NodeConverter> = vec![&TestFilterConverter];
    let nodes: Vec<Box<dyn NodeInstance>> = vec![
        make_node(&CROP_SCHEMA, crop_params(0, 0, 400, 300)),
        make_node(&CONSTRAIN_SCHEMA, constrain_params(200, 150, "fit")),
        make_node(&FILTER_SCHEMA, ParamMap::new()),
        make_node(&FILTER2_SCHEMA, ParamMap::new()),
    ];
    let result = bridge::build_pipeline(source, &nodes, &converters).unwrap();
    // Geometry fused: crop + constrain → Layout
    // Filters fused: exposure + contrast → single PixelTransform
    assert_eq!(result.source.width(), 200);
    assert_eq!(result.source.height(), 150);
}

#[test]
fn filter_between_geometry_breaks_fusion() {
    let source = solid_source(800, 600);
    let converters: Vec<&dyn NodeConverter> = vec![&TestFilterConverter];
    let nodes: Vec<Box<dyn NodeInstance>> = vec![
        make_node(&CONSTRAIN_SCHEMA, constrain_params(400, 300, "fit")),
        make_node(&FILTER_SCHEMA, ParamMap::new()),
        make_node(&CROP_SCHEMA, crop_params(0, 0, 200, 150)),
    ];
    let result = bridge::build_pipeline(source, &nodes, &converters).unwrap();
    // Constrain runs first (400x300), filter runs, then crop to 200x150
    assert_eq!(result.source.width(), 200);
    assert_eq!(result.source.height(), 150);
}

// ═══════════════════════════════════════════════════════════════════════
// Streaming tests — verify the pipeline streams without materializing
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn pipeline_streams_all_strips() {
    let source = solid_source(200, 200);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![make_node(
        &CONSTRAIN_SCHEMA,
        constrain_params(100, 100, "fit"),
    )];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();

    // Pull strips one at a time — this IS streaming
    let mut src = result.source;
    let mut total_rows = 0u32;
    while let Some(strip) = src.next().unwrap() {
        total_rows += strip.rows();
        assert_eq!(strip.width(), 100);
    }
    assert_eq!(total_rows, 100);
}

#[test]
fn format_conversion_streams() {
    // Source is RGBA8_SRGB. Resize needs linear. Conversion should stream, not materialize.
    let source = solid_source(400, 300);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![make_node(
        &CONSTRAIN_SCHEMA,
        constrain_params(200, 150, "fit"),
    )];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();

    let mut src = result.source;
    let mut strips = 0;
    while let Some(_strip) = src.next().unwrap() {
        strips += 1;
    }
    // Should produce multiple strips (streaming), not 1 (materialized)
    assert!(
        strips >= 2,
        "expected streaming (multiple strips), got {strips}"
    );
}

#[test]
fn passthrough_streams() {
    let source = solid_source(100, 100);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();

    let mut src = result.source;
    let mut total_rows = 0u32;
    while let Some(strip) = src.next().unwrap() {
        total_rows += strip.rows();
    }
    assert_eq!(total_rows, 100);
}

// ═══════════════════════════════════════════════════════════════════════
// Materialize tests — verify materialize() works for when you need it
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn materialize_after_fusion() {
    let source = solid_source(400, 300);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![
        make_node(&CROP_SCHEMA, crop_params(0, 0, 200, 150)),
        make_node(&CONSTRAIN_SCHEMA, constrain_params(100, 75, "fit")),
    ];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();
    let mat = result.materialize().unwrap();
    assert_eq!(mat.pixels.width(), 100);
    assert_eq!(mat.pixels.height(), 75);
    assert!(!mat.pixels.data().is_empty());
}

// ═══════════════════════════════════════════════════════════════════════
// Rotate-after-constrain regression tests (imazen/zenpipe#14)
//
// When rotate_90 follows a constrain, the bridge must swap dimensions
// correctly. Previously the coalesced layout used original source
// dimensions instead of the constrain output.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn rotate90_after_constrain_swaps_dimensions() {
    // 600x450 landscape → constrain within 70x70 → 70x53 → rotate90 → 53x70
    let source = solid_source(600, 450);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![
        make_node(&CONSTRAIN_SCHEMA, constrain_params(70, 70, "within")),
        make_node(&ROT90_SCHEMA, ParamMap::new()),
    ];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();
    assert_eq!(result.source.width(), 53, "after constrain+rot90: width should be 53");
    assert_eq!(result.source.height(), 70, "after constrain+rot90: height should be 70");
}

#[test]
fn rotate270_after_constrain_swaps_dimensions() {
    // 600x450 → constrain within 70x70 → 70x53 → rotate270 → 53x70
    let source = solid_source(600, 450);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![
        make_node(&CONSTRAIN_SCHEMA, constrain_params(70, 70, "within")),
        make_node(&ROT270_SCHEMA, ParamMap::new()),
    ];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();
    assert_eq!(result.source.width(), 53, "after constrain+rot270: width should be 53");
    assert_eq!(result.source.height(), 70, "after constrain+rot270: height should be 70");
}

#[test]
fn rotate180_after_constrain_preserves_dimensions() {
    // 600x450 → constrain within 70x70 → 70x53 → rotate180 → 70x53 (no swap)
    let source = solid_source(600, 450);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![
        make_node(&CONSTRAIN_SCHEMA, constrain_params(70, 70, "within")),
        make_node(&ROT180_SCHEMA, ParamMap::new()),
    ];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();
    assert_eq!(result.source.width(), 70, "after constrain+rot180: width should be 70");
    assert_eq!(result.source.height(), 53, "after constrain+rot180: height should be 53");
}

#[test]
fn rotate90_standalone_swaps_dimensions() {
    // 100x60 → rotate90 → 60x100
    let source = solid_source(100, 60);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![
        make_node(&ROT90_SCHEMA, ParamMap::new()),
    ];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();
    assert_eq!(result.source.width(), 60, "standalone rot90: width should be 60");
    assert_eq!(result.source.height(), 100, "standalone rot90: height should be 100");
}

#[test]
fn flip_h_after_constrain_preserves_dimensions() {
    // 600x450 → constrain within 70x70 → 70x53 → flip_h → 70x53
    let source = solid_source(600, 450);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![
        make_node(&CONSTRAIN_SCHEMA, constrain_params(70, 70, "within")),
        make_node(&FLIP_H_SCHEMA, ParamMap::new()),
    ];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();
    assert_eq!(result.source.width(), 70, "after constrain+flip_h: width should be 70");
    assert_eq!(result.source.height(), 53, "after constrain+flip_h: height should be 53");
}

#[test]
fn orient_exif6_after_constrain_swaps_dimensions() {
    // EXIF 6 = Rotate90. 600x450 → constrain within 70x70 → 70x53 → orient(6) → 53x70
    let source = solid_source(600, 450);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![
        make_node(&CONSTRAIN_SCHEMA, constrain_params(70, 70, "within")),
        make_node(&ORIENT_SCHEMA, orient_params(6)),
    ];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();
    assert_eq!(result.source.width(), 53, "after constrain+orient(6): width should be 53");
    assert_eq!(result.source.height(), 70, "after constrain+orient(6): height should be 70");
}

#[test]
fn constrain_after_rotate90_uses_rotated_dimensions() {
    // 600x450 → rotate90 → 450x600 → constrain within 70x70 → 53x70
    let source = solid_source(600, 450);
    let nodes: Vec<Box<dyn NodeInstance>> = vec![
        make_node(&ROT90_SCHEMA, ParamMap::new()),
        make_node(&CONSTRAIN_SCHEMA, constrain_params(70, 70, "within")),
    ];
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();
    assert_eq!(result.source.width(), 53, "after rot90+constrain: width should be 53");
    assert_eq!(result.source.height(), 70, "after rot90+constrain: height should be 70");
}
