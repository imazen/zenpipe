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
/// source coordinates.
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

/// Crop using percentage-based coordinates.
///
/// All coordinates are fractions of source dimensions (0.0 = origin,
/// 1.0 = full extent). `x=0.1, y=0.1, w=0.8, h=0.8` removes 10%
/// from each edge.
///
/// JSON: `{ "x": 0.1, "y": 0.1, "w": 0.8, "h": 0.8 }`
#[derive(Node, Clone, Debug)]
#[node(id = "zenlayout.crop_percent", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan", changes_dimensions)]
#[node(tags("crop", "geometry"))]
pub struct CropPercent {
    /// Left edge as fraction of source width.
    #[param(range(0.0..=1.0), default = 0.0, step = 0.01)]
    #[param(section = "Main", label = "X")]
    pub x: f32,

    /// Top edge as fraction of source height.
    #[param(range(0.0..=1.0), default = 0.0, step = 0.01)]
    #[param(section = "Main", label = "Y")]
    pub y: f32,

    /// Width as fraction of source width.
    #[param(range(0.0..=1.0), default = 1.0, step = 0.01)]
    #[param(section = "Main", label = "Width")]
    pub w: f32,

    /// Height as fraction of source height.
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

/// Crop by removing percentage-based margins from each side.
///
/// CSS-style ordering: top, right, bottom, left.
///
/// JSON: `{ "top": 0.1, "right": 0.05, "bottom": 0.1, "left": 0.05 }`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenlayout.crop_margins", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan", changes_dimensions)]
#[node(tags("crop", "margins", "geometry"))]
pub struct CropMargins {
    #[param(range(0.0..=0.5), default = 0.0, step = 0.01)]
    #[param(section = "Main", label = "Top")]
    pub top: f32,

    #[param(range(0.0..=0.5), default = 0.0, step = 0.01)]
    #[param(section = "Main", label = "Right")]
    pub right: f32,

    #[param(range(0.0..=0.5), default = 0.0, step = 0.01)]
    #[param(section = "Main", label = "Bottom")]
    pub bottom: f32,

    #[param(range(0.0..=0.5), default = 0.0, step = 0.01)]
    #[param(section = "Main", label = "Left")]
    pub left: f32,
}

// ─── Orient ───

/// Apply EXIF orientation correction (values 1-8).
///
/// RIAPI: `?autorotate=true`, `?srotate=90`
/// JSON: `{ "orientation": 6 }`
#[derive(Node, Clone, Debug)]
#[node(id = "zenlayout.orient", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan")]
#[node(tags("orient", "exif", "geometry"))]
pub struct Orient {
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

// ─── FlipH / FlipV ───

/// Flip horizontally (mirror left-right). RIAPI: `?sflip=h`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenlayout.flip_h", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan")]
#[node(tags("flip", "geometry"))]
pub struct FlipH {}

/// Flip vertically (mirror top-bottom). RIAPI: `?sflip=v`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenlayout.flip_v", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan")]
#[node(tags("flip", "geometry"))]
pub struct FlipV {}

// ─── Rotate ───

/// Rotate 90° clockwise. Swaps width/height. RIAPI: `?srotate=90`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenlayout.rotate_90", group = Geometry, role = Orient)]
#[node(changes_dimensions)]
#[node(tags("rotate", "geometry"))]
pub struct Rotate90 {}

/// Rotate 180°. Coalesces (flip-H + flip-V, no axis swap). RIAPI: `?srotate=180`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenlayout.rotate_180", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan")]
#[node(tags("rotate", "geometry"))]
pub struct Rotate180 {}

/// Rotate 270° clockwise (90° CCW). Swaps width/height. RIAPI: `?srotate=270`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenlayout.rotate_270", group = Geometry, role = Orient)]
#[node(changes_dimensions)]
#[node(tags("rotate", "geometry"))]
pub struct Rotate270 {}

// ─── ExpandCanvas ───

/// Expand canvas by adding pixel padding around the image.
///
/// JSON: `{ "left": 10, "top": 10, "right": 10, "bottom": 10, "color": "white" }`
#[derive(Node, Clone, Debug)]
#[node(id = "zenlayout.expand_canvas", group = Canvas, role = Resize)]
#[node(coalesce = "layout_plan", changes_dimensions)]
#[node(tags("pad", "canvas", "geometry"))]
pub struct ExpandCanvas {
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Main", label = "Left")]
    pub left: u32,

    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Main", label = "Top")]
    pub top: u32,

    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Main", label = "Right")]
    pub right: u32,

    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Main", label = "Bottom")]
    pub bottom: u32,

    /// Fill color: "transparent", "white", "black", or hex "#RRGGBB" / "#RRGGBBAA".
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
/// Edge coordinates: `resolved = source_dim * pct + px`.
/// Smaller than source = crop. Beyond source = pad.
#[derive(Node, Clone, Debug)]
#[node(id = "zenlayout.region", group = Geometry, role = Orient)]
#[node(coalesce = "layout_plan", changes_dimensions)]
#[node(tags("region", "viewport", "crop", "pad", "geometry"))]
pub struct RegionViewport {
    #[param(range(-1.0..=2.0), default = 0.0, step = 0.01)]
    #[param(section = "Left", label = "Percent")]
    pub left_pct: f32,
    #[param(range(-65535..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Left", label = "Pixels")]
    pub left_px: i32,
    #[param(range(-1.0..=2.0), default = 0.0, step = 0.01)]
    #[param(section = "Top", label = "Percent")]
    pub top_pct: f32,
    #[param(range(-65535..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Top", label = "Pixels")]
    pub top_px: i32,
    #[param(range(-1.0..=2.0), default = 1.0, step = 0.01)]
    #[param(section = "Right", label = "Percent")]
    pub right_pct: f32,
    #[param(range(-65535..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Right", label = "Pixels")]
    pub right_px: i32,
    #[param(range(-1.0..=2.0), default = 1.0, step = 0.01)]
    #[param(section = "Bottom", label = "Percent")]
    pub bottom_pct: f32,
    #[param(range(-65535..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Bottom", label = "Pixels")]
    pub bottom_px: i32,
    /// Fill color for areas outside source.
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
/// Processing order: max (scale down) → min (scale up) → align (snap).
///
/// JSON: `{ "max_w": 4096, "max_h": 4096, "align_x": 16, "align_mode": "extend" }`
#[derive(Node, Clone, Debug)]
#[node(id = "zenlayout.output_limits", group = Layout, role = Resize)]
#[node(coalesce = "layout_plan", changes_dimensions)]
#[node(tags("limits", "alignment", "layout", "codec"))]
pub struct OutputLimits {
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Max", label = "Width")]
    pub max_w: u32,
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Max", label = "Height")]
    pub max_h: u32,
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Min", label = "Width")]
    pub min_w: u32,
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Min", label = "Height")]
    pub min_h: u32,
    /// Horizontal alignment multiple (0 = none, 8 = DCT, 16 = MCU).
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Align", label = "X Multiple")]
    pub align_x: u32,
    /// Vertical alignment multiple.
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Align", label = "Y Multiple")]
    pub align_y: u32,
    /// "crop", "extend" (default), or "distort".
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
/// Matches imageflow v2's Constrain API: combines layout geometry with
/// resampling hints in a single node. Flat parameter layout with
/// `Option<T>` for "not specified" / auto.
///
/// # Gravity
///
/// - **Named anchor** (`gravity`): "center", "top_left", etc.
/// - **Percentage** (`gravity_x`/`gravity_y`): 0.0–1.0, overrides named anchor.
///
/// # JSON API
///
/// ```json
/// {
///   "constrain": {
///     "w": 800, "h": 600, "mode": "fit_crop",
///     "gravity": "top_left",
///     "down_filter": "lanczos", "sharpen_percent": 15.0
///   }
/// }
/// ```
///
/// RIAPI: `?w=800&h=600&mode=fit_crop&anchor=top_left&down.filter=lanczos&f.sharpen=15`
#[derive(Node, Clone, Debug)]
#[node(id = "zenresize.constrain", json_key = "constrain")]
#[node(group = Geometry, role = Resize)]
#[node(coalesce = "layout_plan")]
#[node(changes_dimensions)]
#[node(format(preferred = LinearF32))]
#[node(tags("resize", "geometry", "scale", "constrain"))]
pub struct Constrain {
    // ─── Dimensions ───
    /// Target width in pixels. None = unconstrained.
    #[param(range(0..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Dimensions", label = "Width")]
    #[kv("w", "width")]
    pub w: Option<u32>,

    /// Target height in pixels. None = unconstrained.
    #[param(range(0..=65535), default = 0, step = 1)]
    #[param(unit = "px", section = "Dimensions", label = "Height")]
    #[kv("h", "height")]
    pub h: Option<u32>,

    // ─── Layout ───
    /// Constraint mode: "distort", "within" (default), "fit",
    /// "within_crop", "fit_crop", "fit_pad", "within_pad",
    /// "pad_within", "aspect_crop".
    #[param(default = "within")]
    #[param(section = "Layout", label = "Mode")]
    #[kv("mode")]
    pub mode: String,

    /// Named anchor: "center", "top_left", "top", "top_right",
    /// "left", "right", "bottom_left", "bottom", "bottom_right".
    /// Overridden by gravity_x/gravity_y when both set.
    #[param(default = "center")]
    #[param(section = "Position", label = "Anchor")]
    #[kv("anchor")]
    pub gravity: String,

    /// Horizontal gravity (0.0 = left, 1.0 = right). Overrides named anchor.
    #[param(range(0.0..=1.0), default = 0.5, step = 0.01)]
    #[param(section = "Position", label = "Gravity X")]
    pub gravity_x: Option<f32>,

    /// Vertical gravity (0.0 = top, 1.0 = bottom). Overrides named anchor.
    #[param(range(0.0..=1.0), default = 0.5, step = 0.01)]
    #[param(section = "Position", label = "Gravity Y")]
    pub gravity_y: Option<f32>,

    /// Canvas color for pad modes. None = transparent.
    /// CSS-style: "white", "#FF0000", "000000FF".
    #[param(default = "")]
    #[param(section = "Position", label = "Canvas Color")]
    #[kv("bgcolor", "canvas_color")]
    pub canvas_color: Option<String>,

    // ─── Resampling (matches v2 ResampleHints) ───
    /// Downscale resampling filter. None = auto (Robidoux).
    #[param(default = "")]
    #[param(section = "Quality", label = "Downscale Filter")]
    #[kv("down.filter")]
    pub down_filter: Option<String>,

    /// Upscale resampling filter. None = auto (Ginseng).
    #[param(default = "")]
    #[param(section = "Quality", label = "Upscale Filter")]
    #[kv("up.filter")]
    pub up_filter: Option<String>,

    /// Color space for resampling: "linear" (default) or "srgb".
    #[param(default = "")]
    #[param(section = "Quality", label = "Scaling Colorspace")]
    pub scaling_colorspace: Option<String>,

    /// Background color for the resample operation itself (not canvas padding).
    /// Used as the matte for alpha-less edge interpolation during resize.
    #[param(default = "")]
    #[param(section = "Quality", label = "Background Color")]
    pub background_color: Option<String>,

    /// When to apply resampling: "size_differs" (default),
    /// "size_differs_or_sharpening_requested", "always".
    #[param(default = "")]
    #[param(section = "Quality", label = "Resample When")]
    pub resample_when: Option<String>,

    // ─── Sharpening ───
    /// Post-resize sharpening (0–100). None = no sharpening.
    #[param(range(0.0..=100.0), default = 0.0, step = 1.0)]
    #[param(unit = "%", section = "Quality", label = "Sharpen")]
    #[kv("f.sharpen")]
    pub sharpen_percent: Option<f32>,

    /// When to apply sharpening: "downscaling" (default),
    /// "upscaling", "size_differs", "always".
    #[param(default = "")]
    #[param(section = "Quality", label = "Sharpen When")]
    pub sharpen_when: Option<String>,

    // ─── Advanced ───
    /// Negative-lobe ratio for in-kernel sharpening. None = filter default.
    #[param(range(0.0..=1.0), default = 0.0, step = 0.01)]
    #[param(section = "Advanced", label = "Lobe Ratio")]
    pub lobe_ratio: Option<f32>,

    /// Kernel width scale (>1 softer, <1 sharper). None = 1.0.
    #[param(range(0.1..=4.0), default = 1.0, step = 0.01)]
    #[param(section = "Advanced", label = "Kernel Width Scale")]
    pub kernel_width_scale: Option<f32>,

    /// Post-resize Gaussian blur sigma. None = no blur.
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
            background_color: None,
            resample_when: None,
            sharpen_percent: None,
            sharpen_when: None,
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
/// Unlike [`Constrain`] which applies constraint modes, this resizes
/// unconditionally. Skipped when input dims match target.
///
/// JSON: `{ "w": 400, "h": 300, "filter": "robidoux" }`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenresize.resize", group = Geometry, role = Resize)]
#[node(changes_dimensions)]
#[node(tags("resize", "scale", "resample"))]
pub struct Resize {
    #[param(range(1..=65535), default = 1, step = 1)]
    #[param(unit = "px", section = "Dimensions", label = "Width")]
    pub w: u32,

    #[param(range(1..=65535), default = 1, step = 1)]
    #[param(unit = "px", section = "Dimensions", label = "Height")]
    pub h: u32,

    /// Resampling filter name. Empty = Robidoux.
    #[param(default = "")]
    #[param(section = "Quality", label = "Filter")]
    pub filter: String,

    /// Post-resize sharpening (0–100). 0 = none.
    #[param(range(0.0..=100.0), default = 0.0, step = 1.0)]
    #[param(unit = "%", section = "Quality", label = "Sharpen")]
    pub sharpen: f32,
}

// ═══════════════════════════════════════════════════════════════════════
//  PIPELINE — operations combining analysis, canvas, or format concerns
// ═══════════════════════════════════════════════════════════════════════

/// Detect and crop uniform borders (whitespace trimming).
///
/// RIAPI: `?trim.threshold=80&trim.percentpadding=0.5`
/// JSON: `{ "threshold": 80, "percent_padding": 0.5 }`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenpipe.crop_whitespace", group = Analysis, role = Resize)]
#[node(changes_dimensions)]
#[node(tags("crop", "whitespace", "trim", "content", "analysis"))]
pub struct CropWhitespace {
    /// Color distance threshold (0–255).
    #[param(range(0..=255), default = 80, step = 1)]
    #[param(section = "Main", label = "Threshold")]
    #[kv("trim.threshold")]
    pub threshold: u32,

    /// Padding as percentage of content dimensions.
    #[param(range(0.0..=50.0), default = 0.0, step = 0.1)]
    #[param(unit = "%", section = "Main", label = "Padding")]
    #[kv("trim.percentpadding")]
    pub percent_padding: f32,
}

/// Fill a rectangle with a solid color.
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
    #[param(range(0..=255), default = 0)]
    pub color_r: u32,
    #[param(range(0..=255), default = 0)]
    pub color_g: u32,
    #[param(range(0..=255), default = 0)]
    pub color_b: u32,
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
/// JSON: `{ "matte_r": 255, "matte_g": 255, "matte_b": 255 }`
#[derive(Node, Clone, Debug)]
#[node(id = "zenpipe.remove_alpha", group = Color, role = Filter)]
#[node(tags("alpha", "matte", "composite", "flatten"))]
pub struct RemoveAlpha {
    #[param(range(0..=255), default = 255)]
    pub matte_r: u32,
    #[param(range(0..=255), default = 255)]
    pub matte_g: u32,
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
/// JSON: `{ "radius": 20.0, "bg_color": [0, 0, 0, 0] }`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenpipe.round_corners", group = Canvas, role = Filter)]
#[node(tags("corners", "rounded", "mask", "border-radius"))]
pub struct RoundCorners {
    #[param(range(0.0..=10000.0), default = 0.0, step = 1.0)]
    #[param(unit = "px", section = "Main", label = "Radius")]
    pub radius: f32,
    #[param(range(0.0..=10000.0), default = -1.0, step = 1.0)]
    pub radius_tl: f32,
    #[param(range(0.0..=10000.0), default = -1.0, step = 1.0)]
    pub radius_tr: f32,
    #[param(range(0.0..=10000.0), default = -1.0, step = 1.0)]
    pub radius_bl: f32,
    #[param(range(0.0..=10000.0), default = -1.0, step = 1.0)]
    pub radius_br: f32,
    /// "pixels" (default), "percentage", "circle",
    /// "pixels_custom", "percentage_custom".
    #[param(default = "pixels")]
    pub mode: String,
    #[param(range(0..=255), default = 0)]
    pub bg_r: u32,
    #[param(range(0..=255), default = 0)]
    pub bg_g: u32,
    #[param(range(0..=255), default = 0)]
    pub bg_b: u32,
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
