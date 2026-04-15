//! Codec intent: parsed user intent for format selection and quality.
//!
//! [`CodecIntent`] captures what the caller wants from encoding —
//! format choice, quality profile, lossless preference, per-codec hints,
//! and allowed formats. It is the input to [`select_format`](crate::select::select_format),
//! which resolves it into a [`FormatDecision`](crate::decision::FormatDecision).
//!
//! These types are codec-agnostic and capture user intent before resolution.
//! Resolution happens when combined with [`ImageFacts`](crate::select::ImageFacts)
//! and the available codec registry.

use alloc::collections::BTreeMap;
use alloc::string::String;

use crate::format_set::FormatSet;
use crate::quality::QualityProfile;

/// Parsed codec-related user intent from querystring parameters.
///
/// Constructed from RIAPI codec keys (`format`, `qp`, `quality`, `accept.*`,
/// `codec.*`) or directly by CLI tools.
///
/// # Examples
///
/// ```
/// use zencodecs::intent::{CodecIntent, FormatChoice, BoolKeep};
/// use zencodecs::quality::QualityProfile;
/// use zencodecs::ImageFormat;
///
/// // High-quality WebP encode
/// let intent = CodecIntent {
///     format: Some(FormatChoice::Specific(ImageFormat::WebP)),
///     quality_profile: Some(QualityProfile::High),
///     ..Default::default()
/// };
///
/// // Auto-select format, preserve source losslessness
/// let intent = CodecIntent {
///     format: Some(FormatChoice::Auto),
///     lossless: Some(BoolKeep::Keep),
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Default)]
pub struct CodecIntent {
    /// Explicit format choice. `None` = context-dependent default.
    pub format: Option<FormatChoice>,
    /// Quality profile from `qp=`.
    pub quality_profile: Option<QualityProfile>,
    /// Fallback quality from `quality=` (0-100). Used when `qp` is absent.
    pub quality_fallback: Option<f32>,
    /// DPR adjustment for quality from `qp.dpr=`.
    pub quality_dpr: Option<f32>,
    /// Global lossless preference from `lossless=`.
    pub lossless: Option<BoolKeep>,
    /// Allowed formats from `accept.*` keys.
    pub allowed: FormatSet,
    /// Per-codec hints (raw key-value pairs for downstream config builders).
    pub hints: PerCodecHints,
    /// Matte color for alpha compositing. From `bgcolor=` when encoding
    /// to a format without alpha (e.g., RGBA source to JPEG output).
    pub matte: Option<[u8; 3]>,
}

/// Explicit format choice from `format=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatChoice {
    /// A specific format: `format=jpeg`, `format=webp`, etc.
    Specific(ImageFormat),
    /// `format=auto` -- let the selector decide.
    Auto,
    /// `format=keep` -- match source format.
    Keep,
}

/// Tri-state for lossless: true, false, or keep (match source).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolKeep {
    /// Force lossless encoding.
    True,
    /// Force lossy encoding.
    False,
    /// Preserve source losslessness. If source was lossless PNG, encode lossless.
    /// If source was lossy JPEG, encode lossy.
    Keep,
}

/// Per-codec encoder hints as raw key-value pairs.
///
/// Untyped and extensible -- adding a new codec hint is a parsing change,
/// not a struct change. The codec adapter interprets the strings downstream.
///
/// Keys are the suffix after the codec prefix. For example, `jpeg.quality=75`
/// becomes `jpeg["quality"] = "75"`.
#[derive(Debug, Clone, Default)]
pub struct PerCodecHints {
    /// JPEG hints (quality, progressive, li).
    pub jpeg: BTreeMap<String, String>,
    /// PNG hints (quality, lossless, min_quality, etc.).
    pub png: BTreeMap<String, String>,
    /// WebP hints (quality, lossless).
    pub webp: BTreeMap<String, String>,
    /// AVIF hints (quality, speed).
    pub avif: BTreeMap<String, String>,
    /// JXL hints (quality, distance, effort, lossless).
    pub jxl: BTreeMap<String, String>,
    /// GIF hints.
    pub gif: BTreeMap<String, String>,
}

impl PerCodecHints {
    /// Get the hints map for a specific format.
    pub fn for_format(&self, format: crate::ImageFormat) -> &BTreeMap<String, String> {
        match format {
            crate::ImageFormat::Jpeg => &self.jpeg,
            crate::ImageFormat::Png => &self.png,
            crate::ImageFormat::WebP => &self.webp,
            crate::ImageFormat::Avif => &self.avif,
            crate::ImageFormat::Jxl => &self.jxl,
            crate::ImageFormat::Gif => &self.gif,
            _ => &self.jpeg, // fallback to empty-ish map; callers check format first
        }
    }

    /// Whether any hints are present for any codec.
    pub fn is_empty(&self) -> bool {
        self.jpeg.is_empty()
            && self.png.is_empty()
            && self.webp.is_empty()
            && self.avif.is_empty()
            && self.jxl.is_empty()
            && self.gif.is_empty()
    }
}

use crate::ImageFormat;

impl CodecIntent {
    /// Resolve the effective quality as a generic 0-100 value.
    ///
    /// Priority: quality_profile (with DPR) > quality_fallback > default (73.0).
    pub fn effective_quality(&self) -> f32 {
        if let Some(profile) = self.quality_profile {
            let base = profile.generic_quality();
            if let Some(dpr) = self.quality_dpr {
                crate::quality::adjust_quality_for_dpr(base, dpr)
            } else {
                base
            }
        } else if let Some(fallback) = self.quality_fallback {
            fallback.clamp(0.0, 100.0)
        } else {
            QualityProfile::default().generic_quality()
        }
    }

    /// Resolve lossless preference given source image facts.
    ///
    /// - `BoolKeep::True` -> true
    /// - `BoolKeep::False` -> false
    /// - `BoolKeep::Keep` -> `is_lossless_source`
    /// - `None` -> false (default to lossy)
    pub fn resolve_lossless(&self, is_lossless_source: bool) -> bool {
        match self.lossless {
            Some(BoolKeep::True) => true,
            Some(BoolKeep::False) => false,
            Some(BoolKeep::Keep) => is_lossless_source,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use super::*;

    #[test]
    fn default_intent() {
        let intent = CodecIntent::default();
        assert!(intent.format.is_none());
        assert!(intent.quality_profile.is_none());
        assert!(intent.quality_fallback.is_none());
        assert!(intent.quality_dpr.is_none());
        assert!(intent.lossless.is_none());
        assert!(intent.matte.is_none());
        assert!(intent.hints.is_empty());
    }

    #[test]
    fn effective_quality_from_profile() {
        let intent = CodecIntent {
            quality_profile: Some(QualityProfile::High),
            ..Default::default()
        };
        assert!((intent.effective_quality() - 91.0).abs() < 0.01);
    }

    #[test]
    fn effective_quality_from_profile_with_dpr() {
        let intent = CodecIntent {
            quality_profile: Some(QualityProfile::Good),
            quality_dpr: Some(1.0),
            ..Default::default()
        };
        // DPR 1.0 should raise quality from 73 to ~91
        assert!((intent.effective_quality() - 91.0).abs() < 0.5);
    }

    #[test]
    fn effective_quality_from_fallback() {
        let intent = CodecIntent {
            quality_fallback: Some(80.0),
            ..Default::default()
        };
        assert!((intent.effective_quality() - 80.0).abs() < 0.01);
    }

    #[test]
    fn effective_quality_default() {
        let intent = CodecIntent::default();
        // Default profile is Good (73.0)
        assert!((intent.effective_quality() - 73.0).abs() < 0.01);
    }

    #[test]
    fn profile_takes_priority_over_fallback() {
        let intent = CodecIntent {
            quality_profile: Some(QualityProfile::High),
            quality_fallback: Some(50.0),
            ..Default::default()
        };
        assert!((intent.effective_quality() - 91.0).abs() < 0.01);
    }

    #[test]
    fn resolve_lossless_true() {
        let intent = CodecIntent {
            lossless: Some(BoolKeep::True),
            ..Default::default()
        };
        assert!(intent.resolve_lossless(false));
        assert!(intent.resolve_lossless(true));
    }

    #[test]
    fn resolve_lossless_false() {
        let intent = CodecIntent {
            lossless: Some(BoolKeep::False),
            ..Default::default()
        };
        assert!(!intent.resolve_lossless(false));
        assert!(!intent.resolve_lossless(true));
    }

    #[test]
    fn resolve_lossless_keep() {
        let intent = CodecIntent {
            lossless: Some(BoolKeep::Keep),
            ..Default::default()
        };
        assert!(!intent.resolve_lossless(false));
        assert!(intent.resolve_lossless(true));
    }

    #[test]
    fn resolve_lossless_none_defaults_lossy() {
        let intent = CodecIntent::default();
        assert!(!intent.resolve_lossless(false));
        assert!(!intent.resolve_lossless(true));
    }

    #[test]
    fn format_choice_variants() {
        let specific = FormatChoice::Specific(ImageFormat::Jpeg);
        assert_eq!(specific, FormatChoice::Specific(ImageFormat::Jpeg));
        assert_ne!(specific, FormatChoice::Auto);
        assert_ne!(specific, FormatChoice::Keep);
        assert_ne!(FormatChoice::Auto, FormatChoice::Keep);
    }

    #[test]
    fn per_codec_hints_for_format() {
        let mut hints = PerCodecHints::default();
        hints.jpeg.insert("quality".into(), "75".into());
        hints.webp.insert("lossless".into(), "true".into());

        assert_eq!(
            hints.for_format(ImageFormat::Jpeg).get("quality"),
            Some(&"75".to_string())
        );
        assert_eq!(
            hints.for_format(ImageFormat::WebP).get("lossless"),
            Some(&"true".to_string())
        );
        assert!(hints.for_format(ImageFormat::Png).is_empty());
    }

    #[test]
    fn per_codec_hints_is_empty() {
        let hints = PerCodecHints::default();
        assert!(hints.is_empty());

        let mut hints2 = PerCodecHints::default();
        hints2.jpeg.insert("quality".into(), "75".into());
        assert!(!hints2.is_empty());
    }

    #[test]
    fn bool_keep_equality() {
        assert_eq!(BoolKeep::True, BoolKeep::True);
        assert_ne!(BoolKeep::True, BoolKeep::False);
        assert_ne!(BoolKeep::True, BoolKeep::Keep);
    }

    // ═══════════════════════════════════════════════════════════════════
    // Regression: CodecIntent construction for e-commerce quality targets
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn codec_intent_good_profile_for_ecommerce() {
        // E-commerce use case: Good or High quality, specific format.
        let intent = CodecIntent {
            format: Some(FormatChoice::Specific(ImageFormat::Jpeg)),
            quality_profile: Some(QualityProfile::Good),
            ..Default::default()
        };
        let q = intent.effective_quality();
        // Good profile should produce quality in the 70-80 range.
        assert!(
            (70.0..=80.0).contains(&q),
            "Good profile effective quality {} should be 70-80 for e-commerce",
            q
        );
    }

    #[test]
    fn codec_intent_high_profile_for_product_photography() {
        // Product photography: High quality.
        let intent = CodecIntent {
            format: Some(FormatChoice::Specific(ImageFormat::Jpeg)),
            quality_profile: Some(QualityProfile::High),
            ..Default::default()
        };
        let q = intent.effective_quality();
        assert!(
            (85.0..=95.0).contains(&q),
            "High profile effective quality {} should be 85-95 for product photography",
            q
        );
    }

    #[test]
    fn codec_intent_dpr_adjustment_propagates() {
        // Verify DPR adjustment is reflected in effective_quality.
        let base_intent = CodecIntent {
            quality_profile: Some(QualityProfile::Good),
            ..Default::default()
        };
        let dpr1_intent = CodecIntent {
            quality_profile: Some(QualityProfile::Good),
            quality_dpr: Some(1.0),
            ..Default::default()
        };
        let dpr6_intent = CodecIntent {
            quality_profile: Some(QualityProfile::Good),
            quality_dpr: Some(6.0),
            ..Default::default()
        };

        let base_q = base_intent.effective_quality();
        let dpr1_q = dpr1_intent.effective_quality();
        let dpr6_q = dpr6_intent.effective_quality();

        assert!(
            dpr1_q > base_q,
            "DPR=1 ({}) should raise quality above base ({})",
            dpr1_q,
            base_q
        );
        assert!(
            dpr6_q < base_q,
            "DPR=6 ({}) should lower quality below base ({})",
            dpr6_q,
            base_q
        );
    }

    #[test]
    fn codec_intent_all_profiles_produce_distinct_effective_quality() {
        // All 8 profiles should produce distinct, increasing effective_quality values.
        let profiles = [
            QualityProfile::Lowest,
            QualityProfile::Low,
            QualityProfile::MediumLow,
            QualityProfile::Medium,
            QualityProfile::Good,
            QualityProfile::High,
            QualityProfile::Highest,
            QualityProfile::Lossless,
        ];
        let values: alloc::vec::Vec<f32> = profiles
            .iter()
            .map(|p| {
                CodecIntent {
                    quality_profile: Some(*p),
                    ..Default::default()
                }
                .effective_quality()
            })
            .collect();
        for w in values.windows(2) {
            assert!(
                w[1] > w[0],
                "effective_quality must be strictly increasing across profiles: {} -> {}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn codec_intent_per_codec_hints_for_avif() {
        // Verify per-codec hints can be set for AVIF.
        let mut intent = CodecIntent {
            format: Some(FormatChoice::Specific(ImageFormat::Avif)),
            quality_profile: Some(QualityProfile::High),
            ..Default::default()
        };
        intent.hints.avif.insert("speed".into(), "4".into());
        intent.hints.avif.insert("quality".into(), "70".into());

        assert_eq!(
            intent.hints.for_format(ImageFormat::Avif).get("speed"),
            Some(&"4".to_string())
        );
        assert_eq!(
            intent.hints.for_format(ImageFormat::Avif).get("quality"),
            Some(&"70".to_string())
        );
        assert!(!intent.hints.is_empty());
    }
}
