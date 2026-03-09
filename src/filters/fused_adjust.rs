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
        let exposure_factor = 2.0f32.powf(self.exposure);
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

    #[test]
    fn default_is_identity() {
        let adjust = FusedAdjust::new();
        assert!(adjust.is_identity());
    }

    #[test]
    fn identity_does_not_modify() {
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.l {
            *v = 0.5;
        }
        for v in &mut planes.a {
            *v = 0.1;
        }
        let l_orig = planes.l.clone();
        let a_orig = planes.a.clone();
        FusedAdjust::new().apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, l_orig);
        assert_eq!(planes.a, a_orig);
    }

    #[test]
    fn exposure_matches_standalone() {
        let mut planes_fused = OklabPlanes::new(4, 4);
        let mut planes_standalone = OklabPlanes::new(4, 4);
        for v in planes_fused
            .l
            .iter_mut()
            .chain(planes_standalone.l.iter_mut())
        {
            *v = 0.3;
        }

        let mut fused = FusedAdjust::new();
        fused.exposure = 1.0;
        fused.apply(&mut planes_fused, &mut FilterContext::new());

        crate::filters::Exposure { stops: 1.0 }
            .apply(&mut planes_standalone, &mut FilterContext::new());

        for i in 0..planes_fused.l.len() {
            assert!(
                (planes_fused.l[i] - planes_standalone.l[i]).abs() < 1e-5,
                "fused vs standalone mismatch at {i}"
            );
        }
    }
}
