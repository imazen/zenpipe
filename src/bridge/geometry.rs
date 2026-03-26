//! Geometry fusion: compile a run of adjacent geometry nodes into a single
//! `NodeOp::Layout` using `zenresize::Pipeline`.

use zennode::NodeInstance;

use crate::error::PipeError;
use crate::graph::NodeOp;

use super::parse::{param_i32, param_str, param_u32, parse_constraint_mode, parse_filter_opt};

/// Schema IDs that are geometry operations eligible for layout fusion.
pub(crate) const GEOMETRY_SCHEMA_IDS: &[&str] = &[
    "zenlayout.crop",
    "zenlayout.crop_percent",
    "zenlayout.region",
    "zenlayout.orient",
    "zenlayout.flip_h",
    "zenlayout.flip_v",
    "zenlayout.rotate_90",
    "zenlayout.rotate_180",
    "zenlayout.rotate_270",
    "zenresize.constrain",
    "zenlayout.constrain",
];

/// Check if a schema ID is a geometry operation.
pub(crate) fn is_geometry_node(schema_id: &str) -> bool {
    GEOMETRY_SCHEMA_IDS.contains(&schema_id)
}

/// Compile a run of adjacent geometry nodes into a single `NodeOp::Layout`.
///
/// Feeds the geometry run through `zenresize::Pipeline` to produce a single
/// `LayoutPlan`, then emits `NodeOp::Layout { plan, filter }`. This avoids
/// creating separate Crop, Orient, Resize graph nodes — everything is fused
/// into one streaming pass.
///
/// `source_w` and `source_h` are needed for layout planning but are not
/// always known at compile time (they depend on the upstream source). When
/// not available (0, 0), falls back to individual node conversion.
pub(crate) fn compile_geometry_run(
    nodes: &[&dyn NodeInstance],
    source_w: u32,
    source_h: u32,
) -> Result<NodeOp, PipeError> {
    if nodes.is_empty() {
        return Err(PipeError::Op("empty geometry run".into()));
    }

    // If source dimensions aren't known, fall back (caller handles this).
    if source_w == 0 || source_h == 0 {
        return Err(PipeError::Op(
            "geometry fusion requires source dimensions".into(),
        ));
    }

    let mut pipeline = zenresize::Pipeline::new(source_w, source_h);
    let mut filter: Option<zenresize::Filter> = None;

    for &node in nodes {
        let id = node.schema().id;
        match id {
            "zenlayout.crop" => {
                let x = param_u32(node, "x")?;
                let y = param_u32(node, "y")?;
                let w = param_u32(node, "w")?;
                let h = param_u32(node, "h")?;
                pipeline = pipeline.crop_pixels(x, y, w, h);
            }
            "zenlayout.orient" => {
                let val = param_i32(node, "orientation")?;
                let exif = u8::try_from(val).unwrap_or(1);
                pipeline = pipeline.auto_orient(exif);
            }
            "zenlayout.flip_h" => {
                pipeline = pipeline.flip_h();
            }
            "zenlayout.flip_v" => {
                pipeline = pipeline.flip_v();
            }
            "zenlayout.rotate_90" => {
                pipeline = pipeline.rotate_90();
            }
            "zenlayout.rotate_180" => {
                pipeline = pipeline.rotate_180();
            }
            "zenlayout.rotate_270" => {
                pipeline = pipeline.rotate_270();
            }
            "zenresize.constrain" => {
                let w = param_u32(node, "w")?;
                let h = param_u32(node, "h")?;
                let mode_str = param_str(node, "mode")?;
                let filter_str = param_str(node, "filter")?;
                let mode = parse_constraint_mode(&mode_str)?;
                pipeline = pipeline.constrain(zenresize::Constraint::new(mode, w, h));
                if let Some(f) = parse_filter_opt(&filter_str) {
                    filter = Some(f);
                }
            }
            "zenlayout.constrain" => {
                let w = param_u32(node, "w")?;
                let h = param_u32(node, "h")?;
                let mode_str = param_str(node, "mode")?;
                let mode = parse_constraint_mode(&mode_str)?;
                pipeline = pipeline.constrain(zenresize::Constraint::new(mode, w, h));
            }
            _ => {
                return Err(PipeError::Op(alloc::format!(
                    "unexpected node '{id}' in geometry run"
                )));
            }
        }
    }

    let (ideal, request) = pipeline
        .plan()
        .map_err(|e| PipeError::Op(alloc::format!("geometry fusion plan failed: {e}")))?;
    let offer = zenresize::DecoderOffer::full_decode(source_w, source_h);
    let plan = ideal.finalize(&request, &offer);
    let f = filter.unwrap_or(zenresize::Filter::Robidoux);

    Ok(NodeOp::Layout { plan, filter: f })
}
