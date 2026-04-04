//! Geometry model — crop, rotate, flip, orient, and padding for the edit pipeline.
//!
//! Distinct from [`RegionModel`] which controls the detail view's viewport.
//! This model represents the user's intentional geometry edits that are part
//! of the pipeline output.
//!
//! CLI spec §2.1, IM spec §4.1: crop modes, rotation types, orientation.

use serde::{Deserialize, Serialize};

/// Complete geometry state for the edit pipeline.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GeometryModel {
    /// Crop applied to the source image.
    pub crop: CropMode,
    /// Rotation applied after crop.
    pub rotation: RotationMode,
    /// Horizontal flip.
    pub flip_h: bool,
    /// Vertical flip.
    pub flip_v: bool,
    /// EXIF orientation handling.
    pub orientation: OrientMode,
    /// Padding added to the canvas.
    pub padding: Padding,
}

/// How to crop the image.
///
/// CLI: `--crop 100,100,800,600` / `--crop 10%,10%,80%,80%` / `--crop auto`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode")]
pub enum CropMode {
    /// No crop.
    #[serde(rename = "none")]
    None,
    /// Absolute pixel coordinates.
    #[serde(rename = "pixels")]
    Pixels { x: u32, y: u32, w: u32, h: u32 },
    /// Percentage of source dimensions (0.0..1.0).
    #[serde(rename = "percent")]
    Percent { x: f32, y: f32, w: f32, h: f32 },
    /// Auto whitespace crop (content detection).
    /// CLI: `--crop auto`, IM: `-trim`
    #[serde(rename = "auto")]
    Auto,
}

impl Default for CropMode {
    fn default() -> Self {
        Self::None
    }
}

/// How to rotate the image.
///
/// CLI: `--rotate 90` / `--rotate 2.5` / `--rotate auto`
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode")]
pub enum RotationMode {
    /// No rotation.
    #[serde(rename = "none")]
    None,
    /// Cardinal rotation (90, 180, 270). Pixel-perfect, no interpolation.
    #[serde(rename = "cardinal")]
    Cardinal { degrees: u16 },
    /// Arbitrary angle rotation with interpolation.
    /// `border` controls how edges are handled.
    #[serde(rename = "arbitrary")]
    Arbitrary { degrees: f32, border: RotationBorder },
    /// Auto-deskew — detect skew angle and correct.
    /// CLI: `--rotate auto` / `--deskew`
    #[serde(rename = "auto_deskew")]
    AutoDeskew,
}

impl Default for RotationMode {
    fn default() -> Self {
        Self::None
    }
}

/// How to handle edges during arbitrary rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationBorder {
    /// Crop to the largest axis-aligned rectangle that fits inside the rotated image.
    #[default]
    Crop,
    /// Expand canvas and fill with the deskew background (white).
    Deskew,
    /// Clamp edge pixels.
    FillClamp,
    /// Fill with black.
    FillBlack,
}

/// EXIF orientation handling.
///
/// CLI: `--orient auto` / `--orient 6`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode")]
pub enum OrientMode {
    /// No orientation change.
    #[default]
    #[serde(rename = "none")]
    None,
    /// Apply EXIF orientation from source metadata and strip the tag.
    /// CLI: `--orient auto`, IM: `-auto-orient`
    #[serde(rename = "auto")]
    Auto,
    /// Force a specific EXIF orientation value (1-8).
    #[serde(rename = "explicit")]
    Explicit { value: u8 },
}

/// Padding added around the image (canvas expansion).
///
/// CLI: `--pad 20` / `--pad 10,20,10,20 --bg black`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Padding {
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
    pub left: u32,
    /// Background color for padding (CSS-style hex, e.g. "#ffffff").
    pub bg_color: String,
}

impl Default for Padding {
    fn default() -> Self {
        Self {
            top: 0,
            right: 0,
            bottom: 0,
            left: 0,
            bg_color: "#ffffff".to_string(),
        }
    }
}

impl Padding {
    /// Whether any padding is set.
    pub fn is_empty(&self) -> bool {
        self.top == 0 && self.right == 0 && self.bottom == 0 && self.left == 0
    }

    /// Uniform padding on all sides.
    pub fn uniform(px: u32) -> Self {
        Self {
            top: px,
            right: px,
            bottom: px,
            left: px,
            bg_color: "#ffffff".to_string(),
        }
    }
}

impl GeometryModel {
    /// Whether any geometry edits are active.
    pub fn is_identity(&self) -> bool {
        self.crop == CropMode::None
            && self.rotation == RotationMode::None
            && !self.flip_h
            && !self.flip_v
            && self.orientation == OrientMode::None
            && self.padding.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_identity() {
        let g = GeometryModel::default();
        assert!(g.is_identity());
    }

    #[test]
    fn crop_pixels_not_identity() {
        let mut g = GeometryModel::default();
        g.crop = CropMode::Pixels {
            x: 10,
            y: 10,
            w: 100,
            h: 100,
        };
        assert!(!g.is_identity());
    }

    #[test]
    fn padding_uniform() {
        let p = Padding::uniform(20);
        assert!(!p.is_empty());
        assert_eq!(p.top, 20);
        assert_eq!(p.right, 20);
    }

    #[test]
    fn rotation_serde_round_trip() {
        let r = RotationMode::Arbitrary {
            degrees: 2.5,
            border: RotationBorder::Crop,
        };
        let json = serde_json::to_string(&r).unwrap();
        let r2: RotationMode = serde_json::from_str(&json).unwrap();
        assert_eq!(r, r2);
    }

    #[test]
    fn crop_mode_serde_round_trip() {
        let c = CropMode::Percent {
            x: 0.1,
            y: 0.1,
            w: 0.8,
            h: 0.8,
        };
        let json = serde_json::to_string(&c).unwrap();
        let c2: CropMode = serde_json::from_str(&json).unwrap();
        assert_eq!(c, c2);
    }
}
