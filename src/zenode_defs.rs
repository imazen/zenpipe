//! Zenode node definitions for codec intent.
//!
//! Provides [`QualityIntentNode`], a self-documenting pipeline node that
//! bridges zenode's parameter system with zencodecs' [`CodecIntent`].
//!
//! Feature-gated behind `feature = "zenode"`.

extern crate alloc;
use alloc::string::String;

use zenode::*;

use crate::ImageFormat;
use crate::format_set::FormatSet;
use crate::intent::{BoolKeep, CodecIntent, FormatChoice};
use crate::quality::QualityProfile;

/// Format selection and quality profile for encoding (zenode node).
///
/// This node controls output format selection and quality. It supports
/// both RIAPI querystring keys and JSON API fields, matching imageflow's
/// established `EncoderPreset::Auto` / `EncoderPreset::Format` ergonomics.
///
/// **RIAPI**: `?qp=high&accept.webp=true&accept.avif=true`
/// **JSON**: `{ "profile": "high", "allow_webp": true, "allow_avif": true }`
///
/// When `format` is empty (default), the pipeline auto-selects the best
/// format from the allowed set. When `format` is set (e.g., "jpeg"),
/// that format is used directly.
///
/// The `profile` field accepts both named presets and numeric values:
/// - Named: lowest, low, medium_low, medium, good, high, highest, lossless
/// - Numeric: 0-100 (mapped to codec-specific quality scales)
///
/// Convert to [`CodecIntent`] via [`to_codec_intent()`](QualityIntentNode::to_codec_intent).
#[derive(Node, Clone, Debug)]
#[node(id = "zencodecs.quality_intent", group = Encode, role = Encode)]
#[node(tags("quality", "auto", "format", "encode"))]
pub struct QualityIntentNode {
    /// Quality profile: named preset or numeric 0-100.
    ///
    /// Named presets: "lowest", "low", "medium_low", "medium",
    /// "good", "high", "highest", "lossless".
    /// Numeric: "0" to "100" (codec-specific mapping).
    #[param(default = "high")]
    #[param(section = "Main", label = "Quality Profile")]
    #[kv("qp")]
    pub profile: String,

    /// Explicit output format. Empty = auto-select from allowed formats.
    ///
    /// Values: "jpeg", "png", "webp", "gif", "avif", "jxl", "keep", or "".
    /// "keep" preserves the source format.
    #[param(default = "")]
    #[param(section = "Main", label = "Output Format")]
    #[kv("format")]
    pub format: String,

    /// Device pixel ratio for quality adjustment.
    ///
    /// Higher DPR screens tolerate lower quality (smaller pixels).
    /// Default 1.0 = no adjustment.
    #[param(range(0.5..=10.0), default = 1.0, identity = 1.0, step = 0.5)]
    #[param(unit = "\u{00d7}", section = "Main")]
    #[kv("qp.dpr", "qp.dppx", "dpr", "dppx")]
    pub dpr: f32,

    /// Global lossless preference. Empty = default (lossy).
    ///
    /// Accepts "true", "false", or "keep" (match source losslessness).
    #[param(default = "")]
    #[param(section = "Main")]
    #[kv("lossless")]
    pub lossless: String,

    /// Allow WebP output. Must be explicitly enabled.
    #[param(default = false)]
    #[param(section = "Allowed Formats")]
    #[kv("accept.webp")]
    pub allow_webp: bool,

    /// Allow AVIF output. Must be explicitly enabled.
    #[param(default = false)]
    #[param(section = "Allowed Formats")]
    #[kv("accept.avif")]
    pub allow_avif: bool,

    /// Allow JPEG XL output. Must be explicitly enabled.
    #[param(default = false)]
    #[param(section = "Allowed Formats")]
    #[kv("accept.jxl")]
    pub allow_jxl: bool,

    /// Allow non-sRGB color profiles in the output.
    #[param(default = false)]
    #[param(section = "Allowed Formats")]
    #[kv("accept.color_profiles")]
    pub allow_color_profiles: bool,
}

impl Default for QualityIntentNode {
    fn default() -> Self {
        Self {
            profile: String::from("high"),
            format: String::new(),
            dpr: 1.0,
            lossless: String::new(),
            allow_webp: false,
            allow_avif: false,
            allow_jxl: false,
            allow_color_profiles: false,
        }
    }
}

impl QualityIntentNode {
    /// Convert this node into a [`CodecIntent`] for use with zencodecs'
    /// format selection and encoding pipeline.
    pub fn to_codec_intent(&self) -> CodecIntent {
        let format = self.parse_format();
        let quality_profile = QualityProfile::parse(&self.profile);
        let quality_dpr = if (self.dpr - 1.0).abs() < f32::EPSILON {
            None
        } else {
            Some(self.dpr)
        };
        let lossless = self.parse_lossless();
        let allowed = self.build_format_set();

        // If qp is set but format is absent, default to Auto
        let format = format.or_else(|| {
            if quality_profile.is_some() {
                Some(FormatChoice::Auto)
            } else {
                None
            }
        });

        CodecIntent {
            format,
            quality_profile,
            quality_fallback: None,
            quality_dpr,
            lossless,
            allowed,
            hints: Default::default(),
            matte: None,
        }
    }

    /// Parse the `format` field into a [`FormatChoice`].
    fn parse_format(&self) -> Option<FormatChoice> {
        if self.format.is_empty() {
            return None;
        }
        Some(match self.format.to_ascii_lowercase().as_str() {
            "auto" => FormatChoice::Auto,
            "keep" => FormatChoice::Keep,
            "jpeg" | "jpg" => FormatChoice::Specific(ImageFormat::Jpeg),
            "png" => FormatChoice::Specific(ImageFormat::Png),
            "gif" => FormatChoice::Specific(ImageFormat::Gif),
            "webp" => FormatChoice::Specific(ImageFormat::WebP),
            "avif" => FormatChoice::Specific(ImageFormat::Avif),
            "jxl" => FormatChoice::Specific(ImageFormat::Jxl),
            "heic" => FormatChoice::Specific(ImageFormat::Heic),
            _ => FormatChoice::Auto,
        })
    }

    /// Parse the `lossless` field into a [`BoolKeep`].
    fn parse_lossless(&self) -> Option<BoolKeep> {
        if self.lossless.is_empty() {
            return None;
        }
        match self.lossless.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Some(BoolKeep::True),
            "false" | "0" | "no" => Some(BoolKeep::False),
            "keep" => Some(BoolKeep::Keep),
            _ => None,
        }
    }

    /// Build a [`FormatSet`] from the `allow_*` booleans.
    ///
    /// Web-safe formats (JPEG, PNG, GIF) are always included as the baseline.
    /// Modern formats (WebP, AVIF, JXL) must be explicitly enabled.
    fn build_format_set(&self) -> FormatSet {
        let mut set = FormatSet::web_safe();
        if self.allow_webp {
            set.insert(ImageFormat::WebP);
        }
        if self.allow_avif {
            set.insert(ImageFormat::Avif);
        }
        if self.allow_jxl {
            set.insert(ImageFormat::Jxl);
        }
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_basics() {
        let schema = QUALITY_INTENT_NODE_NODE.schema();
        assert_eq!(schema.id, "zencodecs.quality_intent");
        assert_eq!(schema.group, NodeGroup::Encode);
        assert_eq!(schema.role, NodeRole::Encode);
        assert!(schema.tags.contains(&"quality"));
        assert!(schema.tags.contains(&"auto"));
        assert!(schema.tags.contains(&"format"));
        assert!(schema.tags.contains(&"encode"));

        let param_names: alloc::vec::Vec<&str> = schema.params.iter().map(|p| p.name).collect();
        assert!(param_names.contains(&"profile"));
        assert!(param_names.contains(&"format"));
        assert!(param_names.contains(&"dpr"));
        assert!(param_names.contains(&"lossless"));
        assert!(param_names.contains(&"allow_webp"));
        assert!(param_names.contains(&"allow_avif"));
        assert!(param_names.contains(&"allow_jxl"));
        assert!(param_names.contains(&"allow_color_profiles"));
    }

    #[test]
    fn default_values() {
        let node = QUALITY_INTENT_NODE_NODE.create_default().unwrap();
        assert_eq!(
            node.get_param("profile"),
            Some(ParamValue::Str("high".into()))
        );
        assert_eq!(
            node.get_param("format"),
            Some(ParamValue::Str(String::new()))
        );
        assert_eq!(node.get_param("dpr"), Some(ParamValue::F32(1.0)));
        assert_eq!(
            node.get_param("lossless"),
            Some(ParamValue::Str(String::new()))
        );
        assert_eq!(node.get_param("allow_webp"), Some(ParamValue::Bool(false)));
        assert_eq!(node.get_param("allow_avif"), Some(ParamValue::Bool(false)));
        assert_eq!(node.get_param("allow_jxl"), Some(ParamValue::Bool(false)));
        assert_eq!(
            node.get_param("allow_color_profiles"),
            Some(ParamValue::Bool(false))
        );
    }

    #[test]
    fn kv_keys_coverage() {
        let schema = QUALITY_INTENT_NODE_NODE.schema();

        let profile_param = schema.params.iter().find(|p| p.name == "profile").unwrap();
        assert_eq!(profile_param.kv_keys, &["qp"]);

        let format_param = schema.params.iter().find(|p| p.name == "format").unwrap();
        assert_eq!(format_param.kv_keys, &["format"]);

        let dpr_param = schema.params.iter().find(|p| p.name == "dpr").unwrap();
        assert!(dpr_param.kv_keys.contains(&"qp.dpr"));
        assert!(dpr_param.kv_keys.contains(&"dpr"));
        assert!(dpr_param.kv_keys.contains(&"dppx"));
    }

    #[test]
    fn kv_parsing_qp_with_accepts() {
        let mut kv = KvPairs::from_querystring("qp=medium&accept.webp=true&accept.avif=true");
        let node = QUALITY_INTENT_NODE_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(
            node.get_param("profile"),
            Some(ParamValue::Str("medium".into()))
        );
        assert_eq!(node.get_param("allow_webp"), Some(ParamValue::Bool(true)));
        assert_eq!(node.get_param("allow_avif"), Some(ParamValue::Bool(true)));
        assert_eq!(node.get_param("allow_jxl"), Some(ParamValue::Bool(false)));
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn kv_parsing_format_explicit() {
        let mut kv = KvPairs::from_querystring("format=webp&qp=good");
        let node = QUALITY_INTENT_NODE_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(
            node.get_param("format"),
            Some(ParamValue::Str("webp".into()))
        );
        assert_eq!(
            node.get_param("profile"),
            Some(ParamValue::Str("good".into()))
        );
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn kv_parsing_no_match() {
        let mut kv = KvPairs::from_querystring("w=800&h=600");
        let result = QUALITY_INTENT_NODE_NODE.from_kv(&mut kv).unwrap();
        assert!(result.is_none());
    }

    // ═══════════════════════════════════════════════════════════════════
    // to_codec_intent tests
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn to_codec_intent_default() {
        let node = QualityIntentNode::default();
        let intent = node.to_codec_intent();
        // profile="high" -> qp triggers auto
        assert_eq!(intent.format, Some(FormatChoice::Auto));
        assert_eq!(intent.quality_profile, Some(QualityProfile::High));
        assert!(intent.quality_dpr.is_none()); // dpr 1.0 -> None
        assert!(intent.lossless.is_none()); // empty string -> None
        // web_safe baseline
        assert!(intent.allowed.contains(ImageFormat::Jpeg));
        assert!(intent.allowed.contains(ImageFormat::Png));
        assert!(intent.allowed.contains(ImageFormat::Gif));
        assert!(!intent.allowed.contains(ImageFormat::WebP));
        assert!(!intent.allowed.contains(ImageFormat::Avif));
        assert!(!intent.allowed.contains(ImageFormat::Jxl));
    }

    #[test]
    fn to_codec_intent_with_format() {
        let node = QualityIntentNode {
            format: String::from("jpeg"),
            ..Default::default()
        };
        let intent = node.to_codec_intent();
        assert_eq!(
            intent.format,
            Some(FormatChoice::Specific(ImageFormat::Jpeg))
        );
    }

    #[test]
    fn to_codec_intent_format_keep() {
        let node = QualityIntentNode {
            format: String::from("keep"),
            ..Default::default()
        };
        let intent = node.to_codec_intent();
        assert_eq!(intent.format, Some(FormatChoice::Keep));
    }

    #[test]
    fn to_codec_intent_dpr_adjustment() {
        let node = QualityIntentNode {
            dpr: 2.0,
            ..Default::default()
        };
        let intent = node.to_codec_intent();
        assert_eq!(intent.quality_dpr, Some(2.0));
    }

    #[test]
    fn to_codec_intent_lossless_true() {
        let node = QualityIntentNode {
            lossless: String::from("true"),
            ..Default::default()
        };
        let intent = node.to_codec_intent();
        assert_eq!(intent.lossless, Some(BoolKeep::True));
    }

    #[test]
    fn to_codec_intent_lossless_keep() {
        let node = QualityIntentNode {
            lossless: String::from("keep"),
            ..Default::default()
        };
        let intent = node.to_codec_intent();
        assert_eq!(intent.lossless, Some(BoolKeep::Keep));
    }

    #[test]
    fn to_codec_intent_allowed_formats() {
        let node = QualityIntentNode {
            allow_webp: true,
            allow_avif: true,
            allow_jxl: true,
            ..Default::default()
        };
        let intent = node.to_codec_intent();
        assert!(intent.allowed.contains(ImageFormat::WebP));
        assert!(intent.allowed.contains(ImageFormat::Avif));
        assert!(intent.allowed.contains(ImageFormat::Jxl));
        // web_safe still present
        assert!(intent.allowed.contains(ImageFormat::Jpeg));
        assert!(intent.allowed.contains(ImageFormat::Png));
        assert!(intent.allowed.contains(ImageFormat::Gif));
    }

    #[test]
    fn to_codec_intent_numeric_profile() {
        let node = QualityIntentNode {
            profile: String::from("55"),
            ..Default::default()
        };
        let intent = node.to_codec_intent();
        assert_eq!(intent.quality_profile, Some(QualityProfile::Medium));
    }

    #[test]
    fn downcast() {
        let node = QUALITY_INTENT_NODE_NODE.create_default().unwrap();
        let qi = node.as_any().downcast_ref::<QualityIntentNode>().unwrap();
        assert_eq!(qi.profile, "high");
        assert!(!qi.allow_webp);
    }
}
