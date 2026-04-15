/// Middle grey in Oklab L (0.1842_f32.cbrt()), used as contrast pivot.
const CONTRAST_PIVOT: f32 = 0.5691;

/// Precomputed SIMD constants for the fused per-pixel adjustment.
///
/// Created from a [`FusedAdjust`](crate::filters::FusedAdjust) via
/// [`FusedAdjustParams::from_adjust`]. These are the derived values that
/// the SIMD kernels actually consume — not the user-facing parameters.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FusedAdjustParams {
    // L-plane adjustments
    pub bp: f32,
    pub inv_range: f32,
    pub wp_exp: f32,
    pub contrast_exp: f32,
    pub contrast_scale: f32,
    pub shadows: f32,
    pub highlights: f32,
    pub dehaze_contrast: f32,

    // AB-plane adjustments
    pub dehaze_chroma: f32,
    pub exposure_chroma: f32,
    pub temp_offset: f32,
    pub tint_offset: f32,
    pub sat: f32,
    pub vib_amount: f32,
    pub vib_protection: f32,
}

impl FusedAdjustParams {
    /// Compute SIMD-ready constants from a `FusedAdjust` filter.
    pub fn from_adjust(adj: &crate::filters::FusedAdjust) -> Self {
        let bp = adj.black_point;
        let range = (1.0 - bp).max(0.01);
        let inv_range = 1.0 / range;
        let wp_inv = 1.0 / adj.white_point.max(0.01);
        let exposure_factor = 2.0f32.powf(adj.exposure / 3.0);
        let wp_exp = wp_inv * exposure_factor;
        let contrast_exp = (1.0 + adj.contrast).max(0.01);
        let contrast_scale = CONTRAST_PIVOT.powf(-adj.contrast);
        let dehaze_contrast = 1.0 + adj.dehaze * 0.3;
        let dehaze_chroma = 1.0 + adj.dehaze * 0.2;
        let temp_offset = adj.temperature * 0.12;
        let tint_offset = adj.tint * 0.12;

        Self {
            bp,
            inv_range,
            wp_exp,
            contrast_exp,
            contrast_scale,
            shadows: adj.shadows,
            highlights: adj.highlights,
            dehaze_contrast,
            dehaze_chroma,
            exposure_chroma: exposure_factor,
            temp_offset,
            tint_offset,
            sat: adj.saturation,
            vib_amount: adj.vibrance,
            vib_protection: adj.vibrance_protection,
        }
    }
}
