use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::simd;
use zenpixels::PlaneMask;

/// Alpha channel scaling.
///
/// Multiplies the alpha plane by a constant factor. Useful for
/// fade effects or transparency adjustments. If no alpha plane
/// exists, this is a no-op.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Alpha {
    /// Alpha multiplier. 0.0 = fully transparent, 1.0 = no change.
    pub factor: f32,
}

impl Default for Alpha {
    fn default() -> Self {
        Self { factor: 1.0 }
    }
}

impl Filter for Alpha {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess {
            reads: PlaneMask::ALPHA,
            writes: PlaneMask::ALPHA,
        }
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if let Some(alpha) = &mut planes.alpha {
            if (self.factor - 1.0).abs() < 1e-6 {
                return;
            }
            simd::scale_plane(alpha, self.factor);
        }
    }
}

static ALPHA_SCHEMA: FilterSchema = FilterSchema {
    name: "alpha",
    label: "Alpha",
    description: "Alpha channel scaling for transparency adjustment",
    group: FilterGroup::Effects,
    params: &[ParamDesc {
        name: "factor",
        label: "Factor",
        description: "Alpha multiplier (0 = transparent, 1 = unchanged)",
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

impl Describe for Alpha {
    fn schema() -> &'static FilterSchema {
        &ALPHA_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "factor" => Some(ParamValue::Float(self.factor)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        let v = match value.as_f32() {
            Some(v) => v,
            None => return false,
        };
        match name {
            "factor" => self.factor = v,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_is_identity() {
        let mut planes = OklabPlanes::with_alpha(4, 4);
        for v in planes.alpha.as_mut().unwrap() {
            *v = 0.8;
        }
        let orig = planes.alpha.clone();
        Alpha { factor: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.alpha, orig);
    }

    #[test]
    fn zero_is_transparent() {
        let mut planes = OklabPlanes::with_alpha(4, 4);
        for v in planes.alpha.as_mut().unwrap() {
            *v = 0.8;
        }
        Alpha { factor: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        for &v in planes.alpha.as_ref().unwrap() {
            assert!(v.abs() < 1e-6);
        }
    }

    #[test]
    fn half_alpha() {
        let mut planes = OklabPlanes::with_alpha(4, 4);
        for v in planes.alpha.as_mut().unwrap() {
            *v = 1.0;
        }
        Alpha { factor: 0.5 }.apply(&mut planes, &mut FilterContext::new());
        for &v in planes.alpha.as_ref().unwrap() {
            assert!((v - 0.5).abs() < 1e-5);
        }
    }

    #[test]
    fn no_alpha_is_noop() {
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.l {
            *v = 0.5;
        }
        let l_orig = planes.l.clone();
        Alpha { factor: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, l_orig);
        assert!(planes.alpha.is_none());
    }

    #[test]
    fn does_not_modify_lab() {
        let mut planes = OklabPlanes::with_alpha(4, 4);
        for v in &mut planes.l {
            *v = 0.5;
        }
        for v in &mut planes.a {
            *v = 0.1;
        }
        for v in &mut planes.b {
            *v = -0.05;
        }
        for v in planes.alpha.as_mut().unwrap() {
            *v = 1.0;
        }
        let l_orig = planes.l.clone();
        let a_orig = planes.a.clone();
        let b_orig = planes.b.clone();
        Alpha { factor: 0.3 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, l_orig);
        assert_eq!(planes.a, a_orig);
        assert_eq!(planes.b, b_orig);
    }
}
