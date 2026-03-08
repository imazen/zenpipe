use crate::access::ChannelAccess;
use crate::blur::{GaussianKernel, gaussian_blur_plane};
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// Clarity: local contrast enhancement via unsharp mask on L channel.
///
/// Computes a Gaussian-blurred version of L, then adds weighted high-pass
/// signal back. This enhances mid-frequency texture without affecting
/// global tone or color.
///
/// At 1080p with sigma=10, this runs at ~25ms (188× faster than naive
/// interleaved approach) thanks to separable SIMD blur on planar data.
pub struct Clarity {
    /// Blur sigma for low-frequency extraction. Larger = coarser features.
    /// Typical: 5.0-15.0.
    pub sigma: f32,
    /// Enhancement amount. Positive = sharper detail, negative = soften.
    /// Typical: 0.1-0.5.
    pub amount: f32,
}

impl Filter for Clarity {
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
        let mut blurred = vec![0.0f32; planes.pixel_count()];
        gaussian_blur_plane(
            &planes.l,
            &mut blurred,
            planes.width,
            planes.height,
            &kernel,
        );

        let mut dst = vec![0.0f32; planes.pixel_count()];
        simd::unsharp_fuse(&planes.l, &blurred, &mut dst, self.amount);
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
            *v = (i as f32 / 1024.0).sin().abs();
        }
        let original = planes.l.clone();
        Clarity {
            sigma: 5.0,
            amount: 0.0,
        }
        .apply(&mut planes);
        assert_eq!(planes.l, original);
    }

    #[test]
    fn positive_amount_enhances_contrast() {
        let mut planes = OklabPlanes::new(64, 64);
        // Create a pattern with local variation
        for y in 0..64 {
            for x in 0..64 {
                let i = y * 64 + x;
                planes.l[i] = if (x / 8 + y / 8) % 2 == 0 { 0.7 } else { 0.3 };
            }
        }
        let before_std = std_dev(&planes.l);
        Clarity {
            sigma: 5.0,
            amount: 0.5,
        }
        .apply(&mut planes);
        let after_std = std_dev(&planes.l);
        assert!(
            after_std > before_std,
            "clarity should increase local contrast: {before_std} -> {after_std}"
        );
    }

    #[test]
    fn does_not_modify_chroma() {
        let mut planes = OklabPlanes::new(32, 32);
        for v in &mut planes.l {
            *v = 0.5;
        }
        for v in &mut planes.a {
            *v = 0.1;
        }
        let a_orig = planes.a.clone();
        Clarity {
            sigma: 3.0,
            amount: 0.5,
        }
        .apply(&mut planes);
        assert_eq!(planes.a, a_orig);
    }

    fn std_dev(data: &[f32]) -> f32 {
        let mean = data.iter().sum::<f32>() / data.len() as f32;
        let variance =
            data.iter().map(|&v| (v - mean) * (v - mean)).sum::<f32>() / data.len() as f32;
        variance.sqrt()
    }
}
