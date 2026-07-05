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
use crate::registry::AllowedFormats;
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

/// Pluggable, content-aware output-format chooser.
///
/// Given a per-image `features` vector (e.g. a zenanalyze feature extraction) and
/// the set of `candidates` that have **already passed every hard constraint**
/// (registered + compiled, allowed by policy, and lossless-capable when lossless
/// is required), return the one to use — or `None` to defer to the built-in
/// heuristic preference order.
///
/// The picker can only ever *re-rank* already-valid candidates; it can never
/// widen the allowed set, so alpha/animation/lossless correctness and the codec
/// policy are enforced by the caller regardless of what the picker returns.
///
/// The actual ML picker (zenpicker + an MLP over zenanalyze features) lives
/// outside this crate so the publishable core takes no model or analysis
/// dependency — a consumer injects an implementation here. An empty/absent
/// `features` slice or a `None` return both fall back to the heuristic.
pub trait FormatPicker {
    /// Choose one of `candidates` for content described by `features`, or `None`
    /// to use the heuristic order. A returned format not in `candidates` is
    /// ignored (treated as `None`).
    fn pick(&self, features: &[f32], candidates: &[ImageFormat]) -> Option<ImageFormat>;

    /// Choose among `candidates` with a per-candidate additive **penalty**
    /// (aligned 1:1 with `candidates`, in the picker's own score space) — e.g. an
    /// encode-budget *degradation* cost for a format only affordable at a cheaper
    /// effort than it was trained at. The default ignores penalties and defers to
    /// [`pick`](Self::pick); a budget-aware picker (the MLP) overrides it to fold
    /// them into its ranking. Driven by [`select_format_with_budget_picker`].
    fn pick_with_penalties(
        &self,
        features: &[f32],
        candidates: &[ImageFormat],
        penalties: &[f32],
    ) -> Option<ImageFormat> {
        let _ = penalties;
        self.pick(features, candidates)
    }
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
    registry: &AllowedFormats,
    policy: &CodecPolicy,
) -> crate::Result<FormatSelection> {
    select_format_inner(facts, intent, registry, policy, None, None, None)
}

/// Like [`select_format`], but an injected content-aware [`FormatPicker`] chooses
/// among the candidates that pass every hard constraint (registry, policy,
/// lossless). The picker only re-ranks already-valid formats — it cannot widen
/// the allowed set. A `None` from the picker, an out-of-set answer, or absent
/// `features` all fall back to the built-in heuristic preference order.
pub fn select_format_with_picker(
    facts: &ImageFacts,
    intent: &QualityIntent,
    registry: &AllowedFormats,
    policy: &CodecPolicy,
    picker: Option<&dyn FormatPicker>,
    features: Option<&[f32]>,
) -> crate::Result<FormatSelection> {
    select_format_inner(facts, intent, registry, policy, picker, features, None)
}

/// Like [`select_format_with_picker`], but honors an encode-resource **budget**.
///
/// `budget(format)` returns `None` for a format infeasible under the budget — it
/// is dropped from the candidate set entirely, so neither the picker nor the
/// heuristic head can choose it — or `Some(penalty)` for a feasible one, where
/// `penalty` is a degradation cost in the picker's score space (e.g. the RD loss
/// from running at a cheaper effort) folded into the ranking via
/// [`FormatPicker::pick_with_penalties`]. This honors both format limits
/// (registry / policy / lossless) and resource limits in one pass.
///
/// A `None` picker (or empty `features`) still applies the feasibility filter:
/// the heuristic head becomes the first *feasible* format. A `None` budget is
/// exactly [`select_format_with_picker`].
#[allow(clippy::too_many_arguments)]
pub fn select_format_with_budget_picker(
    facts: &ImageFacts,
    intent: &QualityIntent,
    registry: &AllowedFormats,
    policy: &CodecPolicy,
    picker: Option<&dyn FormatPicker>,
    features: Option<&[f32]>,
    budget: Option<&dyn Fn(ImageFormat) -> Option<f32>>,
) -> crate::Result<FormatSelection> {
    select_format_inner(facts, intent, registry, policy, picker, features, budget)
}

#[allow(clippy::too_many_arguments)]
fn select_format_inner(
    facts: &ImageFacts,
    intent: &QualityIntent,
    registry: &AllowedFormats,
    policy: &CodecPolicy,
    picker: Option<&dyn FormatPicker>,
    features: Option<&[f32]>,
    budget: Option<&dyn Fn(ImageFormat) -> Option<f32>>,
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
        // Encode-budget feasibility: a `None` from the budget closure means this
        // format can't be produced within the budget — drop it like any other
        // hard constraint, so even the heuristic head is always feasible.
        if let Some(b) = budget
            && b(format).is_none()
        {
            trace.push(SelectionStep::FormatSkipped {
                format,
                reason: "infeasible under encode budget",
            });
            return false;
        }
        true
    };

    let preference_order = build_preference_order(facts, intent);

    // Candidates that pass every hard constraint, kept in heuristic order so the
    // first is the heuristic's choice and the whole list is what the picker sees.
    let mut valid: alloc::vec::Vec<(ImageFormat, &'static str)> = alloc::vec::Vec::new();
    for (format, reason) in &preference_order {
        if try_format(*format, &mut trace) {
            valid.push((*format, *reason));
        }
    }

    if valid.is_empty() {
        trace.push(SelectionStep::Info {
            message: "no suitable format found in preference order",
        });
        return Err(whereat::at!(CodecError::NoSuitableEncoder));
    }

    // Let an injected content-aware picker re-rank among the valid candidates.
    // It can only choose one that already passed every constraint; an absent
    // picker/features, a `None`, or an out-of-set answer uses the heuristic head.
    if let (Some(picker), Some(features)) = (picker, features)
        && !features.is_empty()
    {
        let candidates: alloc::vec::Vec<ImageFormat> = valid.iter().map(|(f, _)| *f).collect();
        // With a budget, fold each feasible candidate's degradation penalty into
        // the ranking; without one, a plain re-rank. Every candidate here already
        // passed the feasibility filter, so each `budget(c)` is `Some`.
        let chosen = match budget {
            Some(b) => {
                let penalties: alloc::vec::Vec<f32> =
                    candidates.iter().map(|&c| b(c).unwrap_or(0.0)).collect();
                picker.pick_with_penalties(features, &candidates, &penalties)
            }
            None => picker.pick(features, &candidates),
        };
        if let Some(chosen) = chosen {
            if candidates.contains(&chosen) {
                trace.push(SelectionStep::FormatChosen {
                    format: chosen,
                    reason: "content-aware picker",
                });
                return Ok(FormatSelection {
                    format: chosen,
                    trace,
                });
            }
            trace.push(SelectionStep::Info {
                message: "picker returned a non-candidate format; using heuristic order",
            });
        }
    }

    let (format, reason) = valid[0];
    trace.push(SelectionStep::FormatChosen { format, reason });
    Ok(FormatSelection { format, trace })
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
///
/// This is the picker-less entry; see [`select_format_from_intent_with_picker`]
/// to inject a content-aware [`FormatPicker`] for the auto-selection path.
pub fn select_format_from_intent(
    intent: &CodecIntent,
    facts: &ImageFacts,
    registry: &AllowedFormats,
    policy: &CodecPolicy,
) -> crate::Result<FormatDecision> {
    select_format_from_intent_with_picker(intent, facts, registry, policy, None, None)
}

/// Like [`select_format_from_intent`], but an injected content-aware
/// [`FormatPicker`] (over a per-image `features` vector) chooses among the valid
/// candidates whenever the format resolves via auto-selection (`FormatChoice::Auto`,
/// or a `Keep` whose source format isn't encodable). An explicit
/// `FormatChoice::Specific` request ignores the picker. A `None`/empty `features`
/// slice or a `None` pick falls back to the heuristic preference order.
pub fn select_format_from_intent_with_picker(
    intent: &CodecIntent,
    facts: &ImageFacts,
    registry: &AllowedFormats,
    policy: &CodecPolicy,
    picker: Option<&dyn FormatPicker>,
    features: Option<&[f32]>,
) -> crate::Result<FormatDecision> {
    let lossless = intent.resolve_lossless(facts.is_lossless_source);
    let quality_value = intent.effective_quality();
    let quality_intent = QualityIntent::from_quality(quality_value).with_lossless(lossless);

    let mut trace_steps = alloc::vec::Vec::new();

    // Resolve format choice
    let format = match intent.format {
        Some(FormatChoice::Specific(fmt)) => {
            // Registry-disabled is DisabledFormat (the codec IS compiled in,
            // just turned off for this request), not UnsupportedFormat (which
            // means the codec isn't compiled in at all) -- aligning with
            // decode.rs/info.rs's registry checks and the CodecError doc
            // comments (bug #26).
            if !registry.can_encode(fmt) {
                return Err(whereat::at!(CodecError::DisabledFormat(fmt)));
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
                        picker,
                        features,
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
                    picker,
                    features,
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
                picker,
                features,
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
    registry: &AllowedFormats,
    policy: &CodecPolicy,
    allowed: &FormatSet,
    picker: Option<&dyn FormatPicker>,
    features: Option<&[f32]>,
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

    select_format_inner(
        facts,
        intent,
        registry,
        &effective_policy,
        picker,
        features,
        None,
    )
}

/// Available output formats, filtered by registry and policy.
pub fn available_encode_formats(registry: &AllowedFormats, policy: &CodecPolicy) -> FormatSet {
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
// Selection tests exercise the full auto-pick corpus; with codecs
// compiled out the picker legitimately returns NoSuitableEncoder.
#[cfg(all(
    feature = "jpeg",
    feature = "webp",
    feature = "png",
    feature = "gif",
    feature = "jxl-decode"
))]
mod tests {
    use alloc::string::ToString;

    use super::*;
    use crate::intent::BoolKeep;

    /// Helper: select format with all codecs available and no policy.
    fn select(facts: &ImageFacts, intent: &QualityIntent) -> ImageFormat {
        let registry = AllowedFormats::all();
        let policy = CodecPolicy::new();
        select_format(facts, intent, &registry, &policy)
            .unwrap()
            .format
    }

    /// A `FormatPicker` returning a fixed answer regardless of features — used to
    /// prove the seam honors a valid pick and defends against invalid ones.
    struct FixedPicker(Option<ImageFormat>);
    impl FormatPicker for FixedPicker {
        fn pick(&self, _features: &[f32], _candidates: &[ImageFormat]) -> Option<ImageFormat> {
            self.0
        }
    }

    #[test]
    fn format_picker_reranks_only_valid_candidates() {
        let facts = ImageFacts {
            pixel_count: 1_000_000,
            ..Default::default()
        };
        let intent = QualityIntent::from_quality(73.0); // lossy opaque
        let registry = AllowedFormats::all();
        let policy = CodecPolicy::new();
        let features = [0.5_f32, 0.5, 0.5, 0.5];

        // Heuristic head with no picker. JPEG/WebP/PNG are always compiled here.
        let head = select_format(&facts, &intent, &registry, &policy)
            .unwrap()
            .format;
        // A different, definitely-valid candidate for a lossy opaque still.
        let target = if head == ImageFormat::Jpeg {
            ImageFormat::WebP
        } else {
            ImageFormat::Jpeg
        };

        // 1) The picker's valid choice overrides the heuristic head, attributed.
        let p = FixedPicker(Some(target));
        let sel = select_format_with_picker(
            &facts,
            &intent,
            &registry,
            &policy,
            Some(&p),
            Some(&features),
        )
        .unwrap();
        assert_eq!(sel.format, target, "picker's valid choice should win");
        assert!(
            sel.trace.steps().iter().any(|s| matches!(
                s,
                SelectionStep::FormatChosen {
                    reason: "content-aware picker",
                    ..
                }
            )),
            "picker decision should be recorded in the trace"
        );

        // 2) `None` falls back to the heuristic head.
        let sel = select_format_with_picker(
            &facts,
            &intent,
            &registry,
            &policy,
            Some(&FixedPicker(None)),
            Some(&features),
        )
        .unwrap();
        assert_eq!(sel.format, head);

        // 3) An out-of-candidate pick (Gif is never valid for a lossy opaque
        //    still) is ignored — the picker cannot widen the allowed set.
        let sel = select_format_with_picker(
            &facts,
            &intent,
            &registry,
            &policy,
            Some(&FixedPicker(Some(ImageFormat::Gif))),
            Some(&features),
        )
        .unwrap();
        assert_eq!(sel.format, head, "out-of-candidate pick must be ignored");

        // 4) Empty features bypass the picker entirely.
        let sel =
            select_format_with_picker(&facts, &intent, &registry, &policy, Some(&p), Some(&[]))
                .unwrap();
        assert_eq!(sel.format, head);
    }

    /// Picks the lowest-penalty candidate (ties: first) — proves the budget seam
    /// threads per-candidate penalties into `pick_with_penalties`.
    struct MinPenaltyPicker;
    impl FormatPicker for MinPenaltyPicker {
        fn pick(&self, _f: &[f32], candidates: &[ImageFormat]) -> Option<ImageFormat> {
            candidates.first().copied()
        }
        fn pick_with_penalties(
            &self,
            _f: &[f32],
            candidates: &[ImageFormat],
            penalties: &[f32],
        ) -> Option<ImageFormat> {
            candidates
                .iter()
                .zip(penalties)
                .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(core::cmp::Ordering::Equal))
                .map(|(c, _)| *c)
        }
    }

    #[test]
    fn budget_drops_infeasible_format_from_heuristic_head() {
        let facts = ImageFacts {
            pixel_count: 1_000_000,
            ..Default::default()
        };
        let intent = QualityIntent::from_quality(73.0);
        let registry = AllowedFormats::all();
        let policy = CodecPolicy::new();

        // The no-budget head is the first encodable in preference order.
        let head = select_format(&facts, &intent, &registry, &policy)
            .unwrap()
            .format;

        // Mark that head infeasible; the next feasible candidate must replace it,
        // even with no picker (the feasibility filter is picker-independent).
        let budget = |f: ImageFormat| -> Option<f32> { if f == head { None } else { Some(0.0) } };
        let sel = select_format_with_budget_picker(
            &facts,
            &intent,
            &registry,
            &policy,
            None,
            None,
            Some(&budget),
        )
        .unwrap();
        assert_ne!(
            sel.format, head,
            "an infeasible heuristic head must be dropped"
        );
        assert!(
            budget(sel.format).is_some(),
            "the chosen format must itself be feasible"
        );
    }

    #[test]
    fn budget_seam_threads_penalties_to_picker() {
        let facts = ImageFacts {
            pixel_count: 1_000_000,
            ..Default::default()
        };
        let intent = QualityIntent::from_quality(73.0);
        let registry = AllowedFormats::all();
        let policy = CodecPolicy::new();
        let features = [0.5_f32; 4];

        // Among the feasible candidates, WebP (always encodable in this test cfg)
        // is cheapest; a penalty-aware picker must choose it over the heuristic head.
        let budget = |f: ImageFormat| -> Option<f32> {
            if f == ImageFormat::WebP {
                Some(0.0)
            } else {
                Some(100.0)
            }
        };
        let sel = select_format_with_budget_picker(
            &facts,
            &intent,
            &registry,
            &policy,
            Some(&MinPenaltyPicker),
            Some(&features),
            Some(&budget),
        )
        .unwrap();
        assert_eq!(
            sel.format,
            ImageFormat::WebP,
            "the min-penalty feasible candidate must win"
        );
    }

    #[test]
    fn lossy_opaque_small_prefers_avif_or_jpeg() {
        let facts = ImageFacts {
            pixel_count: 1_000_000,
            ..Default::default()
        };
        let intent = QualityIntent::from_quality(73.0);
        let format = select(&facts, &intent);
        // Preference order: JXL → AVIF → JPEG → WebP → PNG
        #[cfg(feature = "jxl-encode")]
        assert_eq!(format, ImageFormat::Jxl, "JXL should win when compiled in");
        #[cfg(all(not(feature = "jxl-encode"), feature = "avif-encode"))]
        assert_eq!(format, ImageFormat::Avif, "AVIF should win when JXL absent");
        #[cfg(all(not(feature = "jxl-encode"), not(feature = "avif-encode")))]
        assert_eq!(
            format,
            ImageFormat::Jpeg,
            "JPEG should win when JXL+AVIF absent"
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
        // Preference order for large: JXL → JPEG → AVIF → WebP → PNG
        #[cfg(feature = "jxl-encode")]
        assert_eq!(format, ImageFormat::Jxl, "JXL should win when compiled in");
        #[cfg(not(feature = "jxl-encode"))]
        assert_eq!(
            format,
            ImageFormat::Jpeg,
            "JPEG should win for large images when JXL absent"
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
        // Preference order for lossless: JXL → WebP → PNG → AVIF
        #[cfg(feature = "jxl-encode")]
        assert_eq!(
            format,
            ImageFormat::Jxl,
            "JXL should win lossless when compiled in"
        );
        #[cfg(not(feature = "jxl-encode"))]
        assert_eq!(
            format,
            ImageFormat::WebP,
            "WebP should win lossless when JXL absent"
        );
    }

    #[test]
    fn policy_restricts_format() {
        let facts = ImageFacts {
            pixel_count: 1_000_000,
            ..Default::default()
        };
        let intent = QualityIntent::from_quality(73.0);
        let registry = AllowedFormats::all();
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
        let registry = AllowedFormats::all();
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
        // Preference order for animation: AVIF → WebP → GIF
        #[cfg(feature = "avif-encode")]
        assert_eq!(
            format,
            ImageFormat::Avif,
            "AVIF should win animated when compiled in"
        );
        #[cfg(not(feature = "avif-encode"))]
        assert_eq!(
            format,
            ImageFormat::WebP,
            "WebP should win animated when AVIF absent"
        );
    }

    #[test]
    fn no_encoder_returns_error() {
        let facts = ImageFacts::default();
        let intent = QualityIntent::from_quality(73.0);
        let registry = AllowedFormats::none();
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
        let registry = AllowedFormats::all();
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
        // Lossy opaque small image — no alpha, no animation, not lossless
        let facts = ImageFacts {
            pixel_count: 1_000_000,
            ..Default::default()
        };
        let registry = AllowedFormats::all();
        let policy = CodecPolicy::new();
        let decision = select_format_from_intent(&intent, &facts, &registry, &policy).unwrap();
        // Trace must be non-empty
        assert!(!decision.trace.is_empty());
        // Quality must be in a reasonable range (default is 73.0)
        assert!(
            decision.quality.quality > 0.0 && decision.quality.quality <= 100.0,
            "quality {} is out of range (0, 100]",
            decision.quality.quality
        );
        // Format must be a known valid format (not garbage)
        let known_formats = [
            ImageFormat::Jpeg,
            ImageFormat::Png,
            ImageFormat::WebP,
            ImageFormat::Gif,
            ImageFormat::Avif,
            ImageFormat::Jxl,
            ImageFormat::Bmp,
            ImageFormat::Tiff,
            ImageFormat::Pnm,
            ImageFormat::Farbfeld,
            ImageFormat::Heic,
        ];
        assert!(
            known_formats.contains(&decision.format),
            "auto-selected format {:?} is not a known valid format",
            decision.format
        );
        // Under default features (no jxl-encode, no avif-encode):
        // preference order for lossy opaque small is JXL → AVIF → JPEG → WebP → PNG
        // so JPEG should win.
        #[cfg(feature = "jxl-encode")]
        assert_eq!(
            decision.format,
            ImageFormat::Jxl,
            "JXL should win auto-select when jxl-encode is compiled in"
        );
        #[cfg(all(not(feature = "jxl-encode"), feature = "avif-encode"))]
        assert_eq!(
            decision.format,
            ImageFormat::Avif,
            "AVIF should win auto-select when jxl-encode absent and avif-encode is compiled in"
        );
        #[cfg(all(not(feature = "jxl-encode"), not(feature = "avif-encode")))]
        assert_eq!(
            decision.format,
            ImageFormat::Jpeg,
            "JPEG should win auto-select for lossy opaque small image under default features"
        );
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
        let registry = AllowedFormats::all();
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
        let registry = AllowedFormats::all();
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
        let registry = AllowedFormats::all();
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
        let registry = AllowedFormats::all();
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
        let registry = AllowedFormats::all();
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
        let registry = AllowedFormats::all();
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
    fn intent_specific_registry_disabled_format_errors() {
        // Bug #26 regression: a registry-disabled Specific format is
        // DisabledFormat (the codec IS compiled in, just off for this
        // request via AllowedFormats::none()), not UnsupportedFormat (which
        // means the codec isn't compiled in at all).
        let intent = CodecIntent {
            format: Some(FormatChoice::Specific(ImageFormat::Jpeg)),
            ..Default::default()
        };
        let facts = ImageFacts::default();
        let registry = AllowedFormats::none();
        let policy = CodecPolicy::new();
        let result = select_format_from_intent(&intent, &facts, &registry, &policy);
        assert!(matches!(
            result.as_ref().map_err(|e| e.error()),
            Err(CodecError::DisabledFormat(ImageFormat::Jpeg))
        ));
    }

    #[test]
    fn intent_specific_policy_disallowed_format_errors() {
        // The *policy* (not the registry) rejecting a Specific format is
        // still UnsupportedFormat -- only the registry-disabled case
        // (above) changed for bug #26.
        let intent = CodecIntent {
            format: Some(FormatChoice::Specific(ImageFormat::WebP)),
            ..Default::default()
        };
        let facts = ImageFacts::default();
        let registry = AllowedFormats::all();
        let policy = CodecPolicy::new().with_allowed_formats(FormatSet::EMPTY.with(ImageFormat::Jpeg));
        let result = select_format_from_intent(&intent, &facts, &registry, &policy);
        assert!(matches!(
            result.as_ref().map_err(|e| e.error()),
            Err(CodecError::UnsupportedFormat(ImageFormat::WebP))
        ));
    }

    // ═══════════════════════════════════════════════════════════════════
    // Regression: AVIF in preference hierarchy for lossy opaque images
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn avif_in_preference_order_for_lossy_opaque_small() {
        // Verify AVIF is in the preference order for lossy opaque small
        // images (< 3MP) via build_preference_order.
        let facts = ImageFacts {
            pixel_count: 1_000_000,
            ..Default::default()
        };
        let intent = QualityIntent::from_quality(73.0);
        let order = build_preference_order(&facts, &intent);
        let formats: alloc::vec::Vec<_> = order.iter().map(|(f, _)| *f).collect();
        assert!(
            formats.contains(&ImageFormat::Avif),
            "AVIF should be in preference order for lossy opaque small images, got: {:?}",
            formats
        );
    }

    #[test]
    fn avif_in_preference_order_for_lossy_opaque_large() {
        // For large images (>= 3MP), AVIF should still be in the
        // preference order, but after JPEG.
        let facts = ImageFacts {
            pixel_count: 10_000_000,
            ..Default::default()
        };
        let intent = QualityIntent::from_quality(73.0);
        let order = build_preference_order(&facts, &intent);
        let formats: alloc::vec::Vec<_> = order.iter().map(|(f, _)| *f).collect();
        assert!(
            formats.contains(&ImageFormat::Avif),
            "AVIF should be in preference order for lossy opaque large images, got: {:?}",
            formats
        );
        // AVIF should be after JPEG for large images.
        let jpeg_pos = formats.iter().position(|f| *f == ImageFormat::Jpeg);
        let avif_pos = formats.iter().position(|f| *f == ImageFormat::Avif);
        if let (Some(j), Some(a)) = (jpeg_pos, avif_pos) {
            assert!(
                a > j,
                "AVIF (pos {}) should be after JPEG (pos {}) for large images",
                a,
                j
            );
        }
    }

    #[test]
    #[cfg(feature = "avif-encode")]
    fn avif_encode_selectable_for_lossy_opaque() {
        // When avif-encode feature is compiled in, AVIF should be
        // selectable via select_format_from_intent for lossy opaque images.
        let intent = CodecIntent {
            format: Some(FormatChoice::Specific(ImageFormat::Avif)),
            quality_profile: Some(crate::quality::QualityProfile::Good),
            ..Default::default()
        };
        let facts = ImageFacts {
            pixel_count: 1_000_000,
            ..Default::default()
        };
        let registry = AllowedFormats::all();
        let policy = CodecPolicy::new();
        let decision = select_format_from_intent(&intent, &facts, &registry, &policy).unwrap();
        assert_eq!(decision.format, ImageFormat::Avif);
    }

    #[test]
    #[cfg(feature = "avif-encode")]
    fn avif_auto_selected_for_animated() {
        // Animated images should prefer AVIF (if avif-encode is available).
        let facts = ImageFacts {
            has_animation: true,
            pixel_count: 500_000,
            ..Default::default()
        };
        let intent = QualityIntent::from_quality(73.0);
        let format = select(&facts, &intent);
        assert_eq!(
            format,
            ImageFormat::Avif,
            "AVIF should be preferred for animated lossy images when available"
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // Regression: AVIF calibration produces valid quality/speed values
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn avif_calibration_produces_valid_values() {
        // Verify that AVIF quality and speed values are in valid ranges
        // across a sweep of generic quality values.
        for q in (0..=100).step_by(5) {
            let intent = QualityIntent::from_quality(q as f32);
            let avif_q = intent.avif_quality();
            let avif_s = intent.avif_speed();

            assert!(
                (0.0..=100.0).contains(&avif_q),
                "AVIF quality {} out of range at generic quality {}",
                avif_q,
                q
            );
            assert!(
                avif_s <= 10,
                "AVIF speed {} out of range at generic quality {}",
                avif_s,
                q
            );
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Regression: CodecIntent with specific quality targets
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn codec_intent_with_quality_target_for_ecommerce() {
        // Verify that a CodecIntent with a specific quality target
        // produces expected codec-specific values through the full pipeline.
        let intent = CodecIntent {
            format: Some(FormatChoice::Auto),
            quality_profile: Some(crate::quality::QualityProfile::Good),
            ..Default::default()
        };
        let facts = ImageFacts {
            pixel_count: 1_000_000,
            ..Default::default()
        };
        let registry = AllowedFormats::all();
        let policy = CodecPolicy::new();
        let decision = select_format_from_intent(&intent, &facts, &registry, &policy).unwrap();

        // The decision should carry a quality value matching the Good profile.
        assert!(
            (decision.quality.quality - 73.0).abs() < 0.01,
            "Decision quality {} should match Good profile (73.0)",
            decision.quality.quality
        );
        assert!(!decision.lossless);
    }

    #[test]
    fn codec_intent_quality_fallback_used_when_no_profile() {
        // Verify quality_fallback is used when quality_profile is None.
        let intent = CodecIntent {
            format: Some(FormatChoice::Auto),
            quality_fallback: Some(85.0),
            ..Default::default()
        };
        let facts = ImageFacts {
            pixel_count: 1_000_000,
            ..Default::default()
        };
        let registry = AllowedFormats::all();
        let policy = CodecPolicy::new();
        let decision = select_format_from_intent(&intent, &facts, &registry, &policy).unwrap();
        assert!(
            (decision.quality.quality - 85.0).abs() < 0.01,
            "Decision quality {} should match fallback (85.0)",
            decision.quality.quality
        );
    }
}
