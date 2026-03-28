//! Node-to-NodeOp conversion: coalescing, step conversion, and built-in converters.

use alloc::vec::Vec;

use zennode::NodeInstance;

use alloc::boxed::Box;
use zennode::NodeRole;

use crate::error::PipeError;
use crate::graph::{EdgeKind, NodeId, NodeOp, PipelineGraph};

use super::geometry::{compile_geometry_run, is_geometry_node};
use super::parse::{
    param_f32_opt, param_i32, param_str, param_u32, parse_constraint_mode, parse_filter_opt,
};
use super::{NodeConverter, PipelineStep};

// ─── Shared helpers ───

/// Nodes separated by role into pixel-processing, encode, and decode groups.
pub(crate) struct SeparatedNodes<'a> {
    pub pixel: Vec<&'a dyn NodeInstance>,
    pub encode: Vec<Box<dyn NodeInstance>>,
    pub decode: Vec<Box<dyn NodeInstance>>,
}

/// Separate nodes by role into pixel-processing, encode, and decode groups.
///
/// Encode and decode nodes are cloned into owned boxes; pixel-processing
/// nodes are borrowed from the input.
pub(crate) fn separate_by_role<'a>(
    nodes: impl Iterator<Item = &'a dyn NodeInstance>,
) -> SeparatedNodes<'a> {
    let mut pixel = Vec::new();
    let mut encode = Vec::new();
    let mut decode = Vec::new();
    for node in nodes {
        match node.schema().role {
            NodeRole::Encode => encode.push(node.clone_boxed()),
            NodeRole::Decode => decode.push(node.clone_boxed()),
            _ => pixel.push(node),
        }
    }
    SeparatedNodes {
        pixel,
        encode,
        decode,
    }
}

/// Coalesce and convert nodes into graph nodes wired as a linear chain.
///
/// Calls [`coalesce`] then [`convert_step`] for each step, adding nodes
/// to the graph and wiring them sequentially. Returns `(first, last)` node
/// IDs, or `None` if no nodes were produced.
pub(crate) fn coalesce_and_append_chain(
    pixel_nodes: &[&dyn NodeInstance],
    converters: &[&dyn NodeConverter],
    source_w: u32,
    source_h: u32,
    graph: &mut PipelineGraph,
) -> Result<Option<(NodeId, NodeId)>, PipeError> {
    let steps = coalesce(pixel_nodes);
    let mut first_id = None;
    let mut prev_id = None;

    for step in &steps {
        let ops = convert_step(step, converters, source_w, source_h)?;
        for node_op in ops {
            let gid = graph.add_node(node_op);
            if let Some(prev) = prev_id {
                graph.add_edge(prev, gid, EdgeKind::Input);
            }
            if first_id.is_none() {
                first_id = Some(gid);
            }
            prev_id = Some(gid);
        }
    }

    match (first_id, prev_id) {
        (Some(f), Some(l)) => Ok(Some((f, l))),
        _ => Ok(None),
    }
}

// ─── Coalescing ───

/// Group adjacent fusable nodes that share the same coalesce group.
///
/// Non-fusable nodes pass through as `Single` steps. Adjacent fusable nodes
/// with the same group name are merged into `Coalesced` steps.
pub(crate) fn coalesce<'a>(nodes: &[&'a dyn NodeInstance]) -> Vec<PipelineStep<'a>> {
    let mut steps: Vec<PipelineStep<'a>> = Vec::new();

    for &node in nodes {
        let coalesce = node.schema().coalesce.as_ref();

        if let Some(info) = coalesce {
            if info.fusable || info.is_target {
                // Try to merge with the previous step if same group.
                if let Some(PipelineStep::Coalesced {
                    group,
                    nodes: group_nodes,
                }) = steps.last_mut()
                {
                    if *group == info.group {
                        group_nodes.push(node);
                        continue;
                    }
                }
                // Start a new coalesced group.
                steps.push(PipelineStep::Coalesced {
                    group: info.group,
                    nodes: alloc::vec![node],
                });
                continue;
            }
        }

        // Not fusable — emit as a single step.
        steps.push(PipelineStep::Single(node));
    }

    steps
}

// ─── Step conversion ───

/// Convert a pipeline step to one or more `NodeOp`s, with geometry and filter fusion.
///
/// For coalesced groups where all nodes are geometry nodes, attempts
/// to fuse them via [`compile_geometry_run()`]. For groups handled by
/// a converter with `fuse_group()`, delegates to that. Falls back to
/// individual conversion for anything else.
///
/// May return multiple `NodeOp`s (e.g., when a coalesced group has mixed
/// geometry and non-geometry nodes that can't be fused together).
pub(crate) fn convert_step(
    step: &PipelineStep<'_>,
    converters: &[&dyn NodeConverter],
    source_w: u32,
    source_h: u32,
) -> Result<Vec<NodeOp>, PipeError> {
    match step {
        PipelineStep::Single(node) => {
            let schema_id = node.schema().id;

            // Single geometry node: if source dims are known, fuse it.
            if is_geometry_node(schema_id) && source_w > 0 && source_h > 0 {
                let nodes = &[*node];
                if let Ok(op) = compile_geometry_run(nodes, source_w, source_h) {
                    return Ok(alloc::vec![op]);
                }
            }

            Ok(alloc::vec![convert_single(*node, converters)?])
        }

        PipelineStep::Coalesced { nodes, .. } => {
            // Check if all nodes in the group are geometry nodes.
            let all_geometry = nodes.iter().all(|n| is_geometry_node(n.schema().id));

            if all_geometry && source_w > 0 && source_h > 0 {
                if let Ok(op) = compile_geometry_run(nodes, source_w, source_h) {
                    return Ok(alloc::vec![op]);
                }
            }

            // Try converter fusion (fuse_group).
            let all_same_converter = converters
                .iter()
                .any(|conv| nodes.iter().all(|n| conv.can_convert(n.schema().id)));
            if all_same_converter {
                for conv in converters {
                    if nodes.iter().all(|n| conv.can_convert(n.schema().id)) {
                        if let Some(fused) = conv.fuse_group(nodes)? {
                            return Ok(alloc::vec![fused]);
                        }
                        return Ok(alloc::vec![conv.convert_group(nodes)?]);
                    }
                }
            }

            // Mixed group or no fusion — convert each node individually.
            let mut ops = Vec::new();
            for &node in nodes {
                ops.push(convert_single(node, converters)?);
            }
            Ok(ops)
        }
    }
}

/// Convert a single node to a `NodeOp`.
pub(crate) fn convert_single(
    node: &dyn NodeInstance,
    converters: &[&dyn NodeConverter],
) -> Result<NodeOp, PipeError> {
    let schema_id = node.schema().id;

    // Try extension converters first.
    for conv in converters {
        if conv.can_convert(schema_id) {
            return conv.convert(node);
        }
    }

    // Built-in conversions for geometry/layout nodes.
    match schema_id {
        "zenlayout.crop" => convert_crop(node),
        "zenlayout.orient" => convert_orient(node),
        "zenlayout.flip_h" => Ok(NodeOp::Orient(zenresize::Orientation::FlipH)),
        "zenlayout.flip_v" => Ok(NodeOp::Orient(zenresize::Orientation::FlipV)),
        "zenlayout.rotate_90" => Ok(NodeOp::Orient(zenresize::Orientation::Rotate90)),
        "zenlayout.rotate_180" => Ok(NodeOp::Orient(zenresize::Orientation::Rotate180)),
        "zenlayout.rotate_270" => Ok(NodeOp::Orient(zenresize::Orientation::Rotate270)),
        "zenresize.constrain" => convert_zenresize_constrain(node),

        // zenpipe native nodes + zenresize.resize
        "zenpipe.crop_whitespace" => convert_crop_whitespace(node),
        "zenpipe.fill_rect" => convert_fill_rect(node),
        "zenpipe.remove_alpha" => convert_remove_alpha(node),
        "zenpipe.round_corners" => convert_round_corners(node),
        "zenresize.resize" => convert_resize(node),

        _ => Err(PipeError::Op(alloc::format!(
            "bridge: no converter for node '{schema_id}'"
        ))),
    }
}

// ─── Built-in converters ───

/// Convert a `zenlayout.crop` node to `NodeOp::Crop`.
pub(crate) fn convert_crop(node: &dyn NodeInstance) -> Result<NodeOp, PipeError> {
    let x = param_u32(node, "x")?;
    let y = param_u32(node, "y")?;
    let w = param_u32(node, "w")?;
    let h = param_u32(node, "h")?;
    Ok(NodeOp::Crop { x, y, w, h })
}

/// Convert a `zenlayout.orient` node to `NodeOp::AutoOrient`.
pub(crate) fn convert_orient(node: &dyn NodeInstance) -> Result<NodeOp, PipeError> {
    let orientation = param_i32(node, "orientation")?;
    // Clamp to u8 range — AutoOrient handles values outside 1-8 as identity.
    let value = u8::try_from(orientation).unwrap_or(1);
    Ok(NodeOp::AutoOrient(value))
}

/// Convert a `zenresize.constrain` node to `NodeOp::Constrain`.
pub(crate) fn convert_zenresize_constrain(node: &dyn NodeInstance) -> Result<NodeOp, PipeError> {
    let w = param_u32(node, "w")?;
    let h = param_u32(node, "h")?;
    let mode_str = param_str(node, "mode")?;
    // down_filter is Option<String> — absent means auto (Robidoux).
    let filter = node
        .get_param("down_filter")
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .and_then(|s| parse_filter_opt(&s));
    let _sharpen = param_f32_opt(node, "sharpen_percent");

    let mode = parse_constraint_mode(&mode_str)?;

    Ok(NodeOp::Constrain {
        mode,
        w,
        h,
        orientation: None,
        filter,
    })
}

// ─── zenpipe native node converters ───

/// Convert a `zenpipe.crop_whitespace` node to `NodeOp::CropWhitespace`.
pub(crate) fn convert_crop_whitespace(node: &dyn NodeInstance) -> Result<NodeOp, PipeError> {
    let threshold = param_u32(node, "threshold").unwrap_or(80) as u8;
    let percent_padding = param_f32_opt(node, "percent_padding").unwrap_or(0.0);
    Ok(NodeOp::CropWhitespace {
        threshold,
        percent_padding,
    })
}

/// Convert a `zenpipe.fill_rect` node to `NodeOp::FillRect`.
pub(crate) fn convert_fill_rect(node: &dyn NodeInstance) -> Result<NodeOp, PipeError> {
    let x1 = param_u32(node, "x1")?;
    let y1 = param_u32(node, "y1")?;
    let x2 = param_u32(node, "x2")?;
    let y2 = param_u32(node, "y2")?;
    let color = [
        param_u32(node, "color_r").unwrap_or(0) as u8,
        param_u32(node, "color_g").unwrap_or(0) as u8,
        param_u32(node, "color_b").unwrap_or(0) as u8,
        param_u32(node, "color_a").unwrap_or(255) as u8,
    ];
    Ok(NodeOp::FillRect {
        x1,
        y1,
        x2,
        y2,
        color,
    })
}

/// Convert a `zenpipe.remove_alpha` node to `NodeOp::RemoveAlpha`.
pub(crate) fn convert_remove_alpha(node: &dyn NodeInstance) -> Result<NodeOp, PipeError> {
    let matte = [
        param_u32(node, "matte_r").unwrap_or(255) as u8,
        param_u32(node, "matte_g").unwrap_or(255) as u8,
        param_u32(node, "matte_b").unwrap_or(255) as u8,
    ];
    Ok(NodeOp::RemoveAlpha { matte })
}

/// Convert a `zenpipe.round_corners` node to `NodeOp::Materialize` with mask application.
///
/// Implements the full imageflow V2 round corners algorithm with:
/// - Quadrant-based rendering for correct Circle mode on non-square canvases
/// - Per-corner radii (PixelsCustom / PercentageCustom)
/// - Volumetric offset anti-aliasing
/// - Gamma-correct (linear) blending
pub(crate) fn convert_round_corners(node: &dyn NodeInstance) -> Result<NodeOp, PipeError> {
    let radius = param_f32_opt(node, "radius").unwrap_or(0.0);
    let radius_tl = param_f32_opt(node, "radius_tl").unwrap_or(-1.0);
    let radius_tr = param_f32_opt(node, "radius_tr").unwrap_or(-1.0);
    let radius_bl = param_f32_opt(node, "radius_bl").unwrap_or(-1.0);
    let radius_br = param_f32_opt(node, "radius_br").unwrap_or(-1.0);
    let mode = param_str(node, "mode").unwrap_or_else(|_| alloc::string::String::from("pixels"));
    let bg = [
        param_u32(node, "bg_r").unwrap_or(0) as u8,
        param_u32(node, "bg_g").unwrap_or(0) as u8,
        param_u32(node, "bg_b").unwrap_or(0) as u8,
        param_u32(node, "bg_a").unwrap_or(0) as u8,
    ];

    Ok(NodeOp::Materialize {
        label: "round_corners",
        transform: Box::new(move |data, w, h, fmt| {
            let width = *w;
            let height = *h;
            let bpp = fmt.bytes_per_pixel();
            if width == 0 || height == 0 {
                return;
            }

            let smallest = width.min(height) as f32;

            // Resolve per-corner radii from mode + parameters.
            // Returns [top_left, top_right, bottom_left, bottom_right] clamped to valid range.
            let has_custom =
                radius_tl >= 0.0 || radius_tr >= 0.0 || radius_bl >= 0.0 || radius_br >= 0.0;
            let is_circle = mode == "circle";
            let is_percentage = mode == "percentage" || mode == "percentage_custom";

            let corner_radii: [f32; 4] = if is_circle {
                let r = smallest / 2.0;
                [r, r, r, r]
            } else if has_custom {
                let tl = if radius_tl >= 0.0 { radius_tl } else { radius };
                let tr = if radius_tr >= 0.0 { radius_tr } else { radius };
                let bl = if radius_bl >= 0.0 { radius_bl } else { radius };
                let br = if radius_br >= 0.0 { radius_br } else { radius };
                if is_percentage {
                    [
                        smallest * tl.clamp(0.0, 100.0) / 200.0,
                        smallest * tr.clamp(0.0, 100.0) / 200.0,
                        smallest * bl.clamp(0.0, 100.0) / 200.0,
                        smallest * br.clamp(0.0, 100.0) / 200.0,
                    ]
                } else {
                    let half = smallest / 2.0;
                    [
                        tl.clamp(0.0, half),
                        tr.clamp(0.0, half),
                        bl.clamp(0.0, half),
                        br.clamp(0.0, half),
                    ]
                }
            } else if is_percentage {
                let r = smallest * radius.clamp(0.0, 100.0) / 200.0;
                [r, r, r, r]
            } else {
                let r = radius.clamp(0.0, smallest / 2.0);
                [r, r, r, r]
            };

            // Check if all radii are zero — nothing to do.
            if corner_radii.iter().all(|&r| r <= 0.0) {
                return;
            }

            // For circle mode on non-square canvases, compute offsets to center the circle.
            // This matches V2's plan_quadrants Circle logic: the quadrants are offset so the
            // circular region is centered in the larger dimension.
            let (offset_x, offset_y) = if is_circle {
                let ox = ((width as i64 - height as i64).max(0) / 2) as u32;
                let oy = ((height as i64 - width as i64).max(0) / 2) as u32;
                (ox, oy)
            } else {
                (0, 0)
            };

            // The effective canvas for quadrant computation (smallest dimension for circle).
            let eff_w = if is_circle { smallest as u32 } else { width };
            let eff_h = if is_circle { smallest as u32 } else { height };

            // Integer division to avoid quadrant overlap on odd dimensions (matches V2).
            let right_half_width = eff_w / 2;
            let bottom_half_height = eff_h / 2;
            let left_half_width = eff_w - right_half_width;
            let top_half_height = eff_h - bottom_half_height;

            // Volumetric offset: radius of a circle with the surface area of a 1x1 square.
            let volumetric_offset: f32 = 0.56419; // sqrt(1/pi)

            // Pre-convert bg color to linear space for gamma-correct blending.
            let bg_linear: [f32; 4] = [
                srgb_byte_to_linear(bg[0]),
                srgb_byte_to_linear(bg[1]),
                srgb_byte_to_linear(bg[2]),
                bg[3] as f32 / 255.0,
            ];

            // Fill a rectangular region with the background color.
            let fill_rect = |data: &mut [u8], x1: u32, y1: u32, x2: u32, y2: u32| {
                let x1 = x1 as usize;
                let y1 = y1 as usize;
                let x2 = (x2 as usize).min(width as usize);
                let y2 = (y2 as usize).min(height as usize);
                for y in y1..y2 {
                    let row_start = y * width as usize * bpp;
                    for x in x1..x2 {
                        let off = row_start + x * bpp;
                        for c in 0..bpp.min(4) {
                            data[off + c] = bg[c];
                        }
                    }
                }
            };

            // For circle mode: clear the top/bottom/left/right strips outside the circle region.
            if is_circle {
                // Clear top rows (above the circle region).
                if offset_y > 0 {
                    fill_rect(data, 0, 0, width, offset_y);
                }
                // Clear bottom rows (below the circle region).
                let circle_bottom = offset_y + eff_h;
                if circle_bottom < height {
                    fill_rect(data, 0, circle_bottom, width, height);
                }
                // Clear left columns (left of the circle region) within the circle rows.
                if offset_x > 0 {
                    fill_rect(data, 0, offset_y, offset_x, offset_y + eff_h);
                }
                // Clear right columns (right of the circle region) within the circle rows.
                let circle_right = offset_x + eff_w;
                if circle_right < width {
                    fill_rect(data, circle_right, offset_y, width, offset_y + eff_h);
                }
            }

            // Quadrant definitions: [top_left, top_right, bottom_left, bottom_right]
            // Each: (qx, qy, qw, qh, radius, center_x, center_y, is_top, is_left)
            struct Quadrant {
                qx: u32,
                qy: u32,
                qw: u32,
                qh: u32,
                radius: f32,
                cx: f32,
                cy: f32,
                is_top: bool,
                is_left: bool,
            }

            let quadrants = [
                Quadrant {
                    qx: offset_x,
                    qy: offset_y,
                    qw: left_half_width,
                    qh: top_half_height,
                    radius: corner_radii[0],
                    cx: offset_x as f32 + corner_radii[0],
                    cy: offset_y as f32 + corner_radii[0],
                    is_top: true,
                    is_left: true,
                },
                Quadrant {
                    qx: offset_x + left_half_width,
                    qy: offset_y,
                    qw: right_half_width,
                    qh: top_half_height,
                    radius: corner_radii[1],
                    cx: offset_x as f32 + eff_w as f32 - corner_radii[1],
                    cy: offset_y as f32 + corner_radii[1],
                    is_top: true,
                    is_left: false,
                },
                Quadrant {
                    qx: offset_x,
                    qy: offset_y + top_half_height,
                    qw: left_half_width,
                    qh: bottom_half_height,
                    radius: corner_radii[2],
                    cx: offset_x as f32 + corner_radii[2],
                    cy: offset_y as f32 + eff_h as f32 - corner_radii[2],
                    is_top: false,
                    is_left: true,
                },
                Quadrant {
                    qx: offset_x + left_half_width,
                    qy: offset_y + top_half_height,
                    qw: right_half_width,
                    qh: bottom_half_height,
                    radius: corner_radii[3],
                    cx: offset_x as f32 + eff_w as f32 - corner_radii[3],
                    cy: offset_y as f32 + eff_h as f32 - corner_radii[3],
                    is_top: false,
                    is_left: false,
                },
            ];

            for q in &quadrants {
                if q.radius <= 0.0 {
                    continue;
                }

                let radius_of_influence = q.radius + (1.0 - volumetric_offset);
                let radius_of_solid = q.radius - volumetric_offset;
                let alias_width = radius_of_influence - radius_of_solid;
                let ri2 = radius_of_influence * radius_of_influence;
                let rs2 = radius_of_solid * radius_of_solid;

                let radius_ceil = q.radius.ceil() as u32;

                let start_y = if q.is_top {
                    q.qy as usize
                } else {
                    (q.qy + q.qh).saturating_sub(radius_ceil) as usize
                };
                let end_y = if q.is_top {
                    (q.qy as usize + radius_ceil as usize).min(q.qy as usize + q.qh as usize)
                } else {
                    (q.qy + q.qh) as usize
                };
                let start_x = if q.is_left {
                    q.qx as usize
                } else {
                    (q.qx + q.qw).saturating_sub(radius_ceil) as usize
                };
                let end_x = if q.is_left {
                    (q.qx as usize + radius_ceil as usize).min(q.qx as usize + q.qw as usize)
                } else {
                    (q.qx + q.qw) as usize
                };

                // Clear edges for rows where the quadrant is not rendering an arc.
                // This handles the non-arc rows in each quadrant that still need to be
                // cleared outside the circle region (for non-zero offset scenarios).
                let (clear_x_from, clear_x_to) = if q.is_left {
                    (0u32, q.qx)
                } else {
                    (q.qx + q.qw, width)
                };

                if clear_x_from != clear_x_to {
                    // Rows in the quadrant above/below the arc range.
                    for y in (q.qy..start_y as u32).chain(end_y as u32..(q.qy + q.qh)) {
                        let row_start = y as usize * width as usize * bpp;
                        for x in clear_x_from as usize..clear_x_to as usize {
                            let off = row_start + x * bpp;
                            for c in 0..bpp.min(4) {
                                data[off + c] = bg[c];
                            }
                        }
                    }
                }

                for y in start_y..end_y {
                    let yf = y as f32 + 0.5;
                    let y_dist = (q.cy - yf).abs();
                    let y_dist_sq = y_dist * y_dist;
                    let row_start = y * width as usize * bpp;

                    let x_dist_solid = (rs2 - y_dist_sq).max(0.0).sqrt();
                    let x_dist_influenced = (ri2 - y_dist_sq).max(0.0).sqrt();

                    let edge_solid_x1 = (q.cx - x_dist_solid).ceil().max(0.0) as usize;
                    let edge_solid_x2 = (q.cx + x_dist_solid).floor().min(width as f32) as usize;
                    let edge_influence_x1 = (q.cx - x_dist_influenced).floor().max(0.0) as usize;
                    let edge_influence_x2 =
                        (q.cx + x_dist_influenced).ceil().min(width as f32) as usize;

                    // Clear what we don't need to alias (outside influence region).
                    if q.is_left {
                        for x in start_x..edge_influence_x1.min(end_x) {
                            let off = row_start + x * bpp;
                            for c in 0..bpp.min(4) {
                                data[off + c] = bg[c];
                            }
                        }
                    } else {
                        for x in edge_influence_x2.max(start_x)..end_x {
                            let off = row_start + x * bpp;
                            for c in 0..bpp.min(4) {
                                data[off + c] = bg[c];
                            }
                        }
                    }

                    // Alias pixels in the transition band.
                    let (alias_from, alias_to) = if q.is_left {
                        (edge_influence_x1.max(start_x), edge_solid_x1.min(end_x))
                    } else {
                        (edge_solid_x2.max(start_x), edge_influence_x2.min(end_x))
                    };

                    for x in alias_from..alias_to {
                        let xf = x as f32 + 0.5;
                        let diff_x = q.cx - xf;
                        let distance = (diff_x * diff_x + y_dist_sq).sqrt();

                        if distance > radius_of_influence {
                            let off = row_start + x * bpp;
                            for c in 0..bpp.min(4) {
                                data[off + c] = bg[c];
                            }
                        } else if distance > radius_of_solid {
                            let intensity = (distance - radius_of_solid) / alias_width;
                            let off = row_start + x * bpp;

                            let pixel_a = if bpp >= 4 {
                                data[off + 3] as f32 / 255.0 * (1.0 - intensity)
                            } else {
                                1.0 - intensity
                            };
                            let matte_a = (1.0 - pixel_a) * bg_linear[3];
                            let final_a = matte_a + pixel_a;

                            if final_a > 0.0 {
                                for c in 0..bpp.min(3) {
                                    let src_lin = srgb_byte_to_linear(data[off + c]);
                                    let blended =
                                        (src_lin * pixel_a + bg_linear[c] * matte_a) / final_a;
                                    data[off + c] = linear_to_srgb_byte(blended);
                                }
                                if bpp >= 4 {
                                    data[off + 3] = (final_a * 255.0 + 0.5).min(255.0) as u8;
                                }
                            } else {
                                for c in 0..bpp.min(4) {
                                    data[off + c] = bg[c];
                                }
                            }
                        }
                    }
                }
            }
        }),
    })
}

// ─── Linear/sRGB conversion helpers for gamma-correct blending ───

fn srgb_byte_to_linear(b: u8) -> f32 {
    let s = b as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb_byte(l: f32) -> u8 {
    let s = if l <= 0.0031308 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

/// Convert a `zenresize.resize` node to `NodeOp::Resize`.
pub(crate) fn convert_resize(node: &dyn NodeInstance) -> Result<NodeOp, PipeError> {
    let w = param_u32(node, "w")?;
    let h = param_u32(node, "h")?;
    let filter_str = node
        .get_param("filter")
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    let filter = if filter_str.is_empty() {
        None
    } else {
        parse_filter_opt(&filter_str)
    };
    let sharpen = param_f32_opt(node, "sharpen");
    let sharpen_percent = if sharpen.unwrap_or(0.0) > 0.0 {
        sharpen
    } else {
        None
    };
    Ok(NodeOp::Resize {
        w,
        h,
        filter,
        sharpen_percent,
    })
}
