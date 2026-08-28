//! Zennode definitions for resize and pipeline-specific operations.
//!
//! Codec encode/decode, quantization, and quality-intent nodes live in
//! `zencodecs::zennode_defs`. Filter nodes live in `zenfilters::zennode_defs`.
//! Geometry nodes (crop, orient, flip, rotate, etc.) were consolidated here
//! from zenlayout — zenlayout is a planning-only crate with no zennode dep.
//!
//! # Sections
//!
//! - **Resize** — constrain (layout + resize), forced resize
//! - **Pipeline** — crop whitespace, fill rect, remove alpha, round corners

extern crate alloc;
use alloc::string::String;

use zennode::*;

// ═══════════════════════════════════════════════════════════════════════
//  GEOMETRY — crop, orient, flip, rotate, expand canvas, region, margins
// ═══════════════════════════════════════════════════════════════════════

// ─── Crop (Pixels) ───

/// Crop the image to a pixel rectangle.
///
/// Specifies origin (x, y) and dimensions (w, h) in post-orientation
/// source coordinates. For percentage-based cropping, use
/// [`CropPercent`] or [`CropMargins`] instead.
///
/// RIAPI: `?crop=10,10,90,90`
/// JSON: `{ "x": 10, "y": 10, "w": 80, "h": 80 }`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenlayout.crop", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan", changes_dimensions)]
#[node(tags("crop", "geometry"))]
pub struct Crop {
    /// Left edge X coordinate in pixels.
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Main", label = "X")]
    pub x: u32,

    /// Top edge Y coordinate in pixels.
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Main", label = "Y")]
    pub y: u32,

    /// Width of the crop region in pixels.
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Main", label = "Width")]
    pub w: u32,

    /// Height of the crop region in pixels.
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Main", label = "Height")]
    pub h: u32,
}

// ─── CropPercent ───

/// Crop the image using percentage-based coordinates.
///
/// All coordinates are fractions of source dimensions (0.0 = origin,
/// 1.0 = full extent). For example, `x=0.1, y=0.1, w=0.8, h=0.8`
/// removes 10% from each edge.
///
/// JSON: `{ "x": 0.1, "y": 0.1, "w": 0.8, "h": 0.8 }`
#[derive(Node, Clone, Debug)]
#[node(id = "zenlayout.crop_percent", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan", changes_dimensions)]
#[node(tags("crop", "geometry"))]
pub struct CropPercent {
    /// Left edge as fraction of source width (0.0 = left edge).
    #[param(range(0.0..=1.0), default = 0.0, step = 0.01)]
    #[param(section = "Main", label = "X")]
    pub x: f32,

    /// Top edge as fraction of source height (0.0 = top edge).
    #[param(range(0.0..=1.0), default = 0.0, step = 0.01)]
    #[param(section = "Main", label = "Y")]
    pub y: f32,

    /// Width as fraction of source width (1.0 = full width).
    #[param(range(0.0..=1.0), default = 1.0, step = 0.01)]
    #[param(section = "Main", label = "Width")]
    pub w: f32,

    /// Height as fraction of source height (1.0 = full height).
    #[param(range(0.0..=1.0), default = 1.0, step = 0.01)]
    #[param(section = "Main", label = "Height")]
    pub h: f32,
}

impl Default for CropPercent {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        }
    }
}

// ─── CropMargins ───

/// Crop the image by removing percentage-based margins from each side.
///
/// Each value is a fraction of the corresponding source dimension to
/// remove. CSS-style ordering: top, right, bottom, left.
///
/// JSON: `{ "top": 0.1, "right": 0.05, "bottom": 0.1, "left": 0.05 }`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenlayout.crop_margins", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan", changes_dimensions)]
#[node(tags("crop", "margins", "geometry"))]
pub struct CropMargins {
    /// Top margin as fraction of source height to remove.
    #[param(range(0.0..=0.5), default = 0.0, step = 0.01)]
    #[param(section = "Main", label = "Top")]
    pub top: f32,

    /// Right margin as fraction of source width to remove.
    #[param(range(0.0..=0.5), default = 0.0, step = 0.01)]
    #[param(section = "Main", label = "Right")]
    pub right: f32,

    /// Bottom margin as fraction of source height to remove.
    #[param(range(0.0..=0.5), default = 0.0, step = 0.01)]
    #[param(section = "Main", label = "Bottom")]
    pub bottom: f32,

    /// Left margin as fraction of source width to remove.
    #[param(range(0.0..=0.5), default = 0.0, step = 0.01)]
    #[param(section = "Main", label = "Left")]
    pub left: f32,
}

// ─── Orient ───

/// Apply EXIF orientation correction.
///
/// Orientation values 1-8 follow the EXIF standard:
/// 1 = identity, 2 = flip-H, 3 = rotate-180, 4 = flip-V,
/// 5 = transpose, 6 = rotate-90, 7 = transverse, 8 = rotate-270.
///
/// RIAPI: `?autorotate=true` (uses embedded EXIF), `?srotate=90` (degrees,
/// via the srotate adapter). Querystring: `?orientation=6` (raw EXIF value).
/// JSON: `{ "orientation": 6 }`
#[derive(Node, Clone, Debug)]
#[node(id = "zenlayout.orient", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan")]
#[node(tags("orient", "exif", "geometry"))]
pub struct Orient {
    /// EXIF orientation value (1-8). 1 = no transformation.
    /// 0 is a sentinel meaning "apply the EXIF orientation found at decode".
    ///
    /// Querystring: `?orientation=6` (the raw EXIF value). The RIAPI
    /// `srotate` key is NOT bound here — `srotate` means degrees (source
    /// rotation) and is handled by the srotate adapter; `autorotate=true`
    /// maps to `Orient { orientation: 0 }` via the autorotate adapter.
    #[param(range(0..=8), default = 1, step = 1)]
    #[param(section = "Main", label = "Orientation")]
    #[kv("orientation")]
    pub orientation: i32,
}

impl Default for Orient {
    fn default() -> Self {
        Self { orientation: 1 }
    }
}

// ─── FlipH ───

/// Flip the image horizontally (mirror left-right).
///
/// No parameters. Presence in the pipeline means flip is applied.
/// Composes with other orientation operations into a single transform.
///
/// RIAPI: `?sflip=h`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenlayout.flip_h", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan")]
#[node(tags("flip", "geometry"))]
pub struct FlipH {}

// ─── FlipV ───

/// Flip the image vertically (mirror top-bottom).
///
/// No parameters. Presence in the pipeline means flip is applied.
/// Composes with other orientation operations into a single transform.
///
/// RIAPI: `?sflip=v`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenlayout.flip_v", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan")]
#[node(tags("flip", "geometry"))]
pub struct FlipV {}

// ─── Rotate90 ───

/// Rotate the image 90 degrees clockwise.
///
/// Swaps width and height. Coalesced with other geometry nodes so the
/// layout planner computes correct dimensions through the full chain.
/// Pixel axis-swap happens at execution time.
///
/// RIAPI: `?srotate=90`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenlayout.rotate_90", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan", changes_dimensions)]
#[node(tags("rotate", "geometry"))]
pub struct Rotate90 {}

// ─── Rotate180 ───

/// Rotate the image 180 degrees.
///
/// Decomposes to flip-H + flip-V (no axis swap), so it can be
/// coalesced into the layout plan without pixel materialization.
///
/// RIAPI: `?srotate=180`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenlayout.rotate_180", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan")]
#[node(tags("rotate", "geometry"))]
pub struct Rotate180 {}

// ─── Rotate270 ───

/// Rotate the image 270 degrees clockwise (90 counter-clockwise).
///
/// Swaps width and height. Coalesced with other geometry nodes so the
/// layout planner computes correct dimensions through the full chain.
///
/// RIAPI: `?srotate=270`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenlayout.rotate_270", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan", changes_dimensions)]
#[node(tags("rotate", "geometry"))]
pub struct Rotate270 {}

// ─── AutoDeskew ───

/// Straighten the image by the angle of its dominant line structure
/// (zenpipe#27) — text lines on a scan, table rules, a horizon.
///
/// Content-adaptive: the angle is detected on the materialized frame at
/// execution time (an analysis barrier the planner re-plans past), then
/// applied as a non-cardinal rotation. No coherent structure within
/// `max_angle` leaves the image unchanged.
///
/// JSON: `{ "max_angle": 10, "mode": "inscribed", "method": "projection" }`
///
/// RIAPI: `?autodeskew=1` (10° budget) or `?autodeskew=7` (7° budget).
#[derive(Node, Clone, Debug)]
#[node(id = "zenlayout.auto_deskew", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan", changes_dimensions)]
#[node(tags("deskew", "rotate", "geometry", "content-adaptive"))]
pub struct AutoDeskew {
    /// Largest correction to apply, in degrees (clamped to 45).
    #[param(range(0.5..=45.0), default = 10.0, step = 0.5)]
    #[param(unit = "deg", section = "Main", label = "Max angle")]
    pub max_angle: f32,

    /// Canvas policy for the rotation: `inscribed` (largest axis-aligned
    /// rectangle, output shrinks), `expand` (bounding box, transparent
    /// corners) or `original` (same size, corners lost).
    #[param(default = "inscribed")]
    #[param(section = "Main", label = "Mode")]
    pub mode: String,

    /// Detector: `projection` (projection variance — documents, rulings),
    /// `hough` (edge Hough — photos, shaded forms) or `gradient`
    /// (structure tensor — fastest, coarse).
    #[param(default = "projection")]
    #[param(section = "Main", label = "Method")]
    pub method: String,
}

// ─── ExpandCanvas ───

/// Expand the canvas by adding padding around the image.
///
/// Adds specified pixel amounts to each side. The fill color
/// defaults to "transparent" (premultiplied zero). Accepts CSS-style
/// named colors or hex values.
///
/// JSON: `{ "left": 10, "top": 10, "right": 10, "bottom": 10, "color": "white" }`
#[derive(Node, Clone, Debug)]
#[node(id = "zenlayout.expand_canvas", group = Canvas, role = Resize)]
#[node(coalesce = "layout_plan", changes_dimensions)]
#[node(tags("pad", "canvas", "geometry"))]
pub struct ExpandCanvas {
    /// Left padding in pixels.
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Main", label = "Left")]
    pub left: u32,

    /// Top padding in pixels.
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Main", label = "Top")]
    pub top: u32,

    /// Right padding in pixels.
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Main", label = "Right")]
    pub right: u32,

    /// Bottom padding in pixels.
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Main", label = "Bottom")]
    pub bottom: u32,

    /// Fill color for the expanded area.
    ///
    /// Accepts "transparent", "white", "black", or hex "#RRGGBB" / "#RRGGBBAA".
    #[param(default = "transparent")]
    #[param(section = "Main", label = "Color")]
    pub color: String,

    /// How the expanded area is filled: `solid` (the color above),
    /// `replicate` (clamp to the edge pixel), `mirror` (reflect), or
    /// `repeat` (tile) — sharp's `extendWith`. See
    /// [`CanvasFill`](crate::sources::CanvasFill).
    #[param(default = "solid")]
    #[param(section = "Main", label = "Fill")]
    pub fill: String,
}

impl Default for ExpandCanvas {
    fn default() -> Self {
        Self {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
            color: String::from("transparent"),
            fill: String::from("solid"),
        }
    }
}

// ─── RegionViewport ───

/// Viewport into the source image, unifying crop and pad.
///
/// Defines a rectangular window using edge coordinates, each expressed
/// as a percentage of the source dimension plus a pixel offset:
/// `resolved = source_dim * pct + px`.
///
/// - Viewport smaller than source = crop
/// - Viewport extending beyond source = pad (filled with color)
/// - Viewport entirely outside source = blank canvas
///
/// Coordinates are **edge-based** (left, top, right, bottom), not origin + size.
///
/// Examples:
/// - Crop 10px from each edge: left_px=10, top_px=10, right_pct=1.0, right_px=-10, bottom_pct=1.0, bottom_px=-10
/// - Add 20px padding: left_px=-20, top_px=-20, right_pct=1.0, right_px=20, bottom_pct=1.0, bottom_px=20
/// - Center 50% of image: left_pct=0.25, top_pct=0.25, right_pct=0.75, bottom_pct=0.75
#[derive(Node, Clone, Debug)]
#[node(id = "zenlayout.region", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan", changes_dimensions)]
#[node(tags("region", "viewport", "crop", "pad", "geometry"))]
pub struct RegionViewport {
    /// Left edge: fraction of source width (0.0 = origin).
    #[param(range(-1.0..=2.0), default = 0.0, step = 0.01)]
    #[param(section = "Left", label = "Percent")]
    pub left_pct: f32,

    /// Left edge: pixel offset added after percentage.
    #[param(range(-65535..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Left", label = "Pixels")]
    pub left_px: i32,

    /// Top edge: fraction of source height (0.0 = origin).
    #[param(range(-1.0..=2.0), default = 0.0, step = 0.01)]
    #[param(section = "Top", label = "Percent")]
    pub top_pct: f32,

    /// Top edge: pixel offset added after percentage.
    #[param(range(-65535..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Top", label = "Pixels")]
    pub top_px: i32,

    /// Right edge: fraction of source width (1.0 = far edge).
    #[param(range(-1.0..=2.0), default = 1.0, step = 0.01)]
    #[param(section = "Right", label = "Percent")]
    pub right_pct: f32,

    /// Right edge: pixel offset added after percentage.
    #[param(range(-65535..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Right", label = "Pixels")]
    pub right_px: i32,

    /// Bottom edge: fraction of source height (1.0 = far edge).
    #[param(range(-1.0..=2.0), default = 1.0, step = 0.01)]
    #[param(section = "Bottom", label = "Percent")]
    pub bottom_pct: f32,

    /// Bottom edge: pixel offset added after percentage.
    #[param(range(-65535..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Bottom", label = "Pixels")]
    pub bottom_px: i32,

    /// Fill color for areas outside the source image.
    ///
    /// Accepts "transparent", "white", "black", or hex "#RRGGBB" / "#RRGGBBAA".
    #[param(default = "transparent")]
    #[param(section = "Main", label = "Color")]
    pub color: String,
}

impl Default for RegionViewport {
    fn default() -> Self {
        Self {
            left_pct: 0.0,
            left_px: 0,
            top_pct: 0.0,
            top_px: 0,
            right_pct: 1.0,
            right_px: 0,
            bottom_pct: 1.0,
            bottom_px: 0,
            color: String::from("transparent"),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  LAYOUT — constrain, output limits
// ═══════════════════════════════════════════════════════════════════════

// ─── OutputLimits ───

/// Safety limits and codec alignment for output dimensions.
///
/// Constrains the final output to max/min dimension bounds and optionally
/// aligns dimensions to codec block boundaries (e.g., MCU multiples
/// for JPEG).
///
/// Processing order: max (scale down) → min (scale up) → align (snap).
/// Max always wins over min if they conflict.
///
/// JSON: `{ "max_w": 4096, "max_h": 4096, "align_x": 16, "align_y": 16, "align_mode": "extend" }`
#[derive(Node, Clone, Debug)]
#[node(id = "zenlayout.output_limits", group = Layout, role = Resize)]
#[node(coalesce = "layout_plan", changes_dimensions)]
#[node(tags("limits", "alignment", "layout", "codec"))]
pub struct OutputLimits {
    /// Maximum output width. 0 = no limit. Scales down proportionally if exceeded.
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Max", label = "Width")]
    pub max_w: u32,

    /// Maximum output height. 0 = no limit. Scales down proportionally if exceeded.
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Max", label = "Height")]
    pub max_h: u32,

    /// Minimum output width. 0 = no minimum. Scales up proportionally if below.
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Min", label = "Width")]
    pub min_w: u32,

    /// Minimum output height. 0 = no minimum. Scales up proportionally if below.
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Min", label = "Height")]
    pub min_h: u32,

    /// Horizontal alignment multiple in pixels. 0 = no alignment.
    ///
    /// Output width will be snapped to the nearest multiple of this value.
    /// Common values: 2 (4:2:2), 8 (DCT block), 16 (4:2:0 MCU).
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Align", label = "X Multiple")]
    pub align_x: u32,

    /// Vertical alignment multiple in pixels. 0 = no alignment.
    ///
    /// Output height will be snapped to the nearest multiple of this value.
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Align", label = "Y Multiple")]
    pub align_y: u32,

    /// How to handle alignment snapping.
    ///
    /// - "crop" — round down, lose edge pixels
    /// - "extend" — round up, replicate edge pixels (default)
    /// - "distort" — round to nearest, slight stretch
    #[param(default = "extend")]
    #[param(section = "Align", label = "Align Mode")]
    pub align_mode: String,
}

impl Default for OutputLimits {
    fn default() -> Self {
        Self {
            max_w: 0,
            max_h: 0,
            min_w: 0,
            min_h: 0,
            align_x: 0,
            align_y: 0,
            align_mode: String::from("extend"),
        }
    }
}

// ─── Constrain ───

/// Constrain image dimensions with resize, crop, or pad modes.
///
/// The unified resize/layout node — combines layout geometry with
/// resampling hints in a single node, matching imageflow's ergonomic
/// Constrain API.
///
/// Optional parameters (`Option<T>`) use `None` for "not specified" —
/// the engine picks sensible defaults based on the operation.
///
/// # Gravity
///
/// Two ways to specify gravity:
/// - **Named anchor** (`gravity`): "center", "top_left", "bottom_right", etc.
/// - **Percentage** (`gravity_x`/`gravity_y`): 0.0–1.0, overrides named anchor when set.
///
/// If `gravity_x` and `gravity_y` are both set, the named `gravity` is ignored.
///
/// # JSON API
///
/// Simple (named anchor):
/// ```json
/// { "w": 800, "h": 600, "mode": "fit_crop", "gravity": "top_left" }
/// ```
///
/// Precise (percentage gravity):
/// ```json
/// {
///   "w": 800, "h": 600, "mode": "fit_crop",
///   "gravity_x": 0.33, "gravity_y": 0.0,
///   "down_filter": "lanczos", "unsharp_percent": 15.0
/// }
/// ```
///
/// RIAPI: `?w=800&h=600&mode=fit_crop&anchor=top_left&down.filter=lanczos`
#[derive(Node, Clone, Debug)]
#[node(id = "zenresize.constrain", group = Geometry, role = Resize)]
#[node(coalesce = "layout_plan")]
#[node(changes_dimensions)]
#[node(format(preferred = LinearF32))]
#[node(tags("resize", "geometry", "scale"))]
pub struct Constrain {
    // ─── Dimensions ───
    /// Target width in pixels. None = unconstrained (derive from height + aspect ratio).
    ///
    /// RIAPI: `?w=800` or `?width=800`.
    #[param(range(0..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Dimensions", label = "Width")]
    #[kv("w", "width")]
    pub w: Option<u32>,

    /// Target height in pixels. None = unconstrained (derive from width + aspect ratio).
    ///
    /// RIAPI: `?h=600` or `?height=600`.
    #[param(range(0..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Dimensions", label = "Height")]
    #[kv("h", "height")]
    pub h: Option<u32>,

    /// Upper bound on the resolved target width (RIAPI `maxwidth`).
    ///
    /// Caps the target aspect-preservingly: if the resolved target exceeds
    /// a max on either axis, BOTH axes scale down together (imageflow
    /// `get_wh_from_all` cross-term bounding). With no `w`/`h` at all,
    /// maxes act as `mode=max` targets (never upscale).
    #[param(range(0..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Dimensions", label = "Max Width")]
    #[kv("maxwidth")]
    pub max_w: Option<u32>,

    /// Upper bound on the resolved target height (RIAPI `maxheight`).
    #[param(range(0..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Dimensions", label = "Max Height")]
    #[kv("maxheight")]
    pub max_h: Option<u32>,

    // ─── Layout ───
    /// Constraint mode controlling how the image fits the target dimensions.
    ///
    /// - "distort" / "stretch" — stretch to exact dimensions, ignoring aspect ratio
    /// - "within" / "max" — fit inside target, never upscale (default)
    /// - "fit" — fit inside target, may upscale
    /// - "within_crop" / "crop" — fill target by cropping, never upscale
    /// - "fit_crop" — fill target by cropping, may upscale
    /// - "fit_pad" / "pad" — fit inside target, pad to exact dimensions
    /// - "within_pad" — fit inside target without upscale, pad to exact dimensions
    /// - "pad_within" — never upscale, always pad to exact canvas
    /// - "aspect_crop" / "aspectcrop" — crop to target aspect ratio without resizing
    /// - "larger_than" — upscale if needed to meet target, never downscale
    #[param(default = "within")]
    #[param(section = "Layout", label = "Mode")]
    #[kv("mode")]
    pub mode: String,

    /// Scale control: when to allow scaling.
    ///
    /// - "down" / "downscaleonly" — only downscale, never upscale
    /// - "up" / "upscaleonly" — only upscale, never downscale
    /// - "both" — allow both (default)
    /// - "canvas" / "upscalecanvas" — upscale canvas (pad) only
    ///
    /// RIAPI: `?scale=down`
    #[param(default = "")]
    #[param(section = "Layout", label = "Scale")]
    #[kv("scale")]
    pub scale: Option<String>,

    /// Device pixel ratio / zoom multiplier.
    ///
    /// Multiplies target dimensions. 2.0 means w and h are doubled.
    /// RIAPI: `?zoom=2`, `?dpr=2x`, `?dppx=1.5`
    #[param(range(0.1..=10.0), default = 1.0, step = 0.1)]
    #[param(section = "Layout", label = "DPR / Zoom")]
    #[kv("zoom", "dpr", "dppx")]
    pub zoom: Option<f32>,

    /// Named anchor for crop/pad positioning.
    ///
    /// Controls which part of the image is preserved when cropping,
    /// or where the image is positioned when padding.
    ///
    /// Values: "center", "top_left", "top", "top_right",
    /// "left", "right", "bottom_left", "bottom", "bottom_right".
    ///
    /// Overridden by `gravity_x`/`gravity_y` when both are set.
    #[param(default = "center")]
    #[param(section = "Position", label = "Anchor")]
    #[kv("anchor")]
    pub gravity: String,

    /// Horizontal gravity (0.0 = left, 0.5 = center, 1.0 = right).
    ///
    /// When both `gravity_x` and `gravity_y` are set, they override the
    /// named `gravity` anchor. Use for precise positioning beyond the 9
    /// cardinal points (e.g., rule-of-thirds at 0.33).
    #[param(range(0.0..=1.0), default = 0.5, step = 0.01)]
    #[param(section = "Position", label = "Gravity X")]
    pub gravity_x: Option<f32>,

    /// Vertical gravity (0.0 = top, 0.5 = center, 1.0 = bottom).
    ///
    /// When both `gravity_x` and `gravity_y` are set, they override the
    /// named `gravity` anchor.
    #[param(range(0.0..=1.0), default = 0.5, step = 0.01)]
    #[param(section = "Position", label = "Gravity Y")]
    pub gravity_y: Option<f32>,

    /// Fill color for exterior padding regions added around the image.
    ///
    /// Used by pad modes (fit_pad, within_pad, pad_within) to fill the
    /// canvas area outside the image content. Does NOT affect pixels
    /// inside the image — use `matte_color` for alpha compositing.
    ///
    /// None = transparent. Accepts "transparent", "white", "black", or hex "#RRGGBB" / "#RRGGBBAA".
    #[param(default = "")]
    #[param(section = "Position", label = "Canvas Color")]
    #[kv("bgcolor", "canvas_color")]
    pub canvas_color: Option<String>,

    /// Background color for alpha compositing (matte behind transparent pixels).
    ///
    /// Applied during resize to prevent halo artifacts at transparent edges.
    /// Separate from `canvas_color` which fills exterior padding regions.
    /// When set, transparent pixels are composited against this color before
    /// the resampling kernel samples them.
    ///
    /// None = no matte (preserve transparency). "white" is common for JPEG output.
    #[param(default = "")]
    #[param(section = "Alpha", label = "Matte Color")]
    #[kv("matte", "matte_color", "s.matte")]
    pub matte_color: Option<String>,

    // ─── Resampling ───
    /// Downscale resampling filter. None = auto (Robidoux).
    ///
    /// 31 filters available: "robidoux", "robidoux_sharp", "robidoux_fast",
    /// "lanczos", "lanczos_sharp", "lanczos2", "lanczos2_sharp",
    /// "ginseng", "ginseng_sharp", "mitchell", "catmull_rom", "cubic",
    /// "cubic_sharp", "cubic_b_spline", "hermite", "triangle", "linear",
    /// "box", "fastest", "jinc", "n_cubic", "n_cubic_sharp", etc.
    #[param(default = "")]
    #[param(section = "Quality", label = "Downscale Filter")]
    #[kv("down.filter")]
    pub down_filter: Option<String>,

    /// Upscale resampling filter. None = auto (Ginseng).
    ///
    /// Same filter names as down_filter.
    #[param(default = "")]
    #[param(section = "Quality", label = "Upscale Filter")]
    #[kv("up.filter")]
    pub up_filter: Option<String>,

    /// Color space for resampling math. None = auto ("linear" for most operations).
    ///
    /// - "linear" — resize in linear light (gamma-correct, default)
    /// - "srgb" — resize in sRGB gamma space (faster, less correct)
    ///
    /// RIAPI: `?down.colorspace=linear` or `?up.colorspace=srgb`
    #[param(default = "")]
    #[param(section = "Quality", label = "Scaling Colorspace")]
    #[kv("down.colorspace", "up.colorspace")]
    pub scaling_colorspace: Option<String>,

    // ─── Post-Processing ───
    /// Post-resize unsharp mask strength (0 = none, 100 = maximum).
    ///
    /// Applied as a separate pass AFTER resampling. Adds real computational
    /// cost proportional to output dimensions. For zero-cost sharpening
    /// that adjusts the resampling kernel itself, use `kernel_lobe_ratio`.
    ///
    /// None = no unsharp mask.
    #[param(range(0.0..=100.0), default = 0.0, step = 1.0)]
    #[param(unit = "%", section = "Post-Processing", label = "Unsharp Mask")]
    #[kv("f.sharpen", "unsharp")]
    pub unsharp_percent: Option<f32>,

    // ─── Kernel Shape ───
    /// Negative-lobe ratio for kernel sharpening (zero-cost).
    ///
    /// Adjusts the resampling kernel's negative lobes during weight
    /// computation. 0.0 = flatten (maximum smoothness), above filter's
    /// natural ratio = sharpen. Zero additional cost — changes the
    /// filter shape, not a separate processing step.
    ///
    /// For post-resize unsharp mask (separate pass), use `unsharp_percent`.
    #[param(range(0.0..=1.0), default = 0.0, step = 0.01)]
    #[param(section = "Kernel Shape", label = "Lobe Ratio")]
    #[kv("lobe_ratio", "kernel_lobe_ratio")]
    pub kernel_lobe_ratio: Option<f32>,

    /// Kernel width scale factor (zero-cost).
    ///
    /// Multiplies the resampling kernel window width. >1.0 = softer
    /// (wider window, less aliasing), <1.0 = sharper (narrower, aliasing risk).
    /// Combined with blur. Zero additional cost.
    #[param(range(0.1..=4.0), default = 1.0, step = 0.01)]
    #[param(section = "Kernel Shape", label = "Width Scale")]
    pub kernel_width_scale: Option<f32>,

    /// Post-resize Gaussian blur sigma (real cost).
    ///
    /// Applied as a separable H+V pass after resize. NOT equivalent to
    /// `kernel_width_scale` (which changes the kernel itself at zero cost).
    #[param(range(0.0..=100.0), default = 0.0, step = 0.1)]
    #[param(section = "Post-Processing", label = "Gaussian Blur")]
    pub post_blur: Option<f32>,

    /// When to apply resampling.
    ///
    /// - "size_differs" — only resample when dimensions change (default)
    /// - "size_differs_or_sharpening_requested" — resample when dimensions change
    ///   or when sharpening is requested (allows sharpening without resize)
    /// - "always" — always resample, even at identity dimensions
    #[param(default = "size_differs")]
    #[param(section = "Advanced", label = "Resample When")]
    #[kv("resample_when")]
    pub resample_when: Option<String>,

    /// When to apply sharpening.
    ///
    /// - "downscaling" — sharpen only when downscaling (default)
    /// - "upscaling" — sharpen only when upscaling
    /// - "size_differs" — sharpen whenever dimensions change
    /// - "always" — always sharpen, even at identity dimensions
    #[param(default = "downscaling")]
    #[param(section = "Advanced", label = "Sharpen When")]
    #[kv("sharpen_when", "f.sharpen_when")]
    pub sharpen_when: Option<String>,
}

impl Default for Constrain {
    fn default() -> Self {
        Self {
            w: None,
            h: None,
            max_w: None,
            max_h: None,
            mode: String::from("within"),
            scale: None,
            zoom: None,
            gravity: String::from("center"),
            gravity_x: None,
            gravity_y: None,
            canvas_color: None,
            matte_color: None,
            down_filter: None,
            up_filter: None,
            scaling_colorspace: None,
            unsharp_percent: None,
            kernel_lobe_ratio: None,
            kernel_width_scale: None,
            post_blur: None,
            resample_when: None,
            sharpen_when: None,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  RESIZE — forced resize to exact dimensions
// ═══════════════════════════════════════════════════════════════════════

/// Forced resize to exact dimensions (no layout planning).
///
/// Unlike [`Constrain`] which applies constraint modes (fit, crop, pad),
/// this resizes unconditionally to the specified width × height. Used when
/// the caller has already determined the target dimensions.
///
/// Skipped at compile time when input dimensions match target (identity resize).
///
/// JSON: `{ "w": 400, "h": 300, "filter": "robidoux" }`
/// RIAPI: Not directly exposed — use Constrain for querystring-driven resize.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenresize.resize", group = Geometry, role = Resize)]
#[node(coalesce = "layout_plan", changes_dimensions)]
#[node(tags("resize", "scale", "resample"))]
pub struct Resize {
    /// Target width in pixels.
    #[param(range(1..=65535), default = 1, step = 1)]
    #[param(unit = "px", section = "Dimensions", label = "Width")]
    pub w: u32,

    /// Target height in pixels.
    #[param(range(1..=65535), default = 1, step = 1)]
    #[param(unit = "px", section = "Dimensions", label = "Height")]
    pub h: u32,

    /// Resampling filter name (e.g., "robidoux", "lanczos", "mitchell").
    /// Empty string = default (Robidoux).
    #[param(default = "")]
    #[param(section = "Quality", label = "Filter")]
    pub filter: String,

    /// Post-resize sharpening percentage (0–100). 0 = no sharpening.
    #[param(range(0.0..=100.0), default = 0.0, step = 1.0)]
    #[param(unit = "%", section = "Quality", label = "Sharpen")]
    pub sharpen: f32,
}

// ═══════════════════════════════════════════════════════════════════════
//  PIPELINE — operations that combine analysis, canvas, or format concerns
// ═══════════════════════════════════════════════════════════════════════

/// Detect and crop uniform borders (whitespace trimming).
///
/// Materializes the upstream image, scans inward from each edge to find
/// where pixel values diverge from the border color, then crops to the
/// detected content bounds plus optional padding.
///
/// RIAPI: `?trim.threshold=80&trim.percentpadding=0.5`
/// JSON: `{ "threshold": 80, "percent_padding": 0.5 }`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenpipe.crop_whitespace", group = Analysis, role = Resize)]
#[node(changes_dimensions)]
#[node(tags("crop", "whitespace", "trim", "content", "analysis"))]
pub struct CropWhitespace {
    /// Color distance threshold (0–255).
    ///
    /// Pixels within this distance of the border color are considered
    /// "whitespace". Lower = stricter, higher = more tolerant.
    #[param(range(0..=255), default = 80, step = 1)]
    #[param(section = "Main", label = "Threshold")]
    #[kv("trim.threshold")]
    pub threshold: u32,

    /// Padding around detected content as a percentage of content dimensions.
    ///
    /// 0.0 = tight crop, 0.5 = 0.5% padding on each side.
    #[param(range(0.0..=50.0), default = 0.0, step = 0.1)]
    #[param(unit = "%", section = "Main", label = "Padding")]
    #[kv("trim.percentpadding")]
    pub percent_padding: f32,
}

/// Content-aware smart crop using focus rectangles.
///
/// Materializes the upstream image, computes the optimal crop rectangle
/// based on focus regions and target aspect ratio, then crops. Uses
/// `zenlayout::smart_crop::compute_crop` for the crop computation.
///
/// Not directly addressable from RIAPI — created programmatically by
/// `expand_zen()` when `c.focus` specifies rectangle coordinates.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenpipe.smart_crop_analyze", group = Analysis, role = Analysis)]
#[node(changes_dimensions)]
#[node(tags("crop", "smart", "focus", "analysis"))]
pub struct SmartCropAnalyze {
    /// Focus rectangles as a comma-separated list of percentage coordinates.
    ///
    /// Groups of 4 values: x1,y1,x2,y2 (0-100 range). Multiple rects are
    /// concatenated: "20,30,80,90,10,10,40,40" = two rects.
    #[param(default = "")]
    #[param(section = "Main", label = "Focus Rects")]
    pub rects_csv: String,

    /// Target width for aspect ratio computation.
    #[param(range(0..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Main", label = "Target Width")]
    pub target_w: u32,

    /// Target height for aspect ratio computation.
    #[param(range(0..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Main", label = "Target Height")]
    pub target_h: u32,

    /// Whether to use maximal (tight/zoom) crop mode instead of minimal.
    #[param(default = false)]
    #[param(section = "Main", label = "Zoom")]
    pub zoom: bool,
}

/// Fill a rectangle with a solid color.
///
/// Materializes the upstream image, draws the rectangle, then re-streams.
///
/// JSON: `{ "x1": 10, "y1": 10, "x2": 100, "y2": 100, "color": [255, 0, 0, 255] }`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenpipe.fill_rect", group = Canvas, role = Filter)]
#[node(tags("fill", "rect", "draw", "canvas"))]
pub struct FillRect {
    #[param(range(0..=65535), default = 0)]
    pub x1: u32,
    #[param(range(0..=65535), default = 0)]
    pub y1: u32,
    #[param(range(0..=65535), default = 0)]
    pub x2: u32,
    #[param(range(0..=65535), default = 0)]
    pub y2: u32,
    /// Fill color red channel.
    #[param(range(0..=255), default = 0)]
    pub color_r: u32,
    /// Fill color green channel.
    #[param(range(0..=255), default = 0)]
    pub color_g: u32,
    /// Fill color blue channel.
    #[param(range(0..=255), default = 0)]
    pub color_b: u32,
    /// Fill color alpha channel.
    #[param(range(0..=255), default = 255)]
    pub color_a: u32,
}

impl FillRect {
    /// Get the fill color as [R, G, B, A] bytes.
    pub fn color(&self) -> [u8; 4] {
        [
            self.color_r as u8,
            self.color_g as u8,
            self.color_b as u8,
            self.color_a as u8,
        ]
    }
}

/// Remove alpha channel by compositing onto a solid matte color.
///
/// Produces RGB output suitable for JPEG encoding. The compositing is done
/// in sRGB space (matching browser behavior for CSS background-color).
///
/// JSON: `{ "matte_r": 255, "matte_g": 255, "matte_b": 255 }`
#[derive(Node, Clone, Debug)]
#[node(id = "zenpipe.remove_alpha", group = Color, role = Filter)]
#[node(tags("alpha", "matte", "composite", "flatten"))]
pub struct RemoveAlpha {
    /// Matte red channel (sRGB, 0–255).
    #[param(range(0..=255), default = 255)]
    pub matte_r: u32,
    /// Matte green channel (sRGB, 0–255).
    #[param(range(0..=255), default = 255)]
    pub matte_g: u32,
    /// Matte blue channel (sRGB, 0–255).
    #[param(range(0..=255), default = 255)]
    pub matte_b: u32,
}

impl Default for RemoveAlpha {
    fn default() -> Self {
        Self {
            matte_r: 255,
            matte_g: 255,
            matte_b: 255,
        }
    }
}

impl RemoveAlpha {
    /// Get the matte color as [R, G, B] bytes.
    pub fn matte(&self) -> [u8; 3] {
        [self.matte_r as u8, self.matte_g as u8, self.matte_b as u8]
    }
}

/// Apply rounded corners with anti-aliased masking.
///
/// Generates a `RoundedRectMask` (via zenblend) and applies it to the
/// alpha channel. Transparent corners reveal the background color, or
/// remain transparent for PNG/WebP/AVIF output.
///
/// Supports uniform radius, per-corner radii, percentage-based radii,
/// and circle mode (elliptical crop for non-square images).
///
/// JSON: `{ "radius": 20.0, "bg_color": [0, 0, 0, 0] }`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenpipe.round_corners", group = Canvas, role = Filter)]
#[node(tags("corners", "rounded", "mask", "border-radius"))]
pub struct RoundCorners {
    /// Corner radius (uniform). Interpreted per `mode`: pixels when
    /// "pixels" (default), percent of min(width, height)/2 when
    /// "percentage".
    ///
    /// RIAPI `s.roundcorners` is handled by the ROUNDCORNERS_RIAPI adapter
    /// (1 or 4 comma-separated values, PERCENTAGE semantics like
    /// ImageResizer/imageflow — a bare `#[kv]` here would read the value as
    /// pixels and reject the 4-value form).
    #[param(range(0.0..=10000.0), default = 0.0, step = 1.0)]
    #[param(section = "Main", label = "Radius")]
    pub radius: f32,
    /// Top-left corner radius (for per-corner modes).
    #[param(range(0.0..=10000.0), default = -1.0, step = 1.0)]
    pub radius_tl: f32,
    /// Top-right corner radius (for per-corner modes).
    #[param(range(0.0..=10000.0), default = -1.0, step = 1.0)]
    pub radius_tr: f32,
    /// Bottom-left corner radius (for per-corner modes).
    #[param(range(0.0..=10000.0), default = -1.0, step = 1.0)]
    pub radius_bl: f32,
    /// Bottom-right corner radius (for per-corner modes).
    #[param(range(0.0..=10000.0), default = -1.0, step = 1.0)]
    pub radius_br: f32,
    /// Rounding mode: "pixels" (default), "percentage", "circle",
    /// "pixels_custom", "percentage_custom".
    #[param(default = "pixels")]
    pub mode: String,
    /// Background color red channel (for compositing transparent corners).
    #[param(range(0..=255), default = 0)]
    pub bg_r: u32,
    /// Background color green channel.
    #[param(range(0..=255), default = 0)]
    pub bg_g: u32,
    /// Background color blue channel.
    #[param(range(0..=255), default = 0)]
    pub bg_b: u32,
    /// Background color alpha channel. 0 = transparent (preserve alpha).
    #[param(range(0..=255), default = 0)]
    pub bg_a: u32,
}

impl RoundCorners {
    /// Get the background color as [R, G, B, A] bytes.
    pub fn bg_color(&self) -> [u8; 4] {
        [
            self.bg_r as u8,
            self.bg_g as u8,
            self.bg_b as u8,
            self.bg_a as u8,
        ]
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  COMPOSITING — nodes with multiple inputs (fan-in)
// ═══════════════════════════════════════════════════════════════════════

/// Composite a foreground image onto a background at a position.
///
/// Two inputs required:
/// - **background** (Canvas edge): the base image to draw onto
/// - **foreground** (Input edge): the image being composited
///
/// Both inputs are auto-converted to premultiplied linear f32.
/// Default blend mode is Porter-Duff source-over.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenpipe.composite", group = Composite, role = Composite)]
#[node(changes_dimensions)]
#[node(tags("composite", "blend", "overlay"))]
#[node(inputs(canvas("Background canvas"), input("Foreground image")))]
pub struct Composite {
    /// X position of the foreground on the background canvas.
    #[param(range(0..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Position", label = "X")]
    pub fg_x: u32,

    /// Y position of the foreground on the background canvas.
    #[param(range(0..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Position", label = "Y")]
    pub fg_y: u32,

    /// Blend mode: "source_over" (default), "multiply", "screen", etc.
    #[param(default = "source_over")]
    #[param(section = "Blending", label = "Blend Mode")]
    pub blend_mode: String,
}

/// Overlay a small image (watermark, logo) at absolute coordinates.
///
/// Single input (the background). The overlay image data is provided
/// via io_id (loaded from a separate input buffer).
///
/// Overlay is auto-converted to premultiplied linear f32. Opacity
/// scales the overlay's alpha channel before compositing.
#[derive(Node, Clone, Debug)]
#[node(id = "zenpipe.overlay", group = Composite, role = Composite)]
#[node(tags("overlay", "watermark", "logo", "composite"))]
#[node(inputs(input("Background image"), from_io("Overlay image (io_id)")))]
pub struct Overlay {
    /// I/O ID of the overlay image source.
    #[param(range(0..=1000), default = 0, step = 1)]
    #[param(section = "Source", label = "Overlay IO ID")]
    pub io_id: i32,

    /// X position on the background canvas.
    #[param(range(-65535..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Position", label = "X")]
    pub x: i32,

    /// Y position on the background canvas.
    #[param(range(-65535..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Position", label = "Y")]
    pub y: i32,

    /// Overlay opacity (0.0 = invisible, 1.0 = fully opaque).
    #[param(range(0.0..=1.0), default = 1.0, identity = 1.0, step = 0.01)]
    #[param(section = "Blending", label = "Opacity")]
    pub opacity: f32,

    /// Blend mode: "source_over" (default), "multiply", "screen", etc.
    #[param(default = "source_over")]
    #[param(section = "Blending", label = "Blend Mode")]
    pub blend_mode: String,
}

impl Default for Overlay {
    fn default() -> Self {
        Self {
            io_id: 0,
            x: 0,
            y: 0,
            opacity: 1.0,
            blend_mode: String::from("source_over"),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  RIAPI ADAPTERS — multi-value keys that produce different node types
// ═══════════════════════════════════════════════════════════════════════
//
// These implement NodeDef manually because the #[derive(Node)] macro
// can't handle keys that map to multiple different node structs
// (e.g., `flip=h` → FlipH, `flip=v` → FlipV, `flip=both` → both).

use zennode::{KvPairs, NodeDef, NodeError, NodeInstance, ParamMap};

/// RIAPI `flip` key → [`PostFlip`] node (post-resize flip, applied last).
///
/// Values: `h`/`x` (horizontal), `v`/`y` (vertical), `both`/`xy`, `none`.
/// The pre-crop source flip (`sflip`) is handled by [`SFLIP_RIAPI_DEF`],
/// which emits plain FlipH/FlipV nodes that sort into the source phase.
static FLIP_RIAPI_SCHEMA: NodeSchema = NodeSchema {
    id: "zenpipe.riapi.flip",
    label: "Flip (RIAPI)",
    description: "Flip image horizontally and/or vertically via querystring",
    group: zennode::NodeGroup::Geometry,
    role: zennode::NodeRole::Orient,
    params: &[],
    tags: &["flip", "riapi", "adapter"],
    coalesce: None,
    format: zennode::FormatHint {
        preferred: zennode::PixelFormatPreference::Srgb8,
        alpha: zennode::AlphaHandling::Process,
        changes_dimensions: false,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    deny_unknown_fields: false,
    inputs: &[],
};

pub struct FlipRiapiDef;
pub static FLIP_RIAPI_DEF: FlipRiapiDef = FlipRiapiDef;

impl NodeDef for FlipRiapiDef {
    fn schema(&self) -> &'static NodeSchema {
        &FLIP_RIAPI_SCHEMA
    }

    fn create(&self, _params: &ParamMap) -> core::result::Result<Box<dyn NodeInstance>, NodeError> {
        Err(NodeError::Other("use from_kv() for RIAPI flip".into()))
    }

    fn from_kv(
        &self,
        kv: &mut KvPairs,
    ) -> core::result::Result<Option<Box<dyn NodeInstance>>, NodeError> {
        let consumer = "zenpipe.riapi.flip";
        let val = kv.take_owned("flip", consumer);

        let Some(val) = val else {
            return Ok(None);
        };

        match parse_flip_value(&val) {
            Some((h, v)) if h || v => Ok(Some(Box::new(PostFlip {
                horizontal: h,
                vertical: v,
            }))),
            Some(_) => Ok(None), // "none"
            None => {
                kv.warn(
                    "flip",
                    zennode::kv::KvWarningKind::InvalidValue,
                    alloc::format!("unknown flip value '{val}', expected h/v/both/none"),
                );
                Ok(None)
            }
        }
    }
}

/// Parse a RIAPI flip value into (horizontal, vertical). `None` = invalid.
fn parse_flip_value(val: &str) -> Option<(bool, bool)> {
    match val.to_ascii_lowercase().as_str() {
        "h" | "x" => Some((true, false)),
        "v" | "y" => Some((false, true)),
        "both" | "xy" | "hv" => Some((true, true)),
        "none" | "" => Some((false, false)),
        _ => None,
    }
}

/// Parse a RIAPI rotate value: degrees snapped to the nearest multiple of 90.
/// Returns 0/90/180/270; warns when snapping changed the requested angle.
fn parse_rotate_degrees(kv: &mut KvPairs, key: &str, val: &str) -> i32 {
    let degrees = val.parse::<f32>().unwrap_or_else(|_| {
        kv.warn(
            key,
            zennode::kv::KvWarningKind::InvalidValue,
            alloc::format!("cannot parse '{val}' as degrees"),
        );
        0.0
    });
    let normalized = ((degrees % 360.0 + 360.0) % 360.0).round() as i32;
    let snapped = (((normalized + 45) / 90) * 90) % 360;
    if (normalized % 90) != 0 {
        kv.warn(
            key,
            zennode::kv::KvWarningKind::InvalidValue,
            alloc::format!("{key}={val} is not a multiple of 90; snapping to {snapped}"),
        );
    }
    snapped
}

/// Construct the source-phase rotate node for a snapped angle, if any.
fn source_rotate_node(snapped: i32) -> Option<Box<dyn NodeInstance>> {
    match snapped {
        90 => Some(Box::new(Rotate90 {})),
        180 => Some(Box::new(Rotate180 {})),
        270 => Some(Box::new(Rotate270 {})),
        _ => None,
    }
}

/// RIAPI `rotate` key → Rotate90 / Rotate180 / Rotate270 nodes.
///
/// Values: `90`, `180`, `270`, `360`/`0` (no-op).
static ROTATE_RIAPI_SCHEMA: NodeSchema = NodeSchema {
    id: "zenpipe.riapi.rotate",
    label: "Rotate (RIAPI)",
    description: "Rotate image by 90/180/270 degrees via querystring",
    group: zennode::NodeGroup::Geometry,
    role: zennode::NodeRole::Orient,
    params: &[],
    tags: &["rotate", "riapi", "adapter"],
    coalesce: None,
    format: zennode::FormatHint {
        preferred: zennode::PixelFormatPreference::Srgb8,
        alpha: zennode::AlphaHandling::Process,
        changes_dimensions: true,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    deny_unknown_fields: false,
    inputs: &[],
};

pub struct RotateRiapiDef;
pub static ROTATE_RIAPI_DEF: RotateRiapiDef = RotateRiapiDef;

impl NodeDef for RotateRiapiDef {
    fn schema(&self) -> &'static NodeSchema {
        &ROTATE_RIAPI_SCHEMA
    }

    fn create(&self, _params: &ParamMap) -> core::result::Result<Box<dyn NodeInstance>, NodeError> {
        Err(NodeError::Other("use from_kv() for RIAPI rotate".into()))
    }

    fn from_kv(
        &self,
        kv: &mut KvPairs,
    ) -> core::result::Result<Option<Box<dyn NodeInstance>>, NodeError> {
        let consumer = "zenpipe.riapi.rotate";
        let val = kv.take_owned("rotate", consumer);

        let Some(val) = val else {
            return Ok(None);
        };

        // RIAPI `rotate` is a POST-resize rotation (applied after filters and
        // padding, unlike `srotate`). Emits the dedicated PostRotate node so
        // riapi_order can place it at the end of the pipeline.
        match parse_rotate_degrees(kv, "rotate", &val) {
            0 => Ok(None),
            snapped => Ok(Some(Box::new(PostRotate { degrees: snapped }))),
        }
    }
}

/// RIAPI `autorotate` key → Orient node with EXIF auto-orientation.
///
/// `autorotate=true` means "apply EXIF orientation tag". The actual
/// EXIF value is read at decode time, not from the querystring.
/// We emit Orient with orientation=0 as a sentinel meaning "auto".
static AUTOROTATE_RIAPI_SCHEMA: NodeSchema = NodeSchema {
    id: "zenpipe.riapi.autorotate",
    label: "Auto-Rotate (RIAPI)",
    description: "Apply EXIF orientation correction via querystring",
    group: zennode::NodeGroup::Geometry,
    role: zennode::NodeRole::Orient,
    params: &[],
    tags: &["autorotate", "exif", "riapi", "adapter"],
    coalesce: None,
    format: zennode::FormatHint {
        preferred: zennode::PixelFormatPreference::Srgb8,
        alpha: zennode::AlphaHandling::Process,
        changes_dimensions: true,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    deny_unknown_fields: false,
    inputs: &[],
};

pub struct AutorotateRiapiDef;
pub static AUTOROTATE_RIAPI_DEF: AutorotateRiapiDef = AutorotateRiapiDef;

impl NodeDef for AutorotateRiapiDef {
    fn schema(&self) -> &'static NodeSchema {
        &AUTOROTATE_RIAPI_SCHEMA
    }

    fn create(&self, _params: &ParamMap) -> core::result::Result<Box<dyn NodeInstance>, NodeError> {
        Err(NodeError::Other(
            "use from_kv() for RIAPI autorotate".into(),
        ))
    }

    fn from_kv(
        &self,
        kv: &mut KvPairs,
    ) -> core::result::Result<Option<Box<dyn NodeInstance>>, NodeError> {
        let consumer = "zenpipe.riapi.autorotate";
        let val = kv.take_bool("autorotate", consumer);

        match val {
            Some(true) => {
                // orientation=0 is a sentinel for "use EXIF orientation at decode time"
                Ok(Some(Box::new(Orient { orientation: 0 })))
            }
            Some(false) | None => Ok(None),
        }
    }
}

/// RIAPI `autodeskew` key → [`AutoDeskew`] node (zenpipe#27).
///
/// `autodeskew=true|1|yes|on` straightens with the default 10° budget; a
/// number above 1 (`autodeskew=7`) sets the budget in degrees (≤ 45);
/// `false|0|no|off` disables. Mode and detector keep the node defaults
/// (`inscribed`, `projection`).
static AUTODESKEW_RIAPI_SCHEMA: NodeSchema = NodeSchema {
    id: "zenpipe.riapi.autodeskew",
    label: "Auto-Deskew (RIAPI)",
    description: "Content-adaptive straightening via querystring",
    group: zennode::NodeGroup::Geometry,
    role: zennode::NodeRole::Orient,
    params: &[],
    tags: &["autodeskew", "deskew", "riapi", "adapter"],
    coalesce: None,
    format: zennode::FormatHint {
        preferred: zennode::PixelFormatPreference::Srgb8,
        alpha: zennode::AlphaHandling::Process,
        changes_dimensions: true,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    deny_unknown_fields: false,
    inputs: &[],
};

pub struct AutodeskewRiapiDef;
pub static AUTODESKEW_RIAPI_DEF: AutodeskewRiapiDef = AutodeskewRiapiDef;

impl NodeDef for AutodeskewRiapiDef {
    fn schema(&self) -> &'static NodeSchema {
        &AUTODESKEW_RIAPI_SCHEMA
    }

    fn create(&self, _params: &ParamMap) -> core::result::Result<Box<dyn NodeInstance>, NodeError> {
        Err(NodeError::Other(
            "use from_kv() for RIAPI autodeskew".into(),
        ))
    }

    fn from_kv(
        &self,
        kv: &mut KvPairs,
    ) -> core::result::Result<Option<Box<dyn NodeInstance>>, NodeError> {
        let consumer = "zenpipe.riapi.autodeskew";
        let Some(val) = kv.take_owned("autodeskew", consumer) else {
            return Ok(None);
        };
        let lowered = val.trim().to_ascii_lowercase();
        let max_angle = match lowered.as_str() {
            "" | "true" | "1" | "yes" | "on" => 10.0,
            "false" | "0" | "no" | "off" => return Ok(None),
            other => match other.parse::<f32>() {
                Ok(d) if d > 1.0 && d <= 45.0 => d,
                _ => {
                    kv.warn(
                        "autodeskew",
                        zennode::kv::KvWarningKind::InvalidValue,
                        alloc::format!(
                            "expected true/false or a max angle in degrees (1, 45], got '{val}'"
                        ),
                    );
                    return Ok(None);
                }
            },
        };
        Ok(Some(Box::new(AutoDeskew {
            max_angle,
            mode: "inscribed".into(),
            method: "projection".into(),
        })))
    }
}

/// RIAPI `frame` / `page` key → frame selection hint for the decoder.
///
/// Produces an Orient node with a special sentinel, or a dedicated
/// FrameSelect node if one is defined. For now, we store it as a
/// Decode-phase hint.
static FRAME_RIAPI_SCHEMA: NodeSchema = NodeSchema {
    id: "zenpipe.riapi.frame",
    label: "Frame Select (RIAPI)",
    description: "Select a specific frame from animated/multi-page images",
    group: zennode::NodeGroup::Decode,
    role: zennode::NodeRole::Decode,
    params: &[],
    tags: &["frame", "page", "animation", "riapi", "adapter"],
    coalesce: None,
    format: zennode::FormatHint {
        preferred: zennode::PixelFormatPreference::Srgb8,
        alpha: zennode::AlphaHandling::Process,
        changes_dimensions: false,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    deny_unknown_fields: false,
    inputs: &[],
};

/// Frame selection node — carries the frame index for the decoder.
#[derive(Clone)]
pub struct FrameSelect {
    pub frame: u32,
}

impl NodeInstance for FrameSelect {
    fn schema(&self) -> &'static NodeSchema {
        &FRAME_RIAPI_SCHEMA
    }
    fn to_params(&self) -> ParamMap {
        let mut m = ParamMap::new();
        m.insert("frame".into(), zennode::ParamValue::U32(self.frame));
        m
    }
    fn get_param(&self, name: &str) -> Option<zennode::ParamValue> {
        match name {
            "frame" => Some(zennode::ParamValue::U32(self.frame)),
            _ => None,
        }
    }
    fn set_param(&mut self, name: &str, value: zennode::ParamValue) -> bool {
        match name {
            "frame" => {
                if let Some(v) = value.as_u32() {
                    self.frame = v;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
    fn clone_boxed(&self) -> Box<dyn NodeInstance> {
        Box::new(self.clone())
    }
}

pub struct FrameRiapiDef;
pub static FRAME_RIAPI_DEF: FrameRiapiDef = FrameRiapiDef;

impl NodeDef for FrameRiapiDef {
    fn schema(&self) -> &'static NodeSchema {
        &FRAME_RIAPI_SCHEMA
    }

    fn create(&self, params: &ParamMap) -> core::result::Result<Box<dyn NodeInstance>, NodeError> {
        let frame = params.get("frame").and_then(|v| v.as_u32()).unwrap_or(0);
        Ok(Box::new(FrameSelect { frame }))
    }

    fn from_kv(
        &self,
        kv: &mut KvPairs,
    ) -> core::result::Result<Option<Box<dyn NodeInstance>>, NodeError> {
        let consumer = "zenpipe.riapi.frame";
        let frame = kv
            .take_u32("frame", consumer)
            .or_else(|| kv.take_u32("page", consumer));

        match frame {
            Some(f) => Ok(Some(Box::new(FrameSelect { frame: f }))),
            None => Ok(None),
        }
    }
}

/// RIAPI `crop` / `c` keys → CropPercent node.
///
/// `crop=x1,y1,x2,y2` — four comma-separated coordinates, interpreted
/// in units defined by `cropxunits` / `cropyunits` (default 100).
/// `c=x1,y1,x2,y2` — strict alias that auto-sets units to 100.
///
/// Coordinates are edge-based: (x1,y1) = top-left, (x2,y2) = bottom-right.
/// Converted to CropPercent (origin + size as fractions of source).
static CROP_RIAPI_SCHEMA: NodeSchema = NodeSchema {
    id: "zenpipe.riapi.crop",
    label: "Crop (RIAPI)",
    description: "Crop image to rectangle via querystring coordinates",
    group: zennode::NodeGroup::Geometry,
    role: zennode::NodeRole::Orient,
    params: &[],
    tags: &["crop", "riapi", "adapter"],
    coalesce: None,
    format: zennode::FormatHint {
        preferred: zennode::PixelFormatPreference::Srgb8,
        alpha: zennode::AlphaHandling::Process,
        changes_dimensions: true,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    deny_unknown_fields: false,
    inputs: &[],
};

pub struct CropRiapiDef;
pub static CROP_RIAPI_DEF: CropRiapiDef = CropRiapiDef;

impl NodeDef for CropRiapiDef {
    fn schema(&self) -> &'static NodeSchema {
        &CROP_RIAPI_SCHEMA
    }

    fn create(&self, _params: &ParamMap) -> core::result::Result<Box<dyn NodeInstance>, NodeError> {
        Err(NodeError::Other("use from_kv() for RIAPI crop".into()))
    }

    fn from_kv(
        &self,
        kv: &mut KvPairs,
    ) -> core::result::Result<Option<Box<dyn NodeInstance>>, NodeError> {
        let consumer = "zenpipe.riapi.crop";

        // Try `c` first (strict format, auto-sets units to 100), then `crop`.
        let (val, force_100) = if let Some(v) = kv.take_owned("c", consumer) {
            (v, true)
        } else if let Some(v) = kv.take_owned("crop", consumer) {
            (v, false)
        } else {
            return Ok(None);
        };

        // Parse "x1,y1,x2,y2" — four comma-separated floats.
        let parts: Vec<&str> = val.split(',').collect();
        if parts.len() != 4 {
            kv.warn(
                "crop",
                zennode::kv::KvWarningKind::InvalidValue,
                alloc::format!(
                    "crop requires 4 comma-separated values, got {}",
                    parts.len()
                ),
            );
            return Ok(None);
        }

        let parse_f = |s: &str, idx: usize| -> core::result::Result<f32, NodeError> {
            s.trim().parse::<f32>().map_err(|_| {
                NodeError::Other(alloc::format!(
                    "cannot parse crop coordinate[{idx}] '{s}' as f32"
                ))
            })
        };

        let x1 = parse_f(parts[0], 0)?;
        let y1 = parse_f(parts[1], 1)?;
        let x2 = parse_f(parts[2], 2)?;
        let y2 = parse_f(parts[3], 3)?;

        // Read coordinate units. `c` always uses 100 (percent).
        //
        // RIAPI semantics (matching imageflow_riapi layout.rs:729-775):
        // units of 0 or absent mean SOURCE PIXELS — the coordinate space is
        // the image dimensions, which are unknown at parse time. So the raw
        // coordinates + units are carried in a RiapiCrop node and resolved
        // at the geometry bridge, where source dimensions exist. That is
        // also where negative (bottom-right-relative) coordinates, clamping,
        // and inverted-rect reset are applied.
        let cropxunits = if force_100 {
            // Consume but ignore if present.
            let _ = kv.take_f32("cropxunits", consumer);
            100.0
        } else {
            kv.take_f32("cropxunits", consumer).unwrap_or(0.0)
        };
        let cropyunits = if force_100 {
            let _ = kv.take_f32("cropyunits", consumer);
            100.0
        } else {
            kv.take_f32("cropyunits", consumer).unwrap_or(0.0)
        };

        if !cropxunits.is_finite()
            || !cropyunits.is_finite()
            || cropxunits < 0.0
            || cropyunits < 0.0
        {
            kv.warn(
                "crop",
                zennode::kv::KvWarningKind::InvalidValue,
                "cropxunits and cropyunits must be finite and non-negative",
            );
            return Ok(None);
        }

        Ok(Some(Box::new(RiapiCrop {
            x1,
            y1,
            x2,
            y2,
            xunits: cropxunits,
            yunits: cropyunits,
        })))
    }
}

// ─── RiapiCrop: deferred-units crop carrier ───

/// Raw RIAPI crop coordinates awaiting source dimensions.
///
/// `xunits`/`yunits` of `0.0` mean "source pixels" (the IR4 default when
/// `cropxunits`/`cropyunits` are absent). The geometry bridge resolves this
/// into a pixel window using imageflow's rules: scale by `dim/units`,
/// negative coordinates are bottom/right-relative, values clamp to the
/// image, and an empty/inverted rectangle resets to the full image.
#[derive(Clone, Debug)]
pub struct RiapiCrop {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    /// Coordinate space width for x1/x2. 0.0 = source width (pixels).
    pub xunits: f32,
    /// Coordinate space height for y1/y2. 0.0 = source height (pixels).
    pub yunits: f32,
}

static RIAPI_CROP_CARRIER_SCHEMA: NodeSchema = NodeSchema {
    id: "zenpipe.riapi_crop",
    label: "Crop (RIAPI, deferred units)",
    description: "Raw RIAPI crop window; units resolved against source dimensions at the bridge",
    group: zennode::NodeGroup::Geometry,
    role: zennode::NodeRole::Orient,
    params: &[],
    tags: &["crop", "riapi", "adapter"],
    // Must join the layout_plan run — geometry fusion is the ONLY place
    // that can resolve the deferred units (it has source dimensions).
    coalesce: Some(zennode::CoalesceInfo {
        group: "layout_plan",
        fusable: true,
        is_target: false,
    }),
    format: zennode::FormatHint {
        preferred: zennode::PixelFormatPreference::Srgb8,
        alpha: zennode::AlphaHandling::Process,
        changes_dimensions: true,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    deny_unknown_fields: false,
    inputs: &[],
};

impl NodeInstance for RiapiCrop {
    fn schema(&self) -> &'static NodeSchema {
        &RIAPI_CROP_CARRIER_SCHEMA
    }
    fn to_params(&self) -> ParamMap {
        let mut m = ParamMap::new();
        m.insert("x1".into(), zennode::ParamValue::F32(self.x1));
        m.insert("y1".into(), zennode::ParamValue::F32(self.y1));
        m.insert("x2".into(), zennode::ParamValue::F32(self.x2));
        m.insert("y2".into(), zennode::ParamValue::F32(self.y2));
        m.insert("xunits".into(), zennode::ParamValue::F32(self.xunits));
        m.insert("yunits".into(), zennode::ParamValue::F32(self.yunits));
        m
    }
    fn get_param(&self, name: &str) -> Option<zennode::ParamValue> {
        match name {
            "x1" => Some(zennode::ParamValue::F32(self.x1)),
            "y1" => Some(zennode::ParamValue::F32(self.y1)),
            "x2" => Some(zennode::ParamValue::F32(self.x2)),
            "y2" => Some(zennode::ParamValue::F32(self.y2)),
            "xunits" => Some(zennode::ParamValue::F32(self.xunits)),
            "yunits" => Some(zennode::ParamValue::F32(self.yunits)),
            _ => None,
        }
    }
    fn set_param(&mut self, name: &str, value: zennode::ParamValue) -> bool {
        let Some(v) = value.as_f32() else {
            return false;
        };
        match name {
            "x1" => self.x1 = v,
            "y1" => self.y1 = v,
            "x2" => self.x2 = v,
            "y2" => self.y2 = v,
            "xunits" => self.xunits = v,
            "yunits" => self.yunits = v,
            _ => return false,
        }
        true
    }
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
    fn clone_boxed(&self) -> Box<dyn NodeInstance> {
        Box::new(self.clone())
    }
}

// ─── PostRotate / PostFlip: post-resize orientation (RIAPI `rotate`/`flip`) ───

/// Post-resize rotation in degrees (90/180/270). RIAPI `rotate=`.
///
/// Applied at the END of the pipeline (after filters and padding), unlike
/// `srotate` which rotates the source before cropping.
#[derive(Clone, Debug)]
pub struct PostRotate {
    /// Normalized to 90, 180, or 270.
    pub degrees: i32,
}

static POST_ROTATE_SCHEMA: NodeSchema = NodeSchema {
    id: "zenpipe.post_rotate",
    label: "Rotate (post-resize)",
    description: "Rotate the final image by 90/180/270 degrees (RIAPI `rotate`)",
    group: zennode::NodeGroup::Geometry,
    role: zennode::NodeRole::Orient,
    params: &[],
    tags: &["rotate", "riapi", "post"],
    coalesce: Some(zennode::CoalesceInfo {
        group: "layout_plan",
        fusable: true,
        is_target: false,
    }),
    format: zennode::FormatHint {
        preferred: zennode::PixelFormatPreference::Srgb8,
        alpha: zennode::AlphaHandling::Process,
        changes_dimensions: true,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    deny_unknown_fields: false,
    inputs: &[],
};

impl NodeInstance for PostRotate {
    fn schema(&self) -> &'static NodeSchema {
        &POST_ROTATE_SCHEMA
    }
    fn to_params(&self) -> ParamMap {
        let mut m = ParamMap::new();
        m.insert("degrees".into(), zennode::ParamValue::I32(self.degrees));
        m
    }
    fn get_param(&self, name: &str) -> Option<zennode::ParamValue> {
        match name {
            "degrees" => Some(zennode::ParamValue::I32(self.degrees)),
            _ => None,
        }
    }
    fn set_param(&mut self, name: &str, value: zennode::ParamValue) -> bool {
        if name == "degrees"
            && let Some(v) = value.as_i32()
        {
            self.degrees = v;
            return true;
        }
        false
    }
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
    fn clone_boxed(&self) -> Box<dyn NodeInstance> {
        Box::new(self.clone())
    }
}

/// Post-resize flip. RIAPI `flip=`.
#[derive(Clone, Debug)]
pub struct PostFlip {
    pub horizontal: bool,
    pub vertical: bool,
}

static POST_FLIP_SCHEMA: NodeSchema = NodeSchema {
    id: "zenpipe.post_flip",
    label: "Flip (post-resize)",
    description: "Flip the final image horizontally and/or vertically (RIAPI `flip`)",
    group: zennode::NodeGroup::Geometry,
    role: zennode::NodeRole::Orient,
    params: &[],
    tags: &["flip", "riapi", "post"],
    coalesce: Some(zennode::CoalesceInfo {
        group: "layout_plan",
        fusable: true,
        is_target: false,
    }),
    format: zennode::FormatHint {
        preferred: zennode::PixelFormatPreference::Srgb8,
        alpha: zennode::AlphaHandling::Process,
        changes_dimensions: false,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    deny_unknown_fields: false,
    inputs: &[],
};

impl NodeInstance for PostFlip {
    fn schema(&self) -> &'static NodeSchema {
        &POST_FLIP_SCHEMA
    }
    fn to_params(&self) -> ParamMap {
        let mut m = ParamMap::new();
        m.insert(
            "horizontal".into(),
            zennode::ParamValue::Bool(self.horizontal),
        );
        m.insert("vertical".into(), zennode::ParamValue::Bool(self.vertical));
        m
    }
    fn get_param(&self, name: &str) -> Option<zennode::ParamValue> {
        match name {
            "horizontal" => Some(zennode::ParamValue::Bool(self.horizontal)),
            "vertical" => Some(zennode::ParamValue::Bool(self.vertical)),
            _ => None,
        }
    }
    fn set_param(&mut self, name: &str, value: zennode::ParamValue) -> bool {
        let Some(v) = value.as_bool() else {
            return false;
        };
        match name {
            "horizontal" => self.horizontal = v,
            "vertical" => self.vertical = v,
            _ => return false,
        }
        true
    }
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
    fn clone_boxed(&self) -> Box<dyn NodeInstance> {
        Box::new(self.clone())
    }
}

// ─── srotate / sflip adapters (SOURCE rotate/flip, pre-crop) ───

/// RIAPI `srotate` key → source-phase Rotate90/180/270 nodes.
static SROTATE_RIAPI_SCHEMA: NodeSchema = NodeSchema {
    id: "zenpipe.riapi.srotate",
    label: "Source Rotate (RIAPI)",
    description: "Rotate the source by 90/180/270 degrees before cropping (RIAPI `srotate`)",
    group: zennode::NodeGroup::Geometry,
    role: zennode::NodeRole::Orient,
    params: &[],
    tags: &["rotate", "riapi", "adapter", "source"],
    coalesce: None,
    format: zennode::FormatHint {
        preferred: zennode::PixelFormatPreference::Srgb8,
        alpha: zennode::AlphaHandling::Process,
        changes_dimensions: true,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    deny_unknown_fields: false,
    inputs: &[],
};

pub struct SrotateRiapiDef;
pub static SROTATE_RIAPI_DEF: SrotateRiapiDef = SrotateRiapiDef;

impl NodeDef for SrotateRiapiDef {
    fn schema(&self) -> &'static NodeSchema {
        &SROTATE_RIAPI_SCHEMA
    }

    fn create(&self, _params: &ParamMap) -> core::result::Result<Box<dyn NodeInstance>, NodeError> {
        Err(NodeError::Other("use from_kv() for RIAPI srotate".into()))
    }

    fn from_kv(
        &self,
        kv: &mut KvPairs,
    ) -> core::result::Result<Option<Box<dyn NodeInstance>>, NodeError> {
        let consumer = "zenpipe.riapi.srotate";
        let val = kv.take_owned("srotate", consumer);
        let Some(val) = val else {
            return Ok(None);
        };
        Ok(source_rotate_node(parse_rotate_degrees(
            kv, "srotate", &val,
        )))
    }
}

/// RIAPI `sflip` key → source-phase FlipH/FlipV nodes (pre-crop).
static SFLIP_RIAPI_SCHEMA: NodeSchema = NodeSchema {
    id: "zenpipe.riapi.sflip",
    label: "Source Flip (RIAPI)",
    description: "Flip the source before cropping (RIAPI `sflip`)",
    group: zennode::NodeGroup::Geometry,
    role: zennode::NodeRole::Orient,
    params: &[],
    tags: &["flip", "riapi", "adapter", "source"],
    coalesce: None,
    format: zennode::FormatHint {
        preferred: zennode::PixelFormatPreference::Srgb8,
        alpha: zennode::AlphaHandling::Process,
        changes_dimensions: false,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    deny_unknown_fields: false,
    inputs: &[],
};

pub struct SflipRiapiDef;
pub static SFLIP_RIAPI_DEF: SflipRiapiDef = SflipRiapiDef;

impl NodeDef for SflipRiapiDef {
    fn schema(&self) -> &'static NodeSchema {
        &SFLIP_RIAPI_SCHEMA
    }

    fn create(&self, _params: &ParamMap) -> core::result::Result<Box<dyn NodeInstance>, NodeError> {
        Err(NodeError::Other("use from_kv() for RIAPI sflip".into()))
    }

    fn from_kv(
        &self,
        kv: &mut KvPairs,
    ) -> core::result::Result<Option<Box<dyn NodeInstance>>, NodeError> {
        let consumer = "zenpipe.riapi.sflip";
        let val = kv
            .take_owned("sflip", consumer)
            .or_else(|| kv.take_owned("sourceflip", consumer));
        let Some(val) = val else {
            return Ok(None);
        };
        match parse_flip_value(&val) {
            Some((true, false)) => Ok(Some(Box::new(FlipH {}))),
            Some((false, true)) => Ok(Some(Box::new(FlipV {}))),
            // flip-H + flip-V = rotate 180° (pixel-identical, one pass).
            Some((true, true)) => Ok(Some(Box::new(Rotate180 {}))),
            Some((false, false)) => Ok(None),
            None => {
                kv.warn(
                    "sflip",
                    zennode::kv::KvWarningKind::InvalidValue,
                    alloc::format!("unknown sflip value '{val}', expected h/v/both/none"),
                );
                Ok(None)
            }
        }
    }
}

// ─── s.roundcorners adapter (percentage semantics, 1 or 4 values) ───

/// RIAPI `s.roundcorners` → [`RoundCorners`] with PERCENTAGE semantics.
///
/// ImageResizer/imageflow treat the value as a percentage (100 = full
/// rounding); one value applies to all corners, four values are
/// TL,TR,BR,BL. See `imageflow_riapi` layout.rs (RoundCornersMode::
/// Percentage / PercentageCustom).
static ROUNDCORNERS_RIAPI_SCHEMA: NodeSchema = NodeSchema {
    id: "zenpipe.riapi.roundcorners",
    label: "Round Corners (RIAPI)",
    description: "Rounded corners via querystring (percentage semantics, 1 or 4 values)",
    group: zennode::NodeGroup::Canvas,
    role: zennode::NodeRole::Filter,
    params: &[],
    tags: &["corners", "riapi", "adapter"],
    coalesce: None,
    format: zennode::FormatHint {
        preferred: zennode::PixelFormatPreference::Srgb8,
        alpha: zennode::AlphaHandling::ModifyAlpha,
        changes_dimensions: false,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    deny_unknown_fields: false,
    inputs: &[],
};

pub struct RoundcornersRiapiDef;
pub static ROUNDCORNERS_RIAPI_DEF: RoundcornersRiapiDef = RoundcornersRiapiDef;

impl NodeDef for RoundcornersRiapiDef {
    fn schema(&self) -> &'static NodeSchema {
        &ROUNDCORNERS_RIAPI_SCHEMA
    }

    fn create(&self, _params: &ParamMap) -> core::result::Result<Box<dyn NodeInstance>, NodeError> {
        Err(NodeError::Other(
            "use from_kv() for RIAPI s.roundcorners".into(),
        ))
    }

    fn from_kv(
        &self,
        kv: &mut KvPairs,
    ) -> core::result::Result<Option<Box<dyn NodeInstance>>, NodeError> {
        let consumer = "zenpipe.riapi.roundcorners";
        let Some(val) = kv.take_owned("s.roundcorners", consumer) else {
            return Ok(None);
        };

        let parts: Vec<f32> = val
            .split(',')
            .map(|p| p.trim().parse::<f32>())
            .collect::<Result<_, _>>()
            .unwrap_or_default();

        let clamp = |v: f32| v.clamp(0.0, 100.0);
        match parts.as_slice() {
            [v] if *v > 0.0 => {
                let rc = RoundCorners {
                    mode: "percentage".into(),
                    radius: clamp(*v),
                    ..RoundCorners::default()
                };
                Ok(Some(Box::new(rc)))
            }
            // IR4 order: top-left, top-right, bottom-right, bottom-left.
            [tl, tr, br, bl] => {
                let rc = RoundCorners {
                    mode: "percentage_custom".into(),
                    radius_tl: clamp(*tl),
                    radius_tr: clamp(*tr),
                    radius_br: clamp(*br),
                    radius_bl: clamp(*bl),
                    ..RoundCorners::default()
                };
                Ok(Some(Box::new(rc)))
            }
            [v] if *v <= 0.0 => Ok(None),
            _ => {
                kv.warn(
                    "s.roundcorners",
                    zennode::kv::KvWarningKind::InvalidValue,
                    alloc::format!(
                        "s.roundcorners expects 1 or 4 comma-separated numbers, got '{val}'"
                    ),
                );
                Ok(None)
            }
        }
    }
}

// ─── ignoreicc / ignore_icc_errors adapter (decode-side directives) ───

/// Decode-side ICC directives from the querystring.
///
/// `ignoreicc=true` discards the embedded color profile (decode as sRGB,
/// no CMS transform, no ICC in the output) — imageflow's
/// `DecoderCommand::DiscardColorProfile`. `ignore_icc_errors=true` tolerates
/// malformed profiles; the zen-native CMS path already degrades to
/// passthrough on transform failure, so this is accepted for compatibility
/// and recorded for hosts that want strictness.
#[derive(Clone, Debug, Default)]
pub struct IccDirectives {
    pub discard_profile: bool,
    pub ignore_errors: bool,
}

static ICC_RIAPI_SCHEMA: NodeSchema = NodeSchema {
    id: "zenpipe.riapi.icc",
    label: "ICC Directives (RIAPI)",
    description: "Discard or tolerate embedded ICC profiles via querystring",
    group: zennode::NodeGroup::Decode,
    role: zennode::NodeRole::Decode,
    params: &[],
    tags: &["icc", "cms", "riapi", "adapter"],
    coalesce: None,
    format: zennode::FormatHint {
        preferred: zennode::PixelFormatPreference::Any,
        alpha: zennode::AlphaHandling::Skip,
        changes_dimensions: false,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    deny_unknown_fields: false,
    inputs: &[],
};

impl NodeInstance for IccDirectives {
    fn schema(&self) -> &'static NodeSchema {
        &ICC_RIAPI_SCHEMA
    }
    fn to_params(&self) -> ParamMap {
        let mut m = ParamMap::new();
        m.insert(
            "discard_profile".into(),
            zennode::ParamValue::Bool(self.discard_profile),
        );
        m.insert(
            "ignore_errors".into(),
            zennode::ParamValue::Bool(self.ignore_errors),
        );
        m
    }
    fn get_param(&self, name: &str) -> Option<zennode::ParamValue> {
        match name {
            "discard_profile" => Some(zennode::ParamValue::Bool(self.discard_profile)),
            "ignore_errors" => Some(zennode::ParamValue::Bool(self.ignore_errors)),
            _ => None,
        }
    }
    fn set_param(&mut self, name: &str, value: zennode::ParamValue) -> bool {
        let Some(v) = value.as_bool() else {
            return false;
        };
        match name {
            "discard_profile" => self.discard_profile = v,
            "ignore_errors" => self.ignore_errors = v,
            _ => return false,
        }
        true
    }
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
    fn clone_boxed(&self) -> Box<dyn NodeInstance> {
        Box::new(self.clone())
    }
}

// ─── hdr / gain-map directives adapter (decode-side) ───

/// Decode-side HDR / gain-map policy from the querystring.
///
/// - `hdr=preserve` (default): keep the gain map; it rides through geometry
///   as a sidecar and re-embeds on supporting formats (JPEG/AVIF/JXL).
/// - `hdr=strip` (alias `sdr`): decode the SDR base image only and drop the
///   gain map from the output.
/// - `hdr=reconstruct`: apply the gain map at decode to produce HDR pixels
///   (`hdr.headroom` limits the target headroom in stops; 0 = full).
///   Requires the `job-ultrahdr` feature and a decoder that supports
///   reconstruction (JPEG UltraHDR today) — otherwise the job errors.
/// - `gainmap=preserve|discard` is accepted as a legacy alias.
#[derive(Clone, Debug)]
pub struct HdrDirectives {
    /// "preserve" | "strip" | "reconstruct"
    pub mode: alloc::string::String,
    /// Target headroom in stops for reconstruct mode. 0.0 = full headroom.
    pub headroom: f32,
}

static HDR_RIAPI_SCHEMA: NodeSchema = NodeSchema {
    id: "zenpipe.riapi.hdr",
    label: "HDR / Gain Map (RIAPI)",
    description: "Gain-map policy via querystring: preserve, strip, or reconstruct HDR",
    group: zennode::NodeGroup::Hdr,
    role: zennode::NodeRole::Decode,
    params: &[],
    tags: &["hdr", "gainmap", "riapi", "adapter"],
    coalesce: None,
    format: zennode::FormatHint {
        preferred: zennode::PixelFormatPreference::Any,
        alpha: zennode::AlphaHandling::Skip,
        changes_dimensions: false,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
    json_key: "",
    deny_unknown_fields: false,
    inputs: &[],
};

impl NodeInstance for HdrDirectives {
    fn schema(&self) -> &'static NodeSchema {
        &HDR_RIAPI_SCHEMA
    }
    fn to_params(&self) -> ParamMap {
        let mut m = ParamMap::new();
        m.insert("mode".into(), zennode::ParamValue::Str(self.mode.clone()));
        m.insert("headroom".into(), zennode::ParamValue::F32(self.headroom));
        m
    }
    fn get_param(&self, name: &str) -> Option<zennode::ParamValue> {
        match name {
            "mode" => Some(zennode::ParamValue::Str(self.mode.clone())),
            "headroom" => Some(zennode::ParamValue::F32(self.headroom)),
            _ => None,
        }
    }
    fn set_param(&mut self, name: &str, value: zennode::ParamValue) -> bool {
        match name {
            "mode" => {
                if let Some(v) = value.as_str() {
                    self.mode = v.into();
                    return true;
                }
                false
            }
            "headroom" => {
                if let Some(v) = value.as_f32() {
                    self.headroom = v;
                    return true;
                }
                false
            }
            _ => false,
        }
    }
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
    fn clone_boxed(&self) -> Box<dyn NodeInstance> {
        Box::new(self.clone())
    }
}

pub struct HdrRiapiDef;
pub static HDR_RIAPI_DEF: HdrRiapiDef = HdrRiapiDef;

impl NodeDef for HdrRiapiDef {
    fn schema(&self) -> &'static NodeSchema {
        &HDR_RIAPI_SCHEMA
    }

    fn create(&self, params: &ParamMap) -> core::result::Result<Box<dyn NodeInstance>, NodeError> {
        Ok(Box::new(HdrDirectives {
            mode: params
                .get("mode")
                .and_then(|v| v.as_str().map(alloc::string::String::from))
                .unwrap_or_else(|| "preserve".into()),
            headroom: params
                .get("headroom")
                .and_then(|v| v.as_f32())
                .unwrap_or(0.0),
        }))
    }

    fn from_kv(
        &self,
        kv: &mut KvPairs,
    ) -> core::result::Result<Option<Box<dyn NodeInstance>>, NodeError> {
        let consumer = "zenpipe.riapi.hdr";
        let hdr_val = kv.take_owned("hdr", consumer);
        let gainmap_val = kv.take_owned("gainmap", consumer);
        let headroom = kv.take_f32("hdr.headroom", consumer).unwrap_or(0.0);

        let mode = match (hdr_val, gainmap_val) {
            (Some(v), _) => match v.to_ascii_lowercase().as_str() {
                "preserve" | "keep" => "preserve",
                "strip" | "sdr" | "discard" => "strip",
                "reconstruct" | "hdr" => "reconstruct",
                other => {
                    kv.warn(
                        "hdr",
                        zennode::kv::KvWarningKind::InvalidValue,
                        alloc::format!(
                            "unknown hdr value '{other}', expected preserve/strip/reconstruct"
                        ),
                    );
                    return Ok(None);
                }
            },
            (None, Some(v)) => match v.to_ascii_lowercase().as_str() {
                "preserve" | "keep" | "true" => "preserve",
                "discard" | "strip" | "false" => "strip",
                other => {
                    kv.warn(
                        "gainmap",
                        zennode::kv::KvWarningKind::InvalidValue,
                        alloc::format!(
                            "unknown gainmap value '{other}', expected preserve/discard"
                        ),
                    );
                    return Ok(None);
                }
            },
            (None, None) => return Ok(None),
        };

        Ok(Some(Box::new(HdrDirectives {
            mode: mode.into(),
            headroom,
        })))
    }
}

pub struct IccRiapiDef;
pub static ICC_RIAPI_DEF: IccRiapiDef = IccRiapiDef;

impl NodeDef for IccRiapiDef {
    fn schema(&self) -> &'static NodeSchema {
        &ICC_RIAPI_SCHEMA
    }

    fn create(&self, params: &ParamMap) -> core::result::Result<Box<dyn NodeInstance>, NodeError> {
        Ok(Box::new(IccDirectives {
            discard_profile: params
                .get("discard_profile")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            ignore_errors: params
                .get("ignore_errors")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        }))
    }

    fn from_kv(
        &self,
        kv: &mut KvPairs,
    ) -> core::result::Result<Option<Box<dyn NodeInstance>>, NodeError> {
        let consumer = "zenpipe.riapi.icc";
        let discard = kv.take_bool("ignoreicc", consumer).unwrap_or(false);
        let ignore_errors = kv.take_bool("ignore_icc_errors", consumer).unwrap_or(false);
        if discard || ignore_errors {
            Ok(Some(Box::new(IccDirectives {
                discard_profile: discard,
                ignore_errors,
            })))
        } else {
            Ok(None)
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  REGISTRATION
// ═══════════════════════════════════════════════════════════════════════

/// Register all zenpipe-owned node definitions with a registry.
///
/// This includes geometry, resize, and pipeline-level nodes.
/// Codec, quantization, and quality-intent nodes are in zencodecs.
/// Filter nodes are in zenfilters.
pub fn register(registry: &mut NodeRegistry) {
    for node in ALL {
        registry.register(*node);
    }
}

/// All zenpipe zennode definitions (geometry, resize, pipeline, RIAPI adapters).
pub static ALL: &[&dyn NodeDef] = &[
    // Geometry
    &CROP_NODE,
    &CROP_PERCENT_NODE,
    &CROP_MARGINS_NODE,
    &ORIENT_NODE,
    &FLIP_H_NODE,
    &FLIP_V_NODE,
    &ROTATE90_NODE,
    &ROTATE180_NODE,
    &ROTATE270_NODE,
    &AUTO_DESKEW_NODE,
    &EXPAND_CANVAS_NODE,
    &REGION_VIEWPORT_NODE,
    // Layout
    &OUTPUT_LIMITS_NODE,
    &CONSTRAIN_NODE,
    // Resize
    &RESIZE_NODE,
    // Pipeline
    &CROP_WHITESPACE_NODE,
    &SMART_CROP_ANALYZE_NODE,
    &FILL_RECT_NODE,
    &REMOVE_ALPHA_NODE,
    &ROUND_CORNERS_NODE,
    // Compositing
    &COMPOSITE_NODE,
    &OVERLAY_NODE,
    // RIAPI adapters (multi-value keys → specific node types).
    // Order here is NOT execution order — querystring-parsed instances are
    // re-sorted into IR4 phase order by `crate::riapi::riapi_order`.
    &FLIP_RIAPI_DEF,
    &ROTATE_RIAPI_DEF,
    &SROTATE_RIAPI_DEF,
    &SFLIP_RIAPI_DEF,
    &AUTOROTATE_RIAPI_DEF,
    &AUTODESKEW_RIAPI_DEF,
    &FRAME_RIAPI_DEF,
    &CROP_RIAPI_DEF,
    &ROUNDCORNERS_RIAPI_DEF,
    &ICC_RIAPI_DEF,
    &HDR_RIAPI_DEF,
];
