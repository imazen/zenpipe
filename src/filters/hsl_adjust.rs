use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::fast_math::{fast_atan2, fast_sincos};
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// HSL selective color adjustment.
///
/// Adjusts hue, saturation, and luminance independently for 8 color ranges:
/// red, orange, yellow, green, cyan, blue, purple, magenta.
///
/// Each range has a soft falloff so adjustments blend smoothly between
/// adjacent color regions. Works in Oklab polar coordinates (hue from a/b).
///
/// This is the Oklab equivalent of Lightroom's HSL panel.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct HslAdjust {
    /// Hue shift per range in degrees. Order: R, O, Y, G, C, B, P, M.
    pub hue: [f32; 8],
    /// Saturation scale per range. 1.0 = no change.
    pub saturation: [f32; 8],
    /// Luminance offset per range. 0.0 = no change.
    pub luminance: [f32; 8],
}

impl Default for HslAdjust {
    fn default() -> Self {
        Self {
            hue: [0.0; 8],
            saturation: [1.0; 8],
            luminance: [0.0; 8],
        }
    }
}

static HSL_COLOR_LABELS: &[&str] = &[
    "Red", "Orange", "Yellow", "Green", "Cyan", "Blue", "Purple", "Magenta",
];

static HSL_ADJUST_SCHEMA: FilterSchema = FilterSchema {
    name: "hsl_adjust",
    label: "HSL Adjust",
    description: "Per-color hue, saturation, and luminance adjustment",
    group: FilterGroup::Color,
    params: &[
        ParamDesc {
            name: "hue",
            label: "Hue Shift",
            description: "Hue shift per color range in degrees",
            kind: ParamKind::FloatArray {
                len: 8,
                min: -180.0,
                max: 180.0,
                default: 0.0,
                labels: HSL_COLOR_LABELS,
            },
            unit: "\u{b0}",
            section: "Hue",
            slider: SliderMapping::NotSlider,
        },
        ParamDesc {
            name: "saturation",
            label: "Saturation",
            description: "Saturation scale per color range (1 = no change)",
            kind: ParamKind::FloatArray {
                len: 8,
                min: 0.0,
                max: 3.0,
                default: 1.0,
                labels: HSL_COLOR_LABELS,
            },
            unit: "×",
            section: "Saturation",
            slider: SliderMapping::NotSlider,
        },
        ParamDesc {
            name: "luminance",
            label: "Luminance",
            description: "Luminance offset per color range",
            kind: ParamKind::FloatArray {
                len: 8,
                min: -0.5,
                max: 0.5,
                default: 0.0,
                labels: HSL_COLOR_LABELS,
            },
            unit: "",
            section: "Luminance",
            slider: SliderMapping::NotSlider,
        },
    ],
};

impl Describe for HslAdjust {
    fn schema() -> &'static FilterSchema {
        &HSL_ADJUST_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "hue" => Some(ParamValue::FloatArray(self.hue.to_vec())),
            "saturation" => Some(ParamValue::FloatArray(self.saturation.to_vec())),
            "luminance" => Some(ParamValue::FloatArray(self.luminance.to_vec())),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        if let ParamValue::FloatArray(ref arr) = value {
            if arr.len() != 8 {
                return false;
            }
            match name {
                "hue" => self.hue.copy_from_slice(arr),
                "saturation" => self.saturation.copy_from_slice(arr),
                "luminance" => self.luminance.copy_from_slice(arr),
                _ => return false,
            }
            true
        } else {
            false
        }
    }
}

impl HslAdjust {
    fn is_identity(&self) -> bool {
        self.hue.iter().all(|&v| v.abs() < 1e-6)
            && self.saturation.iter().all(|&v| (v - 1.0).abs() < 1e-6)
            && self.luminance.iter().all(|&v| v.abs() < 1e-6)
    }
}

/// Center hue angles in degrees for each range.
/// Red=0, Orange=30, Yellow=60, Green=120, Cyan=180, Blue=240, Purple=280, Magenta=320.
const RANGE_CENTERS: [f32; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 280.0, 320.0];

/// Half-width of each range in degrees (overlap region for smooth blending).
const RANGE_HALF_WIDTH: f32 = 30.0;

/// Compute blending weight for a given hue angle and range center.
/// Uses a raised cosine window for smooth falloff.
#[inline]
fn range_weight(hue_deg: f32, center: f32) -> f32 {
    // Signed angular distance, wrapped to [-180, 180]
    let mut diff = hue_deg - center;
    if diff > 180.0 {
        diff -= 360.0;
    }
    if diff < -180.0 {
        diff += 360.0;
    }
    let abs_diff = diff.abs();
    if abs_diff >= RANGE_HALF_WIDTH {
        return 0.0;
    }
    // Raised cosine: smooth from 1 at center to 0 at edge
    let t = abs_diff / RANGE_HALF_WIDTH;
    0.5 * (1.0 + (core::f32::consts::PI * t).cos())
}

impl Filter for HslAdjust {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.is_identity() {
            return;
        }

        let n = planes.pixel_count();
        for i in 0..n {
            let a = planes.a[i];
            let b = planes.b[i];
            let chroma = (a * a + b * b).sqrt();

            // Skip near-neutral pixels
            if chroma < 1e-5 {
                continue;
            }

            let hue_rad = fast_atan2(b, a);
            let mut hue_deg = hue_rad.to_degrees();
            if hue_deg < 0.0 {
                hue_deg += 360.0;
            }

            // Accumulate weighted adjustments from all ranges
            let mut hue_shift = 0.0f32;
            let mut sat_scale = 0.0f32;
            let mut lum_offset = 0.0f32;
            let mut total_weight = 0.0f32;

            for (r, &center) in RANGE_CENTERS.iter().enumerate() {
                let w = range_weight(hue_deg, center);
                if w > 1e-6 {
                    hue_shift += w * self.hue[r];
                    sat_scale += w * self.saturation[r];
                    lum_offset += w * self.luminance[r];
                    total_weight += w;
                }
            }

            if total_weight < 1e-6 {
                continue;
            }

            // Normalize by total weight
            let inv_w = 1.0 / total_weight;
            hue_shift *= inv_w;
            sat_scale *= inv_w;
            lum_offset *= inv_w;

            // Apply hue shift
            let new_hue_rad = hue_rad + hue_shift.to_radians();
            let new_chroma = (chroma * sat_scale).max(0.0);

            let (sin_h, cos_h) = fast_sincos(new_hue_rad);
            planes.a[i] = new_chroma * cos_h;
            planes.b[i] = new_chroma * sin_h;
            planes.l[i] = (planes.l[i] + lum_offset).max(0.0);
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
        HslAdjust::default().apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, l_orig);
        assert_eq!(planes.a, a_orig);
        assert_eq!(planes.b, b_orig);
    }

    #[test]
    fn red_saturation_boost_affects_red() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        // Red hue in Oklab: positive a, near-zero b
        planes.a[0] = 0.15;
        planes.b[0] = 0.01;
        let c_before = (planes.a[0] * planes.a[0] + planes.b[0] * planes.b[0]).sqrt();

        let mut adj = HslAdjust::default();
        adj.saturation[0] = 1.5; // boost red
        adj.apply(&mut planes, &mut FilterContext::new());

        let c_after = (planes.a[0] * planes.a[0] + planes.b[0] * planes.b[0]).sqrt();
        assert!(
            c_after > c_before * 1.3,
            "red chroma should increase: {c_before} -> {c_after}"
        );
    }

    #[test]
    fn blue_adjustment_does_not_affect_red() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.15;
        planes.b[0] = 0.01;
        let a_orig = planes.a[0];
        let b_orig = planes.b[0];

        let mut adj = HslAdjust::default();
        adj.saturation[5] = 2.0; // boost blue only
        adj.apply(&mut planes, &mut FilterContext::new());

        assert!(
            (planes.a[0] - a_orig).abs() < 1e-4,
            "blue boost shouldn't affect red pixel"
        );
        assert!(
            (planes.b[0] - b_orig).abs() < 1e-4,
            "blue boost shouldn't affect red pixel"
        );
    }

    #[test]
    fn hue_shift_rotates_color() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.15;
        planes.b[0] = 0.01;
        let hue_before = planes.b[0].atan2(planes.a[0]).to_degrees();

        let mut adj = HslAdjust::default();
        adj.hue[0] = 45.0; // shift red hue by 45 degrees
        adj.apply(&mut planes, &mut FilterContext::new());

        let hue_after = planes.b[0].atan2(planes.a[0]).to_degrees();
        let diff = (hue_after - hue_before - 45.0).abs();
        assert!(
            diff < 5.0,
            "hue should shift ~45 degrees: {hue_before} -> {hue_after}, diff={diff}"
        );
    }

    #[test]
    fn neutral_pixel_unaffected() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.0;
        planes.b[0] = 0.0;

        let mut adj = HslAdjust::default();
        adj.saturation = [2.0; 8];
        adj.hue = [90.0; 8];
        adj.luminance = [0.1; 8];
        adj.apply(&mut planes, &mut FilterContext::new());

        assert!((planes.a[0]).abs() < 1e-5, "neutral should stay neutral");
        assert!((planes.b[0]).abs() < 1e-5, "neutral should stay neutral");
    }

    #[test]
    fn luminance_offset_changes_l() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.15;
        planes.b[0] = 0.01;

        let mut adj = HslAdjust::default();
        adj.luminance[0] = 0.1; // brighten reds
        adj.apply(&mut planes, &mut FilterContext::new());

        assert!(
            planes.l[0] > 0.55,
            "L should increase for red pixel: {}",
            planes.l[0]
        );
    }

    #[test]
    fn range_weight_at_center_is_one() {
        for &center in &RANGE_CENTERS {
            let w = range_weight(center, center);
            assert!(
                (w - 1.0).abs() < 1e-6,
                "weight at center {center} should be 1.0, got {w}"
            );
        }
    }

    #[test]
    fn range_weight_at_boundary_is_zero() {
        let w = range_weight(0.0 + RANGE_HALF_WIDTH, 0.0);
        assert!(w.abs() < 1e-6, "weight at boundary should be 0, got {w}");
    }
}
