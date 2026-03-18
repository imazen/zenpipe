//! Quality mapping: generic quality profiles to per-codec native parameters.
//!
//! Calibration tables are derived from imageflow's perceptual tuning.

/// Named quality presets with per-codec calibrated mappings.
///
/// Each profile maps to a generic quality value (0-100) that is then
/// interpolated through per-codec calibration tables to produce native
/// codec parameters.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum QualityProfile {
    /// Minimal quality, maximum compression. Generic ~15.
    Lowest,
    /// Low quality. Generic ~20.
    Low,
    /// Below-average quality. Generic ~34.
    MediumLow,
    /// Moderate quality. Generic ~55.
    Medium,
    /// Good quality — reasonable default. Generic ~73.
    #[default]
    Good,
    /// High quality, larger files. Generic ~91.
    High,
    /// Near-lossless quality. Generic ~96.
    Highest,
    /// Lossless where supported. Generic 100.
    Lossless,
}

impl QualityProfile {
    /// The generic quality value (0-100) for this profile.
    pub fn generic_quality(self) -> f32 {
        match self {
            Self::Lowest => 15.0,
            Self::Low => 20.0,
            Self::MediumLow => 34.0,
            Self::Medium => 55.0,
            Self::Good => 73.0,
            Self::High => 91.0,
            Self::Highest => 96.0,
            Self::Lossless => 100.0,
        }
    }

    /// Convert to a [`QualityIntent`] with no DPR adjustment.
    pub fn to_intent(self) -> QualityIntent {
        QualityIntent {
            quality: self.generic_quality(),
            effort: None,
            lossless: matches!(self, Self::Lossless),
        }
    }

    /// Convert to a [`QualityIntent`] with DPR (device pixel ratio) adjustment.
    ///
    /// At DPR 1.0, artifacts are magnified 3x by the browser → quality increases.
    /// At DPR 6.0, pixels are tiny on screen → quality decreases.
    /// Baseline DPR is 3.0 (no adjustment).
    pub fn to_intent_with_dpr(self, dpr: f32) -> QualityIntent {
        QualityIntent {
            quality: adjust_quality_for_dpr(self.generic_quality(), dpr),
            effort: None,
            lossless: matches!(self, Self::Lossless),
        }
    }
}

/// Generic encoding quality parameters, codec-agnostic.
///
/// Created from a [`QualityProfile`], a raw quality value, or constructed
/// directly. Per-codec native values are computed via calibration tables.
#[derive(Clone, Debug)]
pub struct QualityIntent {
    /// Generic quality 0.0-100.0.
    pub quality: f32,
    /// Speed/quality tradeoff. Higher = slower + better. Codec-mapped.
    pub effort: Option<u32>,
    /// Force lossless encoding.
    pub lossless: bool,
}

impl QualityIntent {
    /// Create from a raw quality value (0-100).
    pub fn from_quality(quality: f32) -> Self {
        Self {
            quality: quality.clamp(0.0, 100.0),
            effort: None,
            lossless: false,
        }
    }

    /// Create from a quality profile.
    pub fn from_profile(profile: QualityProfile) -> Self {
        profile.to_intent()
    }

    /// Create from a quality profile with DPR adjustment.
    pub fn from_profile_dpr(profile: QualityProfile, dpr: f32) -> Self {
        profile.to_intent_with_dpr(dpr)
    }

    /// Set effort level.
    pub fn with_effort(mut self, effort: u32) -> Self {
        self.effort = Some(effort);
        self
    }

    /// Set lossless flag.
    pub fn with_lossless(mut self, lossless: bool) -> Self {
        self.lossless = lossless;
        self
    }

    // ═══════════════════════════════════════════════════════════════════
    // Per-codec quality lookups (interpolated from calibration tables)
    // ═══════════════════════════════════════════════════════════════════

    /// JPEG quality (0-100, native scale).
    pub fn jpeg_quality(&self) -> u8 {
        interpolate(&JPEG_TABLE, self.quality)
            .round()
            .clamp(0.0, 100.0) as u8
    }

    /// WebP lossy quality (0-100, native scale).
    pub fn webp_quality(&self) -> f32 {
        interpolate(&WEBP_TABLE, self.quality)
    }

    /// WebP method/effort (0-6).
    pub fn webp_method(&self) -> u8 {
        interpolate(&WEBP_METHOD_TABLE, self.quality)
            .round()
            .clamp(0.0, 6.0) as u8
    }

    /// AVIF quality (0-100, native scale).
    pub fn avif_quality(&self) -> f32 {
        interpolate(&AVIF_TABLE, self.quality)
    }

    /// AVIF speed (0-10, lower = slower + better).
    pub fn avif_speed(&self) -> u8 {
        interpolate(&AVIF_SPEED_TABLE, self.quality)
            .round()
            .clamp(0.0, 10.0) as u8
    }

    /// JXL butteraugli distance (0-25, lower = better).
    pub fn jxl_distance(&self) -> f32 {
        interpolate(&JXL_DISTANCE_TABLE, self.quality)
    }

    /// JXL effort (1-9).
    pub fn jxl_effort(&self) -> u8 {
        interpolate(&JXL_EFFORT_TABLE, self.quality)
            .round()
            .clamp(1.0, 9.0) as u8
    }

    /// PNG quantization quality range (min, max) for lossy PNG.
    pub fn png_quality_range(&self) -> (u8, u8) {
        let min = interpolate(&PNG_MIN_TABLE, self.quality)
            .round()
            .clamp(0.0, 100.0) as u8;
        let max = interpolate(&PNG_MAX_TABLE, self.quality)
            .round()
            .clamp(0.0, 100.0) as u8;
        (min, max.max(min))
    }
}

impl Default for QualityIntent {
    fn default() -> Self {
        QualityProfile::default().to_intent()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// DPR adjustment
// ═══════════════════════════════════════════════════════════════════════════

/// Adjust quality for device pixel ratio.
///
/// At DPR 1.0, the browser upscales each source pixel to 3x3 screen pixels
/// (relative to baseline 3.0), magnifying compression artifacts. Quality
/// increases to compensate. At DPR 6.0, source pixels are tiny — artifacts
/// are invisible, so quality can decrease.
///
/// The adjustment is perceptual (applied to the distance from 100), not linear:
/// ```text
/// factor = 3.0 / dpr.clamp(0.1, 12.0)
/// adjusted = 100.0 - (100.0 - base) / factor
/// ```
///
/// Examples:
/// - DPR 1.0, base 73.0 → factor 3.0 → adjusted 91.0
/// - DPR 3.0, base 73.0 → factor 1.0 → adjusted 73.0 (no change)
/// - DPR 6.0, base 73.0 → factor 0.5 → adjusted 46.0
pub fn adjust_quality_for_dpr(base_quality: f32, dpr: f32) -> f32 {
    let factor = 3.0 / dpr.clamp(0.1, 12.0);
    let perceptual_distance = 100.0 - base_quality;
    let adjusted_distance = perceptual_distance / factor;
    (100.0 - adjusted_distance).clamp(5.0, 99.0)
}

// ═══════════════════════════════════════════════════════════════════════════
// Calibration tables
// ═══════════════════════════════════════════════════════════════════════════

/// Anchor point: (generic_quality, codec_native_value).
type AnchorPoint = (f32, f32);

/// Linearly interpolate a value from a table of anchor points.
///
/// The table must be sorted by the first element (generic quality).
/// Values outside the table range are clamped to the nearest endpoint.
fn interpolate(table: &[AnchorPoint], quality: f32) -> f32 {
    if table.is_empty() {
        return quality;
    }
    if quality <= table[0].0 {
        return table[0].1;
    }
    if quality >= table[table.len() - 1].0 {
        return table[table.len() - 1].1;
    }
    // Find the segment containing quality
    for window in table.windows(2) {
        let (q0, v0) = window[0];
        let (q1, v1) = window[1];
        if quality >= q0 && quality <= q1 {
            let t = (quality - q0) / (q1 - q0);
            return v0 + t * (v1 - v0);
        }
    }
    // Shouldn't reach here, but return the last value
    table[table.len() - 1].1
}

// Anchor points: (generic_quality, codec_native_value)
// Derived from imageflow's QualityProfileHints calibration.

/// JPEG quality (mozjpeg/zenjpeg scale, 0-100).
const JPEG_TABLE: [AnchorPoint; 8] = [
    (15.0, 15.0),
    (20.0, 20.0),
    (34.0, 34.0),
    (55.0, 57.0),
    (73.0, 73.0),
    (91.0, 91.0),
    (96.0, 96.0),
    (100.0, 100.0),
];

/// WebP lossy quality (libwebp scale, 0-100).
const WEBP_TABLE: [AnchorPoint; 8] = [
    (15.0, 15.0),
    (20.0, 20.0),
    (34.0, 34.0),
    (55.0, 53.0),
    (73.0, 76.0),
    (91.0, 93.0),
    (96.0, 96.0),
    (100.0, 100.0),
];

/// WebP method/effort (0-6, higher = slower + better).
const WEBP_METHOD_TABLE: [AnchorPoint; 8] = [
    (15.0, 0.0),
    (20.0, 1.0),
    (34.0, 3.0),
    (55.0, 5.0),
    (73.0, 6.0),
    (91.0, 6.0),
    (96.0, 6.0),
    (100.0, 6.0),
];

/// AVIF quality (ravif scale, 0-100).
const AVIF_TABLE: [AnchorPoint; 8] = [
    (15.0, 10.0),
    (20.0, 12.0),
    (34.0, 22.0),
    (55.0, 45.0),
    (73.0, 55.0),
    (91.0, 78.0),
    (96.0, 90.0),
    (100.0, 100.0),
];

/// AVIF speed (0-10, lower = slower + better quality).
const AVIF_SPEED_TABLE: [AnchorPoint; 8] = [
    (15.0, 10.0),
    (20.0, 9.0),
    (34.0, 8.0),
    (55.0, 6.0),
    (73.0, 6.0),
    (91.0, 4.0),
    (96.0, 3.0),
    (100.0, 3.0),
];

/// JXL butteraugli distance (INVERTED: lower = better).
const JXL_DISTANCE_TABLE: [AnchorPoint; 8] = [
    (15.0, 15.0),
    (20.0, 12.0),
    (34.0, 7.0),
    (55.0, 4.0),
    (73.0, 2.58),
    (91.0, 1.0),
    (96.0, 0.3),
    (100.0, 0.0),
];

/// JXL effort (1-9, higher = slower + better).
const JXL_EFFORT_TABLE: [AnchorPoint; 8] = [
    (15.0, 1.0),
    (20.0, 2.0),
    (34.0, 3.0),
    (55.0, 4.0),
    (73.0, 5.0),
    (91.0, 7.0),
    (96.0, 8.0),
    (100.0, 9.0),
];

/// PNG quantization minimum quality.
const PNG_MIN_TABLE: [AnchorPoint; 8] = [
    (15.0, 0.0),
    (20.0, 5.0),
    (34.0, 15.0),
    (55.0, 30.0),
    (73.0, 50.0),
    (91.0, 80.0),
    (96.0, 90.0),
    (100.0, 100.0),
];

/// PNG quantization maximum quality.
const PNG_MAX_TABLE: [AnchorPoint; 8] = [
    (15.0, 30.0),
    (20.0, 40.0),
    (34.0, 60.0),
    (55.0, 80.0),
    (73.0, 100.0),
    (91.0, 100.0),
    (96.0, 100.0),
    (100.0, 100.0),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_generic_quality() {
        assert_eq!(QualityProfile::Lowest.generic_quality(), 15.0);
        assert_eq!(QualityProfile::Good.generic_quality(), 73.0);
        assert_eq!(QualityProfile::Lossless.generic_quality(), 100.0);
    }

    #[test]
    fn default_profile_is_good() {
        assert_eq!(QualityProfile::default(), QualityProfile::Good);
    }

    #[test]
    fn jpeg_quality_at_anchors() {
        let intent = QualityIntent::from_quality(73.0);
        assert_eq!(intent.jpeg_quality(), 73);

        let intent = QualityIntent::from_quality(55.0);
        assert_eq!(intent.jpeg_quality(), 57); // Calibrated differently from generic

        let intent = QualityIntent::from_quality(15.0);
        assert_eq!(intent.jpeg_quality(), 15);
    }

    #[test]
    fn webp_quality_at_anchors() {
        let intent = QualityIntent::from_quality(73.0);
        assert!((intent.webp_quality() - 76.0).abs() < 0.01);

        let intent = QualityIntent::from_quality(55.0);
        assert!((intent.webp_quality() - 53.0).abs() < 0.01);
    }

    #[test]
    fn jxl_distance_at_anchors() {
        let intent = QualityIntent::from_quality(73.0);
        assert!((intent.jxl_distance() - 2.58).abs() < 0.01);

        let intent = QualityIntent::from_quality(100.0);
        assert!((intent.jxl_distance()).abs() < 0.01);
    }

    #[test]
    fn interpolation_between_anchors() {
        // Midpoint between Good (73, JPEG 73) and High (91, JPEG 91)
        let intent = QualityIntent::from_quality(82.0);
        let jpeg_q = intent.jpeg_quality();
        assert_eq!(jpeg_q, 82); // Linear between 73 and 91
    }

    #[test]
    fn interpolation_clamping() {
        // Below minimum anchor
        let intent = QualityIntent::from_quality(0.0);
        assert_eq!(intent.jpeg_quality(), 15); // Clamped to first anchor

        // Above maximum anchor
        let intent = QualityIntent::from_quality(200.0);
        assert_eq!(intent.jpeg_quality(), 100); // Clamped to last anchor
    }

    #[test]
    fn dpr_adjustment_baseline() {
        // DPR 3.0 = baseline = no adjustment
        let adjusted = adjust_quality_for_dpr(73.0, 3.0);
        assert!((adjusted - 73.0).abs() < 0.01);
    }

    #[test]
    fn dpr_adjustment_low_dpr() {
        // DPR 1.0 → quality increases (artifacts magnified)
        let adjusted = adjust_quality_for_dpr(73.0, 1.0);
        assert!((adjusted - 91.0).abs() < 0.01);
    }

    #[test]
    fn dpr_adjustment_high_dpr() {
        // DPR 6.0 → quality decreases (pixels tiny)
        let adjusted = adjust_quality_for_dpr(73.0, 6.0);
        assert!((adjusted - 46.0).abs() < 0.01);
    }

    #[test]
    fn dpr_clamps() {
        // Extreme values don't produce NaN or out-of-range
        let low = adjust_quality_for_dpr(50.0, 0.001);
        assert!((5.0..=99.0).contains(&low));

        let high = adjust_quality_for_dpr(50.0, 1000.0);
        assert!((5.0..=99.0).contains(&high));
    }

    #[test]
    fn png_quality_range() {
        let intent = QualityIntent::from_quality(73.0);
        let (min, max) = intent.png_quality_range();
        assert_eq!(min, 50);
        assert_eq!(max, 100);
        assert!(min <= max);
    }

    #[test]
    fn avif_speed_decreases_with_quality() {
        let low = QualityIntent::from_quality(15.0).avif_speed();
        let high = QualityIntent::from_quality(96.0).avif_speed();
        assert!(
            low > high,
            "AVIF speed should decrease (slower) at higher quality"
        );
    }

    #[test]
    fn profile_to_intent() {
        let intent = QualityProfile::Good.to_intent();
        assert!((intent.quality - 73.0).abs() < 0.01);
        assert!(!intent.lossless);

        let intent = QualityProfile::Lossless.to_intent();
        assert!(intent.lossless);
    }

    #[test]
    fn profile_to_intent_with_dpr() {
        let intent = QualityProfile::Good.to_intent_with_dpr(1.0);
        // DPR 1.0 should raise quality from 73 to ~91
        assert!((intent.quality - 91.0).abs() < 0.5);
    }
}
