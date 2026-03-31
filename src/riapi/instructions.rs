//! RIAPI instruction types: parsed representation of a query string.

use alloc::collections::BTreeMap;
use alloc::string::String;

use alloc::vec::Vec;

use crate::CanvasColor;

/// Parsed `c.focus` value for smart cropping.
///
/// Specifies where the important content is so that cropping preserves it.
/// Values are in percentage coordinates (0–100).
#[derive(Debug, Clone, PartialEq)]
pub enum CFocus {
    /// Focal point `[x, y]` in percentage coords (0–100). Two-value form.
    Point([f64; 2]),
    /// Focus rectangles `[x1, y1, x2, y2]` in percentage coords (0–100).
    Rects(Vec<[f64; 4]>),
    /// Keyword: trigger face detection (requires ML backend).
    Faces,
    /// Keyword: trigger saliency detection only (lightweight or ML).
    Saliency,
    /// Keyword: trigger faces + saliency (requires ML backend).
    Auto,
}

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
    /// Smart crop focus: point, rectangles, or ML keyword.
    pub c_focus: Option<CFocus>,
    /// Whether to zoom into the focus area (`c.zoom`).
    pub c_zoom: Option<bool>,
    /// Override fit mode after smart crop (`c.finalmode`).
    pub c_finalmode: Option<String>,
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
            c_focus: None,
            c_zoom: None,
            c_finalmode: None,
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
        if let Some(ref focus) = self.c_focus {
            match focus {
                CFocus::Point([x, y]) => {
                    check(*x)?;
                    check(*y)?;
                }
                CFocus::Rects(rects) => {
                    for [a, b, c, d] in rects {
                        check(*a)?;
                        check(*b)?;
                        check(*c)?;
                        check(*d)?;
                    }
                }
                CFocus::Faces | CFocus::Auto => {}
            }
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

    /// Convert `c_focus` rects to [`FocusRect`](crate::smart_crop::FocusRect) values.
    ///
    /// Returns an empty vec for keywords, points, or `None`.
    #[cfg(feature = "smart-crop")]
    pub fn focus_rects(&self) -> Vec<crate::smart_crop::FocusRect> {
        match &self.c_focus {
            Some(CFocus::Rects(rects)) => rects
                .iter()
                .map(|[x1, y1, x2, y2]| crate::smart_crop::FocusRect {
                    x1: *x1 as f32,
                    y1: *y1 as f32,
                    x2: *x2 as f32,
                    y2: *y2 as f32,
                    weight: 1.0,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Whether `c_focus` requests any detection engine (faces, saliency, or auto keywords).
    pub fn focus_needs_detection(&self) -> bool {
        matches!(
            self.c_focus,
            Some(CFocus::Faces | CFocus::Saliency | CFocus::Auto)
        )
    }
}
