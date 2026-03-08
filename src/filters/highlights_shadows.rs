use crate::access::ChannelAccess;
use crate::filter::Filter;
use crate::planes::OklabPlanes;

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

    fn apply(&self, planes: &mut OklabPlanes) {
        if self.highlights.abs() < 1e-6 && self.shadows.abs() < 1e-6 {
            return;
        }
        for v in &mut planes.l {
            let l = *v;
            // Shadows: affect dark regions (L < 0.5) smoothly
            if self.shadows.abs() > 1e-6 {
                let shadow_mask = (1.0 - l * 2.0).max(0.0); // 1.0 at black, 0.0 at L=0.5+
                *v += shadow_mask * shadow_mask * self.shadows * 0.5;
            }
            // Highlights: affect bright regions (L > 0.5) smoothly
            if self.highlights.abs() > 1e-6 {
                let highlight_mask = ((l - 0.5) * 2.0).clamp(0.0, 1.0); // 0.0 at L=0.5, 1.0 at white
                *v -= highlight_mask * highlight_mask * self.highlights * 0.5;
            }
        }
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
        .apply(&mut planes);
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
        .apply(&mut planes);
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
        .apply(&mut planes);
        assert!(planes.l[0] < 0.9, "highlights should dim brights");
    }
}
