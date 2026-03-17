use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::simd;

/// Black point adjustment on Oklab L channel.
///
/// Remaps the shadow floor. A black point of 0.05 means values that were
/// L=0.05 become L=0.0, and the range is stretched accordingly.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct BlackPoint {
    /// Black point level. 0.0 = no change, 0.1 = crush bottom 10%.
    pub level: f32,
}

impl Filter for BlackPoint {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.level.abs() < 1e-6 {
            return;
        }
        let bp = self.level;
        let range = (1.0 - bp).max(0.01);
        let inv_range = 1.0 / range;
        simd::black_point_plane(&mut planes.l, bp, inv_range);
    }
}

static BLACK_POINT_SCHEMA: FilterSchema = FilterSchema {
    name: "black_point",
    label: "Black Point",
    description: "Remap shadow floor to crush or lift darkest values",
    group: FilterGroup::ToneRange,
    params: &[ParamDesc {
        name: "level",
        label: "Level",
        description: "Black point level (0 = no change, 0.1 = crush bottom 10%)",
        kind: ParamKind::Float {
            min: 0.0,
            max: 0.5,
            default: 0.0,
            identity: 0.0,
            step: 0.01,
        },
        unit: "",
        section: "Main",
        slider: SliderMapping::Linear,
    }],
};

impl Describe for BlackPoint {
    fn schema() -> &'static FilterSchema {
        &BLACK_POINT_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "level" => Some(ParamValue::Float(self.level)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        let v = match value.as_f32() {
            Some(v) => v,
            None => return false,
        };
        match name {
            "level" => self.level = v,
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
        planes.l[0] = 0.3;
        planes.l[1] = 0.8;
        let original = planes.l.clone();
        BlackPoint { level: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn crushes_shadows() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.05; // just at the black point
        BlackPoint { level: 0.05 }.apply(&mut planes, &mut FilterContext::new());
        assert!(planes.l[0].abs() < 1e-5, "should be near zero");
    }
}
