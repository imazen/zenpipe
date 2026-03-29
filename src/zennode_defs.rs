//! Zennode definitions for all pipeline operations compiled by zenpipe.
//!
//! This is the single source of truth for every node that zenpipe's bridge
//! and graph compiler know how to handle. Geometry, resize, and pipeline-level
//! nodes all live here — no cross-crate schema ID races, no silent key
//! consumption conflicts.
//!
//! Codec nodes (zenjpeg, zenpng, zenwebp, …) and filter nodes (zenfilters)
//! remain in their own crates because zenpipe doesn't own those algorithms.
//!
//! # Sections
//!
//! - **Geometry** — crop, orient, flip, rotate, expand canvas, region viewport
//! - **Layout** — constrain, output limits
//! - **Resize** — forced resize to exact dimensions
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
/// RIAPI: `?autorotate=true` (uses embedded EXIF), `?srotate=90`
/// JSON: `{ "orientation": 6 }`
#[derive(Node, Clone, Debug)]
#[node(id = "zenlayout.orient", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan")]
#[node(tags("orient", "exif", "geometry"))]
pub struct Orient {
    /// EXIF orientation value (1-8). 1 = no transformation.
    #[param(range(1..=8), default = 1, step = 1)]
    #[param(section = "Main", label = "Orientation")]
    #[kv("autorotate", "srotate")]
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
/// Swaps width and height. NOT coalesced because 90/270 degree
/// rotations require pixel materialization (axis swap).
///
/// RIAPI: `?srotate=90`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenlayout.rotate_90", group = Geometry, role = Orient)]
#[node(changes_dimensions)]
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
/// Swaps width and height. NOT coalesced because 90/270 degree
/// rotations require pixel materialization (axis swap).
///
/// RIAPI: `?srotate=270`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenlayout.rotate_270", group = Geometry, role = Orient)]
#[node(changes_dimensions)]
#[node(tags("rotate", "geometry"))]
pub struct Rotate270 {}

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
}

impl Default for ExpandCanvas {
    fn default() -> Self {
        Self {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
            color: String::from("transparent"),
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
///   "down_filter": "lanczos", "sharpen": 15.0
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
    #[param(range(0..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Dimensions", label = "Width")]
    #[kv("w", "width")]
    pub w: Option<u32>,

    /// Target height in pixels. None = unconstrained (derive from width + aspect ratio).
    #[param(range(0..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Dimensions", label = "Height")]
    #[kv("h", "height")]
    pub h: Option<u32>,

    // ─── Layout ───
    /// Constraint mode controlling how the image fits the target dimensions.
    ///
    /// - "distort" — stretch to exact dimensions, ignoring aspect ratio
    /// - "within" — fit inside target, never upscale (default)
    /// - "fit" — fit inside target, may upscale
    /// - "within_crop" — fill target by cropping, never upscale
    /// - "fit_crop" — fill target by cropping, may upscale
    /// - "fit_pad" — fit inside target, pad to exact dimensions
    /// - "within_pad" — fit inside target without upscale, pad to exact dimensions
    /// - "pad_within" — never upscale, always pad to exact canvas
    /// - "aspect_crop" — crop to target aspect ratio without resizing
    #[param(default = "within")]
    #[param(section = "Layout", label = "Mode")]
    #[kv("mode")]
    pub mode: String,

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

    /// Canvas background color for pad modes.
    ///
    /// None = transparent. Accepts CSS-style hex or named colors:
    /// "white", "#FF0000", "000000FF".
    #[param(default = "")]
    #[param(section = "Position", label = "Canvas Color")]
    #[kv("bgcolor", "canvas_color")]
    pub canvas_color: Option<String>,

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
    #[param(default = "")]
    #[param(section = "Quality", label = "Scaling Colorspace")]
    pub scaling_colorspace: Option<String>,

    // ─── Sharpening & Blur ───
    /// Post-resize sharpening amount (0 = none, 100 = maximum).
    ///
    /// None = no sharpening. Applied as an unsharp mask after resize.
    #[param(range(0.0..=100.0), default = 0.0, step = 1.0)]
    #[param(unit = "%", section = "Quality", label = "Sharpen")]
    #[kv("f.sharpen")]
    pub sharpen: Option<f32>,

    /// Negative-lobe ratio for in-kernel sharpening.
    ///
    /// None = use filter's natural ratio. 0.0 = flatten (maximum smoothness).
    /// Above natural = sharpen. This is zero-cost (applied during weight computation).
    #[param(range(0.0..=1.0), default = 0.0, step = 0.01)]
    #[param(section = "Advanced", label = "Lobe Ratio")]
    pub lobe_ratio: Option<f32>,

    /// Kernel width scale factor. None = auto (1.0).
    ///
    /// > 1.0 = softer (widens filter window), < 1.0 = sharper (aliasing risk).
    /// > Combined with blur. This is zero-cost (applied during weight computation).
    #[param(range(0.1..=4.0), default = 1.0, step = 0.01)]
    #[param(section = "Advanced", label = "Kernel Width Scale")]
    pub kernel_width_scale: Option<f32>,

    /// Post-resize Gaussian blur sigma. None = no blur.
    ///
    /// Applied as a separable H+V pass after resize. Not equivalent to
    /// kernel_width_scale (which changes the resampling kernel itself).
    #[param(range(0.0..=100.0), default = 0.0, step = 0.1)]
    #[param(section = "Advanced", label = "Post Blur")]
    pub post_blur: Option<f32>,
}

impl Default for Constrain {
    fn default() -> Self {
        Self {
            w: None,
            h: None,
            mode: String::from("within"),
            gravity: String::from("center"),
            gravity_x: None,
            gravity_y: None,
            canvas_color: None,
            down_filter: None,
            up_filter: None,
            scaling_colorspace: None,
            sharpen: None,
            lobe_ratio: None,
            kernel_width_scale: None,
            post_blur: None,
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
#[node(changes_dimensions)]
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
    /// Corner radius in pixels (uniform). Clamped to min(width, height) / 2.
    /// Used when mode is "pixels" (default) or as fallback.
    #[param(range(0.0..=10000.0), default = 0.0, step = 1.0)]
    #[param(unit = "px", section = "Main", label = "Radius")]
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
//  REGISTRATION
// ═══════════════════════════════════════════════════════════════════════

/// Register all zenpipe-owned node definitions with a registry.
///
/// This includes geometry, resize, and pipeline-level nodes.
/// Codec nodes (JPEG, PNG, …) and filter nodes (zenfilters) register
/// themselves from their own crates.
pub fn register(registry: &mut NodeRegistry) {
    for node in ALL {
        registry.register(*node);
    }
}

/// All zenpipe zennode definitions.
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
    &EXPAND_CANVAS_NODE,
    &REGION_VIEWPORT_NODE,
    // Layout
    &OUTPUT_LIMITS_NODE,
    &CONSTRAIN_NODE,
    // Resize
    &RESIZE_NODE,
    // Pipeline
    &CROP_WHITESPACE_NODE,
    &FILL_RECT_NODE,
    &REMOVE_ALPHA_NODE,
    &ROUND_CORNERS_NODE,
];
