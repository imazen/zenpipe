use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;

/// Parametric tone curve with 4 zone controls and 3 movable dividers.
///
/// Unlike the point-based `ToneCurve`, this offers zone-based control similar
/// to Lightroom's parametric tone curve panel:
///
/// - **Shadows**: darkest zone (0 to `split_shadows`)
/// - **Darks**: lower midtones (`split_shadows` to `split_midtones`)
/// - **Lights**: upper midtones (`split_midtones` to `split_highlights`)
/// - **Highlights**: brightest zone (`split_highlights` to 1.0)
///
/// Each zone slider pushes the curve up (positive) or down (negative) within
/// its region. The result is a smooth, monotonic LUT.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ParametricCurve {
    /// LUT with 256 entries mapping input L [0,1] to output L [0,1].
    lut: Vec<f32>,
}

impl Default for ParametricCurve {
    fn default() -> Self {
        let mut lut = vec![0.0f32; crate::LUT_SIZE];
        for (i, v) in lut.iter_mut().enumerate() {
            *v = i as f32 / crate::LUT_MAX as f32;
        }
        Self { lut }
    }
}

/// Compute smooth zone membership weight. Uses a raised-cosine window so
/// that each input L value is influenced primarily by one zone, with smooth
/// transitions at the boundaries.
#[inline]
fn zone_weight(l: f32, center: f32, half_width: f32) -> f32 {
    let dist = (l - center).abs();
    if dist >= half_width {
        return 0.0;
    }
    let t = dist / half_width;
    0.5 * (1.0 + (core::f32::consts::PI * t).cos())
}

impl ParametricCurve {
    /// Build a parametric curve from zone adjustments and split points.
    ///
    /// - `shadows`, `darks`, `lights`, `highlights`: adjustment amount, -1.0 to 1.0.
    /// - `split_shadows`: boundary between shadows and darks (default 0.25).
    /// - `split_midtones`: boundary between darks and lights (default 0.50).
    /// - `split_highlights`: boundary between lights and highlights (default 0.75).
    pub fn new(
        shadows: f32,
        darks: f32,
        lights: f32,
        highlights: f32,
        split_shadows: f32,
        split_midtones: f32,
        split_highlights: f32,
    ) -> Self {
        let ss = split_shadows.clamp(0.05, 0.45);
        let sm = split_midtones.clamp(ss + 0.05, 0.75);
        let sh = split_highlights.clamp(sm + 0.05, 0.95);

        // Zone centers and half-widths
        let zones: [(f32, f32, f32); 4] = [
            (ss * 0.5, ss * 0.75, shadows),                    // shadows
            ((ss + sm) * 0.5, (sm - ss) * 0.75, darks),        // darks
            ((sm + sh) * 0.5, (sh - sm) * 0.75, lights),       // lights
            ((sh + 1.0) * 0.5, (1.0 - sh) * 0.75, highlights), // highlights
        ];

        // Build the raw adjustment curve
        let mut lut = vec![0.0f32; crate::LUT_SIZE];
        for (i, entry) in lut.iter_mut().enumerate() {
            let l = i as f32 / crate::LUT_MAX as f32;

            // Accumulate weighted adjustments from all zones
            let mut adj = 0.0f32;
            for &(center, half_width, amount) in &zones {
                let w = zone_weight(l, center, half_width);
                adj += w * amount;
            }

            // Scale adjustment (max ~0.25 stops at amount=1.0)
            *entry = l + adj * 0.25;
        }

        // Enforce monotonicity: sweep forward, each entry >= previous
        for i in 1..256 {
            if lut[i] < lut[i - 1] {
                lut[i] = lut[i - 1];
            }
        }

        // Clamp to [0, 1]
        for v in &mut lut {
            *v = v.clamp(0.0, 1.0);
        }

        Self { lut }
    }

    fn is_identity(&self) -> bool {
        self.lut
            .iter()
            .enumerate()
            .all(|(i, &v)| (v - i as f32 / crate::LUT_MAX as f32).abs() < 1e-4)
    }
}

impl Filter for ParametricCurve {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.is_identity() {
            return;
        }

        for v in &mut planes.l {
            let x = (*v * crate::LUT_MAX as f32).clamp(0.0, crate::LUT_MAX as f32);
            let idx = x as usize;
            let frac = x - idx as f32;
            let lo = self.lut[idx.min(crate::LUT_MAX)];
            let hi = self.lut[(idx + 1).min(crate::LUT_MAX)];
            *v = lo + frac * (hi - lo);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_identity() {
        let curve = ParametricCurve::default();
        assert!(curve.is_identity());
    }

    #[test]
    fn zero_adjustments_is_identity() {
        let curve = ParametricCurve::new(0.0, 0.0, 0.0, 0.0, 0.25, 0.50, 0.75);
        assert!(curve.is_identity());
    }

    #[test]
    fn positive_shadows_lifts_darks() {
        let curve = ParametricCurve::new(1.0, 0.0, 0.0, 0.0, 0.25, 0.50, 0.75);
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.1;
        curve.apply(&mut planes, &mut FilterContext::new());
        assert!(
            planes.l[0] > 0.1,
            "positive shadows should lift: {}",
            planes.l[0]
        );
    }

    #[test]
    fn negative_highlights_dims_brights() {
        let curve = ParametricCurve::new(0.0, 0.0, 0.0, -1.0, 0.25, 0.50, 0.75);
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.9;
        curve.apply(&mut planes, &mut FilterContext::new());
        assert!(
            planes.l[0] < 0.9,
            "negative highlights should dim: {}",
            planes.l[0]
        );
    }

    #[test]
    fn curve_is_monotonic() {
        let curve = ParametricCurve::new(0.8, -0.3, 0.5, -0.7, 0.25, 0.50, 0.75);
        for i in 1..256 {
            assert!(
                curve.lut[i] >= curve.lut[i - 1] - 1e-6,
                "curve not monotonic at {}: {} < {}",
                i,
                curve.lut[i],
                curve.lut[i - 1]
            );
        }
    }

    #[test]
    fn output_in_range() {
        let curve = ParametricCurve::new(1.0, 1.0, 1.0, 1.0, 0.25, 0.50, 0.75);
        for &v in &curve.lut {
            assert!(v >= 0.0 && v <= 1.0, "out of range: {v}");
        }
    }

    #[test]
    fn custom_splits_work() {
        let curve = ParametricCurve::new(0.0, 1.0, 0.0, 0.0, 0.15, 0.35, 0.80);
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.25; // in the darks zone with custom splits
        curve.apply(&mut planes, &mut FilterContext::new());
        assert!(
            planes.l[0] > 0.25,
            "darks boost should lift in custom zone: {}",
            planes.l[0]
        );
    }

    #[test]
    fn does_not_modify_chroma() {
        let curve = ParametricCurve::new(0.5, 0.5, 0.5, 0.5, 0.25, 0.50, 0.75);
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.l {
            *v = 0.5;
        }
        for v in &mut planes.a {
            *v = 0.1;
        }
        let a_orig = planes.a.clone();
        curve.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a, a_orig);
    }
}
