use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::simd;

/// Tint adjustment via Oklab a-channel shift.
///
/// Positive values shift toward magenta, negative toward green.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Tint {
    /// Tint shift. -1.0 (green) to +1.0 (magenta). 0.0 = no change.
    pub shift: f32,
}

impl Filter for Tint {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::CHROMA_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.shift.abs() < 1e-6 {
            return;
        }
        // Same 0.12 scale as temperature — ±1.0 shift maps to ±0.12 Oklab a offset.
        // Oklab a axis: positive = magenta/red, negative = green.
        let offset = self.shift * 0.12;
        simd::offset_plane(&mut planes.a, offset);
    }
}

static TINT_SCHEMA: FilterSchema = FilterSchema {
    name: "tint",
    label: "Tint",
    description: "Green-magenta tint adjustment via Oklab a shift",
    group: FilterGroup::Color,
    params: &[ParamDesc {
        name: "shift",
        label: "Tint",
        description: "Tint shift (negative = green, positive = magenta)",
        kind: ParamKind::Float {
            min: -1.0,
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

impl Describe for Tint {
    fn schema() -> &'static FilterSchema {
        &TINT_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "shift" => Some(ParamValue::Float(self.shift)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        let v = match value.as_f32() {
            Some(v) => v,
            None => return false,
        };
        match name {
            "shift" => self.shift = v,
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
        let mut planes = OklabPlanes::new(2, 1);
        planes.a[0] = 0.05;
        let original = planes.a.clone();
        Tint { shift: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a, original);
    }

    #[test]
    fn positive_shifts_magenta() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.a[0] = 0.0;
        Tint { shift: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        assert!(planes.a[0] > 0.0, "positive shift should increase a");
    }
}
