use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::simd;

/// Smart saturation that protects already-saturated colors.
///
/// Boosts chroma of low-saturation pixels more than high-saturation ones,
/// preventing skin tone and sky clipping. The protection curve is:
/// `scale = 1 + amount * (1 - chroma / max_chroma)^protection`
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Vibrance {
    /// Vibrance amount. 0.0 = no change, 1.0 = full boost.
    pub amount: f32,
    /// Protection exponent. Higher = more protection for saturated colors.
    /// Default: 2.0.
    pub protection: f32,
}

impl Default for Vibrance {
    fn default() -> Self {
        Self {
            amount: 0.0,
            protection: 2.0,
        }
    }
}

impl Filter for Vibrance {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::CHROMA_ONLY
    }

    fn tag(&self) -> crate::filter_compat::FilterTag {
        crate::filter_compat::FilterTag::Vibrance
    }
    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.amount.abs() < 1e-6 {
            return;
        }
        simd::vibrance(&mut planes.a, &mut planes.b, self.amount, self.protection);
    }
}

static VIBRANCE_SCHEMA: FilterSchema = FilterSchema {
    name: "vibrance",
    label: "Vibrance",
    description: "Smart saturation that protects already-saturated colors",
    group: FilterGroup::Color,
    params: &[
        ParamDesc {
            name: "amount",
            label: "Amount",
            description: "Vibrance boost (0 = off, 1 = full)",
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
        },
        ParamDesc {
            name: "protection",
            label: "Protection",
            description: "Protection exponent for already-saturated colors",
            kind: ParamKind::Float {
                min: 0.5,
                max: 4.0,
                default: 2.0,
                identity: 2.0,
                step: 0.1,
            },
            unit: "",
            section: "Advanced",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for Vibrance {
    fn schema() -> &'static FilterSchema {
        &VIBRANCE_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "amount" => Some(ParamValue::Float(self.amount)),
            "protection" => Some(ParamValue::Float(self.protection)),
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
            "protection" => self.protection = v,
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
        planes.a[0] = 0.1;
        planes.b[0] = -0.05;
        let a_orig = planes.a.clone();
        let b_orig = planes.b.clone();
        Vibrance {
            amount: 0.0,
            ..Default::default()
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a, a_orig);
        assert_eq!(planes.b, b_orig);
    }

    #[test]
    fn low_saturation_gets_more_boost() {
        let mut planes = OklabPlanes::new(2, 1);
        // Pixel 0: low saturation
        planes.a[0] = 0.02;
        planes.b[0] = 0.01;
        // Pixel 1: high saturation
        planes.a[1] = 0.3;
        planes.b[1] = 0.2;

        let c0_before = (planes.a[0] * planes.a[0] + planes.b[0] * planes.b[0]).sqrt();
        let c1_before = (planes.a[1] * planes.a[1] + planes.b[1] * planes.b[1]).sqrt();

        Vibrance {
            amount: 0.5,
            protection: 2.0,
        }
        .apply(&mut planes, &mut FilterContext::new());

        let c0_after = (planes.a[0] * planes.a[0] + planes.b[0] * planes.b[0]).sqrt();
        let c1_after = (planes.a[1] * planes.a[1] + planes.b[1] * planes.b[1]).sqrt();

        let boost0 = c0_after / c0_before;
        let boost1 = c1_after / c1_before;
        assert!(
            boost0 > boost1,
            "low-sat pixel should get bigger boost: {boost0} vs {boost1}"
        );
    }
}
