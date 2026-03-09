use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::simd;

/// Fused per-pixel adjustment: applies all per-pixel operations in a single
/// pass over the data, avoiding repeated traversal.
///
/// This is equivalent to chaining Exposure + Contrast + BlackPoint +
/// WhitePoint + Saturation + Temperature + Tint + HighlightsShadows +
/// Dehaze + Vibrance, but runs ~3× faster because it only scans the
/// planes once.
///
/// Processing order matches zenimage's AdjustOp:
/// 1. Black/white point
/// 2. Exposure
/// 3. Contrast
/// 4. Highlights/shadows
/// 5. Dehaze
/// 6. Temperature/tint
/// 7. Saturation
/// 8. Vibrance
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct FusedAdjust {
    /// Exposure in stops.
    pub exposure: f32,
    /// Contrast (-1.0 to 1.0).
    pub contrast: f32,
    /// Highlights recovery.
    pub highlights: f32,
    /// Shadows recovery.
    pub shadows: f32,
    /// Vibrance (smart saturation).
    pub vibrance: f32,
    /// Vibrance protection exponent.
    pub vibrance_protection: f32,
    /// Linear saturation factor.
    pub saturation: f32,
    /// Temperature shift (-1.0 to 1.0).
    pub temperature: f32,
    /// Tint shift (-1.0 to 1.0).
    pub tint: f32,
    /// Dehaze strength.
    pub dehaze: f32,
    /// Black point level.
    pub black_point: f32,
    /// White point level.
    pub white_point: f32,
}

impl FusedAdjust {
    pub fn new() -> Self {
        Self {
            white_point: 1.0,
            vibrance_protection: 2.0,
            saturation: 1.0,
            ..Default::default()
        }
    }

    /// Returns true if all parameters are at identity.
    pub fn is_identity(&self) -> bool {
        self.exposure.abs() < 1e-6
            && self.contrast.abs() < 1e-6
            && self.highlights.abs() < 1e-6
            && self.shadows.abs() < 1e-6
            && self.vibrance.abs() < 1e-6
            && (self.saturation - 1.0).abs() < 1e-6
            && self.temperature.abs() < 1e-6
            && self.tint.abs() < 1e-6
            && self.dehaze.abs() < 1e-6
            && self.black_point.abs() < 1e-6
            && (self.white_point - 1.0).abs() < 1e-6
    }
}

impl Filter for FusedAdjust {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.is_identity() {
            return;
        }

        // Pre-compute constants for SIMD dispatch
        let bp = self.black_point;
        let range = (1.0 - bp).max(0.01);
        let inv_range = 1.0 / range;
        let wp_inv = 1.0 / self.white_point.max(0.01);
        // Exposure in Oklab: linear light 2^stops maps to 2^(stops/3) in
        // cube-root domain. Applied to L (with white point) and a,b separately.
        let exposure_factor = 2.0f32.powf(self.exposure / 3.0);
        let wp_exp = wp_inv * exposure_factor;
        let contrast_factor = (1.0 + self.contrast).max(0.01);
        let dehaze_contrast = 1.0 + self.dehaze * 0.3;
        let dehaze_chroma = 1.0 + self.dehaze * 0.2;
        let temp_offset = self.temperature * 0.08;
        let tint_offset = self.tint * 0.08;

        simd::fused_adjust(
            &mut planes.l,
            &mut planes.a,
            &mut planes.b,
            bp,
            inv_range,
            wp_exp,
            contrast_factor,
            self.shadows,
            self.highlights,
            dehaze_contrast,
            dehaze_chroma,
            exposure_factor,
            temp_offset,
            tint_offset,
            self.saturation,
            self.vibrance,
            self.vibrance_protection,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::*;

    /// Create test planes with diverse, non-trivial values across the range.
    fn make_test_planes() -> OklabPlanes {
        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = (i as f32 + 1.0) / 17.0; // 0.059..0.941
        }
        for (i, v) in planes.a.iter_mut().enumerate() {
            *v = (i as f32 - 8.0) / 80.0; // -0.1..0.0875
        }
        for (i, v) in planes.b.iter_mut().enumerate() {
            *v = (i as f32 - 5.0) / 100.0; // -0.05..0.10
        }
        planes
    }

    /// Apply standalone filters in the exact same order as FusedAdjust.
    fn apply_standalone_chain(planes: &mut OklabPlanes, adj: &FusedAdjust) {
        let mut ctx = FilterContext::new();
        BlackPoint {
            level: adj.black_point,
        }
        .apply(planes, &mut ctx);
        WhitePoint {
            level: adj.white_point,
        }
        .apply(planes, &mut ctx);
        Exposure {
            stops: adj.exposure,
        }
        .apply(planes, &mut ctx);
        Contrast {
            amount: adj.contrast,
        }
        .apply(planes, &mut ctx);
        HighlightsShadows {
            highlights: adj.highlights,
            shadows: adj.shadows,
        }
        .apply(planes, &mut ctx);
        Dehaze {
            strength: adj.dehaze,
        }
        .apply(planes, &mut ctx);
        Temperature {
            shift: adj.temperature,
        }
        .apply(planes, &mut ctx);
        Tint { shift: adj.tint }.apply(planes, &mut ctx);
        Saturation {
            factor: adj.saturation,
        }
        .apply(planes, &mut ctx);
        Vibrance {
            amount: adj.vibrance,
            protection: adj.vibrance_protection,
        }
        .apply(planes, &mut ctx);
    }

    fn assert_planes_match(
        fused: &OklabPlanes,
        standalone: &OklabPlanes,
        tolerance: f32,
        label: &str,
    ) {
        for i in 0..fused.l.len() {
            assert!(
                (fused.l[i] - standalone.l[i]).abs() < tolerance,
                "{label}: L mismatch at {i}: fused={} standalone={}",
                fused.l[i],
                standalone.l[i]
            );
            assert!(
                (fused.a[i] - standalone.a[i]).abs() < tolerance,
                "{label}: a mismatch at {i}: fused={} standalone={}",
                fused.a[i],
                standalone.a[i]
            );
            assert!(
                (fused.b[i] - standalone.b[i]).abs() < tolerance,
                "{label}: b mismatch at {i}: fused={} standalone={}",
                fused.b[i],
                standalone.b[i]
            );
        }
    }

    #[test]
    fn default_is_identity() {
        let adjust = FusedAdjust::new();
        assert!(adjust.is_identity());
    }

    #[test]
    fn identity_does_not_modify() {
        let mut planes = make_test_planes();
        let l_orig = planes.l.clone();
        let a_orig = planes.a.clone();
        let b_orig = planes.b.clone();
        FusedAdjust::new().apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, l_orig);
        assert_eq!(planes.a, a_orig);
        assert_eq!(planes.b, b_orig);
    }

    #[test]
    fn exposure_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.exposure = 1.0;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 1e-5, "exposure");
    }

    #[test]
    fn contrast_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.contrast = 0.5;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 1e-5, "contrast");
    }

    #[test]
    fn highlights_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.highlights = 0.8;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 1e-5, "highlights");
    }

    #[test]
    fn shadows_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.shadows = 0.8;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 1e-5, "shadows");
    }

    #[test]
    fn saturation_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.saturation = 1.5;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 1e-5, "saturation");
    }

    #[test]
    fn temperature_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.temperature = 0.5;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 1e-5, "temperature");
    }

    #[test]
    fn tint_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.tint = -0.5;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 1e-5, "tint");
    }

    #[test]
    fn dehaze_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.dehaze = 0.6;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 1e-5, "dehaze");
    }

    #[test]
    fn vibrance_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.vibrance = 0.7;
        adj.vibrance_protection = 2.0;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 1e-5, "vibrance");
    }

    #[test]
    fn black_point_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.black_point = 0.05;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 1e-5, "black_point");
    }

    #[test]
    fn white_point_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.white_point = 0.9;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 1e-5, "white_point");
    }

    #[test]
    fn full_chain_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.exposure = 0.5;
        adj.contrast = 0.3;
        adj.highlights = 0.4;
        adj.shadows = 0.3;
        adj.saturation = 1.2;
        adj.temperature = 0.2;
        adj.tint = -0.1;
        adj.dehaze = 0.3;
        adj.vibrance = 0.4;
        adj.vibrance_protection = 2.0;
        adj.black_point = 0.02;
        adj.white_point = 0.95;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 1e-4, "full_chain");
    }
}
