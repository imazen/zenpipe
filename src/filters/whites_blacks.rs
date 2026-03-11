use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;

/// Whites and Blacks adjustment — targeted luminance control for the extreme
/// ends of the histogram.
///
/// Unlike BlackPoint/WhitePoint (which remap the entire range), Whites/Blacks
/// apply a smooth, localized adjustment:
/// - **Whites** targets the bright end (L > ~0.6) — positive brightens, negative darkens.
/// - **Blacks** targets the dark end (L < ~0.4) — positive lifts, negative crushes.
///
/// This matches Lightroom's Whites/Blacks sliders, which complement the
/// Highlights/Shadows controls by targeting the more extreme tonal ranges.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct WhitesBlacks {
    /// Whites adjustment. Range: -1.0 to 1.0. Positive brightens bright areas.
    pub whites: f32,
    /// Blacks adjustment. Range: -1.0 to 1.0. Positive lifts dark areas.
    pub blacks: f32,
}

/// Attempt to match Lightroom behaviour: the effect is strongest at the extremes
/// and fades to zero at the transition point.
///
/// `smoothstep(edge0, edge1, x)` returns 0 when x <= edge0, 1 when x >= edge1,
/// and a smooth Hermite interpolation between them.
#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl WhitesBlacks {
    fn is_identity(&self) -> bool {
        self.whites.abs() < 1e-6 && self.blacks.abs() < 1e-6
    }
}

impl Filter for WhitesBlacks {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.is_identity() {
            return;
        }

        let whites = self.whites;
        let blacks = self.blacks;

        for v in &mut planes.l {
            let l = *v;

            // Whites: ramp from 0 effect at L=0.6 to full at L=1.0
            if whites.abs() > 1e-6 {
                let w = smoothstep(0.6, 1.0, l);
                // Scale by headroom (1-L) for positive, by L for negative, to avoid clipping
                let room = if whites > 0.0 { (1.0 - l).max(0.0) } else { l };
                *v += whites * w * room * 0.5;
            }

            // Blacks: ramp from 0 effect at L=0.4 to full at L=0.0
            let l = *v;
            if blacks.abs() > 1e-6 {
                let w = smoothstep(0.4, 0.0, l);
                // Scale by L for positive (lift), by (1-L) for negative (crush)
                let room = if blacks > 0.0 { l } else { (1.0 - l).max(0.0) };
                *v += blacks * w * room * 0.5;
            }

            *v = (*v).max(0.0);
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
        WhitesBlacks::default().apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn positive_whites_brightens_highlights() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.85;
        WhitesBlacks {
            whites: 1.0,
            blacks: 0.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert!(
            planes.l[0] > 0.85,
            "positive whites should brighten highlights: {}",
            planes.l[0]
        );
    }

    #[test]
    fn negative_whites_darkens_highlights() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.85;
        WhitesBlacks {
            whites: -1.0,
            blacks: 0.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert!(
            planes.l[0] < 0.85,
            "negative whites should darken highlights: {}",
            planes.l[0]
        );
    }

    #[test]
    fn positive_blacks_lifts_shadows() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.1;
        WhitesBlacks {
            whites: 0.0,
            blacks: 1.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert!(
            planes.l[0] > 0.1,
            "positive blacks should lift shadows: {}",
            planes.l[0]
        );
    }

    #[test]
    fn negative_blacks_crushes_shadows() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.1;
        WhitesBlacks {
            whites: 0.0,
            blacks: -1.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert!(
            planes.l[0] < 0.1,
            "negative blacks should crush shadows: {}",
            planes.l[0]
        );
    }

    #[test]
    fn whites_does_not_affect_shadows() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.2; // well below whites range
        let original = planes.l[0];
        WhitesBlacks {
            whites: 1.0,
            blacks: 0.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert!(
            (planes.l[0] - original).abs() < 1e-5,
            "whites should not affect shadows: {} vs {}",
            planes.l[0],
            original
        );
    }

    #[test]
    fn blacks_does_not_affect_highlights() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.8; // well above blacks range
        let original = planes.l[0];
        WhitesBlacks {
            whites: 0.0,
            blacks: 1.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert!(
            (planes.l[0] - original).abs() < 1e-5,
            "blacks should not affect highlights: {} vs {}",
            planes.l[0],
            original
        );
    }

    #[test]
    fn output_stays_nonnegative() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.05;
        WhitesBlacks {
            whites: -1.0,
            blacks: -1.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert!(planes.l[0] >= 0.0, "output should be non-negative");
    }

    #[test]
    fn does_not_modify_chroma() {
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.l {
            *v = 0.5;
        }
        for v in &mut planes.a {
            *v = 0.1;
        }
        let a_orig = planes.a.clone();
        let b_orig = planes.b.clone();
        WhitesBlacks {
            whites: 0.5,
            blacks: 0.5,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a, a_orig);
        assert_eq!(planes.b, b_orig);
    }
}
