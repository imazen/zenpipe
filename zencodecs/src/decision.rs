//! Format decision: the resolved output of codec selection.
//!
//! [`FormatDecision`] is the result of resolving a [`CodecIntent`](crate::intent::CodecIntent)
//! against [`ImageFacts`](crate::select::ImageFacts), registry, and policy.
//! It contains everything needed to configure an encoder: format, quality,
//! lossless flag, per-codec hints, matte color, and an audit trail.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::ImageFormat;
use crate::quality::QualityIntent;
use crate::trace::SelectionStep;

/// The result of codec selection: what format, what quality, why.
///
/// Produced by [`select_format_from_intent`](crate::select::select_format_from_intent).
/// Contains everything the encoder needs to configure itself.
///
/// # Examples
///
/// ```
/// use zencodecs::decision::FormatDecision;
/// use zencodecs::quality::QualityIntent;
/// use zencodecs::ImageFormat;
///
/// let decision = FormatDecision {
///     format: ImageFormat::WebP,
///     quality: QualityIntent::from_quality(76.0),
///     lossless: false,
///     hints: Default::default(),
///     matte: None,
///     trace: Vec::new(),
/// };
/// assert_eq!(decision.format, ImageFormat::WebP);
/// assert!(!decision.lossless);
/// ```
#[derive(Debug, Clone)]
pub struct FormatDecision {
    /// The selected output format.
    pub format: ImageFormat,
    /// Resolved quality intent with per-codec calibration.
    pub quality: QualityIntent,
    /// Global lossless preference (resolved from BoolKeep + source facts).
    pub lossless: bool,
    /// Per-codec hints for the selected format.
    pub hints: BTreeMap<String, String>,
    /// Matte color for alpha compositing (RGBA to opaque format).
    pub matte: Option<[u8; 3]>,
    /// Explanation trace for debugging/auditing.
    pub trace: Vec<SelectionStep>,
}

impl Default for FormatDecision {
    fn default() -> Self {
        Self {
            format: ImageFormat::Jpeg,
            quality: QualityIntent::default(),
            lossless: false,
            hints: BTreeMap::new(),
            matte: None,
            trace: Vec::new(),
        }
    }
}

impl FormatDecision {
    /// Create a decision for a specific format with default quality.
    ///
    /// Useful when you know the target format and just need a decision struct
    /// for the streaming encoder.
    pub fn for_format(format: ImageFormat) -> Self {
        Self {
            format,
            ..Default::default()
        }
    }

    /// Create a decision for a specific format and quality.
    pub fn for_format_quality(format: ImageFormat, quality: f32) -> Self {
        Self {
            format,
            quality: QualityIntent::from_quality(quality),
            ..Default::default()
        }
    }

    /// Per-codec hints for the selected format.
    ///
    /// Returns a reference to the hints map. Empty if no hints were
    /// specified for this format.
    pub fn hints_for_format(&self) -> &BTreeMap<String, String> {
        &self.hints
    }

    /// Whether any per-codec hints are set.
    pub fn has_hints(&self) -> bool {
        !self.hints.is_empty()
    }

    /// The JPEG quality, considering per-codec hint override.
    ///
    /// If `hints["quality"]` parses to a valid u8, use it.
    /// Otherwise, use the calibration table value.
    pub fn jpeg_quality(&self) -> u8 {
        if let Some(q) = self.hints.get("quality")
            && let Ok(v) = q.parse::<u8>()
        {
            return v;
        }
        self.quality.jpeg_quality()
    }

    /// The WebP quality, considering per-codec hint override.
    pub fn webp_quality(&self) -> f32 {
        if let Some(q) = self.hints.get("quality")
            && let Ok(v) = q.parse::<f32>()
        {
            return v;
        }
        self.quality.webp_quality()
    }

    /// The JXL distance, considering per-codec hint override.
    pub fn jxl_distance(&self) -> f32 {
        if let Some(d) = self.hints.get("distance")
            && let Ok(v) = d.parse::<f32>()
        {
            return v;
        }
        self.quality.jxl_distance()
    }

    /// The AVIF quality, considering per-codec hint override.
    pub fn avif_quality(&self) -> f32 {
        if let Some(q) = self.hints.get("quality")
            && let Ok(v) = q.parse::<f32>()
        {
            return v;
        }
        self.quality.avif_quality()
    }

    /// The PNG quantization quality range (min, max), considering per-codec hint override.
    pub fn png_quality_range(&self) -> (u8, u8) {
        if let Some(q) = self.hints.get("quality")
            && let Ok(v) = q.parse::<u8>()
        {
            // Single value: use as both min and max
            return (v, v);
        }
        self.quality.png_quality_range()
    }

    /// The GIF quality (for quantization), considering per-codec hint override.
    ///
    /// Returns a generic 0-100 value used for quantizer quality settings.
    pub fn gif_quality(&self) -> f32 {
        if let Some(q) = self.hints.get("quality")
            && let Ok(v) = q.parse::<f32>()
        {
            return v;
        }
        self.quality.quality
    }

    /// The JXL effort, considering per-codec hint override.
    pub fn jxl_effort(&self) -> u8 {
        if let Some(e) = self.hints.get("effort")
            && let Ok(v) = e.parse::<u8>()
        {
            return v;
        }
        self.quality.jxl_effort()
    }

    /// The AVIF speed, considering per-codec hint override.
    pub fn avif_speed(&self) -> u8 {
        if let Some(s) = self.hints.get("speed")
            && let Ok(v) = s.parse::<u8>()
        {
            return v;
        }
        self.quality.avif_speed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_defaults() {
        let decision = FormatDecision {
            format: ImageFormat::Jpeg,
            quality: QualityIntent::from_quality(73.0),
            lossless: false,
            hints: BTreeMap::new(),
            matte: None,
            trace: Vec::new(),
        };
        assert_eq!(decision.format, ImageFormat::Jpeg);
        assert!(!decision.lossless);
        assert!(!decision.has_hints());
        assert!(decision.matte.is_none());
        assert!(decision.trace.is_empty());
    }

    #[test]
    fn hints_override_jpeg_quality() {
        let mut hints = BTreeMap::new();
        hints.insert("quality".into(), "75".into());
        let decision = FormatDecision {
            format: ImageFormat::Jpeg,
            quality: QualityIntent::from_quality(73.0),
            lossless: false,
            hints,
            matte: None,
            trace: Vec::new(),
        };
        assert_eq!(decision.jpeg_quality(), 75);
    }

    #[test]
    fn hints_override_webp_quality() {
        let mut hints = BTreeMap::new();
        hints.insert("quality".into(), "80.5".into());
        let decision = FormatDecision {
            format: ImageFormat::WebP,
            quality: QualityIntent::from_quality(73.0),
            lossless: false,
            hints,
            matte: None,
            trace: Vec::new(),
        };
        assert!((decision.webp_quality() - 80.5).abs() < 0.01);
    }

    #[test]
    fn hints_override_jxl_distance() {
        let mut hints = BTreeMap::new();
        hints.insert("distance".into(), "1.5".into());
        let decision = FormatDecision {
            format: ImageFormat::Jxl,
            quality: QualityIntent::from_quality(73.0),
            lossless: false,
            hints,
            matte: None,
            trace: Vec::new(),
        };
        assert!((decision.jxl_distance() - 1.5).abs() < 0.01);
    }

    #[test]
    fn calibration_table_used_when_no_hint() {
        let decision = FormatDecision {
            format: ImageFormat::Jpeg,
            quality: QualityIntent::from_quality(73.0),
            lossless: false,
            hints: BTreeMap::new(),
            matte: None,
            trace: Vec::new(),
        };
        // From calibration table, generic 73 -> JPEG 73
        assert_eq!(decision.jpeg_quality(), 73);
    }

    #[test]
    fn matte_color() {
        let decision = FormatDecision {
            format: ImageFormat::Jpeg,
            quality: QualityIntent::from_quality(73.0),
            lossless: false,
            hints: BTreeMap::new(),
            matte: Some([255, 255, 255]),
            trace: Vec::new(),
        };
        assert_eq!(decision.matte, Some([255, 255, 255]));
    }

    #[test]
    fn trace_records_steps() {
        let decision = FormatDecision {
            format: ImageFormat::WebP,
            quality: QualityIntent::from_quality(73.0),
            lossless: false,
            hints: BTreeMap::new(),
            matte: None,
            trace: alloc::vec![
                SelectionStep::FormatSkipped {
                    format: ImageFormat::Jxl,
                    reason: "not registered",
                },
                SelectionStep::FormatChosen {
                    format: ImageFormat::WebP,
                    reason: "best lossy alpha",
                },
            ],
        };
        assert_eq!(decision.trace.len(), 2);
    }

    #[test]
    fn default_impl() {
        let decision = FormatDecision::default();
        assert_eq!(decision.format, ImageFormat::Jpeg);
        assert!(!decision.lossless);
        assert!(decision.hints.is_empty());
        assert!(decision.matte.is_none());
        assert!(decision.trace.is_empty());
    }

    #[test]
    fn for_format_constructor() {
        let decision = FormatDecision::for_format(ImageFormat::WebP);
        assert_eq!(decision.format, ImageFormat::WebP);
        assert!(!decision.lossless);
    }

    #[test]
    fn for_format_quality_constructor() {
        let decision = FormatDecision::for_format_quality(ImageFormat::Png, 85.0);
        assert_eq!(decision.format, ImageFormat::Png);
        assert!((decision.quality.quality - 85.0).abs() < 0.01);
    }

    #[test]
    fn hints_override_avif_quality() {
        let mut hints = BTreeMap::new();
        hints.insert("quality".into(), "65.0".into());
        let decision = FormatDecision {
            format: ImageFormat::Avif,
            quality: QualityIntent::from_quality(73.0),
            hints,
            ..Default::default()
        };
        assert!((decision.avif_quality() - 65.0).abs() < 0.01);
    }

    #[test]
    fn png_quality_range_from_calibration() {
        let decision = FormatDecision::for_format_quality(ImageFormat::Png, 73.0);
        let (min, max) = decision.png_quality_range();
        assert_eq!(min, 50);
        assert_eq!(max, 100);
    }

    #[test]
    fn hints_override_png_quality() {
        let mut hints = BTreeMap::new();
        hints.insert("quality".into(), "80".into());
        let decision = FormatDecision {
            format: ImageFormat::Png,
            hints,
            ..Default::default()
        };
        let (min, max) = decision.png_quality_range();
        assert_eq!(min, 80);
        assert_eq!(max, 80);
    }

    #[test]
    fn gif_quality_default() {
        let decision = FormatDecision::for_format_quality(ImageFormat::Gif, 73.0);
        assert!((decision.gif_quality() - 73.0).abs() < 0.01);
    }

    #[test]
    fn hints_override_jxl_effort() {
        let mut hints = BTreeMap::new();
        hints.insert("effort".into(), "7".into());
        let decision = FormatDecision {
            format: ImageFormat::Jxl,
            hints,
            ..Default::default()
        };
        assert_eq!(decision.jxl_effort(), 7);
    }

    #[test]
    fn hints_override_avif_speed() {
        let mut hints = BTreeMap::new();
        hints.insert("speed".into(), "4".into());
        let decision = FormatDecision {
            format: ImageFormat::Avif,
            hints,
            ..Default::default()
        };
        assert_eq!(decision.avif_speed(), 4);
    }
}
