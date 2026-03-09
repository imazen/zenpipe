use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// Sigmoid tone mapper: maps scene luminance through an S-curve for display.
///
/// Uses the generalized sigmoid `f(x) = x^c / (x^c + (1-x)^c)` which maps
/// [0,1] → [0,1] with f(0)=0, f(0.5)=0.5, f(1)=1. The `contrast` parameter
/// controls the steepness of the S-curve:
///
/// - `contrast = 1.0`: identity (no change)
/// - `contrast > 1.0`: S-curve, toe + shoulder compression (typical: 1.2-2.5)
/// - `contrast < 1.0`: inverse S-curve, reduces contrast
///
/// Optional `skew` shifts the midpoint using Schlick's bias function,
/// reallocating compression between shadows and highlights:
///
/// - `skew = 0.5`: symmetric (default)
/// - `skew < 0.5`: compress shadows more, expand highlights
/// - `skew > 0.5`: compress highlights more, expand shadows (brighten)
///
/// Inspired by darktable's sigmoid module. Applied to Oklab L channel only.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Sigmoid {
    /// S-curve steepness. 1.0 = identity.
    pub contrast: f32,
    /// Midpoint bias (0.0-1.0). 0.5 = symmetric.
    pub skew: f32,
}

impl Default for Sigmoid {
    fn default() -> Self {
        Self {
            contrast: 1.0,
            skew: 0.5,
        }
    }
}

impl Filter for Sigmoid {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if (self.contrast - 1.0).abs() < 1e-6 && (self.skew - 0.5).abs() < 1e-6 {
            return;
        }

        // Pre-compute Schlick bias parameter: bias_a = 1/skew - 2
        // bias(x) = x / (bias_a * (1 - x) + 1)
        // When skew=0.5: bias_a = 0, bias(x) = x (identity)
        let skew_clamped = self.skew.clamp(0.01, 0.99);
        let bias_a = 1.0 / skew_clamped - 2.0;

        simd::sigmoid_tone_map_plane(&mut planes.l, self.contrast, bias_a);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_at_contrast_1() {
        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = (i as f32 + 1.0) / 17.0;
        }
        let original = planes.l.clone();
        Sigmoid {
            contrast: 1.0,
            skew: 0.5,
        }
        .apply(&mut planes, &mut FilterContext::new());
        for (a, b) in planes.l.iter().zip(original.iter()) {
            assert!((a - b).abs() < 1e-5, "identity failed: {a} vs {b}");
        }
    }

    #[test]
    fn preserves_endpoints() {
        let mut planes = OklabPlanes::new(2, 1);
        planes.l[0] = 0.0;
        planes.l[1] = 1.0;
        Sigmoid {
            contrast: 2.0,
            skew: 0.5,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert!(planes.l[0].abs() < 1e-5, "black shifted: {}", planes.l[0]);
        assert!(
            (planes.l[1] - 1.0).abs() < 1e-5,
            "white shifted: {}",
            planes.l[1]
        );
    }

    #[test]
    fn preserves_midpoint_symmetric() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        Sigmoid {
            contrast: 2.0,
            skew: 0.5,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert!(
            (planes.l[0] - 0.5).abs() < 1e-4,
            "midpoint shifted: {}",
            planes.l[0]
        );
    }

    #[test]
    fn high_contrast_increases_range() {
        let mut planes = OklabPlanes::new(2, 1);
        planes.l[0] = 0.3; // below midpoint
        planes.l[1] = 0.7; // above midpoint
        let range_before = planes.l[1] - planes.l[0]; // 0.4
        Sigmoid {
            contrast: 2.0,
            skew: 0.5,
        }
        .apply(&mut planes, &mut FilterContext::new());
        let range_after = planes.l[1] - planes.l[0];
        assert!(
            range_after > range_before,
            "contrast should increase range: {range_after} vs {range_before}"
        );
        // Dark should get darker, bright should get brighter
        assert!(
            planes.l[0] < 0.3,
            "dark pixel should darken: {}",
            planes.l[0]
        );
        assert!(
            planes.l[1] > 0.7,
            "bright pixel should brighten: {}",
            planes.l[1]
        );
    }

    #[test]
    fn low_contrast_reduces_range() {
        let mut planes = OklabPlanes::new(2, 1);
        planes.l[0] = 0.2;
        planes.l[1] = 0.8;
        let range_before = planes.l[1] - planes.l[0];
        Sigmoid {
            contrast: 0.5,
            skew: 0.5,
        }
        .apply(&mut planes, &mut FilterContext::new());
        let range_after = planes.l[1] - planes.l[0];
        assert!(
            range_after < range_before,
            "low contrast should reduce range: {range_after} vs {range_before}"
        );
    }

    #[test]
    fn skew_shifts_midtones() {
        // skew > 0.5 should brighten midtones
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.4;
        Sigmoid {
            contrast: 1.5,
            skew: 0.7,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert!(
            planes.l[0] > 0.4,
            "high skew should brighten midtones: {}",
            planes.l[0]
        );

        // skew < 0.5 should darken midtones
        let mut planes2 = OklabPlanes::new(1, 1);
        planes2.l[0] = 0.6;
        Sigmoid {
            contrast: 1.5,
            skew: 0.3,
        }
        .apply(&mut planes2, &mut FilterContext::new());
        assert!(
            planes2.l[0] < 0.6,
            "low skew should darken midtones: {}",
            planes2.l[0]
        );
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
        Sigmoid {
            contrast: 2.0,
            skew: 0.5,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a, a_orig);
        assert_eq!(planes.b, b_orig);
    }

    #[test]
    fn monotonic() {
        // Sigmoid must be monotonically increasing for any contrast > 0
        let mut planes = OklabPlanes::new(100, 1);
        for i in 0..100 {
            planes.l[i] = (i as f32 + 0.5) / 100.0;
        }
        Sigmoid {
            contrast: 2.5,
            skew: 0.5,
        }
        .apply(&mut planes, &mut FilterContext::new());
        for i in 1..100 {
            assert!(
                planes.l[i] >= planes.l[i - 1],
                "not monotonic at {i}: {} < {}",
                planes.l[i],
                planes.l[i - 1]
            );
        }
    }

    #[test]
    fn clamps_out_of_range() {
        let mut planes = OklabPlanes::new(2, 1);
        planes.l[0] = -0.1;
        planes.l[1] = 1.2;
        Sigmoid {
            contrast: 2.0,
            skew: 0.5,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert!(planes.l[0] >= 0.0, "negative should clamp: {}", planes.l[0]);
        assert!(planes.l[1] <= 1.0, "over-1 should clamp: {}", planes.l[1]);
    }
}
