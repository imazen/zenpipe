//! RIAPI instruction types: parsed representation of a query string.

use alloc::collections::BTreeMap;
use alloc::string::String;

use crate::CanvasColor;

/// How to fit the image into the target dimensions.
///
/// Maps to the RIAPI `mode` parameter.
#[non_exhaustive]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum FitMode {
    /// Scale proportionally to fit within target box.
    /// Output may be smaller than target on one axis.
    Max,
    /// Scale proportionally to fit within target, pad remainder.
    /// Output is always exactly target dimensions.
    Pad,
    /// Scale proportionally to fill target, crop overflow.
    /// Output is always exactly target dimensions.
    Crop,
    /// Scale to exact target dimensions, distorting aspect ratio.
    Stretch,
    /// Crop to target aspect ratio without scaling.
    AspectCrop,
}

/// Whether to upscale, downscale, or both.
///
/// Maps to the RIAPI `scale` parameter.
#[non_exhaustive]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum ScaleMode {
    /// Never upscale. Default.
    DownscaleOnly,
    /// Never downscale (rare).
    UpscaleOnly,
    /// Scale in both directions.
    Both,
    /// When image is smaller than target, pad instead of upscale.
    UpscaleCanvas,
}

/// 1D anchor position along an axis.
#[non_exhaustive]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Anchor1D {
    /// Near edge (left or top).
    Near,
    /// Center.
    Center,
    /// Far edge (right or bottom).
    Far,
    /// Percentage: 0.0 = near, 100.0 = far.
    Percent(f32),
}

/// Parsed RIAPI instructions.
///
/// Produced by [`crate::riapi::parse()`], consumed by
/// [`to_pipeline()`](Self::to_pipeline).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Instructions {
    /// Target width (`w`, `width`). Merged with `maxwidth`.
    pub w: Option<i32>,
    /// Target height (`h`, `height`). Merged with `maxheight`.
    pub h: Option<i32>,
    /// Legacy `maxwidth` upper bound.
    pub legacy_max_width: Option<i32>,
    /// Legacy `maxheight` upper bound.
    pub legacy_max_height: Option<i32>,
    /// Fit mode (`mode`).
    pub mode: Option<FitMode>,
    /// Scale mode (`scale`).
    pub scale: Option<ScaleMode>,
    /// Post-resize flip: `(horizontal, vertical)`.
    pub flip: Option<(bool, bool)>,
    /// Source flip: `(horizontal, vertical)`.
    pub sflip: Option<(bool, bool)>,
    /// Source rotation in degrees (0, 90, 180, 270).
    pub srotate: Option<i32>,
    /// Post-resize rotation in degrees (0, 90, 180, 270).
    pub rotate: Option<i32>,
    /// Whether to apply EXIF orientation. Default: `true`.
    pub autorotate: Option<bool>,
    /// Anchor for crop/pad: `(horizontal, vertical)`.
    pub anchor: Option<(Anchor1D, Anchor1D)>,
    /// Crop gravity override as `[x%, y%]` (0–100).
    pub c_gravity: Option<[f64; 2]>,
    /// Crop rectangle as `[x1, y1, x2, y2]` in cropxunits/cropyunits space.
    pub crop: Option<[f64; 4]>,
    /// Crop X coordinate space (0 or absent = source pixels).
    pub cropxunits: Option<f64>,
    /// Crop Y coordinate space (0 or absent = source pixels).
    pub cropyunits: Option<f64>,
    /// Zoom/DPR multiplier.
    pub zoom: Option<f64>,
    /// Background color for padding.
    pub bgcolor: Option<CanvasColor>,
    /// Non-layout parameters preserved for downstream consumers.
    pub extras: BTreeMap<String, String>,
}

impl Default for Instructions {
    fn default() -> Self {
        Self::new()
    }
}

impl Instructions {
    /// Create empty instructions.
    pub fn new() -> Self {
        Self {
            w: None,
            h: None,
            legacy_max_width: None,
            legacy_max_height: None,
            mode: None,
            scale: None,
            flip: None,
            sflip: None,
            srotate: None,
            rotate: None,
            autorotate: None,
            anchor: None,
            c_gravity: None,
            crop: None,
            cropxunits: None,
            cropyunits: None,
            zoom: None,
            bgcolor: None,
            extras: BTreeMap::new(),
        }
    }

    /// Access non-layout parameters preserved during parsing.
    pub fn extras(&self) -> &BTreeMap<String, String> {
        &self.extras
    }

    /// Check all float fields for NaN/Inf.
    #[track_caller]
    pub(crate) fn validate_floats(&self) -> Result<(), whereat::At<crate::LayoutError>> {
        let check = |v: f64| -> Result<(), whereat::At<crate::LayoutError>> {
            if v.is_finite() {
                Ok(())
            } else {
                Err(whereat::at!(crate::LayoutError::NonFiniteFloat))
            }
        };

        if let Some(z) = self.zoom {
            check(z)?;
        }
        if let Some([x, y]) = self.c_gravity {
            check(x)?;
            check(y)?;
        }
        if let Some([a, b, c, d]) = self.crop {
            check(a)?;
            check(b)?;
            check(c)?;
            check(d)?;
        }
        if let Some(v) = self.cropxunits {
            check(v)?;
        }
        if let Some(v) = self.cropyunits {
            check(v)?;
        }
        if let Some((ax, ay)) = &self.anchor {
            if let Anchor1D::Percent(p) = ax
                && !p.is_finite()
            {
                return Err(whereat::at!(crate::LayoutError::NonFiniteFloat));
            }
            if let Anchor1D::Percent(p) = ay
                && !p.is_finite()
            {
                return Err(whereat::at!(crate::LayoutError::NonFiniteFloat));
            }
        }
        Ok(())
    }
}
