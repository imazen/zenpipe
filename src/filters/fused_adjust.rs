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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

    fn tag(&self) -> crate::filter_compat::FilterTag {
        crate::filter_compat::FilterTag::FusedAdjust
    }
    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.is_identity() {
            return;
        }

        let params = crate::FusedAdjustParams::from_adjust(self);
        simd::fused_adjust(&mut planes.l, &mut planes.a, &mut planes.b, &params);
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
            headroom: 0.0,
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
            ..Default::default()
        }
        .apply(planes, &mut ctx);
        // FusedAdjust's dehaze is per-pixel (contrast + chroma boost),
        // not the spatial Dehaze filter. Replicate the fused path math.
        if adj.dehaze.abs() > 1e-6 {
            let dc = 1.0 + adj.dehaze * 0.3;
            let dc_chroma = 1.0 + adj.dehaze * 0.2;
            crate::simd::scale_plane(&mut planes.l, dc);
            crate::simd::offset_plane(&mut planes.l, 0.5 * (1.0 - dc));
            crate::simd::scale_plane(&mut planes.a, dc_chroma);
            crate::simd::scale_plane(&mut planes.b, dc_chroma);
        }
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

        assert_planes_match(&fused_planes, &standalone_planes, 5e-3, "exposure");
    }

    #[test]
    fn contrast_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.contrast = 0.5;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 5e-3, "contrast");
    }

    #[test]
    fn highlights_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.highlights = 0.8;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 5e-3, "highlights");
    }

    #[test]
    fn shadows_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.shadows = 0.8;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 5e-3, "shadows");
    }

    #[test]
    fn saturation_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.saturation = 1.5;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 5e-3, "saturation");
    }

    #[test]
    fn temperature_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.temperature = 0.5;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 5e-3, "temperature");
    }

    #[test]
    fn tint_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.tint = -0.5;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 5e-3, "tint");
    }

    #[test]
    fn dehaze_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.dehaze = 0.6;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 5e-3, "dehaze");
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

        assert_planes_match(&fused_planes, &standalone_planes, 5e-3, "vibrance");
    }

    #[test]
    fn black_point_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.black_point = 0.05;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 5e-3, "black_point");
    }

    #[test]
    fn white_point_matches_standalone() {
        let mut fused_planes = make_test_planes();
        let mut standalone_planes = make_test_planes();

        let mut adj = FusedAdjust::new();
        adj.white_point = 0.9;

        adj.apply(&mut fused_planes, &mut FilterContext::new());
        apply_standalone_chain(&mut standalone_planes, &adj);

        assert_planes_match(&fused_planes, &standalone_planes, 5e-3, "white_point");
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

        assert_planes_match(&fused_planes, &standalone_planes, 5e-3, "full_chain");
    }
}
