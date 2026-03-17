use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// Per-pixel dehaze: a cheap point operation that approximates haze removal.
///
/// Unlike [`Dehaze`](super::Dehaze), which uses a spatial dark-channel prior
/// (neighborhood filter, large Gaussian blur), `SimpleDehaze` applies a
/// fixed three-step correction to every pixel independently:
///
/// 1. **Shadow lift:** `L += strength * 0.15 * (1 - L)^2`
/// 2. **S-curve contrast:** power curve centered at 0.5, exponent
///    `1 - (strength * 0.4) * 0.5`
/// 3. **Chroma boost:** `a *= 1 + strength * 0.3; b *= 1 + strength * 0.3`
///
/// Because there is no spatial analysis, the effect is uniform across the
/// image. This makes it suitable for strip-based streaming pipelines where
/// neighborhood context is unavailable or expensive.
///
/// Matches the algorithm used by zenimage's `DehazeOp`.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct SimpleDehaze {
    /// Dehaze strength. 0.0 = no change, 1.0 = full effect.
    /// Negative values are clamped to 0.
    pub strength: f32,
}

impl Filter for SimpleDehaze {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn is_neighborhood(&self) -> bool {
        false
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        let s = self.strength.max(0.0);
        if s < 1e-6 {
            return;
        }

        // --- L channel: shadow lift + S-curve contrast ---

        let contrast_amount = s * 0.4;
        // S-curve power: <1 increases contrast (steeper around midpoint)
        let power = 1.0 - contrast_amount * 0.5;

        for v in planes.l.iter_mut() {
            let mut l = *v;

            // 1. Shadow lift: brighten dark regions.
            //    Quadratic falloff: strongest at L=0, zero at L=1.
            let gap = (1.0 - l).max(0.0);
            l += s * 0.15 * gap * gap;

            // 2. S-curve contrast around midpoint 0.5.
            let l_clamped = l.clamp(0.0, 1.0);
            let centered = (l_clamped - 0.5) * 2.0; // [-1, 1]
            let curved = centered.signum() * centered.abs().powf(power);
            l = 0.5 + curved * 0.5;

            *v = l.max(0.0);
        }

        // --- Chroma boost ---

        let chroma_factor = 1.0 + s * 0.3;
        simd::scale_plane(&mut planes.a, chroma_factor);
        simd::scale_plane(&mut planes.b, chroma_factor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_identity() {
        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = (i as f32 + 1.0) / 17.0;
        }
        for v in &mut planes.a {
            *v = 0.05;
        }
        for v in &mut planes.b {
            *v = -0.03;
        }
        let l_orig = planes.l.clone();
        let a_orig = planes.a.clone();
        let b_orig = planes.b.clone();
        SimpleDehaze { strength: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, l_orig);
        assert_eq!(planes.a, a_orig);
        assert_eq!(planes.b, b_orig);
    }

    #[test]
    fn positive_increases_contrast() {
        let mut planes = OklabPlanes::new(2, 1);
        planes.l[0] = 0.2; // dark
        planes.l[1] = 0.8; // bright
        SimpleDehaze { strength: 0.8 }.apply(&mut planes, &mut FilterContext::new());
        // The S-curve should push the bright endpoint higher.
        assert!(
            planes.l[1] > 0.8,
            "bright pixel should get brighter: {}",
            planes.l[1]
        );
    }

    #[test]
    fn boosts_chroma() {
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.l {
            *v = 0.5;
        }
        for v in &mut planes.a {
            *v = 0.05;
        }
        for v in &mut planes.b {
            *v = -0.04;
        }
        let a_abs_before: f32 = planes.a.iter().map(|v| v.abs()).sum();
        let b_abs_before: f32 = planes.b.iter().map(|v| v.abs()).sum();
        SimpleDehaze { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        let a_abs_after: f32 = planes.a.iter().map(|v| v.abs()).sum();
        let b_abs_after: f32 = planes.b.iter().map(|v| v.abs()).sum();
        assert!(
            a_abs_after > a_abs_before,
            "a chroma should increase: {a_abs_before} -> {a_abs_after}"
        );
        assert!(
            b_abs_after > b_abs_before,
            "b chroma should increase: {b_abs_before} -> {b_abs_after}"
        );
    }

    #[test]
    fn l_stays_non_negative() {
        let mut planes = OklabPlanes::new(4, 4);
        // Test with values at extremes
        planes.l[0] = 0.0;
        planes.l[1] = 0.001;
        planes.l[2] = 1.0;
        planes.l[3] = 0.5;
        for v in planes.l[4..].iter_mut() {
            *v = 0.1;
        }
        SimpleDehaze { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        for (i, &v) in planes.l.iter().enumerate() {
            assert!(v >= 0.0, "L[{i}] should be non-negative: {v}");
        }
    }

    #[test]
    fn negative_strength_treated_as_zero() {
        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = (i as f32 + 1.0) / 17.0;
        }
        for v in &mut planes.a {
            *v = 0.05;
        }
        let l_orig = planes.l.clone();
        let a_orig = planes.a.clone();
        SimpleDehaze { strength: -0.5 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, l_orig, "negative strength should be identity");
        assert_eq!(planes.a, a_orig, "negative strength should not change chroma");
    }

    #[test]
    fn shadow_lift_brightens_darks() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.1; // dark pixel
        SimpleDehaze { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        // Shadow lift formula: 0.1 + 1.0 * 0.15 * (0.9)^2 = 0.1 + 0.1215 = 0.2215
        // Then S-curve further modifies it. Result should be > 0.1.
        assert!(
            planes.l[0] > 0.1,
            "dark pixel should be brightened: {}",
            planes.l[0]
        );
    }

    #[test]
    fn stronger_effect_changes_more() {
        let make = || {
            let mut planes = OklabPlanes::new(4, 4);
            for v in &mut planes.l {
                *v = 0.3;
            }
            for v in &mut planes.a {
                *v = 0.05;
            }
            planes
        };

        let mut weak = make();
        let mut strong = make();
        SimpleDehaze { strength: 0.2 }.apply(&mut weak, &mut FilterContext::new());
        SimpleDehaze { strength: 1.0 }.apply(&mut strong, &mut FilterContext::new());

        // Stronger dehaze should produce a larger L shift
        let weak_delta = (weak.l[0] - 0.3).abs();
        let strong_delta = (strong.l[0] - 0.3).abs();
        assert!(
            strong_delta > weak_delta,
            "stronger should change more: weak_delta={weak_delta}, strong_delta={strong_delta}"
        );

        // Stronger dehaze should boost chroma more
        assert!(
            strong.a[0].abs() > weak.a[0].abs(),
            "stronger should boost chroma more: weak_a={}, strong_a={}",
            weak.a[0],
            strong.a[0]
        );
    }
}
