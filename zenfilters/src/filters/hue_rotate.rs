use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::simd;

/// Hue rotation in Oklab a/b plane.
///
/// Rotates colors around the hue circle by the specified angle in degrees.
/// Preserves lightness and chroma.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct HueRotate {
    /// Rotation angle in degrees. 0.0 = no change, 180.0 = invert hues.
    pub degrees: f32,
}

impl Filter for HueRotate {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::CHROMA_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        let rad = self.degrees.to_radians();
        if rad.abs() < 1e-6 {
            return;
        }
        let cos_r = rad.cos();
        let sin_r = rad.sin();
        simd::hue_rotate(&mut planes.a, &mut planes.b, cos_r, sin_r);
    }
}

static HUE_ROTATE_SCHEMA: FilterSchema = FilterSchema {
    name: "hue_rotate",
    label: "Hue Rotate",
    description: "Rotate all colors around the hue circle",
    group: FilterGroup::Color,
    params: &[ParamDesc {
        name: "degrees",
        label: "Degrees",
        description: "Rotation angle in degrees",
        kind: ParamKind::Float {
            min: -180.0,
            max: 180.0,
            default: 0.0,
            identity: 0.0,
            step: 5.0,
        },
        unit: "\u{b0}",
        section: "Main",
        slider: SliderMapping::Linear,
    }],
};

impl Describe for HueRotate {
    fn schema() -> &'static FilterSchema {
        &HUE_ROTATE_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "degrees" => Some(ParamValue::Float(self.degrees)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        let v = match value.as_f32() {
            Some(v) => v,
            None => return false,
        };
        match name {
            "degrees" => self.degrees = v,
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
        let mut planes = OklabPlanes::new(1, 1);
        planes.a[0] = 0.1;
        planes.b[0] = 0.05;
        let a_orig = planes.a[0];
        let b_orig = planes.b[0];
        HueRotate { degrees: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        assert!((planes.a[0] - a_orig).abs() < 1e-6);
        assert!((planes.b[0] - b_orig).abs() < 1e-6);
    }

    #[test]
    fn full_rotation_is_identity() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.a[0] = 0.1;
        planes.b[0] = 0.05;
        HueRotate { degrees: 360.0 }.apply(&mut planes, &mut FilterContext::new());
        assert!((planes.a[0] - 0.1).abs() < 1e-5);
        assert!((planes.b[0] - 0.05).abs() < 1e-5);
    }

    #[test]
    fn preserves_chroma() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.a[0] = 0.1;
        planes.b[0] = 0.05;
        let c_before = (0.1f32 * 0.1 + 0.05 * 0.05).sqrt();
        HueRotate { degrees: 90.0 }.apply(&mut planes, &mut FilterContext::new());
        let c_after = (planes.a[0] * planes.a[0] + planes.b[0] * planes.b[0]).sqrt();
        assert!((c_before - c_after).abs() < 1e-6);
    }
}
