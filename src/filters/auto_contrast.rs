use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Automatic contrast adjustment based on image statistics.
///
/// Measures the contrast ratio (std_l / mean_l) and applies a power curve
/// to bring it toward the ideal range. Low-contrast images get a boost,
/// high-contrast images get mild compression.
///
/// Uses a pivoted power curve centered on the image median so midtones
/// stay put while shadows deepen and highlights brighten (or vice versa).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct AutoContrast {
    /// Correction strength. 0.0 = off, 1.0 = full adaptive correction.
    pub strength: f32,
}

impl Default for AutoContrast {
    fn default() -> Self {
        Self { strength: 0.0 }
    }
}

impl Filter for AutoContrast {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.strength < 1e-6 {
            return;
        }

        let a = ctx.analyze(planes);

        // Target contrast ratio: 0.20-0.30 is the "looks good" range for most photos.
        let ratio = a.contrast_ratio;
        let amount = if ratio < 0.20 {
            // Flat image: boost contrast
            (0.20 - ratio) / 0.20 * 0.25 * self.strength
        } else if ratio > 0.40 {
            // Over-contrasty: mild compression
            -((ratio - 0.40) / 0.60).min(0.5) * 0.12 * self.strength
        } else {
            return; // Already in the sweet spot
        };

        if amount.abs() < 0.003 {
            return;
        }

        let pivot = a.p50().clamp(0.15, 0.85);
        let exp = 1.0 + amount;
        let scale = pivot.powf(-amount); // keeps pivot unchanged

        for v in planes.l.iter_mut() {
            *v = (crate::fast_math::fast_powf((*v).max(0.0), exp) * scale).max(0.0);
        }
    }
}

static AUTO_CONTRAST_SCHEMA: FilterSchema = FilterSchema {
    name: "auto_contrast",
    label: "Auto Contrast",
    description: "Adaptive contrast correction based on image statistics",
    group: FilterGroup::Auto,
    params: &[ParamDesc {
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
    }],
};

impl Describe for AutoContrast {
    fn schema() -> &'static FilterSchema {
        &AUTO_CONTRAST_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "strength" => Some(ParamValue::Float(self.strength)),
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
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = 0.3 + (i as f32 / 16.0) * 0.4;
        }
        let original = planes.l.clone();
        AutoContrast { strength: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn boosts_flat_image() {
        let mut planes = OklabPlanes::new(100, 1);
        // Very flat: L spans only 0.4-0.6
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = 0.4 + (i as f32 / 99.0) * 0.2;
        }
        let std_before = {
            let mean = planes.l.iter().sum::<f32>() / 100.0;
            (planes.l.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / 100.0).sqrt()
        };
        AutoContrast { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        let std_after = {
            let mean = planes.l.iter().sum::<f32>() / 100.0;
            (planes.l.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / 100.0).sqrt()
        };
        assert!(
            std_after > std_before * 1.05,
            "flat image should get more contrast: {std_before} -> {std_after}"
        );
    }

    #[test]
    fn well_balanced_unchanged() {
        let mut planes = OklabPlanes::new(100, 1);
        // Contrast ratio ~0.25 (in the sweet spot)
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = 0.2 + (i as f32 / 99.0) * 0.6; // 0.2-0.8
        }
        let original = planes.l.clone();
        AutoContrast { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        let max_delta: f32 = planes
            .l
            .iter()
            .zip(original.iter())
            .map(|(&a, &b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(
            max_delta < 0.01,
            "well-balanced image should barely change: max_delta={max_delta}"
        );
    }
}
