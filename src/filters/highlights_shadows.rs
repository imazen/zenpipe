use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// Highlights and shadows recovery in Oklab L channel.
///
/// Positive `highlights` compresses bright areas (recovery).
/// Positive `shadows` lifts dark areas (fill light).
/// Both operate on L with smooth transitions to avoid artifacts.
pub struct HighlightsShadows {
    /// Highlights recovery. Positive = compress highlights, negative = boost.
    pub highlights: f32,
    /// Shadows recovery. Positive = lift shadows, negative = deepen.
    pub shadows: f32,
}

impl Filter for HighlightsShadows {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.highlights.abs() < 1e-6 && self.shadows.abs() < 1e-6 {
            return;
        }
        simd::highlights_shadows(&mut planes.l, self.shadows, self.highlights);
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
        HighlightsShadows {
            highlights: 0.0,
            shadows: 0.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn shadow_lift_brightens_darks() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.1; // dark pixel
        HighlightsShadows {
            highlights: 0.0,
            shadows: 1.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert!(planes.l[0] > 0.1, "shadows should brighten darks");
    }

    #[test]
    fn highlight_recovery_dims_brights() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.9; // bright pixel
        HighlightsShadows {
            highlights: 1.0,
            shadows: 0.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert!(planes.l[0] < 0.9, "highlights should dim brights");
    }
}
