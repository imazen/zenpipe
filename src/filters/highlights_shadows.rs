use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// Default shadow threshold: L values below this are in the "shadow zone".
const DEFAULT_SHADOW_THRESHOLD: f32 = 0.3;
/// Default highlight threshold: L values above this are in the "highlight zone".
const DEFAULT_HIGHLIGHT_THRESHOLD: f32 = 0.7;

/// Highlights and shadows recovery in Oklab L channel.
///
/// Positive `highlights` compresses bright areas (recovery).
/// Positive `shadows` lifts dark areas (fill light).
/// Both operate on L with smooth transitions to avoid artifacts.
///
/// The `shadow_threshold` and `highlight_threshold` fields control where
/// the transition regions begin. When both are at their defaults (0.3 and 0.7),
/// the fast SIMD path is used. Custom thresholds use a scalar fallback with
/// smooth quadratic mask transitions.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct HighlightsShadows {
    /// Highlights recovery. Positive = compress highlights, negative = boost.
    pub highlights: f32,
    /// Shadows recovery. Positive = lift shadows, negative = deepen.
    pub shadows: f32,
    /// L values below this are in the "shadow zone". Default: 0.3.
    pub shadow_threshold: f32,
    /// L values above this are in the "highlight zone". Default: 0.7.
    pub highlight_threshold: f32,
}

impl Default for HighlightsShadows {
    fn default() -> Self {
        Self {
            highlights: 0.0,
            shadows: 0.0,
            shadow_threshold: DEFAULT_SHADOW_THRESHOLD,
            highlight_threshold: DEFAULT_HIGHLIGHT_THRESHOLD,
        }
    }
}

/// Returns true when both thresholds are at their default values,
/// meaning the fast SIMD path can be used.
fn thresholds_are_default(shadow_threshold: f32, highlight_threshold: f32) -> bool {
    (shadow_threshold - DEFAULT_SHADOW_THRESHOLD).abs() < 1e-6
        && (highlight_threshold - DEFAULT_HIGHLIGHT_THRESHOLD).abs() < 1e-6
}

/// Quadratic shadow mask: 1.0 at L=0, 0.0 at L>=threshold.
#[inline]
fn shadow_mask(l: f32, threshold: f32) -> f32 {
    if l >= threshold {
        0.0
    } else {
        let t = 1.0 - l / threshold;
        t * t
    }
}

/// Quadratic highlight mask: 0.0 at L<=threshold, 1.0 at L=1.
#[inline]
fn highlight_mask(l: f32, threshold: f32) -> f32 {
    if l <= threshold {
        0.0
    } else {
        let t = (l - threshold) / (1.0 - threshold);
        t * t
    }
}

/// Scalar fallback for custom thresholds.
fn apply_custom_thresholds(
    plane: &mut [f32],
    shadows: f32,
    highlights: f32,
    shadow_threshold: f32,
    highlight_threshold: f32,
) {
    let shadow_target = 0.5;
    let highlight_target = 0.5;
    for v in plane.iter_mut() {
        let mut l = *v;
        let s_mask = shadow_mask(l, shadow_threshold);
        let h_mask = highlight_mask(l, highlight_threshold);
        l += shadows * s_mask * (shadow_target - l);
        l += highlights * h_mask * (highlight_target - l);
        *v = l;
    }
}

impl Filter for HighlightsShadows {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.highlights.abs() < 1e-6 && self.shadows.abs() < 1e-6 {
            return;
        }
        if thresholds_are_default(self.shadow_threshold, self.highlight_threshold) {
            // Fast SIMD path for default thresholds.
            simd::highlights_shadows(&mut planes.l, self.shadows, self.highlights);
        } else {
            // Scalar fallback with configurable quadratic masks.
            apply_custom_thresholds(
                &mut planes.l,
                self.shadows,
                self.highlights,
                self.shadow_threshold,
                self.highlight_threshold,
            );
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert!(planes.l[0] < 0.9, "highlights should dim brights");
    }

    #[test]
    fn default_thresholds() {
        let hs = HighlightsShadows::default();
        assert!((hs.shadow_threshold - 0.3).abs() < 1e-6);
        assert!((hs.highlight_threshold - 0.7).abs() < 1e-6);
    }

    #[test]
    fn custom_shadow_threshold_narrows_zone() {
        // With a low shadow threshold (0.15), only very dark pixels are affected.
        let mut planes_custom = OklabPlanes::new(1, 2);
        planes_custom.l[0] = 0.05; // very dark — inside both default and custom zones
        planes_custom.l[1] = 0.25; // moderately dark — inside default zone, outside custom zone

        let mut planes_default = OklabPlanes::new(1, 2);
        planes_default.l[0] = 0.05;
        planes_default.l[1] = 0.25;

        HighlightsShadows {
            shadows: 1.0,
            highlights: 0.0,
            shadow_threshold: 0.15,
            highlight_threshold: 0.7,
        }
        .apply(&mut planes_custom, &mut FilterContext::new());

        HighlightsShadows {
            shadows: 1.0,
            highlights: 0.0,
            ..Default::default()
        }
        .apply(&mut planes_default, &mut FilterContext::new());

        // Very dark pixel should still be lifted by both
        assert!(planes_custom.l[0] > 0.05, "custom: very dark pixel should be lifted");
        assert!(planes_default.l[0] > 0.05, "default: very dark pixel should be lifted");

        // Moderately dark pixel: custom threshold 0.15 means L=0.25 is OUTSIDE
        // the shadow zone, so it should be unaffected by custom thresholds.
        assert!(
            (planes_custom.l[1] - 0.25).abs() < 1e-6,
            "custom: L=0.25 should be outside shadow zone (threshold=0.15), got {}",
            planes_custom.l[1]
        );
        // Default threshold 0.3 means L=0.25 IS inside the shadow zone.
        assert!(
            planes_default.l[1] > 0.25,
            "default: L=0.25 should be inside shadow zone (threshold=0.3)"
        );
    }

    #[test]
    fn custom_highlight_threshold_widens_zone() {
        // With a lower highlight threshold (0.5), more pixels are in the highlight zone.
        let mut planes_custom = OklabPlanes::new(1, 1);
        planes_custom.l[0] = 0.6; // above custom threshold (0.5) but below default (0.7)

        let mut planes_default = OklabPlanes::new(1, 1);
        planes_default.l[0] = 0.6;

        HighlightsShadows {
            highlights: 1.0,
            shadows: 0.0,
            shadow_threshold: 0.3,
            highlight_threshold: 0.5,
        }
        .apply(&mut planes_custom, &mut FilterContext::new());

        HighlightsShadows {
            highlights: 1.0,
            shadows: 0.0,
            ..Default::default()
        }
        .apply(&mut planes_default, &mut FilterContext::new());

        // Custom: L=0.6 is above threshold=0.5, so it gets compressed
        assert!(
            planes_custom.l[0] < 0.6,
            "custom: L=0.6 above threshold=0.5 should be compressed, got {}",
            planes_custom.l[0]
        );
    }

    #[test]
    fn custom_thresholds_zero_amounts_identity() {
        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 16.0;
        }
        let original = planes.l.clone();
        HighlightsShadows {
            highlights: 0.0,
            shadows: 0.0,
            shadow_threshold: 0.2,
            highlight_threshold: 0.6,
        }
        .apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn shadow_mask_values() {
        // At L=0, mask should be 1.0
        assert!((shadow_mask(0.0, 0.3) - 1.0).abs() < 1e-6);
        // At L=threshold, mask should be 0.0
        assert!((shadow_mask(0.3, 0.3)).abs() < 1e-6);
        // Above threshold, mask should be 0.0
        assert!((shadow_mask(0.5, 0.3)).abs() < 1e-6);
        // At midpoint, mask should be 0.25 (quadratic: (1-0.5)^2)
        assert!((shadow_mask(0.15, 0.3) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn highlight_mask_values() {
        // At L=1, mask should be 1.0
        assert!((highlight_mask(1.0, 0.7) - 1.0).abs() < 1e-6);
        // At L=threshold, mask should be 0.0
        assert!((highlight_mask(0.7, 0.7)).abs() < 1e-6);
        // Below threshold, mask should be 0.0
        assert!((highlight_mask(0.5, 0.7)).abs() < 1e-6);
        // At midpoint (0.85 = 0.7 + 0.15), t = 0.15/0.3 = 0.5, mask = 0.25
        assert!((highlight_mask(0.85, 0.7) - 0.25).abs() < 1e-6);
    }
}
