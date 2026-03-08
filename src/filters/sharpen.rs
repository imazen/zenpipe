use crate::access::ChannelAccess;
use crate::blur::{GaussianKernel, gaussian_blur_plane};
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// L-channel sharpening via unsharp mask.
///
/// Like clarity but with a smaller sigma for fine detail enhancement.
/// Sharpening in Oklab L avoids the color fringing that RGB sharpening
/// produces at high-contrast edges.
pub struct Sharpen {
    /// Blur sigma. Small values (0.5-2.0) for fine sharpening.
    pub sigma: f32,
    /// Sharpening amount. Typical: 0.3-1.0.
    pub amount: f32,
}

impl Filter for Sharpen {
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
        let mut planes = OklabPlanes::new(16, 16);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 256.0;
        }
        let original = planes.l.clone();
        Sharpen {
            sigma: 1.0,
            amount: 0.0,
        }
        .apply(&mut planes);
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
        .apply(&mut planes);
        // Pixels near the edge should be pushed further apart
        let left = planes.l[planes.index(14, 16)];
        let right = planes.l[planes.index(17, 16)];
        assert!(
            right - left > 0.4,
            "edge should be sharpened: {left} vs {right}"
        );
    }
}
