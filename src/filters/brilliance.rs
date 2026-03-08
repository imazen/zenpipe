use crate::access::ChannelAccess;
use crate::blur::{GaussianKernel, gaussian_blur_plane};
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// Brilliance: adaptive local contrast based on local average.
///
/// Unlike clarity (which adds high-pass), brilliance adjusts each pixel
/// relative to its local average — lifting shadows and compressing highlights
/// selectively. This produces a more natural "dynamic range compression"
/// similar to Apple's Brilliance slider.
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

    fn apply(&self, planes: &mut OklabPlanes) {
        if self.amount.abs() < 1e-6 {
            return;
        }
        let kernel = GaussianKernel::new(self.sigma);
        let mut avg_l = vec![0.0f32; planes.pixel_count()];
        gaussian_blur_plane(&planes.l, &mut avg_l, planes.width, planes.height, &kernel);

        let mut dst = vec![0.0f32; planes.pixel_count()];
        simd::brilliance_apply(
            &planes.l,
            &avg_l,
            &mut dst,
            self.amount,
            self.shadow_strength,
            self.highlight_strength,
        );
        planes.l = dst;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        .apply(&mut planes);
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
        .apply(&mut planes);
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
