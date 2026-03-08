use crate::access::ChannelAccess;
use crate::blur::GaussianKernel;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;

/// Edge-preserving noise reduction (bilateral filter) on all Oklab channels.
///
/// Uses both spatial distance and L-channel range distance for weighting.
/// Smooths noise while preserving edges. Operates on all three channels
/// (L, a, b) with L-distance as the range kernel.
///
/// Note: bilateral filter is non-separable and accesses all channels,
/// so planar layout provides no speedup over interleaved for this filter.
/// It's included for API completeness.
pub struct Bilateral {
    /// Spatial sigma for the Gaussian kernel. Typical: 1.0-3.0.
    pub spatial_sigma: f32,
    /// Range sigma for the L-distance weighting. Smaller = more edge preservation.
    /// Typical: 0.05-0.2.
    pub range_sigma: f32,
    /// Blend strength. 0.0 = no denoising, 1.0 = full effect.
    pub strength: f32,
}

impl Filter for Bilateral {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.strength.abs() < 1e-6 {
            return;
        }
        let w = planes.width as usize;
        let h = planes.height as usize;
        let kernel = GaussianKernel::new(self.spatial_sigma);
        let radius = kernel.radius;
        let range_sigma2 = 2.0 * self.range_sigma * self.range_sigma;
        let strength = self.strength;

        let mut dst_l = ctx.take_f32(w * h);
        let mut dst_a = ctx.take_f32(w * h);
        let mut dst_b = ctx.take_f32(w * h);

        for y in 0..h {
            for x in 0..w {
                let idx = y * w + x;
                let cl = planes.l[idx];
                let ca = planes.a[idx];
                let cb = planes.b[idx];

                let mut sl = 0.0f32;
                let mut sa = 0.0f32;
                let mut sb = 0.0f32;
                let mut wsum = 0.0f32;

                for (dy, &wy) in kernel.weights().iter().enumerate() {
                    let sy = (y as isize + dy as isize - radius as isize).clamp(0, h as isize - 1)
                        as usize;
                    for (kx, &wx) in kernel.weights().iter().enumerate() {
                        let sx = (x as isize + kx as isize - radius as isize)
                            .clamp(0, w as isize - 1) as usize;
                        let sidx = sy * w + sx;
                        let diff = cl - planes.l[sidx];
                        let rw = (-diff * diff / range_sigma2).exp();
                        let weight = wy * wx * rw;
                        sl += planes.l[sidx] * weight;
                        sa += planes.a[sidx] * weight;
                        sb += planes.b[sidx] * weight;
                        wsum += weight;
                    }
                }

                let inv_w = 1.0 / wsum;
                dst_l[idx] = cl * (1.0 - strength) + sl * inv_w * strength;
                dst_a[idx] = ca * (1.0 - strength) + sa * inv_w * strength;
                dst_b[idx] = cb * (1.0 - strength) + sb * inv_w * strength;
            }
        }

        let old_l = core::mem::replace(&mut planes.l, dst_l);
        let old_a = core::mem::replace(&mut planes.a, dst_a);
        let old_b = core::mem::replace(&mut planes.b, dst_b);
        ctx.return_f32(old_l);
        ctx.return_f32(old_a);
        ctx.return_f32(old_b);
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
        // Add noise to a uniform image
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
            "bilateral should reduce noise: {before_var} -> {after_var}"
        );
    }

    fn variance(data: &[f32]) -> f32 {
        let mean = data.iter().sum::<f32>() / data.len() as f32;
        data.iter().map(|&v| (v - mean) * (v - mean)).sum::<f32>() / data.len() as f32
    }
}
