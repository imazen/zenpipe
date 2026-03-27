//! Zennode definitions for zenpipe's native pipeline operations.
//!
//! These are operations that live at the pipeline level — not pure geometry
//! (zenlayout), not pure resampling (zenresize), not pure blending (zenblend),
//! but pipeline-level transformations that combine multiple concerns:
//!
//! - **CropWhitespace**: analysis (pixel scanning) + geometry (crop)
//! - **FillRect**: canvas drawing (pixel fill on materialized buffer)
//! - **RemoveAlpha**: format conversion with compositing semantics
//! - **RoundCorners**: mask generation (zenblend) + alpha application
//!
//! Each definition uses `#[derive(Node)]` for type-safe parameter access
//! and optional `#[kv]` annotations for RIAPI querystring parsing.

extern crate alloc;
use alloc::string::String;

use zennode::*;

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

/// Registration function for zenpipe's native pipeline nodes.
pub fn register(registry: &mut NodeRegistry) {
    registry.register(&CROP_WHITESPACE_NODE);
    registry.register(&FILL_RECT_NODE);
    registry.register(&REMOVE_ALPHA_NODE);
    registry.register(&ROUND_CORNERS_NODE);
}

/// All zenpipe zennode definitions.
pub static ALL: &[&dyn NodeDef] = &[
    &CROP_WHITESPACE_NODE,
    &FILL_RECT_NODE,
    &REMOVE_ALPHA_NODE,
    &ROUND_CORNERS_NODE,
];
