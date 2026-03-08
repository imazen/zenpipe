use crate::access::ChannelAccess;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// Tint adjustment via Oklab a-channel shift.
///
/// Positive values shift toward magenta, negative toward green.
pub struct Tint {
    /// Tint shift. -1.0 (green) to +1.0 (magenta). 0.0 = no change.
    pub shift: f32,
}

impl Filter for Tint {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::CHROMA_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes) {
        if self.shift.abs() < 1e-6 {
            return;
        }
        let offset = self.shift * 0.08;
        simd::offset_plane(&mut planes.a, offset);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_identity() {
        let mut planes = OklabPlanes::new(2, 1);
        planes.a[0] = 0.05;
        let original = planes.a.clone();
        Tint { shift: 0.0 }.apply(&mut planes);
        assert_eq!(planes.a, original);
    }

    #[test]
    fn positive_shifts_magenta() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.a[0] = 0.0;
        Tint { shift: 1.0 }.apply(&mut planes);
        assert!(planes.a[0] > 0.0, "positive shift should increase a");
    }
}
