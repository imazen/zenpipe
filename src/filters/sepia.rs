use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::simd;

/// Sepia tone filter in Oklab space.
///
/// Desaturates the image, then applies a warm brown tint by shifting
/// the a and b channels toward the sepia point. The amount controls
/// the blend between grayscale and full sepia tint.
///
/// In Oklab, sepia is approximately a≈+0.01, b≈+0.04 (warm yellow-brown).
/// This produces more natural results than the classic sRGB sepia matrix
/// because the tint is applied in perceptually uniform space.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Sepia {
    /// Sepia strength. 0.0 = grayscale, 1.0 = full sepia.
    pub amount: f32,
}

impl Default for Sepia {
    fn default() -> Self {
        Self { amount: 1.0 }
    }
}

static SEPIA_SCHEMA: FilterSchema = FilterSchema {
    name: "sepia",
    label: "Sepia",
    description: "Sepia tone effect in perceptual Oklab space",
    group: FilterGroup::Color,
    params: &[ParamDesc {
        name: "amount",
        label: "Amount",
        description: "Sepia strength (0 = grayscale, 1 = full sepia)",
        kind: ParamKind::Float {
            min: 0.0,
            max: 1.0,
            default: 1.0,
            identity: 1.0,
            step: 0.05,
        },
        unit: "",
        section: "Main",
        slider: SliderMapping::Linear,
    }],
};

impl Describe for Sepia {
    fn schema() -> &'static FilterSchema {
        &SEPIA_SCHEMA
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

/// Oklab a component of sepia tone (warm reddish).
const SEPIA_A: f32 = 0.01;
/// Oklab b component of sepia tone (warm yellowish).
const SEPIA_B: f32 = 0.04;

impl Filter for Sepia {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::CHROMA_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        // Zero chroma (grayscale), then set to sepia tint
        let a_target = SEPIA_A * self.amount;
        let b_target = SEPIA_B * self.amount;
        planes.a.fill(0.0);
        planes.b.fill(0.0);
        simd::offset_plane(&mut planes.a, a_target);
        simd::offset_plane(&mut planes.b, b_target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_amount_is_grayscale() {
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.a {
            *v = 0.05;
        }
        for v in &mut planes.b {
            *v = -0.03;
        }
        Sepia { amount: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        for &v in &planes.a {
            assert!(v.abs() < 1e-6);
        }
        for &v in &planes.b {
            assert!(v.abs() < 1e-6);
        }
    }

    #[test]
    fn full_sepia_has_warm_tint() {
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.l {
            *v = 0.5;
        }
        Sepia { amount: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        for &v in &planes.a {
            assert!((v - 0.01).abs() < 1e-5);
        }
        for &v in &planes.b {
            assert!((v - 0.04).abs() < 1e-5);
        }
    }

    #[test]
    fn does_not_modify_l() {
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.l {
            *v = 0.7;
        }
        let l_orig = planes.l.clone();
        Sepia { amount: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, l_orig);
    }
}
