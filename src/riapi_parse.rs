//! RIAPI codec key parsing and engine detection.
//!
//! Parses codec-related RIAPI keys from a querystring partition into
//! a [`CodecIntent`] and detects whether the query requires the modern
//! zencodecs engine or can be handled by legacy imageflow.
//!
//! Feature-gated behind `feature = "riapi"`.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};

use crate::ImageFormat;
use crate::format_set::FormatSet;
use crate::intent::{BoolKeep, CodecIntent, FormatChoice, PerCodecHints};
use crate::quality::QualityProfile;

/// Whether a query uses zencodecs' codec engine or can be handled by legacy imageflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecEngine {
    /// Only `quality=` and `format=<specific>`. No profile system, no auto-selection.
    /// Handleable by imageflow's existing monolithic encoder path.
    Legacy,
    /// Any modern codec key: `qp=`, `accept.*`, `lossless=`, per-codec hints,
    /// `format=auto`, supplements, matte. Requires zencodecs.
    Modern,
}

/// Parse codec-related RIAPI keys and detect which engine is needed.
///
/// Accepts the codec partition from a key router (a `BTreeMap` of canonicalized
/// codec keys) and returns a [`CodecIntent`] plus the detected engine requirement.
///
/// # Examples
///
/// ```
/// use std::collections::BTreeMap;
/// use zencodecs::riapi_parse::{parse_codec_keys, CodecEngine};
///
/// let mut keys = BTreeMap::new();
/// keys.insert("format".to_string(), "webp".to_string());
/// keys.insert("quality".to_string(), "80".to_string());
/// let (intent, engine) = parse_codec_keys(&keys);
/// assert_eq!(engine, CodecEngine::Legacy);
/// ```
///
/// ```
/// use std::collections::BTreeMap;
/// use zencodecs::riapi_parse::{parse_codec_keys, CodecEngine};
///
/// let mut keys = BTreeMap::new();
/// keys.insert("qp".to_string(), "high".to_string());
/// keys.insert("accept.webp".to_string(), "true".to_string());
/// let (intent, engine) = parse_codec_keys(&keys);
/// assert_eq!(engine, CodecEngine::Modern);
/// ```
pub fn parse_codec_keys(keys: &BTreeMap<String, String>) -> (CodecIntent, CodecEngine) {
    let intent = parse_intent(keys);
    let engine = detect_engine(keys);
    (intent, engine)
}

/// Parse a `CodecIntent` from RIAPI codec keys.
fn parse_intent(keys: &BTreeMap<String, String>) -> CodecIntent {
    let format = parse_format_choice(keys);
    let quality_profile = keys.get("qp").and_then(|v| QualityProfile::parse(v));
    let quality_fallback = keys.get("quality").and_then(|v| v.parse::<f32>().ok());
    let quality_dpr = keys
        .get("qp.dpr")
        .or_else(|| keys.get("qp.dppx"))
        .and_then(|v| v.parse::<f32>().ok());
    let lossless = keys.get("lossless").and_then(|v| parse_bool_keep(v));
    let allowed = parse_allowed_formats(keys);
    let hints = parse_per_codec_hints(keys);
    let matte = keys.get("matte").and_then(|v| parse_matte_color(v));

    // Implicit format default: qp triggers auto
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
        quality_fallback,
        quality_dpr,
        lossless,
        allowed,
        hints,
        matte,
    }
}

/// Parse `format=` key into a [`FormatChoice`].
fn parse_format_choice(keys: &BTreeMap<String, String>) -> Option<FormatChoice> {
    keys.get("format")
        .map(|v| match v.to_ascii_lowercase().as_str() {
            "auto" => FormatChoice::Auto,
            "keep" => FormatChoice::Keep,
            "jpeg" | "jpg" => FormatChoice::Specific(ImageFormat::Jpeg),
            "png" => FormatChoice::Specific(ImageFormat::Png),
            "gif" => FormatChoice::Specific(ImageFormat::Gif),
            "webp" => FormatChoice::Specific(ImageFormat::WebP),
            "avif" => FormatChoice::Specific(ImageFormat::Avif),
            "jxl" => FormatChoice::Specific(ImageFormat::Jxl),
            "heic" => FormatChoice::Specific(ImageFormat::Heic),
            // Unknown format string: treat as auto
            _ => FormatChoice::Auto,
        })
}

/// Parse `lossless=` value into a [`BoolKeep`].
fn parse_bool_keep(s: &str) -> Option<BoolKeep> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(BoolKeep::True),
        "false" | "0" | "no" => Some(BoolKeep::False),
        "keep" => Some(BoolKeep::Keep),
        _ => None,
    }
}

/// Parse `accept.*` keys into a [`FormatSet`].
///
/// Starts with `FormatSet::all()` and removes formats with `accept.<fmt>=false`.
/// If any `accept.<fmt>=true` is present, starts with empty set and adds only
/// the explicitly accepted formats.
fn parse_allowed_formats(keys: &BTreeMap<String, String>) -> FormatSet {
    let accept_keys: alloc::vec::Vec<_> = keys
        .iter()
        .filter(|(k, _)| k.starts_with("accept."))
        .collect();

    if accept_keys.is_empty() {
        return FormatSet::all();
    }

    // If any accept key is true, use additive mode (start empty, add trues)
    let has_any_true = accept_keys
        .iter()
        .any(|(_, v)| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes"));

    let mut set = if has_any_true {
        // Always include web-safe as baseline when using additive mode
        FormatSet::web_safe()
    } else {
        FormatSet::all()
    };

    for (key, value) in &accept_keys {
        let format_name = key.strip_prefix("accept.").unwrap_or("");
        let format = match format_name {
            "jpeg" | "jpg" => Some(ImageFormat::Jpeg),
            "png" => Some(ImageFormat::Png),
            "gif" => Some(ImageFormat::Gif),
            "webp" => Some(ImageFormat::WebP),
            "avif" => Some(ImageFormat::Avif),
            "jxl" => Some(ImageFormat::Jxl),
            "heic" => Some(ImageFormat::Heic),
            _ => None,
        };

        if let Some(fmt) = format {
            let enabled = matches!(value.to_ascii_lowercase().as_str(), "true" | "1" | "yes");
            if enabled {
                set.insert(fmt);
            } else {
                set.remove(fmt);
            }
        }
    }

    set
}

/// Parse per-codec hint keys (`jpeg.*`, `png.*`, etc.) into [`PerCodecHints`].
fn parse_per_codec_hints(keys: &BTreeMap<String, String>) -> PerCodecHints {
    let mut hints = PerCodecHints::default();

    for (key, value) in keys {
        if let Some(suffix) = key.strip_prefix("jpeg.") {
            hints.jpeg.insert(suffix.to_string(), value.clone());
        } else if let Some(suffix) = key.strip_prefix("png.") {
            hints.png.insert(suffix.to_string(), value.clone());
        } else if let Some(suffix) = key.strip_prefix("webp.") {
            hints.webp.insert(suffix.to_string(), value.clone());
        } else if let Some(suffix) = key.strip_prefix("avif.") {
            hints.avif.insert(suffix.to_string(), value.clone());
        } else if let Some(suffix) = key.strip_prefix("jxl.") {
            hints.jxl.insert(suffix.to_string(), value.clone());
        } else if let Some(suffix) = key.strip_prefix("gif.") {
            hints.gif.insert(suffix.to_string(), value.clone());
        }
    }

    hints
}

/// Parse a matte color from a hex string (e.g., "FFFFFF", "#FF0000", "rgb(255,0,0)").
fn parse_matte_color(s: &str) -> Option<[u8; 3]> {
    let s = s.trim();

    // Hex color: #RRGGBB or RRGGBB
    let hex = s.strip_prefix('#').unwrap_or(s);
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        return Some([r, g, b]);
    }

    None
}

/// Detect which engine is needed based on the keys present.
fn detect_engine(keys: &BTreeMap<String, String>) -> CodecEngine {
    // Any of these keys requires zencodecs
    const MODERN_KEYS: &[&str] = &[
        "qp",
        "qp.dpr",
        "qp.dppx",
        "lossless",
        "accept.webp",
        "accept.avif",
        "accept.jxl",
        "supplements",
        "supplements.only",
        "matte",
    ];

    for key in MODERN_KEYS {
        if keys.contains_key(*key) {
            return CodecEngine::Modern;
        }
    }

    // Per-codec hints require zencodecs
    if keys.keys().any(|k| {
        k.starts_with("jpeg.")
            || k.starts_with("png.")
            || k.starts_with("webp.")
            || k.starts_with("avif.")
            || k.starts_with("jxl.")
            || k.starts_with("gif.")
    }) {
        return CodecEngine::Modern;
    }

    // format=auto triggers the selection engine
    if keys.get("format").is_some_and(|v| v == "auto") {
        return CodecEngine::Modern;
    }

    // Formats only zencodecs handles
    if let Some(fmt) = keys.get("format")
        && fmt.eq_ignore_ascii_case("heic")
    {
        return CodecEngine::Modern;
    }

    CodecEngine::Legacy
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_keys(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // ═══════════════════════════════════════════════════════════════════
    // CodecEngine detection
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn engine_legacy_quality_only() {
        let keys = make_keys(&[("quality", "80")]);
        let (_, engine) = parse_codec_keys(&keys);
        assert_eq!(engine, CodecEngine::Legacy);
    }

    #[test]
    fn engine_legacy_specific_format() {
        let keys = make_keys(&[("format", "jpeg"), ("quality", "80")]);
        let (_, engine) = parse_codec_keys(&keys);
        assert_eq!(engine, CodecEngine::Legacy);
    }

    #[test]
    fn engine_modern_qp() {
        let keys = make_keys(&[("qp", "high")]);
        let (_, engine) = parse_codec_keys(&keys);
        assert_eq!(engine, CodecEngine::Modern);
    }

    #[test]
    fn engine_modern_lossless() {
        let keys = make_keys(&[("lossless", "true")]);
        let (_, engine) = parse_codec_keys(&keys);
        assert_eq!(engine, CodecEngine::Modern);
    }

    #[test]
    fn engine_modern_accept() {
        let keys = make_keys(&[("accept.webp", "true")]);
        let (_, engine) = parse_codec_keys(&keys);
        assert_eq!(engine, CodecEngine::Modern);
    }

    #[test]
    fn engine_modern_per_codec_hint() {
        let keys = make_keys(&[("jpeg.quality", "75")]);
        let (_, engine) = parse_codec_keys(&keys);
        assert_eq!(engine, CodecEngine::Modern);
    }

    #[test]
    fn engine_modern_format_auto() {
        let keys = make_keys(&[("format", "auto")]);
        let (_, engine) = parse_codec_keys(&keys);
        assert_eq!(engine, CodecEngine::Modern);
    }

    #[test]
    fn engine_modern_format_heic() {
        let keys = make_keys(&[("format", "heic")]);
        let (_, engine) = parse_codec_keys(&keys);
        assert_eq!(engine, CodecEngine::Modern);
    }

    #[test]
    fn engine_modern_matte() {
        let keys = make_keys(&[("matte", "FFFFFF")]);
        let (_, engine) = parse_codec_keys(&keys);
        assert_eq!(engine, CodecEngine::Modern);
    }

    #[test]
    fn engine_modern_supplements() {
        let keys = make_keys(&[("supplements", "strip")]);
        let (_, engine) = parse_codec_keys(&keys);
        assert_eq!(engine, CodecEngine::Modern);
    }

    #[test]
    fn engine_legacy_empty() {
        let keys = make_keys(&[]);
        let (_, engine) = parse_codec_keys(&keys);
        assert_eq!(engine, CodecEngine::Legacy);
    }

    // ═══════════════════════════════════════════════════════════════════
    // CodecIntent parsing
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn parse_format_specific() {
        let keys = make_keys(&[("format", "webp")]);
        let (intent, _) = parse_codec_keys(&keys);
        assert_eq!(
            intent.format,
            Some(FormatChoice::Specific(ImageFormat::WebP))
        );
    }

    #[test]
    fn parse_format_jpeg_alias() {
        let keys = make_keys(&[("format", "jpg")]);
        let (intent, _) = parse_codec_keys(&keys);
        assert_eq!(
            intent.format,
            Some(FormatChoice::Specific(ImageFormat::Jpeg))
        );
    }

    #[test]
    fn parse_format_auto() {
        let keys = make_keys(&[("format", "auto")]);
        let (intent, _) = parse_codec_keys(&keys);
        assert_eq!(intent.format, Some(FormatChoice::Auto));
    }

    #[test]
    fn parse_format_keep() {
        let keys = make_keys(&[("format", "keep")]);
        let (intent, _) = parse_codec_keys(&keys);
        assert_eq!(intent.format, Some(FormatChoice::Keep));
    }

    #[test]
    fn parse_quality_profile() {
        let keys = make_keys(&[("qp", "high")]);
        let (intent, _) = parse_codec_keys(&keys);
        assert_eq!(intent.quality_profile, Some(QualityProfile::High));
        // qp triggers auto format
        assert_eq!(intent.format, Some(FormatChoice::Auto));
    }

    #[test]
    fn parse_quality_fallback() {
        let keys = make_keys(&[("quality", "80")]);
        let (intent, _) = parse_codec_keys(&keys);
        assert_eq!(intent.quality_fallback, Some(80.0));
    }

    #[test]
    fn parse_quality_dpr() {
        let keys = make_keys(&[("qp", "good"), ("qp.dpr", "2.0")]);
        let (intent, _) = parse_codec_keys(&keys);
        assert_eq!(intent.quality_dpr, Some(2.0));
    }

    #[test]
    fn parse_quality_dppx_alias() {
        let keys = make_keys(&[("qp", "good"), ("qp.dppx", "3.0")]);
        let (intent, _) = parse_codec_keys(&keys);
        assert_eq!(intent.quality_dpr, Some(3.0));
    }

    #[test]
    fn parse_lossless_true() {
        let keys = make_keys(&[("lossless", "true")]);
        let (intent, _) = parse_codec_keys(&keys);
        assert_eq!(intent.lossless, Some(BoolKeep::True));
    }

    #[test]
    fn parse_lossless_false() {
        let keys = make_keys(&[("lossless", "false")]);
        let (intent, _) = parse_codec_keys(&keys);
        assert_eq!(intent.lossless, Some(BoolKeep::False));
    }

    #[test]
    fn parse_lossless_keep() {
        let keys = make_keys(&[("lossless", "keep")]);
        let (intent, _) = parse_codec_keys(&keys);
        assert_eq!(intent.lossless, Some(BoolKeep::Keep));
    }

    #[test]
    fn parse_accept_formats_additive() {
        let keys = make_keys(&[("accept.webp", "true"), ("accept.avif", "true")]);
        let (intent, _) = parse_codec_keys(&keys);
        assert!(intent.allowed.contains(ImageFormat::WebP));
        assert!(intent.allowed.contains(ImageFormat::Avif));
        // Web-safe baseline is included
        assert!(intent.allowed.contains(ImageFormat::Jpeg));
        assert!(intent.allowed.contains(ImageFormat::Png));
        assert!(intent.allowed.contains(ImageFormat::Gif));
    }

    #[test]
    fn parse_accept_formats_subtractive() {
        let keys = make_keys(&[("accept.avif", "false")]);
        let (intent, _) = parse_codec_keys(&keys);
        assert!(!intent.allowed.contains(ImageFormat::Avif));
        // Everything else is still allowed
        assert!(intent.allowed.contains(ImageFormat::Jpeg));
        assert!(intent.allowed.contains(ImageFormat::WebP));
    }

    #[test]
    fn parse_per_codec_hints() {
        let keys = make_keys(&[
            ("jpeg.quality", "75"),
            ("jpeg.progressive", "true"),
            ("webp.lossless", "true"),
            ("avif.speed", "4"),
        ]);
        let (intent, _) = parse_codec_keys(&keys);
        assert_eq!(intent.hints.jpeg.get("quality"), Some(&"75".to_string()));
        assert_eq!(
            intent.hints.jpeg.get("progressive"),
            Some(&"true".to_string())
        );
        assert_eq!(intent.hints.webp.get("lossless"), Some(&"true".to_string()));
        assert_eq!(intent.hints.avif.get("speed"), Some(&"4".to_string()));
        assert!(intent.hints.png.is_empty());
        assert!(intent.hints.jxl.is_empty());
    }

    #[test]
    fn parse_matte_hex() {
        let keys = make_keys(&[("matte", "FF0000")]);
        let (intent, _) = parse_codec_keys(&keys);
        assert_eq!(intent.matte, Some([255, 0, 0]));
    }

    #[test]
    fn parse_matte_hex_with_hash() {
        let keys = make_keys(&[("matte", "#00FF00")]);
        let (intent, _) = parse_codec_keys(&keys);
        assert_eq!(intent.matte, Some([0, 255, 0]));
    }

    #[test]
    fn parse_matte_invalid() {
        let keys = make_keys(&[("matte", "not-a-color")]);
        let (intent, _) = parse_codec_keys(&keys);
        assert!(intent.matte.is_none());
    }

    #[test]
    fn implicit_auto_from_qp() {
        // When qp is set but format is absent, format should be Auto
        let keys = make_keys(&[("qp", "medium")]);
        let (intent, _) = parse_codec_keys(&keys);
        assert_eq!(intent.format, Some(FormatChoice::Auto));
    }

    #[test]
    fn explicit_format_overrides_qp_implicit() {
        // When both format and qp are set, explicit format wins
        let keys = make_keys(&[("format", "jpeg"), ("qp", "high")]);
        let (intent, _) = parse_codec_keys(&keys);
        assert_eq!(
            intent.format,
            Some(FormatChoice::Specific(ImageFormat::Jpeg))
        );
        assert_eq!(intent.quality_profile, Some(QualityProfile::High));
    }

    #[test]
    fn empty_keys() {
        let keys = make_keys(&[]);
        let (intent, engine) = parse_codec_keys(&keys);
        assert!(intent.format.is_none());
        assert!(intent.quality_profile.is_none());
        assert!(intent.quality_fallback.is_none());
        assert!(intent.lossless.is_none());
        assert!(intent.matte.is_none());
        assert!(intent.hints.is_empty());
        assert_eq!(engine, CodecEngine::Legacy);
    }

    #[test]
    fn format_decision_trace_from_parse() {
        // Full flow: parse keys -> select format -> get decision with trace
        let keys = make_keys(&[
            ("qp", "high"),
            ("accept.webp", "true"),
            ("jpeg.quality", "92"),
        ]);
        let (intent, engine) = parse_codec_keys(&keys);
        assert_eq!(engine, CodecEngine::Modern);

        let facts = crate::select::ImageFacts {
            pixel_count: 1_000_000,
            ..Default::default()
        };
        let registry = crate::registry::CodecRegistry::all();
        let policy = crate::policy::CodecPolicy::new();
        let decision =
            crate::select::select_format_from_intent(&intent, &facts, &registry, &policy).unwrap();
        assert!(!decision.trace.is_empty());
    }
}
