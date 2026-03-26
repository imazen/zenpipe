//! Zennode definitions for layout and geometry operations.
//!
//! Defines crop, orientation, flip, rotation, expand canvas, constraint,
//! region viewport, crop margins, and output limits nodes with
//! RIAPI-compatible querystring keys.

extern crate alloc;
use alloc::string::String;

use zennode::*;

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
/// Maps to [`SourceCrop::Percent`](crate::SourceCrop).
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
/// Maps to [`SourceCrop::margins_percent`](crate::SourceCrop::margins_percent).
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

// ─── Constrain ───

/// Constrain image dimensions with a fit mode, gravity, and canvas color.
///
/// Combines target width/height with a constraint mode to compute
/// the output layout. Gravity controls positioning for crop and pad
/// modes. Canvas color fills padding areas.
///
/// RIAPI: `?w=800&h=600&mode=crop&anchor=topleft&bgcolor=white`
/// JSON: `{ "w": 800, "h": 600, "mode": "fit_crop", "gravity_x": 0.0, "gravity_y": 0.0 }`
#[derive(Node, Clone, Debug)]
#[node(id = "zenlayout.constrain", group = Layout, role = Resize)]
#[node(coalesce = "layout_plan", changes_dimensions)]
#[node(tags("resize", "constrain", "layout", "geometry"))]
pub struct Constrain {
    /// Target width in pixels. 0 = unconstrained.
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Main", label = "Width")]
    #[kv("w", "width")]
    pub w: u32,

    /// Target height in pixels. 0 = unconstrained.
    #[param(range(0..=u32::MAX), default = 0, step = 1)]
    #[param(unit = "px", section = "Main", label = "Height")]
    #[kv("h", "height")]
    pub h: u32,

    /// Constraint mode: how to fit the image into the target box.
    ///
    /// - "distort" — scale to exact dimensions, stretching if needed
    /// - "fit" — scale to fit within target box (default)
    /// - "within" — fit without upscaling small images
    /// - "fit_crop" — scale to fill and crop excess
    /// - "within_crop" — fill and crop, never upscale
    /// - "fit_pad" — fit and pad to exact target dimensions
    /// - "within_pad" — fit without upscale, pad to target
    /// - "pad_within" — never upscale, always pad to exact canvas
    /// - "aspect_crop" — crop to target aspect ratio without scaling
    #[param(default = "fit")]
    #[param(section = "Main", label = "Mode")]
    #[kv("mode")]
    pub mode: String,

    /// Horizontal gravity for crop/pad positioning (0.0 = left, 0.5 = center, 1.0 = right).
    ///
    /// Controls where the image is placed within the target box for modes
    /// that crop (fit_crop, within_crop, aspect_crop) or pad (fit_pad,
    /// within_pad, pad_within). Ignored by fit, within, and distort modes.
    #[param(range(0.0..=1.0), default = 0.5, step = 0.01)]
    #[param(section = "Position", label = "Gravity X")]
    pub gravity_x: f32,

    /// Vertical gravity for crop/pad positioning (0.0 = top, 0.5 = center, 1.0 = bottom).
    ///
    /// Controls where the image is placed within the target box for modes
    /// that crop or pad. Ignored by fit, within, and distort modes.
    #[param(range(0.0..=1.0), default = 0.5, step = 0.01)]
    #[param(section = "Position", label = "Gravity Y")]
    pub gravity_y: f32,

    /// Canvas background color for pad modes.
    ///
    /// Only used by modes that add padding: fit_pad, within_pad, pad_within.
    /// Accepts "transparent", "white", "black", or hex "#RRGGBB" / "#RRGGBBAA".
    #[param(default = "transparent")]
    #[param(section = "Position", label = "Canvas Color")]
    pub canvas_color: String,
}

impl Default for Constrain {
    fn default() -> Self {
        Self {
            w: 0,
            h: 0,
            mode: String::from("fit"),
            gravity_x: 0.5,
            gravity_y: 0.5,
            canvas_color: String::from("transparent"),
        }
    }
}

// ─── Region ───

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
///
/// Maps to [`Region`](crate::Region) and [`RegionCoord`](crate::RegionCoord).
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
/// Maps to [`OutputLimits`](crate::OutputLimits) and [`Align`](crate::Align).
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

// ─── CropWhitespace ───

/// Detect and crop uniform borders (whitespace trimming).
///
/// Scans inward from each edge to find where pixel values diverge
/// from the border color by more than `threshold`, then crops to the
/// content bounds plus `percent_padding`.
///
/// Requires full-frame materialization — cannot be fused with
/// streaming geometry nodes.
///
/// RIAPI: `?trim.threshold=80&trim.percentpadding=0.5`
/// JSON: `{ "threshold": 80, "percent_padding": 0.5 }`
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenlayout.crop_whitespace", group = Geometry, role = Resize)]
#[node(changes_dimensions)]
#[node(tags("crop", "whitespace", "trim", "content"))]
pub struct CropWhitespace {
    /// Color distance threshold (0–255).
    ///
    /// Pixels within this distance of the border color are considered
    /// "whitespace". Lower values detect only near-identical borders;
    /// higher values tolerate slight color variation (e.g., JPEG artifacts).
    #[param(range(0..=255), default = 80, step = 1)]
    #[param(section = "Main", label = "Threshold")]
    #[kv("trim.threshold")]
    pub threshold: u32,

    /// Padding around detected content as a percentage of content dimensions.
    ///
    /// 0.0 = tight crop, 0.5 = add 0.5% padding on each side. Prevents
    /// overly tight trims that clip into content edges.
    #[param(range(0.0..=50.0), default = 0.0, step = 0.1)]
    #[param(unit = "%", section = "Main", label = "Padding")]
    #[kv("trim.percentpadding")]
    pub percent_padding: f32,
}

// ─── Registration ───

/// Register all zenlayout nodes with a registry.
pub fn register(registry: &mut NodeRegistry) {
    for node in ALL {
        registry.register(*node);
    }
}

/// All zenlayout zennode definitions.
pub static ALL: &[&dyn NodeDef] = &[
    // Crop operations
    &CROP_NODE,
    &CROP_PERCENT_NODE,
    &CROP_MARGINS_NODE,
    &CROP_WHITESPACE_NODE,
    // Orientation operations
    &ORIENT_NODE,
    &FLIP_H_NODE,
    &FLIP_V_NODE,
    &ROTATE90_NODE,
    &ROTATE180_NODE,
    &ROTATE270_NODE,
    // Canvas and layout
    &EXPAND_CANVAS_NODE,
    &CONSTRAIN_NODE,
    &REGION_VIEWPORT_NODE,
    &OUTPUT_LIMITS_NODE,
];

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Crop tests ───

    #[test]
    fn crop_schema() {
        let schema = CROP_NODE.schema();
        assert_eq!(schema.id, "zenlayout.crop");
        assert_eq!(schema.group, NodeGroup::Geometry);
        assert_eq!(schema.role, NodeRole::Orient);
        assert!(schema.tags.contains(&"crop"));
        assert!(schema.tags.contains(&"geometry"));
        assert!(schema.coalesce.is_some());
        assert_eq!(schema.coalesce.as_ref().unwrap().group, "layout_plan");
        assert!(schema.format.changes_dimensions);
    }

    #[test]
    fn crop_defaults() {
        let node = CROP_NODE.create_default().unwrap();
        assert_eq!(node.get_param("x"), Some(ParamValue::U32(0)));
        assert_eq!(node.get_param("y"), Some(ParamValue::U32(0)));
        assert_eq!(node.get_param("w"), Some(ParamValue::U32(0)));
        assert_eq!(node.get_param("h"), Some(ParamValue::U32(0)));
    }

    #[test]
    fn crop_create_with_params() {
        let mut params = ParamMap::new();
        params.insert("x".into(), ParamValue::U32(10));
        params.insert("y".into(), ParamValue::U32(20));
        params.insert("w".into(), ParamValue::U32(100));
        params.insert("h".into(), ParamValue::U32(80));

        let node = CROP_NODE.create(&params).unwrap();
        assert_eq!(node.get_param("x"), Some(ParamValue::U32(10)));
        assert_eq!(node.get_param("y"), Some(ParamValue::U32(20)));
        assert_eq!(node.get_param("w"), Some(ParamValue::U32(100)));
        assert_eq!(node.get_param("h"), Some(ParamValue::U32(80)));
    }

    #[test]
    fn crop_downcast() {
        let node = CROP_NODE.create_default().unwrap();
        let crop = node.as_any().downcast_ref::<Crop>().unwrap();
        assert_eq!(crop.x, 0);
        assert_eq!(crop.w, 0);
    }

    #[test]
    fn crop_round_trip() {
        let crop = Crop {
            x: 50,
            y: 60,
            w: 200,
            h: 150,
        };
        let params = crop.to_params();
        let node = CROP_NODE.create(&params).unwrap();
        assert_eq!(node.get_param("x"), Some(ParamValue::U32(50)));
        assert_eq!(node.get_param("w"), Some(ParamValue::U32(200)));
    }

    // ─── CropPercent tests ───

    #[test]
    fn crop_percent_schema() {
        let schema = CROP_PERCENT_NODE.schema();
        assert_eq!(schema.id, "zenlayout.crop_percent");
        assert_eq!(schema.group, NodeGroup::Geometry);
        assert_eq!(schema.role, NodeRole::Orient);
        assert!(schema.tags.contains(&"crop"));
        assert!(schema.coalesce.is_some());
        assert!(schema.format.changes_dimensions);
        assert_eq!(schema.params.len(), 4);
    }

    #[test]
    fn crop_percent_defaults() {
        let node = CROP_PERCENT_NODE.create_default().unwrap();
        assert_eq!(node.get_param("x"), Some(ParamValue::F32(0.0)));
        assert_eq!(node.get_param("y"), Some(ParamValue::F32(0.0)));
        assert_eq!(node.get_param("w"), Some(ParamValue::F32(1.0)));
        assert_eq!(node.get_param("h"), Some(ParamValue::F32(1.0)));
    }

    #[test]
    fn crop_percent_downcast() {
        let node = CROP_PERCENT_NODE.create_default().unwrap();
        let cp = node.as_any().downcast_ref::<CropPercent>().unwrap();
        assert_eq!(cp.x, 0.0);
        assert_eq!(cp.w, 1.0);
    }

    #[test]
    fn crop_percent_round_trip() {
        let cp = CropPercent {
            x: 0.1,
            y: 0.2,
            w: 0.6,
            h: 0.5,
        };
        let params = cp.to_params();
        let node = CROP_PERCENT_NODE.create(&params).unwrap();
        assert_eq!(node.get_param("x"), Some(ParamValue::F32(0.1)));
        assert_eq!(node.get_param("w"), Some(ParamValue::F32(0.6)));
    }

    // ─── CropMargins tests ───

    #[test]
    fn crop_margins_schema() {
        let schema = CROP_MARGINS_NODE.schema();
        assert_eq!(schema.id, "zenlayout.crop_margins");
        assert_eq!(schema.group, NodeGroup::Geometry);
        assert!(schema.tags.contains(&"margins"));
        assert!(schema.coalesce.is_some());
        assert!(schema.format.changes_dimensions);
        assert_eq!(schema.params.len(), 4);
    }

    #[test]
    fn crop_margins_defaults() {
        let node = CROP_MARGINS_NODE.create_default().unwrap();
        assert_eq!(node.get_param("top"), Some(ParamValue::F32(0.0)));
        assert_eq!(node.get_param("right"), Some(ParamValue::F32(0.0)));
        assert_eq!(node.get_param("bottom"), Some(ParamValue::F32(0.0)));
        assert_eq!(node.get_param("left"), Some(ParamValue::F32(0.0)));
    }

    #[test]
    fn crop_margins_downcast() {
        let cm = CropMargins {
            top: 0.1,
            right: 0.05,
            bottom: 0.1,
            left: 0.05,
        };
        let params = cm.to_params();
        let node = CROP_MARGINS_NODE.create(&params).unwrap();
        let cm2 = node.as_any().downcast_ref::<CropMargins>().unwrap();
        assert!((cm2.top - 0.1).abs() < 1e-6);
        assert!((cm2.right - 0.05).abs() < 1e-6);
    }

    // ─── Orient tests ───

    #[test]
    fn orient_schema() {
        let schema = ORIENT_NODE.schema();
        assert_eq!(schema.id, "zenlayout.orient");
        assert_eq!(schema.group, NodeGroup::Geometry);
        assert_eq!(schema.role, NodeRole::Orient);
        assert!(schema.tags.contains(&"orient"));
        assert!(schema.tags.contains(&"exif"));
        assert!(schema.coalesce.is_some());
    }

    #[test]
    fn orient_defaults() {
        let node = ORIENT_NODE.create_default().unwrap();
        assert_eq!(node.get_param("orientation"), Some(ParamValue::I32(1)));
    }

    #[test]
    fn orient_from_kv() {
        let mut kv = KvPairs::from_querystring("srotate=6");
        let node = ORIENT_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("orientation"), Some(ParamValue::I32(6)));
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn orient_downcast() {
        let mut params = ParamMap::new();
        params.insert("orientation".into(), ParamValue::I32(3));
        let node = ORIENT_NODE.create(&params).unwrap();
        let orient = node.as_any().downcast_ref::<Orient>().unwrap();
        assert_eq!(orient.orientation, 3);
    }

    // ─── Flip tests ───

    #[test]
    fn flip_h_schema() {
        let schema = FLIP_H_NODE.schema();
        assert_eq!(schema.id, "zenlayout.flip_h");
        assert_eq!(schema.group, NodeGroup::Geometry);
        assert!(schema.tags.contains(&"flip"));
        assert!(schema.coalesce.is_some());
        assert_eq!(schema.params.len(), 0);
    }

    #[test]
    fn flip_v_schema() {
        let schema = FLIP_V_NODE.schema();
        assert_eq!(schema.id, "zenlayout.flip_v");
        assert!(schema.tags.contains(&"flip"));
        assert!(schema.coalesce.is_some());
    }

    // ─── Rotation tests ───

    #[test]
    fn rotate_90_not_coalesced() {
        let schema = ROTATE90_NODE.schema();
        assert_eq!(schema.id, "zenlayout.rotate_90");
        assert!(schema.tags.contains(&"rotate"));
        // 90/270 require materialization — no coalesce
        assert!(schema.coalesce.is_none());
        assert!(schema.format.changes_dimensions);
    }

    #[test]
    fn rotate_180_coalesced() {
        let schema = ROTATE180_NODE.schema();
        assert_eq!(schema.id, "zenlayout.rotate_180");
        // 180 decomposes to flip_h + flip_v — can be coalesced
        assert!(schema.coalesce.is_some());
        assert_eq!(schema.coalesce.as_ref().unwrap().group, "layout_plan");
    }

    #[test]
    fn rotate_270_not_coalesced() {
        let schema = ROTATE270_NODE.schema();
        assert_eq!(schema.id, "zenlayout.rotate_270");
        assert!(schema.coalesce.is_none());
        assert!(schema.format.changes_dimensions);
    }

    // ─── ExpandCanvas tests ───

    #[test]
    fn expand_canvas_schema() {
        let schema = EXPAND_CANVAS_NODE.schema();
        assert_eq!(schema.id, "zenlayout.expand_canvas");
        assert_eq!(schema.group, NodeGroup::Canvas);
        assert_eq!(schema.role, NodeRole::Resize);
        assert!(schema.tags.contains(&"pad"));
        assert!(schema.tags.contains(&"canvas"));
        assert!(schema.coalesce.is_some());
        assert!(schema.format.changes_dimensions);
    }

    #[test]
    fn expand_canvas_defaults() {
        let node = EXPAND_CANVAS_NODE.create_default().unwrap();
        assert_eq!(node.get_param("left"), Some(ParamValue::U32(0)));
        assert_eq!(node.get_param("top"), Some(ParamValue::U32(0)));
        assert_eq!(node.get_param("right"), Some(ParamValue::U32(0)));
        assert_eq!(node.get_param("bottom"), Some(ParamValue::U32(0)));
        assert_eq!(
            node.get_param("color"),
            Some(ParamValue::Str("transparent".into()))
        );
    }

    #[test]
    fn expand_canvas_create_with_params() {
        let mut params = ParamMap::new();
        params.insert("left".into(), ParamValue::U32(10));
        params.insert("top".into(), ParamValue::U32(20));
        params.insert("right".into(), ParamValue::U32(10));
        params.insert("bottom".into(), ParamValue::U32(20));
        params.insert("color".into(), ParamValue::Str("white".into()));

        let node = EXPAND_CANVAS_NODE.create(&params).unwrap();
        assert_eq!(node.get_param("left"), Some(ParamValue::U32(10)));
        assert_eq!(
            node.get_param("color"),
            Some(ParamValue::Str("white".into()))
        );
    }

    #[test]
    fn expand_canvas_downcast() {
        let node = EXPAND_CANVAS_NODE.create_default().unwrap();
        let ec = node.as_any().downcast_ref::<ExpandCanvas>().unwrap();
        assert_eq!(ec.left, 0);
        assert_eq!(ec.color, "transparent");
    }

    // ─── Constrain tests ───

    #[test]
    fn constrain_schema() {
        let schema = CONSTRAIN_NODE.schema();
        assert_eq!(schema.id, "zenlayout.constrain");
        assert_eq!(schema.group, NodeGroup::Layout);
        assert_eq!(schema.role, NodeRole::Resize);
        assert!(schema.tags.contains(&"resize"));
        assert!(schema.tags.contains(&"constrain"));
        assert!(schema.coalesce.is_some());
        assert!(schema.format.changes_dimensions);
        // w, h, mode, gravity_x, gravity_y, canvas_color
        assert_eq!(schema.params.len(), 6);
    }

    #[test]
    fn constrain_defaults() {
        let node = CONSTRAIN_NODE.create_default().unwrap();
        assert_eq!(node.get_param("w"), Some(ParamValue::U32(0)));
        assert_eq!(node.get_param("h"), Some(ParamValue::U32(0)));
        assert_eq!(node.get_param("mode"), Some(ParamValue::Str("fit".into())));
        assert_eq!(node.get_param("gravity_x"), Some(ParamValue::F32(0.5)));
        assert_eq!(node.get_param("gravity_y"), Some(ParamValue::F32(0.5)));
        assert_eq!(
            node.get_param("canvas_color"),
            Some(ParamValue::Str("transparent".into()))
        );
    }

    #[test]
    fn constrain_from_kv() {
        let mut kv = KvPairs::from_querystring("w=800&h=600&mode=crop");
        let node = CONSTRAIN_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("w"), Some(ParamValue::U32(800)));
        assert_eq!(node.get_param("h"), Some(ParamValue::U32(600)));
        assert_eq!(node.get_param("mode"), Some(ParamValue::Str("crop".into())));
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn constrain_from_kv_width_only() {
        let mut kv = KvPairs::from_querystring("w=400");
        let node = CONSTRAIN_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(node.get_param("w"), Some(ParamValue::U32(400)));
        assert_eq!(node.get_param("h"), Some(ParamValue::U32(0)));
    }

    #[test]
    fn constrain_from_kv_no_match() {
        let mut kv = KvPairs::from_querystring("quality=85&jpeg.progressive=true");
        let result = CONSTRAIN_NODE.from_kv(&mut kv).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn constrain_downcast() {
        let node = CONSTRAIN_NODE.create_default().unwrap();
        let c = node.as_any().downcast_ref::<Constrain>().unwrap();
        assert_eq!(c.w, 0);
        assert_eq!(c.h, 0);
        assert_eq!(c.mode, "fit");
        assert_eq!(c.gravity_x, 0.5);
        assert_eq!(c.gravity_y, 0.5);
        assert_eq!(c.canvas_color, "transparent");
    }

    #[test]
    fn constrain_round_trip() {
        let c = Constrain {
            w: 1920,
            h: 1080,
            mode: String::from("fit_crop"),
            gravity_x: 0.0,
            gravity_y: 0.0,
            canvas_color: String::from("white"),
        };
        let params = c.to_params();
        let node = CONSTRAIN_NODE.create(&params).unwrap();
        assert_eq!(node.get_param("w"), Some(ParamValue::U32(1920)));
        assert_eq!(node.get_param("h"), Some(ParamValue::U32(1080)));
        assert_eq!(
            node.get_param("mode"),
            Some(ParamValue::Str("fit_crop".into()))
        );
        assert_eq!(node.get_param("gravity_x"), Some(ParamValue::F32(0.0)));
        assert_eq!(
            node.get_param("canvas_color"),
            Some(ParamValue::Str("white".into()))
        );
    }

    #[test]
    fn constrain_gravity_topleft() {
        let mut params = ParamMap::new();
        params.insert("w".into(), ParamValue::U32(800));
        params.insert("h".into(), ParamValue::U32(600));
        params.insert("mode".into(), ParamValue::Str("fit_crop".into()));
        params.insert("gravity_x".into(), ParamValue::F32(0.0));
        params.insert("gravity_y".into(), ParamValue::F32(0.0));
        let node = CONSTRAIN_NODE.create(&params).unwrap();
        let c = node.as_any().downcast_ref::<Constrain>().unwrap();
        assert_eq!(c.gravity_x, 0.0);
        assert_eq!(c.gravity_y, 0.0);
    }

    // ─── Region tests ───

    #[test]
    fn region_schema() {
        let schema = REGION_VIEWPORT_NODE.schema();
        assert_eq!(schema.id, "zenlayout.region");
        assert_eq!(schema.group, NodeGroup::Geometry);
        assert_eq!(schema.role, NodeRole::Orient);
        assert!(schema.tags.contains(&"region"));
        assert!(schema.tags.contains(&"viewport"));
        assert!(schema.tags.contains(&"crop"));
        assert!(schema.tags.contains(&"pad"));
        assert!(schema.coalesce.is_some());
        assert!(schema.format.changes_dimensions);
        // 4 edges * 2 (pct + px) + color = 9 params
        assert_eq!(schema.params.len(), 9);
    }

    #[test]
    fn region_defaults_are_identity() {
        let node = REGION_VIEWPORT_NODE.create_default().unwrap();
        let r = node.as_any().downcast_ref::<RegionViewport>().unwrap();
        // Default region is the full source (identity viewport)
        assert_eq!(r.left_pct, 0.0);
        assert_eq!(r.left_px, 0);
        assert_eq!(r.top_pct, 0.0);
        assert_eq!(r.top_px, 0);
        assert_eq!(r.right_pct, 1.0);
        assert_eq!(r.right_px, 0);
        assert_eq!(r.bottom_pct, 1.0);
        assert_eq!(r.bottom_px, 0);
        assert_eq!(r.color, "transparent");
    }

    #[test]
    fn region_crop_center_50_percent() {
        let r = RegionViewport {
            left_pct: 0.25,
            left_px: 0,
            top_pct: 0.25,
            top_px: 0,
            right_pct: 0.75,
            right_px: 0,
            bottom_pct: 0.75,
            bottom_px: 0,
            color: String::from("transparent"),
        };
        let params = r.to_params();
        let node = REGION_VIEWPORT_NODE.create(&params).unwrap();
        assert_eq!(node.get_param("left_pct"), Some(ParamValue::F32(0.25)));
        assert_eq!(node.get_param("right_pct"), Some(ParamValue::F32(0.75)));
    }

    #[test]
    fn region_pad_with_pixels() {
        let r = RegionViewport {
            left_pct: 0.0,
            left_px: -20,
            top_pct: 0.0,
            top_px: -20,
            right_pct: 1.0,
            right_px: 20,
            bottom_pct: 1.0,
            bottom_px: 20,
            color: String::from("white"),
        };
        let params = r.to_params();
        let node = REGION_VIEWPORT_NODE.create(&params).unwrap();
        assert_eq!(node.get_param("left_px"), Some(ParamValue::I32(-20)));
        assert_eq!(node.get_param("right_px"), Some(ParamValue::I32(20)));
        assert_eq!(
            node.get_param("color"),
            Some(ParamValue::Str("white".into()))
        );
    }

    // ─── OutputLimits tests ───

    #[test]
    fn output_limits_schema() {
        let schema = OUTPUT_LIMITS_NODE.schema();
        assert_eq!(schema.id, "zenlayout.output_limits");
        assert_eq!(schema.group, NodeGroup::Layout);
        assert_eq!(schema.role, NodeRole::Resize);
        assert!(schema.tags.contains(&"limits"));
        assert!(schema.tags.contains(&"codec"));
        assert!(schema.coalesce.is_some());
        assert!(schema.format.changes_dimensions);
        // max_w, max_h, min_w, min_h, align_x, align_y, align_mode
        assert_eq!(schema.params.len(), 7);
    }

    #[test]
    fn output_limits_defaults() {
        let node = OUTPUT_LIMITS_NODE.create_default().unwrap();
        assert_eq!(node.get_param("max_w"), Some(ParamValue::U32(0)));
        assert_eq!(node.get_param("max_h"), Some(ParamValue::U32(0)));
        assert_eq!(node.get_param("min_w"), Some(ParamValue::U32(0)));
        assert_eq!(node.get_param("min_h"), Some(ParamValue::U32(0)));
        assert_eq!(node.get_param("align_x"), Some(ParamValue::U32(0)));
        assert_eq!(node.get_param("align_y"), Some(ParamValue::U32(0)));
        assert_eq!(
            node.get_param("align_mode"),
            Some(ParamValue::Str("extend".into()))
        );
    }

    #[test]
    fn output_limits_downcast() {
        let ol = OutputLimits {
            max_w: 4096,
            max_h: 4096,
            min_w: 0,
            min_h: 0,
            align_x: 16,
            align_y: 16,
            align_mode: String::from("extend"),
        };
        let params = ol.to_params();
        let node = OUTPUT_LIMITS_NODE.create(&params).unwrap();
        let ol2 = node.as_any().downcast_ref::<OutputLimits>().unwrap();
        assert_eq!(ol2.max_w, 4096);
        assert_eq!(ol2.align_x, 16);
        assert_eq!(ol2.align_mode, "extend");
    }

    #[test]
    fn output_limits_jpeg_420_alignment() {
        // JPEG 4:2:0 uses 16x16 MCU blocks
        let mut params = ParamMap::new();
        params.insert("align_x".into(), ParamValue::U32(16));
        params.insert("align_y".into(), ParamValue::U32(16));
        params.insert("align_mode".into(), ParamValue::Str("extend".into()));
        let node = OUTPUT_LIMITS_NODE.create(&params).unwrap();
        let ol = node.as_any().downcast_ref::<OutputLimits>().unwrap();
        assert_eq!(ol.align_x, 16);
        assert_eq!(ol.align_y, 16);
    }

    // ─── Registry tests ───

    #[test]
    fn registry_all_nodes() {
        let mut registry = NodeRegistry::new();
        register(&mut registry);
        assert_eq!(registry.all().len(), 13);
        assert!(registry.get("zenlayout.crop").is_some());
        assert!(registry.get("zenlayout.crop_percent").is_some());
        assert!(registry.get("zenlayout.crop_margins").is_some());
        assert!(registry.get("zenlayout.orient").is_some());
        assert!(registry.get("zenlayout.flip_h").is_some());
        assert!(registry.get("zenlayout.flip_v").is_some());
        assert!(registry.get("zenlayout.rotate_90").is_some());
        assert!(registry.get("zenlayout.rotate_180").is_some());
        assert!(registry.get("zenlayout.rotate_270").is_some());
        assert!(registry.get("zenlayout.expand_canvas").is_some());
        assert!(registry.get("zenlayout.constrain").is_some());
        assert!(registry.get("zenlayout.region").is_some());
        assert!(registry.get("zenlayout.output_limits").is_some());
    }

    #[test]
    fn registry_querystring() {
        let mut registry = NodeRegistry::new();
        register(&mut registry);

        let result = registry.from_querystring("w=800&h=600&mode=crop");
        assert_eq!(result.instances.len(), 1);
        assert_eq!(result.instances[0].schema().id, "zenlayout.constrain");
        assert_eq!(
            result.instances[0].get_param("w"),
            Some(ParamValue::U32(800))
        );
    }
}
