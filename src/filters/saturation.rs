use crate::access::ChannelAccess;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// Uniform saturation adjustment on Oklab a/b channels.
///
/// Scales chroma (a, b) by a constant factor. 1.0 = no change,
/// 0.0 = grayscale, 2.0 = double saturation.
pub struct Saturation {
    /// Saturation factor. 1.0 = no change.
    pub factor: f32,
}

impl Filter for Saturation {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::CHROMA_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes) {
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
        Saturation { factor: 1.0 }.apply(&mut planes);
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
        Saturation { factor: 0.0 }.apply(&mut planes);
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
        Saturation { factor: 2.0 }.apply(&mut planes);
        assert_eq!(planes.l, original);
    }
}
