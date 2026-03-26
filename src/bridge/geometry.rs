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
                let mode = parse_constraint_mode(&mode_str)?;
                pipeline = pipeline.constrain(zenresize::Constraint::new(mode, w, h));
                // down_filter is optional — absent means auto (Robidoux).
                if let Some(f) = node.get_param("down_filter")
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .and_then(|s| parse_filter_opt(&s))
                {
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
            "zenlayout.crop_percent" => {
                // Percentage-based crop: x1/y1/x2/y2 are fractions of source size.
                let x1 = super::parse::param_f32_opt(node, "x1").unwrap_or(0.0);
                let y1 = super::parse::param_f32_opt(node, "y1").unwrap_or(0.0);
                let x2 = super::parse::param_f32_opt(node, "x2").unwrap_or(100.0);
                let y2 = super::parse::param_f32_opt(node, "y2").unwrap_or(100.0);
                // Convert percentages to pixel coords based on source size.
                let px = (x1 / 100.0 * source_w as f32) as u32;
                let py = (y1 / 100.0 * source_h as f32) as u32;
                let pw = ((x2 - x1) / 100.0 * source_w as f32).max(1.0) as u32;
                let ph = ((y2 - y1) / 100.0 * source_h as f32).max(1.0) as u32;
                pipeline = pipeline.crop_pixels(px, py, pw, ph);
            }
            // zenlayout.region is handled by the RegionConverter (needs ExpandCanvas).
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
