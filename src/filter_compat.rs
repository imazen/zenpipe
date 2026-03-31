//!
//! These rules prevent common mistakes when building filter pipelines:
//! - Mutually exclusive filters (e.g., two different tone mappers)
//! - Order dependencies (e.g., denoise must precede sharpen)
//! - Range conflicts (e.g., high saturation + high gamut expansion)
//!
//! The pipeline can use [`validate_pipeline`] to check for problems before
//! processing, and the autotune system uses these rules to avoid generating
//! conflicting parameter combinations.

/// A filter compatibility issue found during pipeline validation.
use crate::prelude::*;
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CompatIssue {
    /// Severity: "error" (will produce bad output), "warning" (risky), "info" (suboptimal).
    pub severity: &'static str,
    /// Human-readable description.
    pub message: String,
}

/// Filter identity tags for compatibility checking.
///
/// Each filter reports its tag via the `Filter` trait. Tags are used to
/// detect conflicts without downcasting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum FilterTag {
    // Tone mapping (mutually exclusive group)
    Sigmoid,
    DtSigmoid,
    BasecurveToneMap,

    // Sharpening (prefer AdaptiveSharpen over Sharpen)
    Sharpen,
    AdaptiveSharpen,

    // Noise reduction
    NoiseReduction,
    Bilateral,

    // Highlight/Shadow recovery
    HighlightRecovery,
    ShadowLift,
    HighlightsShadows,
    WhitesBlacks,

    // Local detail
    Clarity,
    Texture,
    LocalToneMap,

    // Noise / analysis
    MedianBlur,
    EdgeDetect,

    // Color
    Saturation,
    Vibrance,
    GamutExpand,
    Temperature,
    Tint,
    ColorGrading,

    // Contrast
    Contrast,

    // Tone curves
    ToneCurve,
    ParametricCurve,
    ToneEqualizer,

    // Other
    Exposure,
    FusedAdjust,
    Bloom,
    Levels,
    AutoLevels,
    Other,
}

/// Exclusive groups: only one filter from each group should be active.
pub const EXCLUSIVE_GROUPS: &[(&str, &[FilterTag])] = &[
    (
        "tone_mapper",
        &[
            FilterTag::Sigmoid,
            FilterTag::DtSigmoid,
            FilterTag::BasecurveToneMap,
        ],
    ),
    (
        "sharpening",
        &[FilterTag::Sharpen, FilterTag::AdaptiveSharpen],
    ),
    (
        "smoothing",
        &[FilterTag::NoiseReduction, FilterTag::Bilateral],
    ),
];

/// Ordering constraints: (must_come_first, must_come_second).
pub const ORDER_CONSTRAINTS: &[(FilterTag, FilterTag)] = &[
    // Denoise before sharpen — sharpening amplifies noise
    (FilterTag::NoiseReduction, FilterTag::Sharpen),
    (FilterTag::NoiseReduction, FilterTag::AdaptiveSharpen),
    (FilterTag::NoiseReduction, FilterTag::Clarity),
    (FilterTag::NoiseReduction, FilterTag::Texture),
    (FilterTag::Bilateral, FilterTag::Sharpen),
    (FilterTag::Bilateral, FilterTag::AdaptiveSharpen),
    // Median blur before sharpening — same reason as denoise
    (FilterTag::MedianBlur, FilterTag::Sharpen),
    (FilterTag::MedianBlur, FilterTag::AdaptiveSharpen),
    (FilterTag::MedianBlur, FilterTag::Clarity),
    // Recovery before fine-tuning
    (FilterTag::HighlightRecovery, FilterTag::HighlightsShadows),
    (FilterTag::ShadowLift, FilterTag::HighlightsShadows),
    // Tone mapping before per-pixel adjustments
    (FilterTag::Sigmoid, FilterTag::Contrast),
    (FilterTag::DtSigmoid, FilterTag::Contrast),
    (FilterTag::BasecurveToneMap, FilterTag::Contrast),
];

/// Parameter range conflicts: combinations that produce bad results at high values.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RangeConflict {
    pub filter_a: FilterTag,
    pub filter_b: FilterTag,
    pub description: &'static str,
    /// Maximum safe combined intensity (0–1 scale). Above this, artifacts likely.
    pub max_combined_intensity: f32,
}

pub const RANGE_CONFLICTS: &[RangeConflict] = &[
    RangeConflict {
        filter_a: FilterTag::Sigmoid,
        filter_b: FilterTag::Contrast,
        description: "Sigmoid already adds contrast; stacking doubles the effect",
        max_combined_intensity: 0.6,
    },
    RangeConflict {
        filter_a: FilterTag::LocalToneMap,
        filter_b: FilterTag::Clarity,
        description: "Both extract detail from base layer; stacking causes halos",
        max_combined_intensity: 0.7,
    },
    RangeConflict {
        filter_a: FilterTag::Saturation,
        filter_b: FilterTag::GamutExpand,
        description: "GamutExpand boosts reds; high saturation causes gamut clipping",
        max_combined_intensity: 0.6,
    },
    RangeConflict {
        filter_a: FilterTag::Saturation,
        filter_b: FilterTag::Vibrance,
        description: "Both boost chroma; combined high values oversaturate",
        max_combined_intensity: 0.8,
    },
    RangeConflict {
        filter_a: FilterTag::NoiseReduction,
        filter_b: FilterTag::Bilateral,
        description: "Both smooth; combined high values destroy fine texture",
        max_combined_intensity: 0.7,
    },
    RangeConflict {
        filter_a: FilterTag::HighlightsShadows,
        filter_b: FilterTag::WhitesBlacks,
        description: "Overlapping tonal ranges; combined high values double-adjust",
        max_combined_intensity: 0.7,
    },
];

/// Validate a sequence of filter tags for compatibility issues.
pub fn validate_pipeline(tags: &[FilterTag]) -> Vec<CompatIssue> {
    let mut issues = Vec::new();

    // Check exclusive groups
    for (group_name, members) in EXCLUSIVE_GROUPS {
        let active: Vec<_> = tags.iter().filter(|t| members.contains(t)).collect();
        if active.len() > 1 {
            issues.push(CompatIssue {
                severity: if *group_name == "tone_mapper" {
                    "error"
                } else {
                    "warning"
                },
                message: alloc::format!(
                    "{group_name}: {} filters active, use only one",
                    active.len()
                ),
            });
        }
    }

    // Check ordering constraints
    for (first, second) in ORDER_CONSTRAINTS {
        let pos_first = tags.iter().rposition(|t| t == first);
        let pos_second = tags.iter().position(|t| t == second);
        if let (Some(pf), Some(ps)) = (pos_first, pos_second)
            && pf > ps
        {
            issues.push(CompatIssue {
                severity: "error",
                message: alloc::format!(
                    "{first:?} must come before {second:?} (denoise before sharpen, recovery before tuning)"
                ),
            });
        }
    }

    // Check for Sharpen when AdaptiveSharpen is available
    if tags.contains(&FilterTag::Sharpen) && !tags.contains(&FilterTag::AdaptiveSharpen) {
        issues.push(CompatIssue {
            severity: "info",
            message: String::from(
                "Consider AdaptiveSharpen instead of Sharpen (noise-aware, same speed)",
            ),
        });
    }

    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_dual_tone_mapper() {
        let tags = &[FilterTag::Sigmoid, FilterTag::DtSigmoid];
        let issues = validate_pipeline(tags);
        assert!(
            issues.iter().any(|i| i.severity == "error"),
            "should flag dual tone mappers"
        );
    }

    #[test]
    fn detects_wrong_order() {
        let tags = &[FilterTag::AdaptiveSharpen, FilterTag::NoiseReduction];
        let issues = validate_pipeline(tags);
        assert!(
            issues.iter().any(|i| i.severity == "error"),
            "should flag sharpen before denoise"
        );
    }

    #[test]
    fn correct_order_is_clean() {
        let tags = &[
            FilterTag::NoiseReduction,
            FilterTag::AdaptiveSharpen,
            FilterTag::Clarity,
        ];
        let issues = validate_pipeline(tags);
        assert!(
            issues.iter().all(|i| i.severity != "error"),
            "correct order should have no errors: {issues:?}"
        );
    }

    #[test]
    fn suggests_adaptive_sharpen() {
        let tags = &[FilterTag::Sharpen];
        let issues = validate_pipeline(tags);
        assert!(
            issues.iter().any(|i| i.severity == "info"),
            "should suggest AdaptiveSharpen"
        );
    }
}
