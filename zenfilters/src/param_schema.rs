//! Self-describing parameter schemas for generic UI generation.
//!
//! Every filter implements [`Describe`] to emit a [`FilterSchema`] — a complete
//! description of its identity, parameters, ranges, defaults, and display hints.
//! A UI framework can generate excellent controls from this schema alone:
//!
//! ```text
//! schema.label         → "Adaptive Sharpen"
//! schema.description   → "Noise-gated sharpening..."
//! schema.group         → FilterGroup::Detail
//! schema.params[0]     → ParamDesc { name: "amount", label: "Amount",
//!                          kind: ParamKind::Float { min: 0.0, max: 2.0,
//!                          default: 0.0, identity: 0.0, step: 0.01,
//!                          slider_mapping: SliderMapping::Linear },
//!                          unit: "×", group: "Main" }
//! ```
//!
//! # Design
//!
//! - Schemas are `const`-friendly (no heap allocation in descriptors)
//! - Parameters support get/set by name via [`Describe::get_param`] / [`Describe::set_param`]
//! - Slider mappings reference the [`slider`](crate::slider) module functions
//! - Enums describe fixed-choice parameters (e.g., blend mode)

/// Top-level filter category for UI grouping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum FilterGroup {
    /// Exposure, contrast, levels, curves, tone mapping
    Tone,
    /// Highlights, shadows, whites, blacks, recovery, lift
    ToneRange,
    /// Temperature, tint, saturation, vibrance, HSL, color grading
    Color,
    /// Sharpening, clarity, texture, noise reduction
    Detail,
    /// Vignette, grain, bloom, dehaze, chromatic aberration
    Effects,
    /// Sigmoid, basecurve, DtSigmoid (scene→display conversion)
    ToneMap,
    /// Rotation, warp, affine, perspective transforms
    Geometry,
    /// Auto exposure, auto tune
    Auto,
}

/// How a parameter maps to a UI slider.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SliderMapping {
    /// Direct 1:1 — parameter value = slider value. Most common.
    Linear,
    /// Slider squared: `param = slider²`. First half covers useful range.
    /// Used for: contrast, dehaze, NR strength, LTM compression.
    SquareFromSlider,
    /// Factor with offset identity: slider 0–1 maps to factor 0–2, center=1.0=identity.
    /// Used for: saturation.
    FactorCentered,
    /// Not suitable for a simple slider (array, struct, enum).
    NotSlider,
}

/// A single parameter descriptor.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ParamDesc {
    /// Machine name (matches struct field name).
    pub name: &'static str,
    /// Human-readable label for UI.
    pub label: &'static str,
    /// Short tooltip/description.
    pub description: &'static str,
    /// Parameter type and range.
    pub kind: ParamKind,
    /// Display unit (e.g., "stops", "°", "×", "%", "px", "").
    pub unit: &'static str,
    /// Sub-group within the filter (e.g., "Main", "Advanced", "Masking").
    pub section: &'static str,
    /// How this parameter maps to a slider.
    pub slider: SliderMapping,
}

/// Parameter type, range, and default.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum ParamKind {
    /// Continuous float parameter.
    Float {
        min: f32,
        max: f32,
        default: f32,
        /// The value at which the filter has no effect.
        identity: f32,
        /// Suggested increment for arrow keys / scroll.
        step: f32,
    },
    /// Integer parameter (e.g., wavelet scales).
    Int { min: i32, max: i32, default: i32 },
    /// Boolean toggle.
    Bool { default: bool },
    /// Fixed-size float array (e.g., HSL 8-hue adjustments, BW mixer weights).
    FloatArray {
        len: usize,
        min: f32,
        max: f32,
        default: f32,
        labels: &'static [&'static str],
    },
}

/// Complete schema for one filter.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct FilterSchema {
    /// Machine name (e.g., "adaptive_sharpen").
    pub name: &'static str,
    /// Human-readable label (e.g., "Adaptive Sharpen").
    pub label: &'static str,
    /// One-line description.
    pub description: &'static str,
    /// UI category.
    pub group: FilterGroup,
    /// Parameter descriptors, in display order.
    pub params: &'static [ParamDesc],
}

/// Trait for filters that can describe themselves for UI generation.
///
/// Also provides get/set by parameter name for serialization and data binding.
pub trait Describe {
    /// Return the complete schema for this filter type.
    fn schema() -> &'static FilterSchema
    where
        Self: Sized;

    /// Get a parameter value by name. Returns `None` if name is unknown.
    fn get_param(&self, name: &str) -> Option<ParamValue>;

    /// Set a parameter value by name. Returns `false` if name is unknown.
    fn set_param(&mut self, name: &str, value: ParamValue) -> bool;

    /// Get all parameters as name-value pairs (for serialization).
    fn get_all_params(&self) -> alloc::vec::Vec<(&'static str, ParamValue)>
    where
        Self: Sized,
    {
        Self::schema()
            .params
            .iter()
            .filter_map(|p| self.get_param(p.name).map(|v| (p.name, v)))
            .collect()
    }

    /// Set parameters from name-value pairs (for deserialization).
    /// Returns the count of successfully set parameters.
    fn set_all_params(&mut self, params: &[(&str, ParamValue)]) -> usize {
        params
            .iter()
            .filter(|(name, value)| self.set_param(name, value.clone()))
            .count()
    }
}

/// A concrete parameter value for get/set operations.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ParamValue {
    Float(f32),
    Int(i32),
    Bool(bool),
    FloatArray(alloc::vec::Vec<f32>),
}

impl ParamValue {
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Self::Float(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Self::Int(v) => Some(*v),
            _ => None,
        }
    }
}
