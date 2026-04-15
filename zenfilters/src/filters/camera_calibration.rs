use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::fast_math::{fast_atan2, fast_sincos};
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Camera calibration — shifts the hue and saturation of the three color primaries.
///
/// Equivalent to Lightroom's Camera Calibration panel. Adjusts how
/// the camera's RGB primaries map to final color by rotating the hue
/// and scaling the saturation of pixels near each primary.
///
/// - Red primary: Oklab hue ~0° (positive a)
/// - Green primary: Oklab hue ~140° (negative a, positive b)
/// - Blue primary: Oklab hue ~265° (negative a, negative b)
///
/// Uses wider hue ranges than HSL adjust (~60° half-width) since primaries
/// have broader influence than the 8 HSL color ranges.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct CameraCalibration {
    /// Red primary hue shift in degrees.
    pub red_hue: f32,
    /// Red primary saturation scale. 1.0 = no change.
    pub red_saturation: f32,
    /// Green primary hue shift in degrees.
    pub green_hue: f32,
    /// Green primary saturation scale. 1.0 = no change.
    pub green_saturation: f32,
    /// Blue primary hue shift in degrees.
    pub blue_hue: f32,
    /// Blue primary saturation scale. 1.0 = no change.
    pub blue_saturation: f32,
    /// Shadow tint: shifts green-magenta balance in shadows.
    /// Negative = green, positive = magenta. Range: -1.0 to 1.0.
    pub shadow_tint: f32,
}

impl Default for CameraCalibration {
    fn default() -> Self {
        Self {
            red_hue: 0.0,
            red_saturation: 1.0,
            green_hue: 0.0,
            green_saturation: 1.0,
            blue_hue: 0.0,
            blue_saturation: 1.0,
            shadow_tint: 0.0,
        }
    }
}

static CAMERA_CALIBRATION_SCHEMA: FilterSchema = FilterSchema {
    name: "camera_calibration",
    label: "Camera Calibration",
    description: "Primary color hue and saturation calibration with shadow tint",
    group: FilterGroup::Color,
    params: &[
        ParamDesc {
            name: "red_hue",
            label: "Red Hue",
            description: "Red primary hue shift",
            kind: ParamKind::Float {
                min: -60.0,
                max: 60.0,
                default: 0.0,
                identity: 0.0,
                step: 1.0,
            },
            unit: "\u{b0}",
            section: "Red Primary",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "red_saturation",
            label: "Red Saturation",
            description: "Red primary saturation scale",
            kind: ParamKind::Float {
                min: 0.0,
                max: 3.0,
                default: 1.0,
                identity: 1.0,
                step: 0.05,
            },
            unit: "×",
            section: "Red Primary",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "green_hue",
            label: "Green Hue",
            description: "Green primary hue shift",
            kind: ParamKind::Float {
                min: -60.0,
                max: 60.0,
                default: 0.0,
                identity: 0.0,
                step: 1.0,
            },
            unit: "\u{b0}",
            section: "Green Primary",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "green_saturation",
            label: "Green Saturation",
            description: "Green primary saturation scale",
            kind: ParamKind::Float {
                min: 0.0,
                max: 3.0,
                default: 1.0,
                identity: 1.0,
                step: 0.05,
            },
            unit: "×",
            section: "Green Primary",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "blue_hue",
            label: "Blue Hue",
            description: "Blue primary hue shift",
            kind: ParamKind::Float {
                min: -60.0,
                max: 60.0,
                default: 0.0,
                identity: 0.0,
                step: 1.0,
            },
            unit: "\u{b0}",
            section: "Blue Primary",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "blue_saturation",
            label: "Blue Saturation",
            description: "Blue primary saturation scale",
            kind: ParamKind::Float {
                min: 0.0,
                max: 3.0,
                default: 1.0,
                identity: 1.0,
                step: 0.05,
            },
            unit: "×",
            section: "Blue Primary",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "shadow_tint",
            label: "Shadow Tint",
            description: "Shadow green-magenta balance (negative = green, positive = magenta)",
            kind: ParamKind::Float {
                min: -1.0,
                max: 1.0,
                default: 0.0,
                identity: 0.0,
                step: 0.05,
            },
            unit: "",
            section: "Shadows",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for CameraCalibration {
    fn schema() -> &'static FilterSchema {
        &CAMERA_CALIBRATION_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "red_hue" => Some(ParamValue::Float(self.red_hue)),
            "red_saturation" => Some(ParamValue::Float(self.red_saturation)),
            "green_hue" => Some(ParamValue::Float(self.green_hue)),
            "green_saturation" => Some(ParamValue::Float(self.green_saturation)),
            "blue_hue" => Some(ParamValue::Float(self.blue_hue)),
            "blue_saturation" => Some(ParamValue::Float(self.blue_saturation)),
            "shadow_tint" => Some(ParamValue::Float(self.shadow_tint)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        let v = match value.as_f32() {
            Some(v) => v,
            None => return false,
        };
        match name {
            "red_hue" => self.red_hue = v,
            "red_saturation" => self.red_saturation = v,
            "green_hue" => self.green_hue = v,
            "green_saturation" => self.green_saturation = v,
            "blue_hue" => self.blue_hue = v,
            "blue_saturation" => self.blue_saturation = v,
            "shadow_tint" => self.shadow_tint = v,
            _ => return false,
        }
        true
    }
}

/// Primary hue centers in degrees (Oklab polar coordinates).
const PRIMARY_CENTERS: [f32; 3] = [0.0, 140.0, 265.0];

/// Wider half-width than HSL ranges — primaries have broad influence.
const PRIMARY_HALF_WIDTH: f32 = 60.0;

#[inline]
fn primary_weight(hue_deg: f32, center: f32) -> f32 {
    let mut diff = hue_deg - center;
    if diff > 180.0 {
        diff -= 360.0;
    }
    if diff < -180.0 {
        diff += 360.0;
    }
    let abs_diff = diff.abs();
    if abs_diff >= PRIMARY_HALF_WIDTH {
        return 0.0;
    }
    let t = abs_diff / PRIMARY_HALF_WIDTH;
    0.5 * (1.0 + (core::f32::consts::PI * t).cos())
}

impl CameraCalibration {
    fn is_identity(&self) -> bool {
        self.red_hue.abs() < 1e-6
            && (self.red_saturation - 1.0).abs() < 1e-6
            && self.green_hue.abs() < 1e-6
            && (self.green_saturation - 1.0).abs() < 1e-6
            && self.blue_hue.abs() < 1e-6
            && (self.blue_saturation - 1.0).abs() < 1e-6
            && self.shadow_tint.abs() < 1e-6
    }
}

impl Filter for CameraCalibration {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.is_identity() {
            return;
        }

        let hue_shifts = [self.red_hue, self.green_hue, self.blue_hue];
        let sat_scales = [
            self.red_saturation,
            self.green_saturation,
            self.blue_saturation,
        ];

        let n = planes.pixel_count();
        for i in 0..n {
            let a = planes.a[i];
            let b = planes.b[i];
            let chroma = (a * a + b * b).sqrt();

            // Shadow tint: apply green-magenta shift in shadows
            if self.shadow_tint.abs() > 1e-6 {
                let l = planes.l[i];
                // Ramp: full effect below L=0.3, fading to zero at L=0.5
                let shadow_weight = ((0.5 - l) / 0.2).clamp(0.0, 1.0);
                if shadow_weight > 0.0 {
                    // Tint shifts along the a axis (green-magenta in Oklab)
                    planes.a[i] += self.shadow_tint * 0.02 * shadow_weight;
                }
            }

            if chroma < 1e-5 {
                continue;
            }

            let hue_rad = fast_atan2(b, a);
            let mut hue_deg = hue_rad.to_degrees();
            if hue_deg < 0.0 {
                hue_deg += 360.0;
            }

            // Accumulate weighted hue shift and saturation scale from primaries
            let mut hue_shift = 0.0f32;
            let mut sat_scale = 0.0f32;
            let mut total_weight = 0.0f32;

            for (p, &center) in PRIMARY_CENTERS.iter().enumerate() {
                let w = primary_weight(hue_deg, center);
                if w > 1e-6 {
                    hue_shift += w * hue_shifts[p];
                    sat_scale += w * sat_scales[p];
                    total_weight += w;
                }
            }

            if total_weight < 1e-6 {
                continue;
            }

            let inv_w = 1.0 / total_weight;
            hue_shift *= inv_w;
            sat_scale *= inv_w;

            let new_hue_rad = hue_rad + hue_shift.to_radians();
            let new_chroma = (chroma * sat_scale).max(0.0);

            let (sin_h, cos_h) = fast_sincos(new_hue_rad);
            planes.a[i] = new_chroma * cos_h;
            planes.b[i] = new_chroma * sin_h;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_identity() {
        let mut planes = OklabPlanes::new(2, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.1;
        planes.b[0] = 0.05;
        let l_orig = planes.l.clone();
        let a_orig = planes.a.clone();
        let b_orig = planes.b.clone();
        CameraCalibration::default().apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, l_orig);
        assert_eq!(planes.a, a_orig);
        assert_eq!(planes.b, b_orig);
    }

    #[test]
    fn red_hue_shift_rotates_red() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.15; // red
        planes.b[0] = 0.01;
        let hue_before = planes.b[0].atan2(planes.a[0]).to_degrees();

        let mut cal = CameraCalibration::default();
        cal.red_hue = 30.0;
        cal.apply(&mut planes, &mut FilterContext::new());

        let hue_after = planes.b[0].atan2(planes.a[0]).to_degrees();
        let diff = (hue_after - hue_before - 30.0).abs();
        assert!(
            diff < 5.0,
            "red hue should shift ~30°: {hue_before} -> {hue_after}"
        );
    }

    #[test]
    fn blue_saturation_boost() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        planes.a[0] = -0.05; // blue direction
        planes.b[0] = -0.15;
        let c_before = (planes.a[0] * planes.a[0] + planes.b[0] * planes.b[0]).sqrt();

        let mut cal = CameraCalibration::default();
        cal.blue_saturation = 1.5;
        cal.apply(&mut planes, &mut FilterContext::new());

        let c_after = (planes.a[0] * planes.a[0] + planes.b[0] * planes.b[0]).sqrt();
        assert!(
            c_after > c_before * 1.3,
            "blue saturation boost: {c_before} -> {c_after}"
        );
    }

    #[test]
    fn shadow_tint_affects_dark_pixels() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.2; // shadow
        planes.a[0] = 0.0;
        planes.b[0] = 0.0;

        let mut cal = CameraCalibration::default();
        cal.shadow_tint = 1.0; // magenta
        cal.apply(&mut planes, &mut FilterContext::new());

        assert!(
            planes.a[0] > 0.0,
            "shadow tint should shift a positive: {}",
            planes.a[0]
        );
    }

    #[test]
    fn shadow_tint_does_not_affect_highlights() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.8; // highlight
        planes.a[0] = 0.0;
        planes.b[0] = 0.0;

        let mut cal = CameraCalibration::default();
        cal.shadow_tint = 1.0;
        cal.apply(&mut planes, &mut FilterContext::new());

        assert!(
            planes.a[0].abs() < 1e-5,
            "shadow tint should not affect highlights: {}",
            planes.a[0]
        );
    }

    #[test]
    fn neutral_pixels_unaffected_by_primaries() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.8; // highlight (avoids shadow tint)
        planes.a[0] = 0.0;
        planes.b[0] = 0.0;

        let mut cal = CameraCalibration::default();
        cal.red_hue = 30.0;
        cal.red_saturation = 2.0;
        cal.apply(&mut planes, &mut FilterContext::new());

        assert!(planes.a[0].abs() < 1e-5);
        assert!(planes.b[0].abs() < 1e-5);
    }
}
