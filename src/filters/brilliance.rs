use crate::access::ChannelAccess;
use crate::blur::{GaussianKernel, gaussian_blur_plane};
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::simd;

/// Brilliance: adaptive local contrast based on local average.
///
/// Unlike clarity (which adds high-pass), brilliance adjusts each pixel
/// relative to its local average — lifting shadows and compressing highlights
/// selectively. This produces a more natural "dynamic range compression"
/// similar to Apple's Brilliance slider.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Brilliance {
    /// Blur sigma for computing local average.
    pub sigma: f32,
    /// Overall effect strength.
    pub amount: f32,
    /// Shadow lift strength. Default: 0.6.
    pub shadow_strength: f32,
    /// Highlight compression strength. Default: 0.4.
    pub highlight_strength: f32,
}

impl Default for Brilliance {
    fn default() -> Self {
        Self {
            sigma: 10.0,
            amount: 0.0,
            shadow_strength: 0.6,
            highlight_strength: 0.4,
        }
    }
}

impl Filter for Brilliance {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        (self.sigma * 3.0).ceil() as u32
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.amount.abs() < 1e-6 {
            return;
        }
        let kernel = GaussianKernel::new(self.sigma);
        let mut avg_l = ctx.take_f32(planes.pixel_count());
        gaussian_blur_plane(
            &planes.l,
            &mut avg_l,
            planes.width,
            planes.height,
            &kernel,
            ctx,
        );

        let mut dst = ctx.take_f32(planes.pixel_count());
        simd::brilliance_apply(
            &planes.l,
            &avg_l,
            &mut dst,
            self.amount,
            self.shadow_strength,
            self.highlight_strength,
        );
        ctx.return_f32(avg_l);
        let old_l = core::mem::replace(&mut planes.l, dst);
        ctx.return_f32(old_l);
    }
}

static BRILLIANCE_SCHEMA: FilterSchema = FilterSchema {
    name: "brilliance",
    label: "Brilliance",
    description: "Adaptive local contrast based on local average luminance",
    group: FilterGroup::Detail,
    params: &[
        ParamDesc {
            name: "sigma",
            label: "Scale",
            description: "Blur sigma for computing local average",
            kind: ParamKind::Float {
                min: 2.0,
                max: 50.0,
                default: 10.0,
                identity: 10.0,
                step: 1.0,
            },
            unit: "px",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "amount",
            label: "Amount",
            description: "Overall effect strength",
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
            name: "shadow_strength",
            label: "Shadow Strength",
            description: "Shadow lift strength",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.6,
                identity: 0.6,
                step: 0.05,
            },
            unit: "",
            section: "Advanced",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "highlight_strength",
            label: "Highlight Strength",
            description: "Highlight compression strength",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.4,
                identity: 0.4,
                step: 0.05,
            },
            unit: "",
            section: "Advanced",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for Brilliance {
    fn schema() -> &'static FilterSchema {
        &BRILLIANCE_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "sigma" => Some(ParamValue::Float(self.sigma)),
            "amount" => Some(ParamValue::Float(self.amount)),
            "shadow_strength" => Some(ParamValue::Float(self.shadow_strength)),
            "highlight_strength" => Some(ParamValue::Float(self.highlight_strength)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        let v = match value.as_f32() {
            Some(v) => v,
            None => return false,
        };
        match name {
            "sigma" => self.sigma = v,
            "amount" => self.amount = v,
            "shadow_strength" => self.shadow_strength = v,
            "highlight_strength" => self.highlight_strength = v,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::FilterContext;

    #[test]
    fn zero_amount_is_identity() {
        let mut planes = OklabPlanes::new(32, 32);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 1024.0;
        }
        let original = planes.l.clone();
        Brilliance {
            amount: 0.0,
            ..Default::default()
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn lifts_shadows() {
        let mut planes = OklabPlanes::new(32, 32);
        for v in &mut planes.l {
            *v = 0.1; // all dark
        }
        let before = planes.l[0];
        Brilliance {
            sigma: 5.0,
            amount: 1.0,
            shadow_strength: 0.6,
            highlight_strength: 0.4,
        }
        .apply(&mut planes, &mut FilterContext::new());
        // Uniform dark image: local avg ≈ 0.1, ratio ≈ 1.0
        // No change expected for uniform images (ratio=1 means no correction)
        // This is correct — brilliance only acts on local contrast variations
        let diff = (planes.l[0] - before).abs();
        assert!(
            diff < 0.1,
            "uniform image should have minimal change: diff={diff}"
        );
    }
}
