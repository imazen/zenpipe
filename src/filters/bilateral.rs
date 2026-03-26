use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::filters::guided_filter::guided_filter_plane;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Edge-preserving smoothing on all Oklab channels.
///
/// Uses a guided filter (He et al., TPAMI 2013) with L as the guide image.
/// This is O(1) per pixel regardless of radius (uses Gaussian blurs internally),
/// compared to O(r²) for a traditional bilateral filter.
///
/// The guided filter produces locally-linear output that preserves edges from
/// the luminance channel while smoothing noise in all three channels.
///
/// eps controls the smoothing/edge-preservation tradeoff:
/// - Small eps (0.001): strong edge preservation, less smoothing
/// - Large eps (0.1): more smoothing, softer edges
///
/// This replaces the traditional bilateral filter because:
/// - O(1) vs O(r²) per pixel — practical for any radius
/// - Gradient-preserving (not just edge-preserving)
/// - No exp() per neighbor — numerically stable
/// - Naturally separable via Gaussian blurs (SIMD-friendly)
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Bilateral {
    /// Spatial sigma for the smoothing window. Typical: 2.0-8.0.
    pub spatial_sigma: f32,
    /// Edge preservation parameter (eps in the guided filter).
    /// Smaller = more edge preservation. Typical: 0.001-0.05.
    /// Relates to range_sigma² of a traditional bilateral: eps ≈ range_sigma².
    pub range_sigma: f32,
    /// Blend strength. 0.0 = no effect, 1.0 = full smoothing.
    pub strength: f32,
}

impl Default for Bilateral {
    fn default() -> Self {
        Self {
            spatial_sigma: 2.0,
            range_sigma: 0.1,
            strength: 0.0,
        }
    }
}

impl Filter for Bilateral {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        (self.spatial_sigma * 3.0).ceil() as u32
    }

    fn resize_phase(&self) -> crate::filter::ResizePhase {
        crate::filter::ResizePhase::PreResize
    }
    fn scale_for_resolution(&mut self, scale: f32) {
        self.spatial_sigma = (self.spatial_sigma * scale).max(0.5);
    }
    fn tag(&self) -> crate::filter_compat::FilterTag {
        crate::filter_compat::FilterTag::Bilateral
    }
    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.strength.abs() < 1e-6 {
            return;
        }

        let w = planes.width;
        let h = planes.height;
        let n = (w as usize) * (h as usize);

        // eps = range_sigma² (maps traditional bilateral range parameter to guided filter eps)
        let eps = self.range_sigma * self.range_sigma;
        let strength = self.strength;

        // Guided filter each channel with L as guide
        let mut guide = ctx.take_f32(n);
        guide.copy_from_slice(&planes.l);

        // Filter L (self-guided)
        let mut filtered = ctx.take_f32(n);
        guided_filter_plane(
            &planes.l,
            &guide,
            &mut filtered,
            w,
            h,
            self.spatial_sigma,
            eps,
            ctx,
        );
        for (l, f) in planes.l.iter_mut().zip(filtered.iter()).take(n) {
            *l = *l * (1.0 - strength) + *f * strength;
        }

        // Filter a (L-guided)
        guided_filter_plane(
            &planes.a,
            &guide,
            &mut filtered,
            w,
            h,
            self.spatial_sigma,
            eps,
            ctx,
        );
        for (a, f) in planes.a.iter_mut().zip(filtered.iter()).take(n) {
            *a = *a * (1.0 - strength) + *f * strength;
        }

        // Filter b (L-guided)
        guided_filter_plane(
            &planes.b,
            &guide,
            &mut filtered,
            w,
            h,
            self.spatial_sigma,
            eps,
            ctx,
        );
        for (b, f) in planes.b.iter_mut().zip(filtered.iter()).take(n) {
            *b = *b * (1.0 - strength) + *f * strength;
        }

        ctx.return_f32(filtered);
        ctx.return_f32(guide);
    }
}

static BILATERAL_SCHEMA: FilterSchema = FilterSchema {
    name: "bilateral",
    label: "Bilateral Filter",
    description: "Edge-preserving smoothing via guided filter",
    group: FilterGroup::Detail,
    params: &[
        ParamDesc {
            name: "spatial_sigma",
            label: "Spatial Sigma",
            description: "Smoothing window size",
            kind: ParamKind::Float {
                min: 0.5,
                max: 20.0,
                default: 2.0,
                identity: 2.0,
                step: 0.5,
            },
            unit: "px",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "range_sigma",
            label: "Range Sigma",
            description: "Edge preservation (smaller = sharper edges)",
            kind: ParamKind::Float {
                min: 0.001,
                max: 0.5,
                default: 0.1,
                identity: 0.1,
                step: 0.01,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "strength",
            label: "Strength",
            description: "Blend strength (0 = off, 1 = full smoothing)",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.0,
                identity: 0.0,
                step: 0.05,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for Bilateral {
    fn schema() -> &'static FilterSchema {
        &BILATERAL_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "spatial_sigma" => Some(ParamValue::Float(self.spatial_sigma)),
            "range_sigma" => Some(ParamValue::Float(self.range_sigma)),
            "strength" => Some(ParamValue::Float(self.strength)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        let v = match value.as_f32() {
            Some(v) => v,
            None => return false,
        };
        match name {
            "spatial_sigma" => self.spatial_sigma = v,
            "range_sigma" => self.range_sigma = v,
            "strength" => self.strength = v,
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
    fn zero_strength_is_identity() {
        let mut planes = OklabPlanes::new(8, 8);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 64.0;
        }
        let original = planes.l.clone();
        Bilateral {
            spatial_sigma: 2.0,
            range_sigma: 0.1,
            strength: 0.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn smooths_uniform_noise() {
        let mut planes = OklabPlanes::new(16, 16);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = 0.5 + if i % 3 == 0 { 0.05 } else { -0.02 };
        }
        let before_var = variance(&planes.l);
        Bilateral {
            spatial_sigma: 2.0,
            range_sigma: 0.1,
            strength: 1.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        let after_var = variance(&planes.l);
        assert!(
            after_var < before_var,
            "guided filter should reduce noise: {before_var} -> {after_var}"
        );
    }

    #[test]
    fn preserves_edges() {
        // Create a sharp edge: left half = 0.3, right half = 0.7
        let mut planes = OklabPlanes::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                planes.l[y * 32 + x] = if x < 16 { 0.3 } else { 0.7 };
            }
        }

        Bilateral {
            spatial_sigma: 3.0,
            range_sigma: 0.05,
            strength: 1.0,
        }
        .apply(&mut planes, &mut FilterContext::new());

        // Edge should be preserved: far-left should still be much darker than far-right
        let left = planes.l[16 * 32 + 2]; // well inside left region
        let right = planes.l[16 * 32 + 29]; // well inside right region
        assert!(
            (right - left) > 0.3,
            "edge should be preserved: left={left}, right={right}, diff={}",
            right - left
        );
    }

    fn variance(data: &[f32]) -> f32 {
        let mean = data.iter().sum::<f32>() / data.len() as f32;
        data.iter().map(|&v| (v - mean) * (v - mean)).sum::<f32>() / data.len() as f32
    }
}
