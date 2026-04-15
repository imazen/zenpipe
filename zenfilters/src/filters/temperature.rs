use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::simd;

/// Color temperature adjustment via Oklab b-channel shift.
///
/// Positive values warm the image (shift toward yellow/orange).
/// Negative values cool it (shift toward blue).
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Temperature {
    /// Temperature shift. -1.0 (cool) to +1.0 (warm). 0.0 = no change.
    pub shift: f32,
}

impl Filter for Temperature {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::CHROMA_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.shift.abs() < 1e-6 {
            return;
        }
        // Scale: ±1.0 shift maps to ±0.12 Oklab b offset.
        // 0.12 gives shift=0.25 → 0.03 offset, clearly visible as a warm/cool
        // tint. Shift=1.0 → 0.12 offset ≈ 1500K temperature change.
        // Oklab b axis spans roughly [-0.3, +0.3] for saturated colors,
        // so 0.12 is a strong but non-destructive shift.
        let offset = self.shift * 0.12;
        simd::offset_plane(&mut planes.b, offset);
    }
}

static TEMPERATURE_SCHEMA: FilterSchema = FilterSchema {
    name: "temperature",
    label: "Temperature",
    description: "Color temperature adjustment (warm/cool) via Oklab b shift",
    group: FilterGroup::Color,
    params: &[ParamDesc {
        name: "shift",
        label: "Temperature",
        description: "Color temperature shift (negative = cool, positive = warm)",
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

impl Describe for Temperature {
    fn schema() -> &'static FilterSchema {
        &TEMPERATURE_SCHEMA
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
        planes.b[0] = 0.05;
        let original = planes.b.clone();
        Temperature { shift: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.b, original);
    }

    #[test]
    fn positive_warms() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.b[0] = 0.0;
        Temperature { shift: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        assert!(planes.b[0] > 0.0, "positive shift should increase b");
    }
}
