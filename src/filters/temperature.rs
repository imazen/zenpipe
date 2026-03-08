use crate::access::ChannelAccess;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// Color temperature adjustment via Oklab b-channel shift.
///
/// Positive values warm the image (shift toward yellow/orange).
/// Negative values cool it (shift toward blue).
pub struct Temperature {
    /// Temperature shift. -1.0 (cool) to +1.0 (warm). 0.0 = no change.
    pub shift: f32,
}

impl Filter for Temperature {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::CHROMA_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes) {
        if self.shift.abs() < 1e-6 {
            return;
        }
        // Scale: 0.08 per unit produces natural results
        let offset = self.shift * 0.08;
        simd::offset_plane(&mut planes.b, offset);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_identity() {
        let mut planes = OklabPlanes::new(2, 1);
        planes.b[0] = 0.05;
        let original = planes.b.clone();
        Temperature { shift: 0.0 }.apply(&mut planes);
        assert_eq!(planes.b, original);
    }

    #[test]
    fn positive_warms() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.b[0] = 0.0;
        Temperature { shift: 1.0 }.apply(&mut planes);
        assert!(planes.b[0] > 0.0, "positive shift should increase b");
    }
}
