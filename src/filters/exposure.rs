use crate::access::ChannelAccess;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// Exposure adjustment in Oklab L channel.
///
/// Scales lightness by `2^stops`. +1 stop doubles brightness, -1 halves it.
/// Only modifies the L plane — colors are preserved exactly.
pub struct Exposure {
    /// Exposure adjustment in stops. 0.0 = no change.
    pub stops: f32,
}

impl Filter for Exposure {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes) {
        let factor = 2.0f32.powf(self.stops);
        simd::scale_plane(&mut planes.l, factor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_stops_is_identity() {
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.l {
            *v = 0.5;
        }
        let original = planes.l.clone();
        Exposure { stops: 0.0 }.apply(&mut planes);
        assert_eq!(planes.l, original);
    }

    #[test]
    fn positive_stops_brighten() {
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.l {
            *v = 0.3;
        }
        Exposure { stops: 1.0 }.apply(&mut planes);
        for &v in &planes.l {
            assert!((v - 0.6).abs() < 1e-5, "expected ~0.6, got {v}");
        }
    }

    #[test]
    fn does_not_modify_chroma() {
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.a {
            *v = 0.1;
        }
        for v in &mut planes.b {
            *v = -0.05;
        }
        let a_orig = planes.a.clone();
        let b_orig = planes.b.clone();
        Exposure { stops: 2.0 }.apply(&mut planes);
        assert_eq!(planes.a, a_orig);
        assert_eq!(planes.b, b_orig);
    }
}
