use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use zenpixels_convert::oklab;

/// ASC CDL (American Society of Cinematographers Color Decision List).
///
/// Industry-standard per-channel color correction with three operations
/// applied in sequence: **slope** (gain), **offset** (lift), **power** (gamma).
///
/// Per-channel formula (combined):
/// ```text
/// out = clamp(pow(max(slope * in + offset, 0), power), 0, 1)
/// ```
///
/// Plus a global **saturation** control that scales chroma relative to luma.
///
/// The filter converts Oklab → linear RGB, applies CDL, then converts back.
/// This matches the ASC CDL spec which defines operations on scene-linear data.
///
/// Slope, offset, and power each have R/G/B values. Slope=1, offset=0,
/// power=1, saturation=1 is identity.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct AscCdl {
    /// Per-channel slope (gain). 1.0 = no change. Range: 0.0–4.0.
    pub slope: [f32; 3],
    /// Per-channel offset (lift). 0.0 = no change. Range: -1.0–1.0.
    pub offset: [f32; 3],
    /// Per-channel power (gamma). 1.0 = no change. Range: 0.1–4.0.
    pub power: [f32; 3],
    /// Global saturation. 1.0 = no change. 0.0 = monochrome.
    pub saturation: f32,
}

impl Default for AscCdl {
    fn default() -> Self {
        Self {
            slope: [1.0; 3],
            offset: [0.0; 3],
            power: [1.0; 3],
            saturation: 1.0,
        }
    }
}

static ASC_CDL_SCHEMA: FilterSchema = FilterSchema {
    name: "asc_cdl",
    label: "ASC CDL",
    description: "Industry-standard slope/offset/power color correction",
    group: FilterGroup::Color,
    params: &[
        ParamDesc {
            name: "slope_r",
            label: "Slope Red",
            description: "Red channel gain",
            kind: ParamKind::Float {
                min: 0.0,
                max: 4.0,
                default: 1.0,
                identity: 1.0,
                step: 0.01,
            },
            unit: "×",
            section: "Slope",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "slope_g",
            label: "Slope Green",
            description: "Green channel gain",
            kind: ParamKind::Float {
                min: 0.0,
                max: 4.0,
                default: 1.0,
                identity: 1.0,
                step: 0.01,
            },
            unit: "×",
            section: "Slope",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "slope_b",
            label: "Slope Blue",
            description: "Blue channel gain",
            kind: ParamKind::Float {
                min: 0.0,
                max: 4.0,
                default: 1.0,
                identity: 1.0,
                step: 0.01,
            },
            unit: "×",
            section: "Slope",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "offset_r",
            label: "Offset Red",
            description: "Red channel offset (lift)",
            kind: ParamKind::Float {
                min: -1.0,
                max: 1.0,
                default: 0.0,
                identity: 0.0,
                step: 0.005,
            },
            unit: "",
            section: "Offset",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "offset_g",
            label: "Offset Green",
            description: "Green channel offset (lift)",
            kind: ParamKind::Float {
                min: -1.0,
                max: 1.0,
                default: 0.0,
                identity: 0.0,
                step: 0.005,
            },
            unit: "",
            section: "Offset",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "offset_b",
            label: "Offset Blue",
            description: "Blue channel offset (lift)",
            kind: ParamKind::Float {
                min: -1.0,
                max: 1.0,
                default: 0.0,
                identity: 0.0,
                step: 0.005,
            },
            unit: "",
            section: "Offset",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "power_r",
            label: "Power Red",
            description: "Red channel gamma",
            kind: ParamKind::Float {
                min: 0.1,
                max: 4.0,
                default: 1.0,
                identity: 1.0,
                step: 0.01,
            },
            unit: "",
            section: "Power",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "power_g",
            label: "Power Green",
            description: "Green channel gamma",
            kind: ParamKind::Float {
                min: 0.1,
                max: 4.0,
                default: 1.0,
                identity: 1.0,
                step: 0.01,
            },
            unit: "",
            section: "Power",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "power_b",
            label: "Power Blue",
            description: "Blue channel gamma",
            kind: ParamKind::Float {
                min: 0.1,
                max: 4.0,
                default: 1.0,
                identity: 1.0,
                step: 0.01,
            },
            unit: "",
            section: "Power",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "saturation",
            label: "Saturation",
            description: "Global saturation (0 = mono, 1 = unchanged)",
            kind: ParamKind::Float {
                min: 0.0,
                max: 4.0,
                default: 1.0,
                identity: 1.0,
                step: 0.05,
            },
            unit: "×",
            section: "Main",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for AscCdl {
    fn schema() -> &'static FilterSchema {
        &ASC_CDL_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "slope_r" => Some(ParamValue::Float(self.slope[0])),
            "slope_g" => Some(ParamValue::Float(self.slope[1])),
            "slope_b" => Some(ParamValue::Float(self.slope[2])),
            "offset_r" => Some(ParamValue::Float(self.offset[0])),
            "offset_g" => Some(ParamValue::Float(self.offset[1])),
            "offset_b" => Some(ParamValue::Float(self.offset[2])),
            "power_r" => Some(ParamValue::Float(self.power[0])),
            "power_g" => Some(ParamValue::Float(self.power[1])),
            "power_b" => Some(ParamValue::Float(self.power[2])),
            "saturation" => Some(ParamValue::Float(self.saturation)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        let v = match value.as_f32() {
            Some(v) => v,
            None => return false,
        };
        match name {
            "slope_r" => self.slope[0] = v,
            "slope_g" => self.slope[1] = v,
            "slope_b" => self.slope[2] = v,
            "offset_r" => self.offset[0] = v,
            "offset_g" => self.offset[1] = v,
            "offset_b" => self.offset[2] = v,
            "power_r" => self.power[0] = v,
            "power_g" => self.power[1] = v,
            "power_b" => self.power[2] = v,
            "saturation" => self.saturation = v,
            _ => return false,
        }
        true
    }
}

impl AscCdl {
    fn is_identity(&self) -> bool {
        self.slope.iter().all(|&v| (v - 1.0).abs() < 1e-6)
            && self.offset.iter().all(|&v| v.abs() < 1e-6)
            && self.power.iter().all(|&v| (v - 1.0).abs() < 1e-6)
            && (self.saturation - 1.0).abs() < 1e-6
    }
}

/// BT.709 luma weights for ASC CDL saturation.
const LUMA_R: f32 = 0.2126;
const LUMA_G: f32 = 0.7152;
const LUMA_B: f32 = 0.0722;

impl Filter for AscCdl {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.is_identity() {
            return;
        }

        let m1_inv = oklab::lms_to_rgb_matrix(zenpixels::ColorPrimaries::Bt709)
            .expect("BT.709 always supported");
        let m1 = oklab::rgb_to_lms_matrix(zenpixels::ColorPrimaries::Bt709)
            .expect("BT.709 always supported");

        let n = planes.pixel_count();
        let [sr, sg, sb] = self.slope;
        let [or, og, ob] = self.offset;
        let [pr, pg, pb] = self.power;
        let sat = self.saturation;

        for i in 0..n {
            // Oklab → linear RGB
            let [r, g, b] = oklab::oklab_to_rgb(planes.l[i], planes.a[i], planes.b[i], &m1_inv);

            // SOP: slope * in + offset, clamped, then power
            let r2 = (sr * r + or).max(0.0).powf(pr).min(1.0);
            let g2 = (sg * g + og).max(0.0).powf(pg).min(1.0);
            let b2 = (sb * b + ob).max(0.0).powf(pb).min(1.0);

            // Saturation: blend toward luma
            let (r3, g3, b3) = if (sat - 1.0).abs() > 1e-6 {
                let luma = LUMA_R * r2 + LUMA_G * g2 + LUMA_B * b2;
                (
                    (luma + sat * (r2 - luma)).clamp(0.0, 1.0),
                    (luma + sat * (g2 - luma)).clamp(0.0, 1.0),
                    (luma + sat * (b2 - luma)).clamp(0.0, 1.0),
                )
            } else {
                (r2, g2, b2)
            };

            // Linear RGB → Oklab
            let [l, oa, ob] = oklab::rgb_to_oklab(r3, g3, b3, &m1);
            planes.l[i] = l;
            planes.a[i] = oa;
            planes.b[i] = ob;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_identity() {
        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = 0.3 + (i as f32) * 0.01;
        }
        for v in &mut planes.a {
            *v = 0.05;
        }
        for v in &mut planes.b {
            *v = -0.03;
        }
        let l_orig = planes.l.clone();
        let a_orig = planes.a.clone();
        let b_orig = planes.b.clone();
        AscCdl::default().apply(&mut planes, &mut FilterContext::new());
        for i in 0..planes.pixel_count() {
            assert!(
                (planes.l[i] - l_orig[i]).abs() < 1e-4,
                "L[{i}]: {} vs {}",
                planes.l[i],
                l_orig[i]
            );
            assert!(
                (planes.a[i] - a_orig[i]).abs() < 1e-4,
                "a[{i}]: {} vs {}",
                planes.a[i],
                a_orig[i]
            );
            assert!(
                (planes.b[i] - b_orig[i]).abs() < 1e-4,
                "b[{i}]: {} vs {}",
                planes.b[i],
                b_orig[i]
            );
        }
    }

    #[test]
    fn slope_scales_brightness() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.0;
        planes.b[0] = 0.0;
        let l_before = planes.l[0];

        let mut cdl = AscCdl::default();
        cdl.slope = [2.0, 2.0, 2.0];
        cdl.apply(&mut planes, &mut FilterContext::new());

        assert!(
            planes.l[0] > l_before,
            "doubling slope should brighten: {} vs {}",
            planes.l[0],
            l_before
        );
    }

    #[test]
    fn offset_lifts_shadows() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.1;
        planes.a[0] = 0.0;
        planes.b[0] = 0.0;
        let l_before = planes.l[0];

        let mut cdl = AscCdl::default();
        cdl.offset = [0.1, 0.1, 0.1];
        cdl.apply(&mut planes, &mut FilterContext::new());

        assert!(
            planes.l[0] > l_before,
            "positive offset should lift shadows: {} vs {}",
            planes.l[0],
            l_before
        );
    }

    #[test]
    fn power_adjusts_gamma() {
        let mut planes_light = OklabPlanes::new(1, 1);
        planes_light.l[0] = 0.5;
        planes_light.a[0] = 0.0;
        planes_light.b[0] = 0.0;
        let mut planes_dark = planes_light.clone();

        let mut cdl_light = AscCdl::default();
        cdl_light.power = [0.5, 0.5, 0.5]; // < 1 brightens midtones
        cdl_light.apply(&mut planes_light, &mut FilterContext::new());

        let mut cdl_dark = AscCdl::default();
        cdl_dark.power = [2.0, 2.0, 2.0]; // > 1 darkens midtones
        cdl_dark.apply(&mut planes_dark, &mut FilterContext::new());

        assert!(
            planes_light.l[0] > planes_dark.l[0],
            "power<1 should be brighter than power>1: {} vs {}",
            planes_light.l[0],
            planes_dark.l[0]
        );
    }

    #[test]
    fn saturation_zero_is_mono() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.1;
        planes.b[0] = 0.05;

        let mut cdl = AscCdl::default();
        cdl.saturation = 0.0;
        cdl.apply(&mut planes, &mut FilterContext::new());

        // After desaturation, chroma should be near zero
        let chroma = (planes.a[0] * planes.a[0] + planes.b[0] * planes.b[0]).sqrt();
        assert!(
            chroma < 0.02,
            "sat=0 should produce near-neutral: chroma={}",
            chroma
        );
    }

    #[test]
    fn per_channel_slope_shifts_color() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.0;
        planes.b[0] = 0.0;

        let mut cdl = AscCdl::default();
        cdl.slope = [2.0, 1.0, 1.0]; // boost red only
        cdl.apply(&mut planes, &mut FilterContext::new());

        // Should shift toward red (positive a in Oklab)
        assert!(
            planes.a[0] > 0.01,
            "red slope boost should shift a positive: {}",
            planes.a[0]
        );
    }

    #[test]
    fn output_clamped() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.8;
        planes.a[0] = 0.0;
        planes.b[0] = 0.0;

        let mut cdl = AscCdl::default();
        cdl.slope = [4.0, 4.0, 4.0];
        cdl.offset = [0.5, 0.5, 0.5];
        cdl.apply(&mut planes, &mut FilterContext::new());

        // Output should be finite and reasonable
        assert!(planes.l[0].is_finite(), "L should be finite");
        assert!(planes.l[0] <= 1.1, "L should be bounded: {}", planes.l[0]);
    }
}
