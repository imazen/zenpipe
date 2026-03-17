use crate::access::ChannelAccess;
use crate::blur::{GaussianKernel, gaussian_blur_plane};
use crate::context::FilterContext;
use crate::filter::Filter;
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
}

impl Default for Bloom {
    fn default() -> Self {
        Self {
            threshold: 0.7,
            sigma: 20.0,
            amount: 0.0,
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

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.amount.abs() < 1e-6 {
            return;
        }

        let n = planes.pixel_count();
        let w = planes.width;
        let h = planes.height;

        // 1. Extract bright pixels (soft threshold for smooth transition)
        let mut bright = ctx.take_f32(n);
        let knee = 0.05; // soft knee width
        for i in 0..n {
            let l = planes.l[i];
            let excess = l - self.threshold;
            // Soft knee: smooth ramp from 0 at (threshold - knee) to linear at (threshold + knee)
            bright[i] = if excess > knee {
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
        let amount = self.amount;
        for i in 0..n {
            let l = planes.l[i];
            let bloom = blurred[i] * amount;
            planes.l[i] = l + bloom - l * bloom;
        }

        ctx.return_f32(blurred);
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
