use crate::access::ChannelAccess;
use crate::blur::{GaussianKernel, gaussian_blur_plane};
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;

/// Dehaze: spatially-adaptive haze removal on Oklab planes.
///
/// Uses a dark channel prior analog in Oklab space: hazy regions have
/// uniformly high L values with low local variance. The algorithm:
///
/// 1. Estimate local atmosphere (coarse Gaussian blur of L)
/// 2. Compute per-pixel transmission estimate from local/global contrast
/// 3. Recover scene luminance: `L' = (L - atm) / max(t, t_min) + atm`
/// 4. Boost chroma proportionally (haze desaturates)
///
/// This is much better than the naive global contrast+chroma approach
/// because hazy regions get strong correction while clear regions are
/// barely affected.
///
/// Inspired by He et al.'s dark channel prior, adapted for Oklab L.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct Dehaze {
    /// Dehaze strength. 0.0 = no change, 1.0 = full correction.
    ///
    /// Response is compressive (inverse transmission): small values already
    /// have visible effect. For slider integration, use [`Dehaze::from_slider`]
    /// which applies sqrt remapping.
    pub strength: f32,
}

impl Dehaze {
    /// Create from a perceptual slider value (0.0 to 1.0).
    ///
    /// Sqrt remapping: slider 0.5 → internal 0.25 (moderate dehaze).
    pub fn from_slider(slider: f32) -> Self {
        Self {
            strength: crate::slider::dehaze_from_slider(slider.clamp(0.0, 1.0)),
        }
    }
}

impl Filter for Dehaze {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, width: u32, height: u32) -> u32 {
        let sigma = (width.min(height) as f32 / 8.0).max(10.0);
        (sigma * 3.0).ceil() as u32
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.strength.abs() < 1e-6 {
            return;
        }

        let pc = planes.pixel_count();
        let w = planes.width;
        let h = planes.height;
        let s = self.strength;

        // Estimate atmospheric light via large-scale blur.
        // This gives a spatially-varying estimate of the "haze level" at each pixel.
        let sigma = (w.min(h) as f32 / 8.0).max(10.0);
        let kernel = GaussianKernel::new(sigma);
        let mut atmosphere = ctx.take_f32(pc);
        gaussian_blur_plane(&planes.l, &mut atmosphere, w, h, &kernel, ctx);

        // Estimate global airlight from the top 0.1% brightest pixels in atmosphere map
        let airlight = estimate_airlight(&atmosphere);
        let airlight = airlight.max(0.3); // safety floor

        // Minimum transmission to avoid division explosion
        let t_min = 0.1f32;

        // Apply dehaze to L: recover scene luminance from hazy observation
        for (l_val, atm) in planes.l.iter_mut().zip(atmosphere.iter()) {
            // Transmission estimate: how much "scene" vs "haze" at this pixel.
            // t = 1 - strength * (atm / airlight)
            // High atm relative to airlight → low transmission → heavy haze
            let t = (1.0 - s * (*atm / airlight)).max(t_min);

            // Recover: L_scene = (L_observed - airlight*(1-t)) / t
            // Simplified: L' = (L - airlight) / t + airlight
            let l_recovered = (*l_val - airlight) / t + airlight;
            *l_val = l_recovered.max(0.0);
        }

        // Boost chroma: haze desaturates, so reverse it proportionally
        // Use same per-pixel transmission for chroma recovery
        if planes.a.len() == pc && planes.b.len() == pc {
            for ((a_val, b_val), atm) in planes
                .a
                .iter_mut()
                .zip(planes.b.iter_mut())
                .zip(atmosphere.iter())
            {
                let t = (1.0 - s * (*atm / airlight)).max(t_min);
                // Chroma scales inversely with transmission (weaker boost than L)
                let chroma_boost = 1.0 + (1.0 / t - 1.0) * 0.5;
                *a_val *= chroma_boost;
                *b_val *= chroma_boost;
            }
        }

        ctx.return_f32(atmosphere);
    }
}

/// Estimate airlight from the top 0.1% of atmosphere map values.
fn estimate_airlight(atmosphere: &[f32]) -> f32 {
    // Find 99.9th percentile using partial sort approximation.
    // For efficiency, sample at most 10000 values.
    let step = (atmosphere.len() / 10000).max(1);
    let mut samples: alloc::vec::Vec<f32> = atmosphere.iter().step_by(step).copied().collect();
    samples.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    let idx = (samples.len() as f32 * 0.999) as usize;
    samples[idx.min(samples.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_identity() {
        let mut planes = OklabPlanes::new(32, 32);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = (i as f32 + 1.0) / (32.0 * 32.0 + 1.0);
        }
        for v in &mut planes.a {
            *v = 0.05;
        }
        let l_orig = planes.l.clone();
        let a_orig = planes.a.clone();
        Dehaze { strength: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, l_orig);
        assert_eq!(planes.a, a_orig);
    }

    #[test]
    fn positive_strength_increases_contrast() {
        let mut planes = OklabPlanes::new(64, 64);
        // Simulate a hazy image: L values clustered around 0.6-0.8
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = 0.6 + 0.2 * (i as f32 / (64.0 * 64.0));
        }
        for v in &mut planes.a {
            *v = 0.02;
        }
        let l_std_before = std_dev(&planes.l);
        Dehaze { strength: 0.8 }.apply(&mut planes, &mut FilterContext::new());
        let l_std_after = std_dev(&planes.l);
        assert!(
            l_std_after > l_std_before,
            "dehaze should increase L spread: {l_std_before} -> {l_std_after}"
        );
    }

    #[test]
    fn boosts_chroma() {
        let mut planes = OklabPlanes::new(32, 32);
        for v in &mut planes.l {
            *v = 0.7;
        }
        for v in &mut planes.a {
            *v = 0.03;
        }
        for v in &mut planes.b {
            *v = -0.02;
        }
        let a_orig = planes.a.clone();
        Dehaze { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        // Chroma should be boosted (absolute values increase)
        let a_sum_before: f32 = a_orig.iter().map(|v| v.abs()).sum();
        let a_sum_after: f32 = planes.a.iter().map(|v| v.abs()).sum();
        assert!(
            a_sum_after >= a_sum_before,
            "dehaze should boost chroma: {a_sum_before} -> {a_sum_after}"
        );
    }

    #[test]
    fn does_not_go_negative() {
        let mut planes = OklabPlanes::new(32, 32);
        for v in &mut planes.l {
            *v = 0.1; // dark pixel in hazy scene
        }
        Dehaze { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        for v in &planes.l {
            assert!(*v >= 0.0, "L should not go negative: {v}");
        }
    }

    fn std_dev(data: &[f32]) -> f32 {
        let mean = data.iter().sum::<f32>() / data.len() as f32;
        let variance =
            data.iter().map(|&v| (v - mean) * (v - mean)).sum::<f32>() / data.len() as f32;
        variance.sqrt()
    }
}
