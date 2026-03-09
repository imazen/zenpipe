use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// Dehaze: global contrast + saturation enhancement.
///
/// Increases both L-channel contrast and chroma to cut through haze.
/// Operates on L, a, and b planes.
pub struct Dehaze {
    /// Dehaze strength. 0.0 = no change, 1.0 = full effect.
    pub strength: f32,
}

impl Filter for Dehaze {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.strength.abs() < 1e-6 {
            return;
        }
        let s = self.strength;
        // Boost contrast on L
        let contrast_factor = 1.0 + s * 0.3;
        // Boost chroma
        let chroma_factor = 1.0 + s * 0.2;

        // L' = 0.5 + (L - 0.5) * cf = L * cf + 0.5 * (1 - cf)
        simd::scale_plane(&mut planes.l, contrast_factor);
        simd::offset_plane(&mut planes.l, 0.5 * (1.0 - contrast_factor));
        simd::scale_plane(&mut planes.a, chroma_factor);
        simd::scale_plane(&mut planes.b, chroma_factor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_identity() {
        let mut planes = OklabPlanes::new(2, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.1;
        planes.b[0] = -0.05;
        let l_orig = planes.l.clone();
        let a_orig = planes.a.clone();
        Dehaze { strength: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, l_orig);
        assert_eq!(planes.a, a_orig);
    }

    #[test]
    fn positive_strength_boosts() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.7; // above midpoint
        planes.a[0] = 0.1;
        Dehaze { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        assert!(planes.l[0] > 0.7, "L should increase above midpoint");
        assert!(planes.a[0] > 0.1, "chroma should increase");
    }
}
