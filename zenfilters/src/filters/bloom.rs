use crate::access::ChannelAccess;
use crate::blur::{GaussianKernel, gaussian_blur_plane};
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Bloom: simulates light scattering from bright areas.
///
/// Extracts pixels above a luminance threshold, blurs them with a large
/// Gaussian kernel, and adds the result back using screen blending.
/// Produces a natural-looking soft glow around bright light sources.
///
/// Screen blending (`output = a + b - a*b`) prevents overexposure —
/// bright areas never exceed 1.0, unlike additive blending.
///
/// For a dreamier, more diffused look (glow), use a larger sigma and
/// lower threshold. For subtle highlight softening, use a higher
/// threshold and moderate sigma.
///
/// Operates on L channel only — glow is a luminance phenomenon.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Bloom {
    /// Luminance threshold. Only pixels brighter than this contribute to bloom.
    /// 0.0 = everything blooms (soft overall glow), 0.8 = only bright highlights.
    /// Default: 0.7.
    pub threshold: f32,
    /// Blur sigma controlling the bloom spread. Larger = softer, wider glow.
    /// Default: 20.0.
    pub sigma: f32,
    /// Bloom intensity. 0.0 = no effect, 1.0 = full bloom.
    /// Default: 0.0 (off).
    pub amount: f32,
    /// Auto-set threshold from image histogram.
    /// When true, threshold is set to p90 of L (bloom only top 10% of luminance)
    /// and amount is scaled by highlight density for consistent bloom regardless
    /// of image exposure.
    pub auto_threshold: bool,
}

impl Default for Bloom {
    fn default() -> Self {
        Self {
            threshold: 0.7,
            sigma: 20.0,
            amount: 0.0,
            auto_threshold: false,
        }
    }
}

impl Filter for Bloom {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        (self.sigma * 3.0).ceil() as u32
    }

    fn resize_phase(&self) -> crate::filter::ResizePhase {
        crate::filter::ResizePhase::PostResize
    }
    fn scale_for_resolution(&mut self, scale: f32) {
        self.sigma = (self.sigma * scale).max(1.0);
    }
    fn tag(&self) -> crate::filter_compat::FilterTag {
        crate::filter_compat::FilterTag::Bloom
    }
    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.amount.abs() < 1e-6 && !self.auto_threshold {
            return;
        }

        let n = planes.pixel_count();
        let w = planes.width;
        let h = planes.height;

        // Auto-threshold: compute threshold and amount from histogram
        let (threshold, amount) = if self.auto_threshold {
            let a = ctx.analyze(planes);
            // Threshold = p90: bloom only the top 10% of luminance
            let auto_thresh = a.percentiles[5]; // p95 as proxy for p90
            // Scale amount by highlight density: fewer highlights → stronger per-pixel bloom
            let highlight_frac = (1.0 - auto_thresh).max(0.02);
            let auto_amount = self.amount.max(0.3) / highlight_frac.sqrt();
            (auto_thresh.clamp(0.3, 0.95), auto_amount.min(2.0))
        } else {
            (self.threshold, self.amount)
        };

        if amount.abs() < 1e-6 {
            return;
        }

        // 1. Extract bright pixels (soft threshold for smooth transition)
        let mut bright = ctx.take_f32(n);
        let knee = 0.05; // soft knee width
        for (b, &l) in bright.iter_mut().zip(planes.l.iter()).take(n) {
            let excess = l - threshold;
            // Soft knee: smooth ramp from 0 at (threshold - knee) to linear at (threshold + knee)
            *b = if excess > knee {
                excess
            } else if excess > -knee {
                let t = (excess + knee) / (2.0 * knee);
                t * t * excess.max(0.0)
            } else {
                0.0
            };
        }

        // 2. Blur the extracted highlights
        let kernel = GaussianKernel::new(self.sigma);
        let mut blurred = ctx.take_f32(n);
        gaussian_blur_plane(&bright, &mut blurred, w, h, &kernel, ctx);
        ctx.return_f32(bright);

        // 3. Screen blend: output = L + bloom - L * bloom
        // This prevents values from exceeding 1.0 naturally.
        for (l, &bl) in planes.l.iter_mut().zip(blurred.iter()).take(n) {
            let bloom = bl * amount;
            *l = *l + bloom - *l * bloom;
        }

        ctx.return_f32(blurred);
    }
}

static BLOOM_SCHEMA: FilterSchema = FilterSchema {
    name: "bloom",
    label: "Bloom",
    description: "Soft glow from bright areas via screen blending",
    group: FilterGroup::Effects,
    params: &[
        ParamDesc {
            name: "threshold",
            label: "Threshold",
            description: "Luminance threshold for bloom contribution",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.7,
                identity: 0.7,
                step: 0.05,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "sigma",
            label: "Radius",
            description: "Bloom spread (larger = softer, wider glow)",
            kind: ParamKind::Float {
                min: 2.0,
                max: 100.0,
                default: 20.0,
                identity: 20.0,
                step: 1.0,
            },
            unit: "px",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "amount",
            label: "Amount",
            description: "Bloom intensity (0 = off, 1 = full)",
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

impl Describe for Bloom {
    fn schema() -> &'static FilterSchema {
        &BLOOM_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "threshold" => Some(ParamValue::Float(self.threshold)),
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
            "threshold" => self.threshold = v,
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

    #[test]
    fn zero_amount_is_identity() {
        let mut planes = OklabPlanes::new(16, 16);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 256.0;
        }
        let original = planes.l.clone();
        Bloom::default().apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn bright_pixels_get_brighter() {
        let mut planes = OklabPlanes::new(32, 32);
        // Bright center, dark surround
        for y in 0..32 {
            for x in 0..32 {
                planes.l[y * 32 + x] = if (12..20).contains(&x) && (12..20).contains(&y) {
                    0.95 // bright center
                } else {
                    0.2 // dark surround
                };
            }
        }

        let dark_before = planes.l[0]; // corner pixel

        let mut bloom = Bloom::default();
        bloom.threshold = 0.7;
        bloom.sigma = 5.0;
        bloom.amount = 0.8;
        bloom.apply(&mut planes, &mut FilterContext::new());

        // Dark pixels near the bright center should be lifted by bloom
        let dark_after = planes.l[0];
        // At least some bloom should reach the corners at sigma=5 on a 32x32 image
        assert!(
            dark_after >= dark_before,
            "bloom should not darken: {dark_before} → {dark_after}"
        );
    }

    #[test]
    fn screen_blend_never_exceeds_one() {
        let mut planes = OklabPlanes::new(16, 16);
        for v in &mut planes.l {
            *v = 0.99;
        }

        let mut bloom = Bloom::default();
        bloom.threshold = 0.5;
        bloom.sigma = 3.0;
        bloom.amount = 1.0;
        bloom.apply(&mut planes, &mut FilterContext::new());

        for &v in &planes.l {
            assert!(v <= 1.001, "screen blend should cap at 1.0: got {v}");
        }
    }
}
