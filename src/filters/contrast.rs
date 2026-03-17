use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::simd;

/// Pivot for the contrast power curve in Oklab L.
///
/// `MIDDLE_GREY_LINEAR^(1/3)` where MIDDLE_GREY_LINEAR = 0.1842 (18.42%).
/// This matches darktable's middle grey anchor for contrast adjustments,
/// ensuring that mid-tones remain stable when contrast is adjusted.
pub(crate) const CONTRAST_PIVOT: f32 = 0.5691; // 0.1842_f32.cbrt()

/// Contrast adjustment via power curve on Oklab L channel.
///
/// Uses a power curve `L' = L^(1+amount) * pivot^(-amount)` that pivots
/// at the perceptual equivalent of 18.42% middle grey in Oklab space.
/// This matches the contrast behavior of darktable's basicadj module.
///
/// Positive values increase contrast (darks darker, lights lighter).
/// Negative values reduce contrast.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Contrast {
    /// Contrast amount. 0.0 = no change, 1.0 = strong increase, -1.0 = flatten.
    ///
    /// For ergonomic slider integration, use [`slider::contrast_from_slider`]
    /// which applies sqrt remapping so the first half of the slider covers
    /// the most useful range (0–0.25 internal contrast).
    pub amount: f32,
}

impl Contrast {
    /// Create from a perceptual slider value (-1.0 to +1.0).
    ///
    /// Applies sqrt remapping: slider 0.5 → internal 0.25 (moderate contrast).
    /// This makes equal slider movements produce equal perceived changes.
    pub fn from_slider(slider: f32) -> Self {
        Self {
            amount: crate::slider::contrast_from_slider(slider),
        }
    }
}

impl Filter for Contrast {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn tag(&self) -> crate::filter_compat::FilterTag {
        crate::filter_compat::FilterTag::Contrast
    }
    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.amount.abs() < 1e-6 {
            return;
        }
        // Power curve: L' = L^exp * scale, pivot at CONTRAST_PIVOT ≈ 0.569
        // exp = 1 + amount, scale = pivot^(-amount)
        let exp = (1.0 + self.amount).max(0.01);
        let scale = CONTRAST_PIVOT.powf(-self.amount);
        simd::power_contrast_plane(&mut planes.l, exp, scale);
    }
}

static CONTRAST_SCHEMA: FilterSchema = FilterSchema {
    name: "contrast",
    label: "Contrast",
    description: "Power-curve contrast adjustment pivoted at middle grey",
    group: FilterGroup::Tone,
    params: &[ParamDesc {
        name: "amount",
        label: "Amount",
        description: "Contrast strength (positive = increase, negative = flatten)",
        kind: ParamKind::Float {
            min: -1.0,
            max: 1.0,
            default: 0.0,
            identity: 0.0,
            step: 0.05,
        },
        unit: "",
        section: "Main",
        slider: SliderMapping::SquareFromSlider,
    }],
};

impl Describe for Contrast {
    fn schema() -> &'static FilterSchema {
        &CONTRAST_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "amount" => Some(ParamValue::Float(self.amount)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        let v = match value.as_f32() {
            Some(v) => v,
            None => return false,
        };
        match name {
            "amount" => self.amount = v,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_identity() {
        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = (i as f32 + 1.0) / 17.0; // avoid 0.0 for pow
        }
        let original = planes.l.clone();
        Contrast { amount: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        for (a, b) in planes.l.iter().zip(original.iter()) {
            assert!((a - b).abs() < 1e-5, "identity failed: {a} vs {b}");
        }
    }

    #[test]
    fn positive_increases_range() {
        let mut planes = OklabPlanes::new(2, 1);
        planes.l[0] = 0.3; // below pivot
        planes.l[1] = 0.8; // above pivot
        Contrast { amount: 0.5 }.apply(&mut planes, &mut FilterContext::new());
        // 0.3 < pivot → should get darker
        assert!(
            planes.l[0] < 0.3,
            "dark pixel should darken: {}",
            planes.l[0]
        );
        // 0.8 > pivot → should get brighter
        assert!(
            planes.l[1] > 0.8,
            "bright pixel should brighten: {}",
            planes.l[1]
        );
    }

    #[test]
    fn pivot_unchanged() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = CONTRAST_PIVOT;
        Contrast { amount: 0.8 }.apply(&mut planes, &mut FilterContext::new());
        assert!(
            (planes.l[0] - CONTRAST_PIVOT).abs() < 1e-4,
            "pivot should be unchanged: {} vs {}",
            planes.l[0],
            CONTRAST_PIVOT
        );
    }

    #[test]
    fn negative_reduces_range() {
        let mut planes = OklabPlanes::new(2, 1);
        planes.l[0] = 0.2;
        planes.l[1] = 0.9;
        let range_before = planes.l[1] - planes.l[0];
        Contrast { amount: -0.5 }.apply(&mut planes, &mut FilterContext::new());
        let range_after = planes.l[1] - planes.l[0];
        assert!(
            range_after < range_before,
            "negative contrast should reduce range: {range_after} vs {range_before}"
        );
    }
}
