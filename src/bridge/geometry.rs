//! Geometry fusion: compile a run of adjacent geometry nodes into a single
//! `NodeOp::Layout` (or `NodeOp::ResizeAdvanced` when resize-time extras like
//! sharpening or kernel shaping are requested) using `zenlayout::Pipeline`.

use zennode::NodeInstance;

#[allow(unused_imports)]
use whereat::at;

use crate::error::PipeError;
use crate::graph::NodeOp;

use super::parse::{
    param_f32_opt, param_i32, param_str, param_u32_opt, parse_canvas_color, parse_constraint_mode,
    parse_filter_opt, parse_gravity_anchor,
};

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
    "zenlayout.rotate_angle",
    "zenresize.constrain",
    "zenlayout.constrain",
    "zenpipe.riapi_crop",
    "zenpipe.post_rotate",
    "zenpipe.post_flip",
];

/// Check if a schema ID is a geometry operation.
pub(crate) fn is_geometry_node(schema_id: &str) -> bool {
    GEOMETRY_SCHEMA_IDS.contains(&schema_id)
}

/// RIAPI `scale=` values (when scaling is permitted).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RiapiScale {
    /// `down` / `downscaleonly` — never enlarge (the RIAPI default).
    Down,
    /// `up` / `upscaleonly` — only enlarge; larger sources pass through.
    Up,
    /// `both` — always scale to the target.
    Both,
    /// `canvas` / `upscalecanvas` — never enlarge pixels; pad the canvas.
    Canvas,
}

fn parse_riapi_scale(s: &str) -> Option<RiapiScale> {
    match s.to_ascii_lowercase().as_str() {
        "down" | "downscaleonly" | "" => Some(RiapiScale::Down),
        "up" | "upscaleonly" => Some(RiapiScale::Up),
        "both" => Some(RiapiScale::Both),
        "canvas" | "upscalecanvas" => Some(RiapiScale::Canvas),
        _ => None,
    }
}

/// Resolve the target box for scale gating, deriving the missing axis from
/// the source aspect ratio (mirrors `imageflow_riapi` `get_ideal_target_size`).
fn gating_box(src_w: u32, src_h: u32, w: Option<u32>, h: Option<u32>) -> (u32, u32) {
    match (w, h) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => {
            let h = ((src_h as f64) * (w as f64) / (src_w as f64)).round() as u32;
            (w, h.max(1))
        }
        (None, Some(h)) => {
            let w = ((src_w as f64) * (h as f64) / (src_h as f64)).round() as u32;
            (w.max(1), h)
        }
        (None, None) => (src_w, src_h),
    }
}

/// Compose a RIAPI fit mode (`max`/`pad`/`crop`/`stretch`/`aspectcrop`) with a
/// `scale=` value into a zenlayout [`ConstraintMode`], or pass zen-native mode
/// names through directly (they already encode their scaling permission).
///
/// Returns `Ok(None)` when the composition is a no-op for these dimensions
/// (e.g. `mode=stretch&scale=down` on a source that is not larger than the
/// target — imageflow's `skip_unless(Cond::Either(Ordering::Greater))`).
///
/// Reference: `imageflow_riapi/src/ir4/layout.rs:162-280` (`build_constraints`).
/// Aspect-fit box of the source inside the target ("inner box"): the
/// largest source-aspect rectangle fitting the target. imageflow's
/// `scale=canvas` pads to THIS box, not to the raw target dimensions.
fn inner_box(src_w: u32, src_h: u32, bw: u32, bh: u32) -> (u32, u32) {
    let scale = ((bw as f64) / (src_w as f64)).min((bh as f64) / (src_h as f64));
    (
        ((src_w as f64) * scale).round().max(1.0) as u32,
        ((src_h as f64) * scale).round().max(1.0) as u32,
    )
}

/// Resolved constraint: the zenlayout mode plus (possibly adjusted) target
/// dimensions — `scale=canvas` replaces the target with the aspect-correct
/// inner box.
type ResolvedConstraint = (zenlayout::ConstraintMode, Option<u32>, Option<u32>);

fn resolve_constraint_mode(
    mode_str: &str,
    scale_str: Option<&str>,
    src_w: u32,
    src_h: u32,
    w: Option<u32>,
    h: Option<u32>,
) -> crate::PipeResult<Option<ResolvedConstraint>> {
    use zenlayout::ConstraintMode as M;

    let scale = match scale_str {
        Some(s) => parse_riapi_scale(s).ok_or_else(|| {
            at!(PipeError::Op(alloc::format!(
                "bridge: unknown scale value '{s}' (expected down/up/both/canvas)"
            )))
        })?,
        None => RiapiScale::Down,
    };

    let (bw, bh) = gating_box(src_w, src_h, w, h);
    // imageflow gating conditions:
    // "Either(Greater)": at least one source dimension exceeds the box.
    let src_larger = src_w > bw || src_h > bh;
    // "Neither(Greater)": source fits inside the box on both axes.
    let src_fits = src_w <= bw && src_h <= bh;
    // scale=canvas pads to the aspect-correct inner box of the target
    // (imageflow layout.rs Max+UpscaleCanvas: "Pad to the inner box").
    let canvas_box = || {
        let (iw, ih) = inner_box(src_w, src_h, bw, bh);
        (Some(iw), Some(ih))
    };

    let mode = mode_str.to_ascii_lowercase();
    let composed: Option<ResolvedConstraint> = match mode.as_str() {
        // ── RIAPI fit modes: compose with `scale` ──
        "max" => match scale {
            RiapiScale::Down => Some((M::Within, w, h)),
            RiapiScale::Both => Some((M::Fit, w, h)),
            // imageflow's larger_than IS Max+UpscaleOnly (ir4/layout.rs:307);
            // zenlayout's LargerThan self-gates, no dimension check needed.
            RiapiScale::Up => Some((M::LargerThan, w, h)),
            RiapiScale::Canvas => {
                let (cw, ch) = canvas_box();
                Some((M::PadWithin, cw, ch))
            }
        },
        "pad" => match scale {
            RiapiScale::Down => Some((M::WithinPad, w, h)),
            RiapiScale::Both => Some((M::FitPad, w, h)),
            RiapiScale::Up => src_fits.then_some((M::FitPad, w, h)),
            RiapiScale::Canvas => {
                let (cw, ch) = canvas_box();
                Some((M::PadWithin, cw, ch))
            }
        },
        "crop" => match scale {
            RiapiScale::Down => Some((M::WithinCrop, w, h)),
            RiapiScale::Both => Some((M::FitCrop, w, h)),
            RiapiScale::Up => src_fits.then_some((M::FitCrop, w, h)),
            // imageflow does a partwise crop + virtual canvas here; zenlayout
            // has no direct equivalent. WithinCrop is the closest behavior
            // (documented divergence — see IMAGEFLOW-PARITY.md W1 notes).
            RiapiScale::Canvas => Some((M::WithinCrop, w, h)),
        },
        "stretch" => match scale {
            RiapiScale::Both => Some((M::Distort, w, h)),
            RiapiScale::Down => src_larger.then_some((M::Distort, w, h)),
            RiapiScale::Up => src_fits.then_some((M::Distort, w, h)),
            // Rare combination; imageflow pads the distorted result's canvas.
            RiapiScale::Canvas => Some((M::Distort, w, h)),
        },
        // AspectCrop ignores `scale` entirely (imageflow layout.rs:274-277).
        "aspectcrop" => Some((M::AspectCrop, w, h)),
        // ── zen-native names: pass through (scale is encoded in the mode) ──
        _ => return parse_constraint_mode(&mode).map(|m| Some((m, w, h))),
    };
    Ok(composed)
}

/// Resolve a RIAPI crop window (raw coordinates + units) into a pixel
/// rectangle, following imageflow's rules
/// (`imageflow_riapi/src/ir4/layout.rs:700-775`):
///
/// - units of `0` mean the coordinate space is the source dimensions (pixels)
/// - coordinates scale by `dim / units`
/// - negative `x1`/`y1` (or non-positive `x2`/`y2`) are bottom/right-relative
/// - everything clamps to the image bounds
/// - an empty or inverted window resets to the full image
fn resolve_riapi_crop(
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    xunits: f32,
    yunits: f32,
    src_w: u32,
    src_h: u32,
) -> (u32, u32, u32, u32) {
    let wf = src_w as f64;
    let hf = src_h as f64;
    let xu = if xunits <= 0.0 { wf } else { xunits as f64 };
    let yu = if yunits <= 0.0 { hf } else { yunits as f64 };

    let scale_x = |v: f32| -> f64 { (v as f64) * wf / xu };
    let scale_y = |v: f32| -> f64 { (v as f64) * hf / yu };

    let mut x1f = scale_x(x1);
    let mut y1f = scale_y(y1);
    let mut x2f = scale_x(x2);
    let mut y2f = scale_y(y2);

    // Bottom/right-relative coordinates.
    if x1f < 0.0 {
        x1f += wf;
    }
    if y1f < 0.0 {
        y1f += hf;
    }
    if x2f <= 0.0 {
        x2f += wf;
    }
    if y2f <= 0.0 {
        y2f += hf;
    }

    let x1c = x1f.clamp(0.0, wf).round() as u32;
    let y1c = y1f.clamp(0.0, hf).round() as u32;
    let x2c = x2f.clamp(0.0, wf).round() as u32;
    let y2c = y2f.clamp(0.0, hf).round() as u32;

    if x2c <= x1c || y2c <= y1c {
        // Empty/inverted window → whole image (IR4 behavior).
        return (0, 0, src_w, src_h);
    }
    (x1c, y1c, x2c - x1c, y2c - y1c)
}

/// Resize-time extras collected from a Constrain node that the plain
/// streaming Layout op cannot express. Their presence routes the fused run
/// through `NodeOp::ResizeAdvanced` (full `zenresize::ResizeConfig`).
#[derive(Default)]
struct ResizeExtras {
    sharpen_percent: Option<f32>,
    scaling_linear: Option<bool>,
    kernel_width_scale: Option<f32>,
    kernel_lobe_ratio: Option<f32>,
    post_blur: Option<f32>,
}

impl ResizeExtras {
    fn any(&self) -> bool {
        self.sharpen_percent.is_some()
            || self.scaling_linear.is_some()
            || self.kernel_width_scale.is_some()
            || self.kernel_lobe_ratio.is_some()
            || self.post_blur.is_some()
    }
}

/// Compile a run of adjacent geometry nodes into a single `NodeOp`.
///
/// Feeds the geometry run through `zenlayout::Pipeline` to produce a single
/// `LayoutPlan`. Without resize-time extras this emits the streaming
/// `NodeOp::Layout { plan, filter }`; with extras (unsharp mask, sRGB-space
/// scaling, kernel shaping, post blur) it emits
/// `NodeOp::ResizeAdvanced(config)` built via
/// [`crate::execute_layout::config_from_plan`] — the same execution path the
/// rich `NodeOp::Constrain` variant uses.
///
/// `source_w` and `source_h` are needed for layout planning but are not
/// always known at compile time (they depend on the upstream source). When
/// not available (0, 0), falls back to individual node conversion.
pub(crate) fn compile_geometry_run(
    nodes: &[&dyn NodeInstance],
    source_w: u32,
    source_h: u32,
) -> crate::PipeResult<NodeOp> {
    if nodes.is_empty() {
        return Err(at!(PipeError::Op("empty geometry run".into())));
    }

    // If source dimensions aren't known, fall back (caller handles this).
    if source_w == 0 || source_h == 0 {
        return Err(at!(PipeError::Op(
            "geometry fusion requires source dimensions".into(),
        )));
    }

    let mut pipeline = zenlayout::Pipeline::new(source_w, source_h);
    let mut down_filter: Option<zenresize::Filter> = None;
    let mut up_filter: Option<zenresize::Filter> = None;
    let mut extras = ResizeExtras::default();
    let mut sharpen_when: Option<alloc::string::String> = None;
    let mut unsharp_request: Option<f32> = None;
    // Dimensions of the image at the current point in the composed run
    // (updated by crops and axis-swapping orientations). Used to resolve
    // percentage/unit crops and to gate mode×scale composition. zenlayout
    // recomputes exact geometry at plan time; this tracker only needs to be
    // correct for ops that appear BEFORE the constraint, which the RIAPI
    // ordering guarantees.
    let mut cur_w = source_w;
    let mut cur_h = source_h;

    for &node in nodes {
        let id = node.schema().id;
        match id {
            "zenlayout.crop" => {
                let x = super::parse::param_u32(node, "x")?;
                let y = super::parse::param_u32(node, "y")?;
                let w = super::parse::param_u32(node, "w")?;
                let h = super::parse::param_u32(node, "h")?;
                pipeline = pipeline.crop_pixels(x, y, w, h);
                cur_w = w;
                cur_h = h;
            }
            "zenlayout.orient" => {
                let val = param_i32(node, "orientation")?;
                let exif = u8::try_from(val).unwrap_or(1);
                pipeline = pipeline.auto_orient(exif);
                if (5..=8).contains(&exif) {
                    core::mem::swap(&mut cur_w, &mut cur_h);
                }
            }
            "zenlayout.flip_h" => {
                pipeline = pipeline.flip_h();
            }
            "zenlayout.flip_v" => {
                pipeline = pipeline.flip_v();
            }
            "zenlayout.rotate_90" => {
                pipeline = pipeline.rotate_90();
                core::mem::swap(&mut cur_w, &mut cur_h);
            }
            "zenlayout.rotate_180" => {
                pipeline = pipeline.rotate_180();
            }
            "zenlayout.rotate_270" => {
                pipeline = pipeline.rotate_270();
                core::mem::swap(&mut cur_w, &mut cur_h);
            }
            // Post-resize rotate/flip (RIAPI `rotate=` / `flip=`). These sit
            // at the end of the fused run; zenlayout composes them onto the
            // final canvas orientation.
            "zenpipe.post_rotate" => {
                let degrees = param_i32(node, "degrees")?;
                pipeline = match degrees {
                    90 => {
                        core::mem::swap(&mut cur_w, &mut cur_h);
                        pipeline.rotate_90()
                    }
                    180 => pipeline.rotate_180(),
                    270 => {
                        core::mem::swap(&mut cur_w, &mut cur_h);
                        pipeline.rotate_270()
                    }
                    0 => pipeline,
                    other => {
                        return Err(at!(PipeError::Op(alloc::format!(
                            "post_rotate degrees must be 0/90/180/270, got {other}"
                        ))));
                    }
                };
            }
            "zenpipe.post_flip" => {
                let h = node.get_param("horizontal").and_then(|v| v.as_bool());
                let v = node.get_param("vertical").and_then(|v| v.as_bool());
                if h == Some(true) {
                    pipeline = pipeline.flip_h();
                }
                if v == Some(true) {
                    pipeline = pipeline.flip_v();
                }
            }
            "zenlayout.rotate_angle" => {
                let degrees = param_f32_opt(node, "degrees").unwrap_or(0.0);
                let mode_str = node
                    .get_param("mode")
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .unwrap_or_default();
                let mode = match mode_str.as_str() {
                    "expand" => zenlayout::RotateMode::Expand {
                        color: zenlayout::CanvasColor::Transparent,
                    },
                    "original" => zenlayout::RotateMode::CropToOriginal,
                    _ => zenlayout::RotateMode::InscribedCrop,
                };
                pipeline = pipeline.rotate_angle(degrees, mode);
            }
            "zenresize.constrain" | "zenlayout.constrain" => {
                let mut raw_w = param_u32_opt(node, "w").filter(|&v| v > 0);
                let mut raw_h = param_u32_opt(node, "h").filter(|&v| v > 0);
                let max_w = param_u32_opt(node, "max_w").filter(|&v| v > 0);
                let max_h = param_u32_opt(node, "max_h").filter(|&v| v > 0);

                // ── maxwidth/maxheight bounding (imageflow get_wh_from_all) ──
                // Same axis: the smaller wins. Cross axis: the resolved
                // target box scales down (aspect-preserving) until it fits
                // under every cap. Maxes alone act as the target (the
                // preprocess layer injects mode=max for that case).
                if raw_w.is_none() && raw_h.is_none() {
                    raw_w = max_w;
                    raw_h = max_h;
                } else {
                    if let (Some(w0), Some(mw)) = (raw_w, max_w) {
                        raw_w = Some(w0.min(mw));
                    }
                    if let (Some(h0), Some(mh)) = (raw_h, max_h) {
                        raw_h = Some(h0.min(mh));
                    }
                    // Cross axis: the max clamps only the DERIVED axis (the
                    // exact value stays; pad modes fill the difference —
                    // legacy `h=200&maxwidth=100` yields a 100×200 canvas).
                    let cross_w = max_w.filter(|_| raw_w.is_none());
                    let cross_h = max_h.filter(|_| raw_h.is_none());
                    if cross_w.is_some() || cross_h.is_some() {
                        let (bw, bh) = gating_box(cur_w, cur_h, raw_w, raw_h);
                        if let Some(mw) = cross_w {
                            raw_w = Some(bw.min(mw));
                        }
                        if let Some(mh) = cross_h {
                            raw_h = Some(bh.min(mh));
                        }
                    }
                }

                // ── zoom / dpr multiplies the requested dimensions ──
                let zoom = param_f32_opt(node, "zoom")
                    .filter(|z| z.is_finite() && *z > 0.0)
                    .unwrap_or(1.0);
                let apply_zoom = |d: u32| -> u32 {
                    ((d as f64) * (zoom as f64))
                        .round()
                        .clamp(1.0, i32::MAX as f64) as u32
                };
                let w = raw_w.map(apply_zoom);
                let h = raw_h.map(apply_zoom);

                let mode_str = param_str(node, "mode")?;
                let scale_str = node
                    .get_param("scale")
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .filter(|s| !s.is_empty());

                // Dimensionless Constrain nodes are carriers for matte /
                // filter / kernel params — there is nothing to constrain
                // (zenlayout rejects 0×0 targets). Fall through to the
                // extras collection below.
                let apply_constraint = w.is_some() || h.is_some();

                // cur_w/cur_h: dimensions entering the constraint (after any
                // crop or orientation already composed into the pipeline).
                let resolved = if apply_constraint {
                    resolve_constraint_mode(&mode_str, scale_str.as_deref(), cur_w, cur_h, w, h)?
                } else {
                    None
                };

                if let Some((mode, w, h)) = resolved {
                    let mut constraint = match (w, h) {
                        (Some(w), Some(h)) => zenlayout::Constraint::new(mode, w, h),
                        (Some(w), None) => zenlayout::Constraint::width_only(mode, w),
                        (None, Some(h)) => zenlayout::Constraint::height_only(mode, h),
                        (None, None) => unreachable!("gated by apply_constraint"),
                    };

                    // ── gravity: explicit gravity_x/gravity_y beats anchor ──
                    let gx = param_f32_opt(node, "gravity_x");
                    let gy = param_f32_opt(node, "gravity_y");
                    let gravity = match (gx, gy) {
                        (Some(x), Some(y)) => Some(zenlayout::Gravity::Percentage(
                            x.clamp(0.0, 1.0),
                            y.clamp(0.0, 1.0),
                        )),
                        _ => {
                            let anchor = param_str(node, "gravity").unwrap_or_default();
                            parse_gravity_anchor(&anchor)
                                .map(|(x, y)| zenlayout::Gravity::Percentage(x, y))
                        }
                    };
                    if let Some(g) = gravity {
                        constraint = constraint.gravity(g);
                    }

                    // ── canvas color (bgcolor) for pad modes ──
                    if let Some(cc) = node
                        .get_param("canvas_color")
                        .and_then(|v| v.as_str().map(|s| s.to_string()))
                        .filter(|s| !s.is_empty())
                        .and_then(|s| parse_canvas_color(&s))
                    {
                        constraint = constraint.canvas_color(cc);
                    }
                    // matte_color is resolved at the job layer (codec intent →
                    // MatteFlattenOp during alpha removal), not here.

                    pipeline = pipeline.constrain(constraint);
                }

                // ── filters & resize-time extras ──
                if let Some(f) = node
                    .get_param("down_filter")
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .and_then(|s| parse_filter_opt(&s))
                {
                    down_filter = Some(f);
                }
                if let Some(f) = node
                    .get_param("up_filter")
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .and_then(|s| parse_filter_opt(&s))
                {
                    up_filter = Some(f);
                }
                if let Some(cs) = node
                    .get_param("scaling_colorspace")
                    .and_then(|v| v.as_str().map(|s| s.to_ascii_lowercase()))
                {
                    match cs.as_str() {
                        "srgb" | "gamma" => extras.scaling_linear = Some(false),
                        "linear" => extras.scaling_linear = Some(true),
                        _ => {}
                    }
                }
                unsharp_request = param_f32_opt(node, "unsharp_percent").filter(|&v| v > 0.0);
                sharpen_when = node
                    .get_param("sharpen_when")
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .filter(|s| !s.is_empty());
                extras.kernel_width_scale =
                    param_f32_opt(node, "kernel_width_scale").filter(|&v| v > 0.0 && v != 1.0);
                extras.kernel_lobe_ratio =
                    param_f32_opt(node, "kernel_lobe_ratio").filter(|&v| v > 0.0);
                extras.post_blur = param_f32_opt(node, "post_blur").filter(|&v| v > 0.0);
            }
            "zenlayout.crop_percent" => {
                // Fraction-based crop: x/y/w/h are 0.0–1.0 fractions of the
                // CURRENT size (matches the CropPercent schema fields).
                let x = param_f32_opt(node, "x").unwrap_or(0.0).clamp(0.0, 1.0);
                let y = param_f32_opt(node, "y").unwrap_or(0.0).clamp(0.0, 1.0);
                let w = param_f32_opt(node, "w").unwrap_or(1.0).clamp(0.0, 1.0);
                let h = param_f32_opt(node, "h").unwrap_or(1.0).clamp(0.0, 1.0);
                let px = (x * cur_w as f32).round() as u32;
                let py = (y * cur_h as f32).round() as u32;
                let pw = ((w * cur_w as f32).round() as u32).max(1);
                let ph = ((h * cur_h as f32).round() as u32).max(1);
                pipeline = pipeline.crop_pixels(px, py, pw, ph);
                cur_w = pw;
                cur_h = ph;
            }
            "zenpipe.riapi_crop" => {
                let x1 = param_f32_opt(node, "x1").unwrap_or(0.0);
                let y1 = param_f32_opt(node, "y1").unwrap_or(0.0);
                let x2 = param_f32_opt(node, "x2").unwrap_or(0.0);
                let y2 = param_f32_opt(node, "y2").unwrap_or(0.0);
                let xu = param_f32_opt(node, "xunits").unwrap_or(0.0);
                let yu = param_f32_opt(node, "yunits").unwrap_or(0.0);
                let (px, py, pw, ph) = resolve_riapi_crop(x1, y1, x2, y2, xu, yu, cur_w, cur_h);
                if (px, py, pw, ph) != (0, 0, cur_w, cur_h) {
                    pipeline = pipeline.crop_pixels(px, py, pw, ph);
                    cur_w = pw;
                    cur_h = ph;
                }
            }
            // zenlayout.region is handled by the RegionConverter (needs ExpandCanvas).
            _ => {
                return Err(at!(PipeError::Op(alloc::format!(
                    "unexpected node '{id}' in geometry run"
                ))));
            }
        }
    }

    let (ideal, request) = pipeline.plan().map_err(|e| {
        at!(PipeError::Op(alloc::format!(
            "geometry fusion plan failed: {e}"
        )))
    })?;
    let offer = zenlayout::DecoderOffer::full_decode(source_w, source_h);
    let plan = ideal.finalize(&request, &offer);

    // ── choose the filter by net scale direction (up vs down) ──
    // Pre-resize dims = the trimmed window when present, else the source.
    let (pre_w, pre_h) = plan
        .trim
        .map(|t| (t.width, t.height))
        .unwrap_or((source_w, source_h));
    let out_area = plan.canvas.width as u64 * plan.canvas.height as u64;
    let in_area = pre_w as u64 * pre_h as u64;
    let is_upscaling = out_area > in_area;
    let is_downscaling = out_area < in_area;
    let size_differs = plan.canvas.width != pre_w || plan.canvas.height != pre_h;
    let filter = if is_upscaling {
        up_filter.unwrap_or(zenresize::Filter::Ginseng)
    } else {
        down_filter.unwrap_or(zenresize::Filter::Robidoux)
    };

    // ── sharpen_when gating (same policy as NodeOp::Constrain's compile) ──
    extras.sharpen_percent = unsharp_request.and_then(|pct| {
        let should = match sharpen_when.as_deref() {
            Some("upscaling") => is_upscaling,
            Some("size_differs") | Some("sizediffers") => size_differs,
            Some("always") => true,
            _ => is_downscaling, // "downscaling" is the RIAPI/zen default
        };
        if should { Some(pct) } else { None }
    });

    if extras.any() {
        // Full zenresize config path — same as NodeOp::Constrain's advanced
        // branch (graph.rs), so sharpening/kernel/colorspace behave
        // identically whether the geometry arrived fused or as one node.
        let mut config = crate::execute_layout::config_from_plan(
            source_w,
            source_h,
            &plan,
            zenresize::PixelDescriptor::RGBA8_SRGB,
            filter,
        );
        if let Some(pct) = extras.sharpen_percent {
            config.post_sharpen = pct;
        }
        if let Some(false) = extras.scaling_linear {
            config.linear = false;
        }
        if let Some(kws) = extras.kernel_width_scale {
            config.kernel_width_scale = Some(kws as f64);
        }
        if let Some(lr) = extras.kernel_lobe_ratio {
            config.lobe_ratio = zenresize::LobeRatio::Exact(lr);
        }
        if let Some(blur) = extras.post_blur {
            config.post_blur_sigma = blur;
        }
        if let Some(uf) = up_filter {
            config.up_filter = Some(uf);
        }
        return Ok(NodeOp::ResizeAdvanced(config));
    }

    Ok(NodeOp::Layout { plan, filter })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenlayout::ConstraintMode as M;

    // ─── resolve_riapi_crop: imageflow crop-window semantics ───

    #[test]
    fn crop_absent_units_are_source_pixels() {
        // ?crop=10,20,110,220 on a 400×300 source = a 100×200 pixel window.
        assert_eq!(
            resolve_riapi_crop(10.0, 20.0, 110.0, 220.0, 0.0, 0.0, 400, 300),
            (10, 20, 100, 200)
        );
    }

    #[test]
    fn crop_percent_units() {
        // c=25,25,75,75 (units 100) on 400×200 → x 100..300, y 50..150.
        assert_eq!(
            resolve_riapi_crop(25.0, 25.0, 75.0, 75.0, 100.0, 100.0, 400, 200),
            (100, 50, 200, 100)
        );
    }

    #[test]
    fn crop_negative_coords_are_bottom_right_relative() {
        // crop=-100,-100,0,0 → the bottom-right 100×100 corner
        // (x2/y2 of 0 also wrap to the far edge, per IR4).
        assert_eq!(
            resolve_riapi_crop(-100.0, -100.0, 0.0, 0.0, 0.0, 0.0, 400, 300),
            (300, 200, 100, 100)
        );
    }

    #[test]
    fn crop_inverted_window_resets_to_full_image() {
        assert_eq!(
            resolve_riapi_crop(300.0, 10.0, 100.0, 200.0, 0.0, 0.0, 400, 300),
            (0, 0, 400, 300)
        );
    }

    #[test]
    fn crop_clamps_to_image() {
        assert_eq!(
            resolve_riapi_crop(-50.0, 0.0, 9999.0, 9999.0, 100.0, 100.0, 400, 300),
            // -50% wraps to x=200 (=-200px +400); x2/y2 clamp to the edges.
            (200, 0, 200, 300)
        );
    }

    // ─── resolve_constraint_mode: RIAPI mode×scale composition ───

    fn resolve(mode: &str, scale: Option<&str>, sw: u32, sh: u32) -> Option<M> {
        resolve_constraint_mode(mode, scale, sw, sh, Some(100), Some(100))
            .unwrap()
            .map(|(m, _, _)| m)
    }

    #[test]
    fn scale_canvas_uses_aspect_inner_box() {
        // 400×300 into a 100×100 canvas request → pad to the 100×75 inner
        // box (imageflow "pad to the inner box of the target"), not 100×100.
        let r = resolve_constraint_mode("max", Some("canvas"), 400, 300, Some(100), Some(100))
            .unwrap()
            .unwrap();
        assert_eq!(r, (M::PadWithin, Some(100), Some(75)));
    }

    #[test]
    fn riapi_modes_default_to_downscale_only() {
        assert_eq!(resolve("max", None, 400, 300), Some(M::Within));
        assert_eq!(resolve("pad", None, 400, 300), Some(M::WithinPad));
        assert_eq!(resolve("crop", None, 400, 300), Some(M::WithinCrop));
    }

    #[test]
    fn riapi_modes_with_scale_both_may_upscale() {
        assert_eq!(resolve("max", Some("both"), 50, 50), Some(M::Fit));
        assert_eq!(resolve("pad", Some("both"), 50, 50), Some(M::FitPad));
        assert_eq!(resolve("crop", Some("both"), 50, 50), Some(M::FitCrop));
        assert_eq!(resolve("stretch", Some("both"), 50, 50), Some(M::Distort));
    }

    #[test]
    fn scale_up_only_skips_larger_sources() {
        // max+up == imageflow larger_than (self-gating in zenlayout).
        assert_eq!(resolve("max", Some("up"), 400, 300), Some(M::LargerThan));
        assert_eq!(resolve("max", Some("up"), 50, 50), Some(M::LargerThan));
        // stretch+up still needs the dimension gate (no self-gating mode).
        assert_eq!(resolve("stretch", Some("up"), 400, 300), None);
        assert_eq!(resolve("stretch", Some("up"), 50, 50), Some(M::Distort));
    }

    #[test]
    fn stretch_downscale_only_skips_smaller_sources() {
        // imageflow: distort gated on "either dimension greater".
        assert_eq!(resolve("stretch", None, 50, 50), None);
        assert_eq!(resolve("stretch", None, 400, 300), Some(M::Distort));
    }

    #[test]
    fn scale_canvas_pads_without_upscaling() {
        assert_eq!(resolve("max", Some("canvas"), 400, 300), Some(M::PadWithin));
        assert_eq!(resolve("pad", Some("canvas"), 400, 300), Some(M::PadWithin));
    }

    #[test]
    fn aspectcrop_ignores_scale() {
        assert_eq!(
            resolve("aspectcrop", Some("up"), 400, 300),
            Some(M::AspectCrop)
        );
        assert_eq!(resolve("aspect_crop", None, 400, 300), Some(M::AspectCrop));
    }

    #[test]
    fn zen_native_names_pass_through() {
        assert_eq!(resolve("within", Some("both"), 400, 300), Some(M::Within));
        assert_eq!(resolve("fit_crop", None, 400, 300), Some(M::FitCrop));
        assert_eq!(resolve("distort", None, 50, 50), Some(M::Distort));
    }

    #[test]
    fn unknown_scale_value_errors() {
        assert!(
            resolve_constraint_mode("max", Some("sideways"), 400, 300, Some(100), Some(100))
                .is_err()
        );
    }

    #[test]
    fn gating_box_derives_missing_axis_from_aspect() {
        assert_eq!(gating_box(400, 200, Some(100), None), (100, 50));
        assert_eq!(gating_box(400, 200, None, Some(100)), (200, 100));
        assert_eq!(gating_box(400, 200, Some(80), Some(90)), (80, 90));
    }

    // ─── compile_geometry_run: fused-run smoke tests ───

    fn constrain(mode: &str, w: Option<u32>, h: Option<u32>) -> crate::zennode_defs::Constrain {
        crate::zennode_defs::Constrain {
            w,
            h,
            mode: mode.into(),
            ..Default::default()
        }
    }

    fn layout_canvas(op: &NodeOp) -> (u32, u32) {
        match op {
            NodeOp::Layout { plan, .. } => (plan.canvas.width, plan.canvas.height),
            _ => panic!("expected NodeOp::Layout"),
        }
    }

    #[test]
    fn fused_pixel_crop_then_pad_mode() {
        // crop=100,50,300,250 (pixels) then mode=pad w=100&h=100:
        // 200×200 window, downscaled and padded onto a 100×100 canvas.
        let crop = crate::zennode_defs::RiapiCrop {
            x1: 100.0,
            y1: 50.0,
            x2: 300.0,
            y2: 250.0,
            xunits: 0.0,
            yunits: 0.0,
        };
        let c = constrain("pad", Some(100), Some(100));
        let nodes: alloc::vec::Vec<&dyn NodeInstance> = alloc::vec![&crop, &c];
        let op = compile_geometry_run(&nodes, 400, 300).unwrap();
        assert_eq!(layout_canvas(&op), (100, 100));
    }

    #[test]
    fn zoom_multiplies_target_dimensions() {
        let mut c = constrain("max", Some(100), Some(100));
        c.zoom = Some(2.0);
        let nodes: alloc::vec::Vec<&dyn NodeInstance> = alloc::vec![&c];
        let op = compile_geometry_run(&nodes, 800, 800).unwrap();
        assert_eq!(layout_canvas(&op), (200, 200));
    }

    #[test]
    fn stretch_default_scale_skips_smaller_source() {
        // 50×50 source, mode=stretch (default scale=down) to 100×100 →
        // no constraint applies; the canvas stays 50×50.
        let c = constrain("stretch", Some(100), Some(100));
        let nodes: alloc::vec::Vec<&dyn NodeInstance> = alloc::vec![&c];
        let op = compile_geometry_run(&nodes, 50, 50).unwrap();
        assert_eq!(layout_canvas(&op), (50, 50));
    }

    #[test]
    fn post_rotate_90_swaps_canvas_axes() {
        let c = constrain("pad", Some(100), Some(50));
        let rot = crate::zennode_defs::PostRotate { degrees: 90 };
        let nodes: alloc::vec::Vec<&dyn NodeInstance> = alloc::vec![&c, &rot];
        let op = compile_geometry_run(&nodes, 400, 300).unwrap();
        assert_eq!(layout_canvas(&op), (50, 100));
    }

    #[test]
    fn unsharp_request_routes_to_resize_advanced() {
        let mut c = constrain("max", Some(100), None);
        c.unsharp_percent = Some(20.0);
        let nodes: alloc::vec::Vec<&dyn NodeInstance> = alloc::vec![&c];
        let op = compile_geometry_run(&nodes, 400, 300).unwrap();
        match op {
            NodeOp::ResizeAdvanced(cfg) => {
                assert!((cfg.post_sharpen - 20.0).abs() < 0.001);
            }
            _ => panic!("expected ResizeAdvanced"),
        }
    }
}
