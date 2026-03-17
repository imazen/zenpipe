//! Format auto-selection engine.
//!
//! Given image properties, encoding intent, and policy constraints,
//! selects the best output format from available encoders.
//!
//! The preference hierarchy is derived from imageflow's
//! `codec_decisions.rs` calibration.

use crate::format_set::FormatSet;
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
}

impl ImageFacts {
    /// Derive facts from [`ImageInfo`](crate::ImageInfo).
    pub fn from_image_info(info: &zencodec::ImageInfo) -> Self {
        Self {
            has_alpha: info.has_alpha,
            has_animation: info.sequence.is_animation(),
            is_lossless_source: matches!(
                info.format,
                ImageFormat::Png | ImageFormat::Gif | ImageFormat::Bmp
                    | ImageFormat::Pnm | ImageFormat::Farbfeld
            ),
            pixel_count: info.width as u64 * info.height as u64,
            is_hdr: info
                .source_color
                .cicp
                .as_ref()
                .is_some_and(|c| {
                    matches!(
                        c.transfer_function_enum(),
                        zenpixels::TransferFunction::Pq | zenpixels::TransferFunction::Hlg
                    )
                }),
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

/// Available output formats, filtered by registry and policy.
pub fn available_encode_formats(
    registry: &CodecRegistry,
    policy: &CodecPolicy,
) -> FormatSet {
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
    use super::*;

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
            format == ImageFormat::Avif || format == ImageFormat::Jpeg || format == ImageFormat::Jxl,
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
            format == ImageFormat::Jxl
                || format == ImageFormat::WebP
                || format == ImageFormat::Png,
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
}
