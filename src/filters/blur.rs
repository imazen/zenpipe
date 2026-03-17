use crate::access::ChannelAccess;
use crate::blur::{GaussianKernel, gaussian_blur_plane};
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;

/// Full-image Gaussian blur across all Oklab channels.
///
/// Unlike the L-only blur used internally by clarity/sharpen, this blurs
/// the entire image (L, a, b, and alpha if present). This is the Oklab
/// equivalent of imageflow4's RGB Gaussian blur, but perceptually correct —
/// blurring in Oklab avoids the darkening artifacts that sRGB gamma-space
/// blurs produce at color boundaries.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Blur {
    /// Gaussian sigma in pixels. Larger = more blur.
    pub sigma: f32,
}

impl Filter for Blur {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::ALL
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        (self.sigma * 3.0).ceil() as u32
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.sigma < 0.01 {
            return;
        }
        let kernel = GaussianKernel::new(self.sigma);
        let n = planes.pixel_count();
        let w = planes.width;
        let h = planes.height;

        let mut tmp = ctx.take_f32(n);

        // Blur L
        gaussian_blur_plane(&planes.l, &mut tmp, w, h, &kernel, ctx);
        core::mem::swap(&mut planes.l, &mut tmp);

        // Blur a
        tmp.fill(0.0);
        gaussian_blur_plane(&planes.a, &mut tmp, w, h, &kernel, ctx);
        core::mem::swap(&mut planes.a, &mut tmp);

        // Blur b
        tmp.fill(0.0);
        gaussian_blur_plane(&planes.b, &mut tmp, w, h, &kernel, ctx);
        core::mem::swap(&mut planes.b, &mut tmp);

        // Blur alpha if present
        if let Some(alpha) = &mut planes.alpha {
            tmp.fill(0.0);
            gaussian_blur_plane(alpha, &mut tmp, w, h, &kernel, ctx);
            core::mem::swap(alpha, &mut tmp);
            ctx.return_f32(tmp);
        } else {
            ctx.return_f32(tmp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::FilterContext;

    #[test]
    fn zero_sigma_is_identity() {
        let mut planes = OklabPlanes::new(16, 16);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 256.0;
        }
        let orig = planes.l.clone();
        Blur { sigma: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, orig);
    }

    #[test]
    fn constant_plane_stays_constant() {
        let mut planes = OklabPlanes::new(32, 32);
        planes.l.fill(0.5);
        planes.a.fill(0.02);
        planes.b.fill(-0.03);
        Blur { sigma: 3.0 }.apply(&mut planes, &mut FilterContext::new());
        for &v in &planes.l {
            assert!((v - 0.5).abs() < 0.01, "L should stay ~0.5, got {v}");
        }
        for &v in &planes.a {
            assert!((v - 0.02).abs() < 0.01, "a should stay ~0.02, got {v}");
        }
        for &v in &planes.b {
            assert!((v + 0.03).abs() < 0.01, "b should stay ~-0.03, got {v}");
        }
    }

    #[test]
    fn blur_reduces_contrast() {
        let mut planes = OklabPlanes::new(32, 32);
        // Step edge
        for y in 0..32u32 {
            for x in 0..32u32 {
                let i = planes.index(x, y);
                planes.l[i] = if x < 16 { 0.2 } else { 0.8 };
            }
        }
        Blur { sigma: 2.0 }.apply(&mut planes, &mut FilterContext::new());
        // After blur, the step edge should be softened
        let left = planes.l[planes.index(8, 16)];
        let right = planes.l[planes.index(24, 16)];
        let edge_l = planes.l[planes.index(15, 16)];
        let edge_r = planes.l[planes.index(16, 16)];
        assert!(left < edge_l, "interior left should be darker");
        assert!(right > edge_r, "interior right should be brighter");
        assert!((edge_r - edge_l).abs() < 0.6, "edge should be softened");
    }

    #[test]
    fn blurs_alpha_when_present() {
        let mut planes = OklabPlanes::with_alpha(32, 32);
        // Step edge in alpha
        for y in 0..32u32 {
            for x in 0..32u32 {
                let i = planes.index(x, y);
                planes.alpha.as_mut().unwrap()[i] = if x < 16 { 0.0 } else { 1.0 };
            }
        }
        Blur { sigma: 2.0 }.apply(&mut planes, &mut FilterContext::new());
        // Edge pixels should be intermediate
        let edge = planes.alpha.as_ref().unwrap()[planes.index(16, 16)];
        assert!(
            edge > 0.1 && edge < 0.9,
            "alpha edge should be blurred, got {edge}"
        );
    }
}
