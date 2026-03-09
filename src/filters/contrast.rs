use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// Contrast adjustment via S-curve on Oklab L channel.
///
/// Positive values increase contrast (darks darker, lights lighter).
/// Negative values reduce contrast. The pivot point is L=0.5.
pub struct Contrast {
    /// Contrast amount. 0.0 = no change, 1.0 = maximum increase, -1.0 = flatten.
    pub amount: f32,
}

impl Filter for Contrast {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.amount.abs() < 1e-6 {
            return;
        }
        // S-curve: L' = 0.5 + (L - 0.5) * (1 + amount)
        // Clamped to not go fully flat at -1.0
        let factor = (1.0 + self.amount).max(0.01);
        // L' = 0.5 + (L - 0.5) * factor = L * factor + 0.5 * (1 - factor)
        simd::scale_plane(&mut planes.l, factor);
        simd::offset_plane(&mut planes.l, 0.5 * (1.0 - factor));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_identity() {
        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 16.0;
        }
        let original = planes.l.clone();
        Contrast { amount: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn positive_increases_range() {
        let mut planes = OklabPlanes::new(2, 1);
        planes.l[0] = 0.3; // below midpoint
        planes.l[1] = 0.7; // above midpoint
        Contrast { amount: 0.5 }.apply(&mut planes, &mut FilterContext::new());
        // 0.3 should get darker, 0.7 should get brighter
        assert!(planes.l[0] < 0.3);
        assert!(planes.l[1] > 0.7);
    }

    #[test]
    fn midpoint_unchanged() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        Contrast { amount: 0.8 }.apply(&mut planes, &mut FilterContext::new());
        assert!((planes.l[0] - 0.5).abs() < 1e-6);
    }
}
