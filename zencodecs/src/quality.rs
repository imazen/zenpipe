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
    /// Parse a quality profile from a string.
    ///
    /// Accepts named profiles (case-insensitive): `lowest`, `low`, `medium_low`,
    /// `medium`, `good`, `high`, `highest`, `lossless`.
    ///
    /// Also accepts numeric values 0-100, which are mapped to the nearest profile.
    ///
    /// Returns `None` for unrecognized strings.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "lowest" => Some(Self::Lowest),
            "low" => Some(Self::Low),
            "medium_low" | "mediumlow" | "medium-low" => Some(Self::MediumLow),
            "medium" | "med" => Some(Self::Medium),
            "good" | "default" => Some(Self::Good),
            "high" => Some(Self::High),
            "highest" => Some(Self::Highest),
            "lossless" => Some(Self::Lossless),
            other => {
                // Try parsing as a number and mapping to nearest profile
                let q: f32 = other.parse().ok()?;
                Some(Self::from_quality(q))
            }
        }
    }

    /// Map a numeric quality (0-100) to the nearest named profile.
    pub fn from_quality(q: f32) -> Self {
        if q >= 98.0 {
            Self::Lossless
        } else if q >= 93.5 {
            Self::Highest
        } else if q >= 82.0 {
            Self::High
        } else if q >= 64.0 {
            Self::Good
        } else if q >= 44.5 {
            Self::Medium
        } else if q >= 27.0 {
            Self::MediumLow
        } else if q >= 17.5 {
            Self::Low
        } else {
            Self::Lowest
        }
    }

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

    // ═══════════════════════════════════════════════════════════════════
    // Aliases matching imageflow / proposal naming
    // ═══════════════════════════════════════════════════════════════════

    /// Alias for [`jpeg_quality`](Self::jpeg_quality) -- mozjpeg native quality.
    pub fn mozjpeg_quality(&self) -> u8 {
        self.jpeg_quality()
    }

    /// Alias for [`webp_quality`](Self::webp_quality) -- libwebp native quality.
    pub fn libwebp_quality(&self) -> f32 {
        self.webp_quality()
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

    #[test]
    fn profile_parse_named() {
        assert_eq!(
            QualityProfile::parse("lowest"),
            Some(QualityProfile::Lowest)
        );
        assert_eq!(QualityProfile::parse("Low"), Some(QualityProfile::Low));
        assert_eq!(
            QualityProfile::parse("MEDIUM_LOW"),
            Some(QualityProfile::MediumLow)
        );
        assert_eq!(
            QualityProfile::parse("medium-low"),
            Some(QualityProfile::MediumLow)
        );
        assert_eq!(
            QualityProfile::parse("mediumlow"),
            Some(QualityProfile::MediumLow)
        );
        assert_eq!(
            QualityProfile::parse("medium"),
            Some(QualityProfile::Medium)
        );
        assert_eq!(QualityProfile::parse("med"), Some(QualityProfile::Medium));
        assert_eq!(QualityProfile::parse("good"), Some(QualityProfile::Good));
        assert_eq!(QualityProfile::parse("default"), Some(QualityProfile::Good));
        assert_eq!(QualityProfile::parse("HIGH"), Some(QualityProfile::High));
        assert_eq!(
            QualityProfile::parse("highest"),
            Some(QualityProfile::Highest)
        );
        assert_eq!(
            QualityProfile::parse("lossless"),
            Some(QualityProfile::Lossless)
        );
    }

    #[test]
    fn profile_parse_numeric() {
        assert_eq!(QualityProfile::parse("73"), Some(QualityProfile::Good));
        assert_eq!(QualityProfile::parse("91"), Some(QualityProfile::High));
        assert_eq!(QualityProfile::parse("100"), Some(QualityProfile::Lossless));
        assert_eq!(QualityProfile::parse("15"), Some(QualityProfile::Lowest));
    }

    #[test]
    fn profile_parse_invalid() {
        assert_eq!(QualityProfile::parse("bogus"), None);
        assert_eq!(QualityProfile::parse(""), None);
    }

    #[test]
    fn mozjpeg_quality_alias() {
        let intent = QualityIntent::from_quality(73.0);
        assert_eq!(intent.mozjpeg_quality(), intent.jpeg_quality());
    }

    #[test]
    fn libwebp_quality_alias() {
        let intent = QualityIntent::from_quality(73.0);
        assert!((intent.libwebp_quality() - intent.webp_quality()).abs() < 0.001);
    }

    // ═══════════════════════════════════════════════════════════════════
    // Regression: all profiles produce distinct, monotonically increasing values
    // ═══════════════════════════════════════════════════════════════════

    /// All 8 QualityProfile variants, ordered from lowest to highest quality.
    const ALL_PROFILES: [QualityProfile; 8] = [
        QualityProfile::Lowest,
        QualityProfile::Low,
        QualityProfile::MediumLow,
        QualityProfile::Medium,
        QualityProfile::Good,
        QualityProfile::High,
        QualityProfile::Highest,
        QualityProfile::Lossless,
    ];

    #[test]
    fn all_profiles_generic_quality_monotonically_increasing() {
        let values: alloc::vec::Vec<f32> =
            ALL_PROFILES.iter().map(|p| p.generic_quality()).collect();
        for w in values.windows(2) {
            assert!(
                w[1] > w[0],
                "generic_quality must be strictly increasing: {} -> {}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn all_profiles_produce_strictly_increasing_jpeg_quality() {
        let values: alloc::vec::Vec<u8> = ALL_PROFILES
            .iter()
            .map(|p| QualityIntent::from_profile(*p).jpeg_quality())
            .collect();
        for w in values.windows(2) {
            assert!(
                w[1] > w[0],
                "JPEG quality must be strictly increasing across profiles: {} -> {}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn all_profiles_produce_strictly_increasing_webp_quality() {
        let values: alloc::vec::Vec<f32> = ALL_PROFILES
            .iter()
            .map(|p| QualityIntent::from_profile(*p).webp_quality())
            .collect();
        for w in values.windows(2) {
            assert!(
                w[1] > w[0],
                "WebP quality must be strictly increasing across profiles: {} -> {}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn all_profiles_produce_strictly_increasing_avif_quality() {
        let values: alloc::vec::Vec<f32> = ALL_PROFILES
            .iter()
            .map(|p| QualityIntent::from_profile(*p).avif_quality())
            .collect();
        for w in values.windows(2) {
            assert!(
                w[1] > w[0],
                "AVIF quality must be strictly increasing across profiles: {} -> {}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn all_profiles_produce_monotonic_jxl_distance() {
        // JXL distance is inverted: lower = better quality.
        // So distance should be non-increasing across profiles.
        let values: alloc::vec::Vec<f32> = ALL_PROFILES
            .iter()
            .map(|p| QualityIntent::from_profile(*p).jxl_distance())
            .collect();
        for w in values.windows(2) {
            assert!(
                w[1] <= w[0],
                "JXL distance must be non-increasing (lower = better) across profiles: {} -> {}",
                w[0],
                w[1]
            );
        }
        assert!(
            values.first().unwrap() > values.last().unwrap(),
            "JXL distance must differ between Lowest and Lossless"
        );
    }

    #[test]
    fn all_profiles_png_quality_range_valid() {
        for profile in &ALL_PROFILES {
            let intent = QualityIntent::from_profile(*profile);
            let (min, max) = intent.png_quality_range();
            assert!(
                min <= max,
                "PNG quality range min ({}) > max ({}) for profile {:?}",
                min,
                max,
                profile
            );
        }
        // Verify min quality increases across profiles
        let mins: alloc::vec::Vec<u8> = ALL_PROFILES
            .iter()
            .map(|p| QualityIntent::from_profile(*p).png_quality_range().0)
            .collect();
        for w in mins.windows(2) {
            assert!(
                w[1] >= w[0],
                "PNG min quality must be non-decreasing across profiles: {} -> {}",
                w[0],
                w[1]
            );
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Regression: calibration table monotonicity
    // ═══════════════════════════════════════════════════════════════════

    fn assert_table_sorted_by_generic_quality(table: &[AnchorPoint], name: &str) {
        for w in table.windows(2) {
            assert!(
                w[1].0 > w[0].0,
                "{name}: generic quality keys must be strictly increasing: {} -> {}",
                w[0].0,
                w[1].0
            );
        }
    }

    fn assert_table_values_non_decreasing(table: &[AnchorPoint], name: &str) {
        for w in table.windows(2) {
            assert!(
                w[1].1 >= w[0].1,
                "{name}: native values must be non-decreasing: {} -> {}",
                w[0].1,
                w[1].1
            );
        }
    }

    fn assert_table_values_non_increasing(table: &[AnchorPoint], name: &str) {
        for w in table.windows(2) {
            assert!(
                w[1].1 <= w[0].1,
                "{name}: native values must be non-increasing: {} -> {}",
                w[0].1,
                w[1].1
            );
        }
    }

    #[test]
    fn calibration_table_jpeg_monotonic() {
        assert_table_sorted_by_generic_quality(&JPEG_TABLE, "JPEG_TABLE");
        assert_table_values_non_decreasing(&JPEG_TABLE, "JPEG_TABLE");
    }

    #[test]
    fn calibration_table_webp_monotonic() {
        assert_table_sorted_by_generic_quality(&WEBP_TABLE, "WEBP_TABLE");
        assert_table_values_non_decreasing(&WEBP_TABLE, "WEBP_TABLE");
    }

    #[test]
    fn calibration_table_webp_method_monotonic() {
        assert_table_sorted_by_generic_quality(&WEBP_METHOD_TABLE, "WEBP_METHOD_TABLE");
        assert_table_values_non_decreasing(&WEBP_METHOD_TABLE, "WEBP_METHOD_TABLE");
    }

    #[test]
    fn calibration_table_avif_monotonic() {
        assert_table_sorted_by_generic_quality(&AVIF_TABLE, "AVIF_TABLE");
        assert_table_values_non_decreasing(&AVIF_TABLE, "AVIF_TABLE");
    }

    #[test]
    fn calibration_table_avif_speed_monotonic() {
        // AVIF speed is inverted: lower = slower + better.
        // Higher generic quality should produce lower (or equal) speed values.
        assert_table_sorted_by_generic_quality(&AVIF_SPEED_TABLE, "AVIF_SPEED_TABLE");
        assert_table_values_non_increasing(&AVIF_SPEED_TABLE, "AVIF_SPEED_TABLE");
    }

    #[test]
    fn calibration_table_jxl_distance_monotonic() {
        // JXL distance is inverted: lower = better quality.
        assert_table_sorted_by_generic_quality(&JXL_DISTANCE_TABLE, "JXL_DISTANCE_TABLE");
        assert_table_values_non_increasing(&JXL_DISTANCE_TABLE, "JXL_DISTANCE_TABLE");
    }

    #[test]
    fn calibration_table_jxl_effort_monotonic() {
        assert_table_sorted_by_generic_quality(&JXL_EFFORT_TABLE, "JXL_EFFORT_TABLE");
        assert_table_values_non_decreasing(&JXL_EFFORT_TABLE, "JXL_EFFORT_TABLE");
    }

    #[test]
    fn calibration_table_png_min_monotonic() {
        assert_table_sorted_by_generic_quality(&PNG_MIN_TABLE, "PNG_MIN_TABLE");
        assert_table_values_non_decreasing(&PNG_MIN_TABLE, "PNG_MIN_TABLE");
    }

    #[test]
    fn calibration_table_png_max_monotonic() {
        assert_table_sorted_by_generic_quality(&PNG_MAX_TABLE, "PNG_MAX_TABLE");
        assert_table_values_non_decreasing(&PNG_MAX_TABLE, "PNG_MAX_TABLE");
    }

    #[test]
    fn calibration_table_png_min_le_max_at_all_anchors() {
        assert_eq!(
            PNG_MIN_TABLE.len(),
            PNG_MAX_TABLE.len(),
            "PNG min/max tables must have same length"
        );
        for (min_anchor, max_anchor) in PNG_MIN_TABLE.iter().zip(PNG_MAX_TABLE.iter()) {
            assert!(
                (min_anchor.0 - max_anchor.0).abs() < 0.001,
                "PNG min/max tables must have matching generic quality keys"
            );
            assert!(
                min_anchor.1 <= max_anchor.1,
                "PNG min ({}) must be <= max ({}) at generic quality {}",
                min_anchor.1,
                max_anchor.1,
                min_anchor.0
            );
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Regression: DPR adjustment across all profiles
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn dpr_1_raises_quality_for_all_non_lossless_profiles() {
        // DPR 1.0 means artifacts are magnified 3x -- quality should increase.
        for profile in &ALL_PROFILES[..7] {
            // Skip Lossless (always 100)
            let base = profile.generic_quality();
            let adjusted = adjust_quality_for_dpr(base, 1.0);
            assert!(
                adjusted > base,
                "DPR=1 should raise quality for {:?}: base={}, adjusted={}",
                profile,
                base,
                adjusted
            );
        }
    }

    #[test]
    fn dpr_3_is_neutral_for_all_profiles() {
        // DPR 3.0 is baseline -- no adjustment.
        for profile in &ALL_PROFILES[..7] {
            let base = profile.generic_quality();
            let adjusted = adjust_quality_for_dpr(base, 3.0);
            assert!(
                (adjusted - base).abs() < 0.01,
                "DPR=3 should be neutral for {:?}: base={}, adjusted={}",
                profile,
                base,
                adjusted
            );
        }
    }

    #[test]
    fn dpr_6_lowers_quality_for_all_non_lossless_profiles() {
        // DPR 6.0 means pixels are tiny -- quality can decrease.
        for profile in &ALL_PROFILES[..7] {
            let base = profile.generic_quality();
            let adjusted = adjust_quality_for_dpr(base, 6.0);
            assert!(
                adjusted < base,
                "DPR=6 should lower quality for {:?}: base={}, adjusted={}",
                profile,
                base,
                adjusted
            );
        }
    }

    #[test]
    fn dpr_adjusted_quality_stays_in_valid_range() {
        let dpr_values = [0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0];
        for profile in &ALL_PROFILES {
            let base = profile.generic_quality();
            for &dpr in &dpr_values {
                let adjusted = adjust_quality_for_dpr(base, dpr);
                assert!(
                    (5.0..=99.0).contains(&adjusted),
                    "DPR={} adjusted quality out of range for {:?}: {}",
                    dpr,
                    profile,
                    adjusted
                );
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Regression: e-commerce quality profiles produce reasonable ranges
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn good_profile_jpeg_quality_in_ecommerce_range() {
        // E-commerce typically uses JPEG quality 65-85.
        // "Good" profile should produce a value in a reasonable range.
        let intent = QualityIntent::from_profile(QualityProfile::Good);
        let q = intent.jpeg_quality();
        assert!(
            (65..=85).contains(&q),
            "Good profile JPEG quality {} should be in e-commerce range 65-85",
            q
        );
    }

    #[test]
    fn high_profile_jpeg_quality_above_good() {
        // "High" should produce notably higher JPEG quality than "Good".
        let good = QualityIntent::from_profile(QualityProfile::Good).jpeg_quality();
        let high = QualityIntent::from_profile(QualityProfile::High).jpeg_quality();
        assert!(
            high > good,
            "High ({}) should produce higher JPEG quality than Good ({})",
            high,
            good
        );
        // High should be >=85 for e-commerce "high quality" use cases.
        assert!(
            high >= 85,
            "High profile JPEG quality {} should be >= 85 for product photography",
            high
        );
    }

    #[test]
    fn good_profile_avif_quality_reasonable() {
        // Good profile should produce AVIF quality in a useful range (40-70).
        let intent = QualityIntent::from_profile(QualityProfile::Good);
        let q = intent.avif_quality();
        assert!(
            (40.0..=70.0).contains(&q),
            "Good profile AVIF quality {} should be in range 40-70",
            q
        );
    }

    #[test]
    fn good_profile_webp_quality_reasonable() {
        // Good profile should produce WebP quality in a useful range (65-85).
        let intent = QualityIntent::from_profile(QualityProfile::Good);
        let q = intent.webp_quality();
        assert!(
            (65.0..=85.0).contains(&q),
            "Good profile WebP quality {} should be in range 65-85",
            q
        );
    }

    #[test]
    fn good_profile_jxl_distance_reasonable() {
        // Good profile should produce JXL distance in a useful range (1.0-4.0).
        let intent = QualityIntent::from_profile(QualityProfile::Good);
        let d = intent.jxl_distance();
        assert!(
            (1.0..=4.0).contains(&d),
            "Good profile JXL distance {} should be in range 1.0-4.0",
            d
        );
    }
}
