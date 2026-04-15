use crate::access::ChannelAccess;
use crate::blur::{GaussianKernel, gaussian_blur_plane};
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::simd;

/// L-channel sharpening via unsharp mask.
///
/// Like clarity but with a smaller sigma for fine detail enhancement.
/// Sharpening in Oklab L avoids the color fringing that RGB sharpening
/// produces at high-contrast edges.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Sharpen {
    /// Blur sigma. Small values (0.5-2.0) for fine sharpening.
    pub sigma: f32,
    /// Sharpening amount. Typical: 0.3-1.0.
    pub amount: f32,
}

impl Default for Sharpen {
    fn default() -> Self {
        Self {
            sigma: 1.0,
            amount: 0.0,
        }
    }
}

impl Filter for Sharpen {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn plane_semantics(&self) -> crate::filter::PlaneSemantics {
        crate::filter::PlaneSemantics::Any
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        (self.sigma * 3.0).ceil() as u32
    }

    fn resize_phase(&self) -> crate::filter::ResizePhase {
        crate::filter::ResizePhase::PreResize
    }
    fn scale_for_resolution(&mut self, scale: f32) {
        self.sigma = (self.sigma * scale).max(0.5);
    }
    fn tag(&self) -> crate::filter_compat::FilterTag {
        crate::filter_compat::FilterTag::Sharpen
    }
    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.amount.abs() < 1e-6 {
            return;
        }
        let kernel = GaussianKernel::new(self.sigma);
        let mut blurred = ctx.take_f32(planes.pixel_count());
        gaussian_blur_plane(
            &planes.l,
            &mut blurred,
            planes.width,
            planes.height,
            &kernel,
            ctx,
        );

        let mut dst = ctx.take_f32(planes.pixel_count());
        simd::unsharp_fuse(&planes.l, &blurred, &mut dst, self.amount);
        ctx.return_f32(blurred);
        let old_l = core::mem::replace(&mut planes.l, dst);
        ctx.return_f32(old_l);
    }
}

static SHARPEN_SCHEMA: FilterSchema = FilterSchema {
    name: "sharpen",
    label: "Sharpen",
    description: "Unsharp mask sharpening on L channel",
    group: FilterGroup::Detail,
    params: &[
        ParamDesc {
            name: "sigma",
            label: "Radius",
            description: "Blur sigma for detail extraction",
            kind: ParamKind::Float {
                min: 0.5,
                max: 3.0,
                default: 1.0,
                identity: 1.0,
                step: 0.1,
            },
            unit: "px",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "amount",
            label: "Amount",
            description: "Sharpening strength",
            kind: ParamKind::Float {
                min: 0.0,
                max: 2.0,
                default: 0.0,
                identity: 0.0,
                step: 0.05,
            },
            unit: "×",
            section: "Main",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for Sharpen {
    fn schema() -> &'static FilterSchema {
        &SHARPEN_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "sigma" => Some(ParamValue::Float(self.sigma)),
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
            "sigma" => self.sigma = v,
            "amount" => self.amount = v,
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
        let mut planes = OklabPlanes::new(16, 16);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 256.0;
        }
        let original = planes.l.clone();
        Sharpen {
            sigma: 1.0,
            amount: 0.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn enhances_edges() {
        let mut planes = OklabPlanes::new(32, 32);
        // Step edge
        for y in 0..32u32 {
            for x in 0..32u32 {
                let i = planes.index(x, y);
                planes.l[i] = if x < 16 { 0.3 } else { 0.7 };
            }
        }
        Sharpen {
            sigma: 1.0,
            amount: 1.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        // Pixels near the edge should be pushed further apart
        let left = planes.l[planes.index(14, 16)];
        let right = planes.l[planes.index(17, 16)];
        assert!(
            right - left > 0.4,
            "edge should be sharpened: {left} vs {right}"
        );
    }
}
