//! Geometry model — crop, rotate, flip, orient, and padding for the edit pipeline.
//!
//! Distinct from [`RegionModel`] which controls the detail view's viewport.
//! This model represents the user's intentional geometry edits that are part
//! of the pipeline output.
//!
//! SPEC.md §11 (geometry tools), §13 (existing capabilities), §18.4 (crop UX).

use serde::{Deserialize, Serialize};

/// Complete geometry state for the edit pipeline.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GeometryModel {
    /// Crop applied to the source image.
    pub crop: CropMode,
    /// Locked aspect ratio for the crop (§11.2 presets).
    pub aspect_ratio: Option<AspectRatio>,
    /// Rotation applied after crop.
    pub rotation: RotationMode,
    /// Horizontal flip.
    pub flip_h: bool,
    /// Vertical flip.
    pub flip_v: bool,
    /// EXIF orientation handling.
    pub orientation: OrientMode,
    /// Padding added to the canvas (§11.5 margins).
    pub padding: Padding,
}

/// How to crop the image.
///
/// SPEC.md §11.2: freeform crop with handles, aspect ratio presets.
/// Normalized coordinates for recipe portability (§12.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode")]
pub enum CropMode {
    /// No crop.
    #[serde(rename = "none")]
    None,
    /// Absolute pixel coordinates.
    #[serde(rename = "pixels")]
    Pixels { x: u32, y: u32, w: u32, h: u32 },
    /// Normalized coordinates (0.0..1.0) — portable across resolutions.
    #[serde(rename = "percent")]
    Percent { x: f32, y: f32, w: f32, h: f32 },
}

impl Default for CropMode {
    fn default() -> Self {
        Self::None
    }
}

/// Aspect ratio constraint for cropping (§11.2).
///
/// Displayed as pills in the crop UI: Free | 1:1 | 4:3 | 3:2 | 16:9 | Custom
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode")]
pub enum AspectRatio {
    /// Freeform — no constraint.
    #[serde(rename = "free")]
    Free,
    /// Fixed ratio (width:height). Stored as a fraction for precision.
    #[serde(rename = "fixed")]
    Fixed { w: u32, h: u32 },
}

/// Named crop set definition for CMS mode (§14.5, §12.1).
///
/// Defines a named aspect ratio + anchor for serve-time cropping.
/// Compatible with imageflow/zenpipe server crop API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CropSetEntry {
    /// Aspect ratio (e.g. w=16, h=9 for 16:9).
    pub aspect_w: u32,
    pub aspect_h: u32,
    /// Anchor point for the crop (where to center when source doesn't match).
    pub anchor: CropAnchor,
}

/// Where to anchor a crop when the source aspect doesn't match the target.
///
/// SPEC.md §14.5: center, face-detect, rule-of-thirds, manual point.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CropAnchor {
    #[default]
    Center,
    /// Face-detect anchor (uses saliency if available).
    Face,
    /// Rule-of-thirds intersection (top-left power point).
    ThirdsTopLeft,
    /// Rule-of-thirds intersection (top-right power point).
    ThirdsTopRight,
}

/// How to rotate the image.
///
/// SPEC.md §11.3: 90° buttons, arbitrary slider, straighten wheel (§20.5).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode")]
pub enum RotationMode {
    /// No rotation.
    #[serde(rename = "none")]
    None,
    /// Cardinal rotation (90, 180, 270). Pixel-perfect, no interpolation.
    /// §13.1: `Warp::rotate_90/180/270()` — pixel-perfect copy.
    #[serde(rename = "cardinal")]
    Cardinal { degrees: u16 },
    /// Arbitrary angle rotation with interpolation.
    /// §13.1: `Rotate` struct with Lanczos3 interpolation.
    /// `border` controls edge handling (crop or expand with white fill).
    #[serde(rename = "arbitrary")]
    Arbitrary { degrees: f32, border: RotationBorder },
}

impl Default for RotationMode {
    fn default() -> Self {
        Self::None
    }
}

/// How to handle edges during arbitrary rotation.
///
/// SPEC.md §11.3: "Preview shows rotated image with transparent/fill corners"
/// §13.1: Rotate supports Crop and Deskew border modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationBorder {
    /// Crop to the largest axis-aligned rectangle inside the rotated image.
    #[default]
    Crop,
    /// Expand canvas and fill with white (deskew mode).
    Expand,
}

/// EXIF orientation handling.
///
/// SPEC.md §13.3: 8 EXIF orientations, applied in pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrientMode {
    /// No orientation change.
    #[default]
    None,
    /// Apply EXIF orientation from source metadata and strip the tag.
    Auto,
}

/// Padding added around the image (canvas expansion).
///
/// SPEC.md §11.5: margins for document mode.
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
            && self.aspect_ratio.is_none()
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

    #[test]
    fn aspect_ratio_serde() {
        let a = AspectRatio::Fixed { w: 16, h: 9 };
        let json = serde_json::to_string(&a).unwrap();
        let a2: AspectRatio = serde_json::from_str(&json).unwrap();
        assert_eq!(a, a2);
    }

    #[test]
    fn crop_set_entry_serde() {
        let e = CropSetEntry {
            aspect_w: 16,
            aspect_h: 9,
            anchor: CropAnchor::Face,
        };
        let json = serde_json::to_string(&e).unwrap();
        let e2: CropSetEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(e, e2);
    }
}
