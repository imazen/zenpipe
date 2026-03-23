//! Format auto-selection engine.
//!
//! Given image properties, encoding intent, and policy constraints,
//! selects the best output format from available encoders.
//!
//! The preference hierarchy is derived from imageflow's
//! `codec_decisions.rs` calibration.
//!
//! Two entry points:
//! - [`select_format`] -- low-level, takes `QualityIntent` directly
//! - [`select_format_from_intent`] -- high-level, takes [`CodecIntent`] and resolves
//!   format choice, lossless, quality, and per-codec hints into a [`FormatDecision`]

use alloc::collections::BTreeMap;
use alloc::string::String;

use crate::decision::FormatDecision;
use crate::format_set::FormatSet;
use crate::intent::{CodecIntent, FormatChoice};
use crate::policy::CodecPolicy;
use crate::quality::QualityIntent;
use crate::registry::CodecRegistry;
use crate::trace::{SelectionStep, SelectionTrace};
use crate::{CodecError, ImageFormat};

/// Facts about the image being encoded. Drives format selection.
///
/// Callers provide these facts from decode output, image info, or
/// direct knowledge of the source image.
#[derive(Clone, Debug, Default)]
pub struct ImageFacts {
    /// Image has meaningful alpha (not all 255).
    pub has_alpha: bool,
    /// Image is animated (multiple frames).
    pub has_animation: bool,
    /// Source was a lossless format (PNG, lossless WebP, GIF, etc.).
    pub is_lossless_source: bool,
    /// Total pixels (width * height). Used for large-image heuristics.
    pub pixel_count: u64,
    /// Image uses HDR transfer functions (PQ, HLG, linear >1.0).
    pub is_hdr: bool,
    /// Source format, if known. Used for `FormatChoice::Keep`.
    pub source_format: Option<ImageFormat>,
}

impl ImageFacts {
    /// Derive facts from [`ImageInfo`](crate::ImageInfo).
    pub fn from_image_info(info: &zencodec::ImageInfo) -> Self {
        Self {
            has_alpha: info.has_alpha,
            has_animation: info.sequence.is_animation(),
            is_lossless_source: matches!(
                info.format,
                ImageFormat::Png
                    | ImageFormat::Gif
                    | ImageFormat::Bmp
                    | ImageFormat::Pnm
                    | ImageFormat::Farbfeld
            ),
            pixel_count: info.width as u64 * info.height as u64,
            is_hdr: info.source_color.cicp.as_ref().is_some_and(|c| {
                matches!(
                    c.transfer_function_enum(),
                    zenpixels::TransferFunction::Pq | zenpixels::TransferFunction::Hlg
                )
            }),
            source_format: Some(info.format),
        }
    }
}

/// Result of format auto-selection.
pub struct FormatSelection {
    /// The chosen format.
    pub format: ImageFormat,
    /// Decision audit trail.
    pub trace: SelectionTrace,
}

/// Select the best output format for encoding.
///
/// Considers image facts, encoding intent, policy restrictions, and
/// which encoders are registered and enabled.
///
/// # Preference hierarchy (from imageflow)
///
/// | Condition | Order |
/// |-----------|-------|
/// | Lossless | JXL → WebP → PNG → AVIF |
/// | Animation (lossy) | AVIF → WebP → GIF |
/// | Animation (lossless) | WebP → GIF |
/// | Alpha (lossy) | JXL → AVIF → WebP → PNG |
/// | Lossy opaque (< 3MP) | JXL → AVIF → JPEG → WebP → PNG |
/// | Lossy opaque (≥ 3MP) | JXL → JPEG → AVIF → WebP → PNG |
pub fn select_format(
    facts: &ImageFacts,
    intent: &QualityIntent,
    registry: &CodecRegistry,
    policy: &CodecPolicy,
) -> crate::Result<FormatSelection> {
    let mut trace = SelectionTrace::new();

    // Collect candidate formats: registered as encodable + allowed by policy
    let try_format = |format: ImageFormat, trace: &mut SelectionTrace| -> bool {
        if !registry.can_encode(format) {
            trace.push(SelectionStep::FormatSkipped {
                format,
                reason: "not registered or compiled",
            });
            return false;
        }
        if !policy.is_format_allowed(format) {
            trace.push(SelectionStep::FormatSkipped {
                format,
                reason: "not allowed by policy",
            });
            return false;
        }
        if intent.lossless && !format.supports_lossless() {
            trace.push(SelectionStep::FormatSkipped {
                format,
                reason: "no lossless support",
            });
            return false;
        }
        true
    };

    let preference_order = build_preference_order(facts, intent);

    for (format, reason) in &preference_order {
        if try_format(*format, &mut trace) {
            trace.push(SelectionStep::FormatChosen {
                format: *format,
                reason,
            });
            return Ok(FormatSelection {
                format: *format,
                trace,
            });
        }
    }

    trace.push(SelectionStep::Info {
        message: "no suitable format found in preference order",
    });
    Err(whereat::at!(CodecError::NoSuitableEncoder))
}

/// Select the best output format from a [`CodecIntent`] and image facts.
///
/// This is the high-level entry point that resolves a full [`FormatDecision`]
/// from user intent. It handles:
/// - `FormatChoice::Specific` / `Auto` / `Keep`
/// - `BoolKeep` lossless resolution against source facts
/// - Quality profile / fallback / DPR resolution
/// - Per-codec hint extraction for the selected format
/// - Matte color passthrough
///
/// For `FormatChoice::Keep`, the `source_format` from ImageFacts is used.
/// If `source_format` is `None` and format is `Keep`, falls back to auto-selection.
pub fn select_format_from_intent(
    intent: &CodecIntent,
    facts: &ImageFacts,
    registry: &CodecRegistry,
    policy: &CodecPolicy,
) -> crate::Result<FormatDecision> {
    let lossless = intent.resolve_lossless(facts.is_lossless_source);
    let quality_value = intent.effective_quality();
    let quality_intent = QualityIntent::from_quality(quality_value).with_lossless(lossless);

    let mut trace_steps = alloc::vec::Vec::new();

    // Resolve format choice
    let format = match intent.format {
        Some(FormatChoice::Specific(fmt)) => {
            // Validate format is available
            if !registry.can_encode(fmt) {
                return Err(whereat::at!(CodecError::UnsupportedFormat(fmt)));
            }
            if !policy.is_format_allowed(fmt) {
                return Err(whereat::at!(CodecError::UnsupportedFormat(fmt)));
            }
            trace_steps.push(SelectionStep::FormatChosen {
                format: fmt,
                reason: "explicitly requested",
            });
            fmt
        }
        Some(FormatChoice::Keep) => {
            // Use source format if known
            if let Some(src_fmt) = facts.source_format {
                if registry.can_encode(src_fmt) && policy.is_format_allowed(src_fmt) {
                    trace_steps.push(SelectionStep::FormatChosen {
                        format: src_fmt,
                        reason: "keep source format",
                    });
                    src_fmt
                } else {
                    // Source format not encodable, fall through to auto
                    trace_steps.push(SelectionStep::FormatSkipped {
                        format: src_fmt,
                        reason: "source format not encodable, falling back to auto",
                    });
                    let sel = select_format_with_allowed(
                        facts,
                        &quality_intent,
                        registry,
                        policy,
                        &intent.allowed,
                    )?;
                    trace_steps.extend(sel.trace.steps().iter().cloned());
                    sel.format
                }
            } else {
                // No source format known, auto-select
                trace_steps.push(SelectionStep::Info {
                    message: "no source format known, falling back to auto",
                });
                let sel = select_format_with_allowed(
                    facts,
                    &quality_intent,
                    registry,
                    policy,
                    &intent.allowed,
                )?;
                trace_steps.extend(sel.trace.steps().iter().cloned());
                sel.format
            }
        }
        Some(FormatChoice::Auto) | None => {
            let sel = select_format_with_allowed(
                facts,
                &quality_intent,
                registry,
                policy,
                &intent.allowed,
            )?;
            trace_steps.extend(sel.trace.steps().iter().cloned());
            sel.format
        }
    };

    // Extract per-codec hints for the selected format
    let hints: BTreeMap<String, String> = intent.hints.for_format(format).clone();

    Ok(FormatDecision {
        format,
        quality: quality_intent,
        lossless,
        hints,
        matte: intent.matte,
        trace: trace_steps,
    })
}

/// Internal: select format with an additional `allowed` FormatSet filter.
fn select_format_with_allowed(
    facts: &ImageFacts,
    intent: &QualityIntent,
    registry: &CodecRegistry,
    policy: &CodecPolicy,
    allowed: &FormatSet,
) -> crate::Result<FormatSelection> {
    // Create a merged policy that intersects with the allowed set
    let effective_policy = if *allowed != FormatSet::all() {
        let base_formats = policy
            .allowed_formats()
            .cloned()
            .unwrap_or_else(FormatSet::all);
        let merged = base_formats.intersection(allowed);
        CodecPolicy::new().with_allowed_formats(merged)
    } else {
        policy.clone()
    };

    select_format(facts, intent, registry, &effective_policy)
}

/// Available output formats, filtered by registry and policy.
pub fn available_encode_formats(registry: &CodecRegistry, policy: &CodecPolicy) -> FormatSet {
    let mut set = FormatSet::EMPTY;
    for format in registry.encodable_formats() {
        if policy.is_format_allowed(format) {
            set.insert(format);
        }
    }
    set
}

/// Build the preference order for format selection.
///
/// Returns a list of (format, reason) pairs in preference order.
/// The first format that passes the availability check wins.
fn build_preference_order<'a>(
    facts: &ImageFacts,
    intent: &QualityIntent,
) -> alloc::vec::Vec<(ImageFormat, &'a str)> {
    let mut order = alloc::vec::Vec::with_capacity(6);

    if intent.lossless {
        // Lossless path
        order.push((ImageFormat::Jxl, "best lossless compression"));
        order.push((ImageFormat::WebP, "good lossless compression"));
        order.push((ImageFormat::Png, "universal lossless"));
        order.push((ImageFormat::Avif, "lossless available"));
    } else if facts.has_animation {
        // Animation path
        order.push((ImageFormat::Avif, "best animated compression"));
        order.push((ImageFormat::WebP, "animated, good compression"));
        order.push((ImageFormat::Gif, "animated, universal fallback"));
    } else if facts.has_alpha {
        // Lossy with alpha
        order.push((ImageFormat::Jxl, "best lossy alpha compression"));
        order.push((ImageFormat::Avif, "excellent lossy alpha"));
        order.push((ImageFormat::WebP, "good lossy alpha"));
        order.push((ImageFormat::Png, "alpha fallback (lossless)"));
    } else if facts.pixel_count < 3_000_000 {
        // Lossy opaque, small images
        order.push((ImageFormat::Jxl, "best compression"));
        order.push((ImageFormat::Avif, "excellent for small images"));
        order.push((ImageFormat::Jpeg, "universal lossy"));
        order.push((ImageFormat::WebP, "good lossy fallback"));
        order.push((ImageFormat::Png, "last resort"));
    } else {
        // Lossy opaque, large images
        // AVIF demoted: slower for large images
        order.push((ImageFormat::Jxl, "best compression"));
        order.push((ImageFormat::Jpeg, "fast universal lossy"));
        order.push((ImageFormat::Avif, "good but slower for large images"));
        order.push((ImageFormat::WebP, "lossy fallback"));
        order.push((ImageFormat::Png, "last resort"));
    }

    order
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use super::*;
    use crate::intent::BoolKeep;

    /// Helper: select format with all codecs available and no policy.
    fn select(facts: &ImageFacts, intent: &QualityIntent) -> ImageFormat {
        let registry = CodecRegistry::all();
        let policy = CodecPolicy::new();
        select_format(facts, intent, &registry, &policy)
            .unwrap()
            .format
    }

    #[test]
    fn lossy_opaque_small_prefers_avif_or_jpeg() {
        let facts = ImageFacts {
            pixel_count: 1_000_000,
            ..Default::default()
        };
        let intent = QualityIntent::from_quality(73.0);
        let format = select(&facts, &intent);
        // Without JXL encode, should get AVIF (if available) or JPEG
        assert!(
            format == ImageFormat::Avif
                || format == ImageFormat::Jpeg
                || format == ImageFormat::Jxl,
            "got {format:?}"
        );
    }

    #[test]
    fn lossy_opaque_large_prefers_jpeg() {
        let facts = ImageFacts {
            pixel_count: 10_000_000,
            ..Default::default()
        };
        let intent = QualityIntent::from_quality(73.0);
        let format = select(&facts, &intent);
        // JPEG should come before AVIF for large images (unless JXL available)
        assert!(
            format == ImageFormat::Jpeg || format == ImageFormat::Jxl,
            "got {format:?}"
        );
    }

    #[test]
    fn alpha_skips_jpeg() {
        let facts = ImageFacts {
            has_alpha: true,
            pixel_count: 1_000_000,
            ..Default::default()
        };
        let intent = QualityIntent::from_quality(73.0);
        let format = select(&facts, &intent);
        assert_ne!(format, ImageFormat::Jpeg, "JPEG can't encode alpha");
    }

    #[test]
    fn lossless_prefers_jxl_or_webp() {
        let facts = ImageFacts {
            pixel_count: 1_000_000,
            ..Default::default()
        };
        let intent = QualityIntent::from_quality(100.0).with_lossless(true);
        let format = select(&facts, &intent);
        assert!(
            format == ImageFormat::Jxl || format == ImageFormat::WebP || format == ImageFormat::Png,
            "got {format:?}"
        );
    }

    #[test]
    fn policy_restricts_format() {
        let facts = ImageFacts {
            pixel_count: 1_000_000,
            ..Default::default()
        };
        let intent = QualityIntent::from_quality(73.0);
        let registry = CodecRegistry::all();
        let policy = CodecPolicy::web_safe_output();
        let result = select_format(&facts, &intent, &registry, &policy).unwrap();
        assert!(
            result.format == ImageFormat::Jpeg
                || result.format == ImageFormat::Png
                || result.format == ImageFormat::Gif,
            "web_safe should only allow JPEG/PNG/GIF, got {:?}",
            result.format
        );
    }

    #[test]
    fn trace_records_decisions() {
        let facts = ImageFacts::default();
        let intent = QualityIntent::from_quality(73.0);
        let registry = CodecRegistry::all();
        let policy = CodecPolicy::web_safe_output();
        let result = select_format(&facts, &intent, &registry, &policy).unwrap();
        assert!(!result.trace.is_empty());
        assert!(result.trace.chosen_format().is_some());
    }

    #[test]
    fn animation_prefers_avif_or_webp() {
        let facts = ImageFacts {
            has_animation: true,
            pixel_count: 500_000,
            ..Default::default()
        };
        let intent = QualityIntent::from_quality(73.0);
        let format = select(&facts, &intent);
        assert!(
            format == ImageFormat::Avif
                || format == ImageFormat::WebP
                || format == ImageFormat::Gif,
            "animated should prefer AVIF/WebP/GIF, got {format:?}"
        );
    }

    #[test]
    fn no_encoder_returns_error() {
        let facts = ImageFacts::default();
        let intent = QualityIntent::from_quality(73.0);
        let registry = CodecRegistry::none();
        let policy = CodecPolicy::new();
        let result = select_format(&facts, &intent, &registry, &policy);
        assert!(result.is_err());
    }

    // ═══════════════════════════════════════════════════════════════════
    // select_format_from_intent tests
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn intent_specific_format() {
        let intent = CodecIntent {
            format: Some(FormatChoice::Specific(ImageFormat::WebP)),
            quality_profile: Some(crate::quality::QualityProfile::High),
            ..Default::default()
        };
        let facts = ImageFacts {
            pixel_count: 1_000_000,
            ..Default::default()
        };
        let registry = CodecRegistry::all();
        let policy = CodecPolicy::new();
        let decision = select_format_from_intent(&intent, &facts, &registry, &policy).unwrap();
        assert_eq!(decision.format, ImageFormat::WebP);
        assert!(!decision.lossless);
        // Quality should be High profile's generic value (91.0)
        assert!((decision.quality.quality - 91.0).abs() < 0.01);
    }

    #[test]
    fn intent_auto_format() {
        let intent = CodecIntent {
            format: Some(FormatChoice::Auto),
            ..Default::default()
        };
        let facts = ImageFacts {
            pixel_count: 1_000_000,
            ..Default::default()
        };
        let registry = CodecRegistry::all();
        let policy = CodecPolicy::new();
        let decision = select_format_from_intent(&intent, &facts, &registry, &policy).unwrap();
        // Auto should select something reasonable
        assert!(!decision.trace.is_empty());
    }

    #[test]
    fn intent_keep_format() {
        let intent = CodecIntent {
            format: Some(FormatChoice::Keep),
            ..Default::default()
        };
        let facts = ImageFacts {
            pixel_count: 1_000_000,
            source_format: Some(ImageFormat::Png),
            is_lossless_source: true,
            ..Default::default()
        };
        let registry = CodecRegistry::all();
        let policy = CodecPolicy::new();
        let decision = select_format_from_intent(&intent, &facts, &registry, &policy).unwrap();
        assert_eq!(decision.format, ImageFormat::Png);
    }

    #[test]
    fn intent_lossless_keep() {
        let intent = CodecIntent {
            format: Some(FormatChoice::Auto),
            lossless: Some(BoolKeep::Keep),
            ..Default::default()
        };
        // Source is lossless PNG
        let facts = ImageFacts {
            pixel_count: 1_000_000,
            is_lossless_source: true,
            source_format: Some(ImageFormat::Png),
            ..Default::default()
        };
        let registry = CodecRegistry::all();
        let policy = CodecPolicy::new();
        let decision = select_format_from_intent(&intent, &facts, &registry, &policy).unwrap();
        assert!(decision.lossless);
    }

    #[test]
    fn intent_with_per_codec_hints() {
        let mut intent = CodecIntent {
            format: Some(FormatChoice::Specific(ImageFormat::Jpeg)),
            ..Default::default()
        };
        intent.hints.jpeg.insert("quality".into(), "75".into());
        intent
            .hints
            .jpeg
            .insert("progressive".into(), "true".into());

        let facts = ImageFacts::default();
        let registry = CodecRegistry::all();
        let policy = CodecPolicy::new();
        let decision = select_format_from_intent(&intent, &facts, &registry, &policy).unwrap();
        assert_eq!(decision.hints.get("quality"), Some(&"75".to_string()));
        assert_eq!(decision.hints.get("progressive"), Some(&"true".to_string()));
    }

    #[test]
    fn intent_with_matte() {
        let intent = CodecIntent {
            format: Some(FormatChoice::Specific(ImageFormat::Jpeg)),
            matte: Some([255, 0, 0]),
            ..Default::default()
        };
        let facts = ImageFacts::default();
        let registry = CodecRegistry::all();
        let policy = CodecPolicy::new();
        let decision = select_format_from_intent(&intent, &facts, &registry, &policy).unwrap();
        assert_eq!(decision.matte, Some([255, 0, 0]));
    }

    #[test]
    fn intent_with_dpr_adjusts_quality() {
        let intent = CodecIntent {
            format: Some(FormatChoice::Specific(ImageFormat::Jpeg)),
            quality_profile: Some(crate::quality::QualityProfile::Good),
            quality_dpr: Some(1.0),
            ..Default::default()
        };
        let facts = ImageFacts::default();
        let registry = CodecRegistry::all();
        let policy = CodecPolicy::new();
        let decision = select_format_from_intent(&intent, &facts, &registry, &policy).unwrap();
        // DPR 1.0 should raise quality from 73 to ~91
        assert!((decision.quality.quality - 91.0).abs() < 0.5);
    }

    #[test]
    fn intent_allowed_formats_restrict_auto() {
        let intent = CodecIntent {
            format: Some(FormatChoice::Auto),
            allowed: FormatSet::web_safe(),
            ..Default::default()
        };
        let facts = ImageFacts {
            pixel_count: 1_000_000,
            ..Default::default()
        };
        let registry = CodecRegistry::all();
        let policy = CodecPolicy::new();
        let decision = select_format_from_intent(&intent, &facts, &registry, &policy).unwrap();
        assert!(
            decision.format == ImageFormat::Jpeg
                || decision.format == ImageFormat::Png
                || decision.format == ImageFormat::Gif,
            "allowed web_safe should restrict to JPEG/PNG/GIF, got {:?}",
            decision.format
        );
    }

    #[test]
    fn intent_specific_unsupported_format_errors() {
        let intent = CodecIntent {
            format: Some(FormatChoice::Specific(ImageFormat::Jpeg)),
            ..Default::default()
        };
        let facts = ImageFacts::default();
        let registry = CodecRegistry::none();
        let policy = CodecPolicy::new();
        let result = select_format_from_intent(&intent, &facts, &registry, &policy);
        assert!(result.is_err());
    }
}
