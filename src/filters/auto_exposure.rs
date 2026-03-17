use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::simd;

/// Auto exposure: normalizes image brightness to a target middle grey.
///
/// Measures the geometric mean of L (log-average luminance) and applies
/// an exposure correction to bring it to the target. This is the most
/// fundamental "auto" operation — it ensures every image has reasonable
/// overall brightness regardless of the scene.
///
/// The geometric mean is more robust than arithmetic mean because it's
/// less affected by small bright areas (sky, lights) that would bias
/// the arithmetic mean upward.
///
/// Strength controls blending: 0.0 = no change, 1.0 = full correction.
/// Partial strength is useful for batch processing where you want
/// consistent brightness without making every photo identical.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct AutoExposure {
    /// How much to correct. 0.0 = off, 1.0 = full correction to target.
    pub strength: f32,
    /// Target middle grey in Oklab L. Default 0.5 (perceptual mid-grey).
    pub target: f32,
    /// Maximum correction in stops (EV). Prevents extreme adjustments
    /// on intentionally dark/bright images. Default: 2.0.
    pub max_correction: f32,
}

impl Default for AutoExposure {
    fn default() -> Self {
        Self {
            strength: 0.0,
            target: 0.5,
            max_correction: 2.0,
        }
    }
}

impl Filter for AutoExposure {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.strength < 1e-6 {
            return;
        }

        let pc = planes.pixel_count();
        if pc == 0 {
            return;
        }

        // Compute geometric mean of L (log-average)
        // geo_mean = exp(mean(log(L))) for L > 0
        let epsilon = 1e-6;
        let log_sum: f64 = planes.l.iter().map(|&v| (v.max(epsilon) as f64).ln()).sum();
        let geo_mean = (log_sum / pc as f64).exp() as f32;

        if geo_mean < epsilon {
            return; // near-black image, don't correct
        }

        // Compute required exposure correction in Oklab L
        // In Oklab, L is roughly cube-root of luminance, so
        // multiplying L by a factor is equivalent to multiplying
        // linear luminance by factor³.
        let correction = self.target / geo_mean;

        // Clamp to max_correction (in Oklab L domain, which is cube-root)
        // max_correction in stops → linear factor = 2^stops
        // In Oklab L: factor = 2^(stops/3)
        let max_factor = 2.0f32.powf(self.max_correction / 3.0);
        let min_factor = 1.0 / max_factor;
        let correction = correction.clamp(min_factor, max_factor);

        // Blend with identity based on strength
        let factor = 1.0 + (correction - 1.0) * self.strength;

        if (factor - 1.0).abs() < 1e-6 {
            return;
        }

        // Apply to all planes (L and chroma scale together in Oklab
        // to preserve color relationships)
        simd::scale_plane(&mut planes.l, factor);
        simd::scale_plane(&mut planes.a, factor);
        simd::scale_plane(&mut planes.b, factor);
    }
}

static AUTO_EXPOSURE_SCHEMA: FilterSchema = FilterSchema {
    name: "auto_exposure",
    label: "Auto Exposure",
    description: "Normalize image brightness to target middle grey",
    group: FilterGroup::Auto,
    params: &[
        ParamDesc {
            name: "strength",
            label: "Strength",
            description: "Correction strength (0 = off, 1 = full)",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.0,
                identity: 0.0,
                step: 0.05,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "target",
            label: "Target",
            description: "Target middle grey in Oklab L",
            kind: ParamKind::Float {
                min: 0.2,
                max: 0.8,
                default: 0.5,
                identity: 0.5,
                step: 0.05,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "max_correction",
            label: "Max Correction",
            description: "Maximum correction in stops (prevents extreme adjustments)",
            kind: ParamKind::Float {
                min: 0.5,
                max: 5.0,
                default: 2.0,
                identity: 2.0,
                step: 0.5,
            },
            unit: "EV",
            section: "Advanced",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for AutoExposure {
    fn schema() -> &'static FilterSchema {
        &AUTO_EXPOSURE_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "strength" => Some(ParamValue::Float(self.strength)),
            "target" => Some(ParamValue::Float(self.target)),
            "max_correction" => Some(ParamValue::Float(self.max_correction)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        let v = match value.as_f32() {
            Some(v) => v,
            None => return false,
        };
        match name {
            "strength" => self.strength = v,
            "target" => self.target = v,
            "max_correction" => self.max_correction = v,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_strength_is_identity() {
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.l {
            *v = 0.3;
        }
        let original = planes.l.clone();
        AutoExposure {
            strength: 0.0,
            ..Default::default()
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn brightens_dark_image() {
        let mut planes = OklabPlanes::new(100, 1);
        for v in &mut planes.l {
            *v = 0.2; // dark
        }
        AutoExposure {
            strength: 1.0,
            target: 0.5,
            max_correction: 3.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert!(
            planes.l[0] > 0.3,
            "dark image should be brightened: {}",
            planes.l[0]
        );
    }

    #[test]
    fn darkens_bright_image() {
        let mut planes = OklabPlanes::new(100, 1);
        for v in &mut planes.l {
            *v = 0.8; // bright
        }
        AutoExposure {
            strength: 1.0,
            target: 0.5,
            max_correction: 3.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert!(
            planes.l[0] < 0.7,
            "bright image should be darkened: {}",
            planes.l[0]
        );
    }

    #[test]
    fn respects_max_correction() {
        let mut planes = OklabPlanes::new(100, 1);
        for v in &mut planes.l {
            *v = 0.05; // very dark
        }
        AutoExposure {
            strength: 1.0,
            target: 0.5,
            max_correction: 1.0, // only 1 stop max
        }
        .apply(&mut planes, &mut FilterContext::new());
        // With 1 stop max, factor = 2^(1/3) ≈ 1.26
        // 0.05 * 1.26 = 0.063 (not 0.5!)
        assert!(
            planes.l[0] < 0.15,
            "max correction should limit adjustment: {}",
            planes.l[0]
        );
    }

    #[test]
    fn partial_strength_blends() {
        let mut planes = OklabPlanes::new(100, 1);
        for v in &mut planes.l {
            *v = 0.3;
        }
        let original = planes.l[0];

        let mut planes_half = planes.clone();
        let mut planes_full = planes.clone();

        AutoExposure {
            strength: 0.5,
            target: 0.5,
            max_correction: 3.0,
        }
        .apply(&mut planes_half, &mut FilterContext::new());
        AutoExposure {
            strength: 1.0,
            target: 0.5,
            max_correction: 3.0,
        }
        .apply(&mut planes_full, &mut FilterContext::new());

        let half_correction = planes_half.l[0] - original;
        let full_correction = planes_full.l[0] - original;
        assert!(
            (half_correction - full_correction * 0.5).abs() < full_correction * 0.15,
            "half strength should give roughly half correction: half={half_correction} full={full_correction}"
        );
    }

    #[test]
    fn scales_chroma_with_luminance() {
        let mut planes = OklabPlanes::new(100, 1);
        for v in &mut planes.l {
            *v = 0.3;
        }
        for v in &mut planes.a {
            *v = 0.1;
        }
        AutoExposure {
            strength: 1.0,
            target: 0.5,
            max_correction: 3.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        // Chroma should scale proportionally to L
        assert!(
            planes.a[0] > 0.1,
            "chroma should scale with luminance: {}",
            planes.a[0]
        );
    }

    #[test]
    fn already_correct_is_identity() {
        let mut planes = OklabPlanes::new(100, 1);
        for v in &mut planes.l {
            *v = 0.5; // already at target
        }
        let original = planes.l.clone();
        AutoExposure {
            strength: 1.0,
            target: 0.5,
            max_correction: 3.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        for (a, b) in planes.l.iter().zip(original.iter()) {
            assert!(
                (a - b).abs() < 1e-4,
                "at-target should be identity: {a} vs {b}"
            );
        }
    }
}
