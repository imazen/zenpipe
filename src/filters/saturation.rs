use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// Uniform saturation adjustment on Oklab a/b channels.
///
/// Scales chroma (a, b) by a constant factor. 1.0 = no change,
/// 0.0 = grayscale, 2.0 = double saturation.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Saturation {
    /// Saturation factor. 1.0 = no change, 0.0 = grayscale, 2.0 = double.
    ///
    /// For slider integration, use [`Saturation::from_slider`] which maps
    /// a 0.0–1.0 range with 0.5 as the identity point (no change).
    pub factor: f32,
}

impl Saturation {
    /// Create from a 0.0–1.0 slider where 0.5 = identity (no change).
    ///
    /// - Slider 0.0 → factor 0.0 (grayscale)
    /// - Slider 0.5 → factor 1.0 (no change)
    /// - Slider 1.0 → factor 2.0 (double saturation)
    pub fn from_slider(slider: f32) -> Self {
        Self {
            factor: crate::slider::saturation_from_slider(slider.clamp(0.0, 1.0)),
        }
    }
}

impl Default for Saturation {
    fn default() -> Self {
        Self { factor: 1.0 }
    }
}

impl Filter for Saturation {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::CHROMA_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if (self.factor - 1.0).abs() < 1e-6 {
            return;
        }
        simd::scale_plane(&mut planes.a, self.factor);
        simd::scale_plane(&mut planes.b, self.factor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_is_identity() {
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.a {
            *v = 0.1;
        }
        let original = planes.a.clone();
        Saturation { factor: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a, original);
    }

    #[test]
    fn zero_is_grayscale() {
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.a {
            *v = 0.1;
        }
        for v in &mut planes.b {
            *v = -0.05;
        }
        Saturation { factor: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        for &v in &planes.a {
            assert!(v.abs() < 1e-6);
        }
        for &v in &planes.b {
            assert!(v.abs() < 1e-6);
        }
    }

    #[test]
    fn does_not_modify_l() {
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.l {
            *v = 0.5;
        }
        let original = planes.l.clone();
        Saturation { factor: 2.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }
}
