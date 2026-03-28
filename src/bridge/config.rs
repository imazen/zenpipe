//! Decode and encode configuration types extracted from zennode node params.

use alloc::boxed::Box;
use alloc::string::{String, ToString};

use zencodecs::quality::{QualityIntent, QualityProfile};
use zennode::NodeInstance;

/// Decode-time configuration extracted from the Decode node's params.
///
/// Provides convenient typed access to decode settings without requiring
/// callers to downcast the node instance or read params individually.
#[derive(Clone, Debug)]
pub struct DecodeConfig {
    /// HDR mode: `"sdr_only"`, `"hdr_reconstruct"`, `"preserve"`.
    pub hdr_mode: String,
    /// Color intent: `"preserve"`, `"srgb"`.
    pub color_intent: String,
    /// JPEG prescale hint (minimum output dimension). 0 = no prescaling.
    pub min_size: u32,
}

impl Default for DecodeConfig {
    fn default() -> Self {
        Self {
            hdr_mode: String::from("sdr_only"),
            color_intent: String::from("preserve"),
            min_size: 0,
        }
    }
}

impl DecodeConfig {
    /// Extract decode configuration from a list of decode-phase nodes.
    ///
    /// Reads params from the first node with schema ID `"zennode.decode"`.
    /// If no such node is found, returns defaults.
    pub fn from_nodes(nodes: &[Box<dyn NodeInstance>]) -> Self {
        for node in nodes {
            if node.schema().id == "zennode.decode" {
                return Self::from_node(node.as_ref());
            }
        }
        Self::default()
    }

    /// Extract decode configuration from a single decode node.
    fn from_node(node: &dyn NodeInstance) -> Self {
        let hdr_mode = node
            .get_param("hdr_mode")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| String::from("sdr_only"));

        let color_intent = node
            .get_param("color_intent")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| String::from("preserve"));

        let min_size = node
            .get_param("min_size")
            .and_then(|v| v.as_u32())
            .unwrap_or(0);

        Self {
            hdr_mode,
            color_intent,
            min_size,
        }
    }
}

/// Encode configuration extracted from encode-phase nodes.
///
/// Reads quality intent and per-codec params from the encode node list.
/// Handles both `"zennode.quality_intent"` and `"zencodecs.quality_intent"`
/// schema IDs so callers don't need to know where QualityIntent is defined.
pub struct EncodeConfig {
    /// Quality profile string (from QualityIntent node, if present).
    ///
    /// Named presets: `"lowest"`, `"low"`, `"medium_low"`, `"medium"`,
    /// `"good"`, `"high"`, `"highest"`, `"lossless"`. Or numeric `"0"`-`"100"`.
    pub quality_profile: Option<String>,
    /// Output format string (from QualityIntent node).
    ///
    /// `""` = auto-select, `"jpeg"`, `"png"`, `"webp"`, `"avif"`, `"jxl"`, `"keep"`.
    pub format: Option<String>,
    /// Device pixel ratio for quality adjustment.
    pub dpr: f32,
    /// Lossless preference.
    pub lossless: Option<bool>,
    /// Per-codec params from an explicit encode node (e.g., `zenjpeg.encode`).
    ///
    /// Stored as the raw node instance for downstream code to downcast
    /// via [`NodeInstance::as_any()`].
    pub codec_params: Option<Box<dyn NodeInstance>>,
}

impl core::fmt::Debug for EncodeConfig {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EncodeConfig")
            .field("quality_profile", &self.quality_profile)
            .field("format", &self.format)
            .field("dpr", &self.dpr)
            .field("lossless", &self.lossless)
            .field(
                "codec_params",
                &self.codec_params.as_ref().map(|n| n.schema().id),
            )
            .finish()
    }
}

impl Clone for EncodeConfig {
    fn clone(&self) -> Self {
        Self {
            quality_profile: self.quality_profile.clone(),
            format: self.format.clone(),
            dpr: self.dpr,
            lossless: self.lossless,
            codec_params: self.codec_params.as_ref().map(|n| n.clone_boxed()),
        }
    }
}

impl Default for EncodeConfig {
    fn default() -> Self {
        Self {
            quality_profile: None,
            format: None,
            dpr: 1.0,
            lossless: None,
            codec_params: None,
        }
    }
}

impl EncodeConfig {
    /// Extract encode configuration from a list of encode-phase nodes.
    ///
    /// Looks for:
    /// - `"zennode.quality_intent"` or `"zencodecs.quality_intent"` for quality/format settings
    /// - Any other encode-phase node as codec-specific params
    pub fn from_nodes(nodes: &[Box<dyn NodeInstance>]) -> Self {
        let mut config = Self::default();

        for node in nodes {
            let id = node.schema().id;
            if id == "zennode.quality_intent" || id == "zencodecs.quality_intent" {
                config.quality_profile = node
                    .get_param("profile")
                    .and_then(|v| v.as_str().map(|s| s.to_string()));

                config.format = node.get_param("format").and_then(|v| {
                    let s = v.as_str()?.to_string();
                    if s.is_empty() { None } else { Some(s) }
                });

                config.dpr = node
                    .get_param("dpr")
                    .and_then(|v| v.as_f32())
                    .unwrap_or(1.0);

                config.lossless = node.get_param("lossless").and_then(|v| v.as_bool());
            } else {
                // Any other encode-phase node is treated as codec-specific config.
                config.codec_params = Some(node.clone_boxed());
            }
        }

        config
    }

    /// Resolve the quality profile string to a [`QualityProfile`].
    ///
    /// Parses named presets (`"good"`, `"high"`, `"lossless"`, etc.) and
    /// numeric values (`"85"` maps to nearest profile). Returns
    /// [`QualityProfile::Good`] if no profile is set or parsing fails.
    pub fn resolve_profile(&self) -> QualityProfile {
        self.quality_profile
            .as_deref()
            .and_then(QualityProfile::parse)
            .unwrap_or_default()
    }

    /// Resolve the string quality profile to a [`QualityIntent`] with
    /// per-codec calibrated quality tables.
    ///
    /// Applies DPR adjustment (baseline DPR 3.0: DPR 1.0 boosts quality,
    /// DPR 6.0 lowers it). Applies lossless override when set.
    ///
    /// # Example
    ///
    /// ```
    /// use zenpipe::EncodeConfig;
    ///
    /// let config = EncodeConfig {
    ///     quality_profile: Some("high".into()),
    ///     dpr: 1.0,
    ///     lossless: Some(false),
    ///     ..Default::default()
    /// };
    /// let intent = config.resolve_quality();
    /// assert_eq!(intent.jpeg_quality(), 97); // DPR 1.0 boosts "high" (91) to ~97
    /// assert!(!intent.lossless);
    /// ```
    pub fn resolve_quality(&self) -> QualityIntent {
        let profile = self.resolve_profile();

        // Apply DPR adjustment. The baseline DPR is 3.0 (no adjustment).
        // Typical web usage: dpr=1.0 (desktop retina) to dpr=3.0 (high-res mobile).
        // The default dpr field is 1.0, matching the common "prepare for retina
        // upscaling" use case. When the caller hasn't set DPR at all (field == 1.0
        // from default), apply the adjustment — a DPR 1.0 image needs higher quality
        // because the browser will magnify it ~3x.
        let mut intent = profile.to_intent_with_dpr(self.dpr);

        // Apply lossless override.
        if self.lossless == Some(true) {
            intent.lossless = true;
        }

        intent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_profile_named_good() {
        let config = EncodeConfig {
            quality_profile: Some("good".into()),
            ..Default::default()
        };
        assert_eq!(config.resolve_profile(), QualityProfile::Good);
    }

    #[test]
    fn resolve_profile_named_high() {
        let config = EncodeConfig {
            quality_profile: Some("high".into()),
            ..Default::default()
        };
        assert_eq!(config.resolve_profile(), QualityProfile::High);
    }

    #[test]
    fn resolve_profile_named_lossless() {
        let config = EncodeConfig {
            quality_profile: Some("lossless".into()),
            ..Default::default()
        };
        assert_eq!(config.resolve_profile(), QualityProfile::Lossless);
    }

    #[test]
    fn resolve_profile_numeric() {
        let config = EncodeConfig {
            quality_profile: Some("85".into()),
            ..Default::default()
        };
        // 85 maps to High (82-93.5 range)
        assert_eq!(config.resolve_profile(), QualityProfile::High);
    }

    #[test]
    fn resolve_profile_none_defaults_to_good() {
        let config = EncodeConfig::default();
        assert_eq!(config.resolve_profile(), QualityProfile::Good);
    }

    #[test]
    fn resolve_profile_invalid_defaults_to_good() {
        let config = EncodeConfig {
            quality_profile: Some("bogus".into()),
            ..Default::default()
        };
        assert_eq!(config.resolve_profile(), QualityProfile::Good);
    }

    #[test]
    fn resolve_quality_good_at_dpr_3() {
        // DPR 3.0 = baseline = no DPR adjustment.
        let config = EncodeConfig {
            quality_profile: Some("good".into()),
            dpr: 3.0,
            ..Default::default()
        };
        let intent = config.resolve_quality();
        // Good profile generic quality = 73.0, JPEG table maps 73 -> 73.
        assert_eq!(intent.jpeg_quality(), 73);
        assert!(!intent.lossless);
    }

    #[test]
    fn resolve_quality_good_at_dpr_1_boosts() {
        // DPR 1.0 boosts quality: artifacts are magnified ~3x by browser.
        let config = EncodeConfig {
            quality_profile: Some("good".into()),
            dpr: 1.0,
            ..Default::default()
        };
        let intent = config.resolve_quality();
        // Good (73) with DPR 1.0 -> quality ~91 -> JPEG ~91
        assert_eq!(intent.jpeg_quality(), 91);
    }

    #[test]
    fn resolve_quality_good_at_dpr_6_lowers() {
        // DPR 6.0 lowers quality: source pixels are tiny on screen.
        let config = EncodeConfig {
            quality_profile: Some("good".into()),
            dpr: 6.0,
            ..Default::default()
        };
        let intent = config.resolve_quality();
        // Good (73) with DPR 6.0 -> quality ~46 -> JPEG ~46
        // (interpolated between anchors 34->34 and 55->57)
        let jpeg_q = intent.jpeg_quality();
        assert!(
            jpeg_q < 60,
            "DPR 6.0 should lower JPEG quality below 60, got {jpeg_q}"
        );
    }

    #[test]
    fn resolve_quality_lossless_override() {
        let config = EncodeConfig {
            quality_profile: Some("good".into()),
            dpr: 3.0,
            lossless: Some(true),
            ..Default::default()
        };
        let intent = config.resolve_quality();
        assert!(intent.lossless);
    }

    #[test]
    fn resolve_quality_lossless_false_not_set() {
        let config = EncodeConfig {
            quality_profile: Some("good".into()),
            dpr: 3.0,
            lossless: Some(false),
            ..Default::default()
        };
        let intent = config.resolve_quality();
        assert!(!intent.lossless);
    }

    #[test]
    fn resolve_quality_lossless_none_not_set() {
        let config = EncodeConfig {
            quality_profile: Some("good".into()),
            dpr: 3.0,
            lossless: None,
            ..Default::default()
        };
        let intent = config.resolve_quality();
        assert!(!intent.lossless);
    }

    #[test]
    fn resolve_quality_default_config() {
        // Default config: no profile, dpr=1.0, no lossless.
        let config = EncodeConfig::default();
        let intent = config.resolve_quality();
        // Default profile is Good (73), DPR 1.0 boosts -> ~91.
        assert_eq!(intent.jpeg_quality(), 91);
        assert!(!intent.lossless);
    }

    #[test]
    fn resolve_quality_per_codec_calibration() {
        // Verify that different codecs get different values from the same profile.
        let config = EncodeConfig {
            quality_profile: Some("good".into()),
            dpr: 3.0,
            ..Default::default()
        };
        let intent = config.resolve_quality();
        assert_eq!(intent.jpeg_quality(), 73);
        // WebP has slightly different calibration: generic 73 -> WebP 76
        assert!((intent.webp_quality() - 76.0).abs() < 0.01);
        // JXL distance: generic 73 -> distance 2.58
        assert!((intent.jxl_distance() - 2.58).abs() < 0.01);
    }

    #[test]
    fn resolve_quality_lossless_profile() {
        let config = EncodeConfig {
            quality_profile: Some("lossless".into()),
            dpr: 3.0,
            ..Default::default()
        };
        let intent = config.resolve_quality();
        // Lossless profile sets the flag automatically.
        assert!(intent.lossless);
        // DPR adjustment clamps quality to 5..99 range, so even lossless
        // (generic 100) gets clamped to 99 -> JPEG 99. The lossless flag
        // is what actually controls lossless encoding, not the quality number.
        assert_eq!(intent.jpeg_quality(), 99);
    }

    #[test]
    fn resolve_quality_high_at_dpr_1() {
        let config = EncodeConfig {
            quality_profile: Some("high".into()),
            dpr: 1.0,
            lossless: Some(false),
            ..Default::default()
        };
        let intent = config.resolve_quality();
        // High (91) with DPR 1.0 -> quality ~97 -> JPEG 97
        assert_eq!(intent.jpeg_quality(), 97);
        assert!(!intent.lossless);
    }
}
