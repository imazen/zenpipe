//! Zenode node definitions for all zenfilters operations.
//!
//! Each filter's parameters are mirrored as a `#[derive(Node)]` struct that
//! produces a `NodeSchema` with the same field names, types, and ranges as the
//! existing `FilterSchema` / `impl Describe`. These coexist -- the existing
//! `Describe` trait is not replaced (that is a future breaking change).
//!
//! Node IDs follow the pattern `zenfilters.<filter_name>`, matching the
//! `FilterSchema.name` field.
//!
//! Struct names match the existing filter struct names. The generated statics
//! follow the pattern `<SCREAMING_SNAKE>_NODE` (e.g., `EXPOSURE_NODE` for
//! struct `Exposure`). Access via `zenode_defs::EXPOSURE_NODE` etc.

use alloc::string::String;
use zennode::*;

// ═══════════════════════════════════════════════════════════════════
// TONE
// ═══════════════════════════════════════════════════════════════════

/// Exposure adjustment in photographic stops.
///
/// +1 stop doubles linear light, -1 halves it. Preserves hue and saturation
/// by scaling all Oklab channels proportionally.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenfilters.exposure", group = Tone, role = Filter)]
#[node(label = "Exposure")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(coalesce = "fused_adjust")]
pub struct Exposure {
    /// Exposure compensation in stops (+/-)
    ///
    /// Note: RIAPI `s.brightness` historically used a -1..1 sRGB offset, not
    /// photographic stops. The kv alias is provided for discoverability; callers
    /// should be aware of the different scale.
    #[param(range(-5.0..=5.0), default = 0.0, identity = 0.0, step = 0.1)]
    #[param(unit = "EV", section = "Main", slider = Linear)]
    #[kv("s.brightness")]
    pub stops: f32,
}

/// Power-curve contrast adjustment pivoted at middle grey.
///
/// Uses a power curve that pivots at the perceptual equivalent of 18.42%
/// middle grey in Oklab space. Positive values increase contrast.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenfilters.contrast", group = Tone, role = Filter)]
#[node(label = "Contrast")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(coalesce = "fused_adjust")]
pub struct Contrast {
    /// Contrast strength (positive = increase, negative = flatten)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = SquareFromSlider)]
    #[kv("s.contrast")]
    pub amount: f32,
}

/// Black point adjustment on Oklab L channel.
///
/// Remaps the shadow floor. A black point of 0.05 means values that were
/// L=0.05 become L=0.0, and the range is stretched accordingly.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenfilters.black_point", group = ToneRange, role = Filter)]
#[node(label = "Black Point")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(coalesce = "fused_adjust")]
pub struct BlackPoint {
    /// Black point level (0 = no change, 0.1 = crush bottom 10%)
    #[param(range(0.0..=0.5), default = 0.0, identity = 0.0, step = 0.01)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub level: f32,
}

/// White point adjustment on Oklab L channel.
///
/// Scales the L range so that `level` maps to L=1.0. Values < 1.0 brighten
/// highlights; values > 1.0 extend the dynamic range. Optional soft-clip
/// headroom compresses super-whites instead of hard clipping.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.white_point", group = ToneRange, role = Filter)]
#[node(label = "White Point")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(coalesce = "fused_adjust")]
pub struct WhitePoint {
    /// White point level (1.0 = no change, <1 = brighten highlights)
    #[param(range(0.5..=2.0), default = 1.0, identity = 1.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub level: f32,

    /// Soft-clip rolloff fraction above white point
    #[param(range(0.0..=0.5), default = 0.0, identity = 0.0, step = 0.01)]
    #[param(unit = "", section = "Advanced", slider = Linear)]
    pub headroom: f32,
}

impl Default for WhitePoint {
    fn default() -> Self {
        Self {
            level: 1.0,
            headroom: 0.0,
        }
    }
}

/// Whites and Blacks adjustment -- targeted luminance control for the extreme
/// ends of the histogram.
///
/// Unlike BlackPoint/WhitePoint (which remap the entire range), Whites/Blacks
/// apply a smooth, localized adjustment that matches Lightroom's Whites/Blacks
/// sliders.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenfilters.whites_blacks", group = ToneRange, role = Filter)]
#[node(label = "Whites / Blacks")]
#[node(format(preferred = OklabF32, alpha = Skip))]
pub struct WhitesBlacks {
    /// Whites adjustment (positive = brighten highlights)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub whites: f32,

    /// Blacks adjustment (positive = lift shadows)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub blacks: f32,
}

/// S-curve tone mapping with skew and chroma compression.
///
/// Uses the generalized sigmoid f(x) = x^c / (x^c + (1-x)^c). Contrast
/// controls steepness, skew shifts the midpoint, and chroma_compression
/// adapts saturation to luminance changes.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.sigmoid", group = Tone, role = Filter)]
#[node(label = "Sigmoid")]
#[node(format(preferred = OklabF32, alpha = Skip))]
pub struct Sigmoid {
    /// S-curve steepness (1 = identity, >1 = more contrast)
    #[param(range(0.5..=3.0), default = 1.0, identity = 1.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub contrast: f32,

    /// Midpoint bias (0.5 = symmetric, <0.5 = darken, >0.5 = brighten)
    #[param(range(0.1..=0.9), default = 0.5, identity = 0.5, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub skew: f32,

    /// How much chroma adapts to luminance changes (0 = L-only, 1 = full)
    #[param(range(0.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Advanced", slider = Linear)]
    pub chroma_compression: f32,
}

impl Default for Sigmoid {
    fn default() -> Self {
        Self {
            contrast: 1.0,
            skew: 0.5,
            chroma_compression: 0.0,
        }
    }
}

/// Arbitrary tone curve via control points with cubic spline interpolation
///
/// Control points define an input→output mapping on the L channel.
/// Points are encoded as a comma-separated string of "x:y" pairs,
/// e.g., "0.0:0.0,0.25:0.15,0.75:0.85,1.0:1.0".
/// The execution layer parses this and calls ToneCurve::from_points().
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.tone_curve", group = Tone, role = Filter)]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(tags("tone", "curve"))]
pub struct ToneCurve {
    /// Control points as "x:y" pairs, comma-separated.
    ///
    /// Each point is input_L:output_L in [0,1] range.
    /// Default is identity (diagonal line): "0:0,1:1".
    /// Example S-curve: "0:0,0.25:0.15,0.75:0.85,1:1".
    #[param(default = "0:0,1:1")]
    #[param(section = "Main", label = "Control Points", slider = NotSlider)]
    pub points: String,
}

impl Default for ToneCurve {
    fn default() -> Self {
        Self {
            points: alloc::string::String::from("0:0,1:1"),
        }
    }
}

/// Camera-matched basecurve tone mapping from darktable presets
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.basecurve_tonemap", group = ToneMap, role = Filter)]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(tags("tonemap", "camera", "basecurve"))]
pub struct BasecurveToneMap {
    /// Camera preset name (e.g., "nikon_d7000", "canon_eos_5d_mark_ii")
    #[param(default = "")]
    #[param(section = "Main", label = "Preset")]
    pub preset: String,

    /// Chroma compression strength (0=L-only, 1=full RGB-like desaturation)
    #[param(range(0.0..=1.0), default = 0.4, identity = 0.0, step = 0.05)]
    #[param(section = "Main")]
    pub chroma_compression: f32,
}

impl Default for BasecurveToneMap {
    fn default() -> Self {
        Self {
            preset: String::new(),
            chroma_compression: 0.4,
        }
    }
}

/// Parametric tone curve with 4 zone controls and 3 movable dividers.
///
/// Zone-based control similar to Lightroom's parametric tone curve panel.
/// Each zone slider pushes the curve up or down within its region.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenfilters.parametric_curve", group = Tone, role = Filter)]
#[node(label = "Parametric Curve")]
#[node(format(preferred = OklabF32, alpha = Skip))]
pub struct ParametricCurve {
    /// Shadows zone adjustment
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Zones", slider = Linear)]
    pub shadows: f32,

    /// Darks (lower midtones) zone adjustment
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Zones", slider = Linear)]
    pub darks: f32,

    /// Lights (upper midtones) zone adjustment
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Zones", slider = Linear)]
    pub lights: f32,

    /// Highlights zone adjustment
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Zones", slider = Linear)]
    pub highlights: f32,

    /// Boundary between shadows and darks zones
    #[param(range(0.05..=0.45), default = 0.25, identity = 0.25, step = 0.05)]
    #[param(unit = "", section = "Splits", slider = Linear)]
    pub split_shadows: f32,

    /// Boundary between darks and lights zones
    #[param(range(0.30..=0.75), default = 0.50, identity = 0.50, step = 0.05)]
    #[param(unit = "", section = "Splits", slider = Linear)]
    pub split_midtones: f32,

    /// Boundary between lights and highlights zones
    #[param(range(0.55..=0.95), default = 0.75, identity = 0.75, step = 0.05)]
    #[param(unit = "", section = "Splits", slider = Linear)]
    pub split_highlights: f32,
}

// ═══════════════════════════════════════════════════════════════════
// TONEMAP
// ═══════════════════════════════════════════════════════════════════

/// darktable-compatible sigmoid tone mapper.
///
/// Implements the generalized log-logistic sigmoid from darktable's sigmoid
/// module. Operates per-channel in linear RGB space (not Oklab).
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.dt_sigmoid", group = ToneMap, role = Filter)]
#[node(label = "DtSigmoid")]
#[node(format(preferred = LinearF32, alpha = Skip))]
pub struct DtSigmoid {
    /// Middle-grey contrast
    #[param(range(0.1..=10.0), default = 1.5, identity = 1.5, step = 0.1)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub contrast: f32,

    /// Contrast skewness (-1 to 1, 0 = symmetric)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub skew: f32,

    /// Hue preservation (0.0 = per-channel, 1.0 = full hue preservation)
    #[param(range(0.0..=1.0), default = 1.0, identity = 1.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub hue_preservation: f32,
}

impl Default for DtSigmoid {
    fn default() -> Self {
        Self {
            contrast: 1.5,
            skew: 0.0,
            hue_preservation: 1.0,
        }
    }
}

/// Input/output range remapping with gamma correction.
///
/// The classic Photoshop/Lightroom Levels dialog: clip input range, adjust
/// midtone gamma, and remap output range.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.levels", group = ToneMap, role = Filter)]
#[node(label = "Levels")]
#[node(format(preferred = OklabF32, alpha = Skip))]
pub struct Levels {
    /// Input black point (clip shadows)
    #[param(range(0.0..=1.0), default = 0.0, identity = 0.0, step = 0.01)]
    #[param(unit = "", section = "Input", slider = Linear)]
    pub in_black: f32,

    /// Input white point (clip highlights)
    #[param(range(0.0..=1.0), default = 1.0, identity = 1.0, step = 0.01)]
    #[param(unit = "", section = "Input", slider = Linear)]
    pub in_white: f32,

    /// Midtone adjustment (1 = linear, >1 = brighten, <1 = darken)
    #[param(range(0.1..=10.0), default = 1.0, identity = 1.0, step = 0.05)]
    #[param(unit = "", section = "Midtone", slider = Linear)]
    pub gamma: f32,

    /// Minimum output luminance
    #[param(range(0.0..=1.0), default = 0.0, identity = 0.0, step = 0.01)]
    #[param(unit = "", section = "Output", slider = Linear)]
    pub out_black: f32,

    /// Maximum output luminance
    #[param(range(0.0..=1.0), default = 1.0, identity = 1.0, step = 0.01)]
    #[param(unit = "", section = "Output", slider = Linear)]
    pub out_white: f32,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            in_black: 0.0,
            in_white: 1.0,
            gamma: 1.0,
            out_black: 0.0,
            out_white: 1.0,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// TONERANGE
// ═══════════════════════════════════════════════════════════════════

/// Targeted highlight recovery and shadow lift.
///
/// Positive highlights compresses bright areas (recovery). Positive shadows
/// lifts dark areas (fill light). Custom thresholds control where transitions
/// begin.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.highlights_shadows", group = ToneRange, role = Filter)]
#[node(label = "Highlights / Shadows")]
#[node(format(preferred = OklabF32, alpha = Skip))]
pub struct HighlightsShadows {
    /// Highlight recovery (positive = compress, negative = boost)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub highlights: f32,

    /// Shadow recovery (positive = lift, negative = deepen)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub shadows: f32,

    /// L value below which pixels are in the shadow zone
    #[param(range(0.05..=0.5), default = 0.3, identity = 0.3, step = 0.05)]
    #[param(unit = "", section = "Advanced", slider = Linear)]
    pub shadow_threshold: f32,

    /// L value above which pixels are in the highlight zone
    #[param(range(0.5..=0.95), default = 0.7, identity = 0.7, step = 0.05)]
    #[param(unit = "", section = "Advanced", slider = Linear)]
    pub highlight_threshold: f32,
}

impl Default for HighlightsShadows {
    fn default() -> Self {
        Self {
            highlights: 0.0,
            shadows: 0.0,
            shadow_threshold: 0.3,
            highlight_threshold: 0.7,
        }
    }
}

/// Automatic toe-curve recovery for crushed shadows.
///
/// Analyzes the L histogram to detect crushed shadow content, then applies
/// a proportional toe lift curve. Images with properly exposed shadows
/// are barely affected.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenfilters.shadow_lift", group = ToneRange, role = Filter)]
#[node(label = "Shadow Lift")]
#[node(format(preferred = OklabF32, alpha = Skip))]
pub struct ShadowLift {
    /// Lift strength (0 = off, 1 = full)
    #[param(range(0.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub strength: f32,
}

/// Automatic soft-clip recovery for blown highlights.
///
/// Analyzes the L histogram to detect blown highlight content, then applies
/// a proportional soft knee compression. Images with properly exposed
/// highlights are barely affected.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenfilters.highlight_recovery", group = ToneRange, role = Filter)]
#[node(label = "Highlight Recovery")]
#[node(format(preferred = OklabF32, alpha = Skip))]
pub struct HighlightRecovery {
    /// Recovery strength (0 = off, 1 = full)
    #[param(range(0.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub strength: f32,
}

// ═══════════════════════════════════════════════════════════════════
// COLOR
// ═══════════════════════════════════════════════════════════════════

/// Uniform chroma scaling on Oklab a/b channels.
///
/// Scales chroma by a constant factor. 1.0 = no change, 0.0 = grayscale,
/// 2.0 = double saturation.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.saturation", group = Color, role = Filter)]
#[node(label = "Saturation")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(coalesce = "fused_adjust")]
pub struct Saturation {
    /// Saturation multiplier (0 = grayscale, 1 = unchanged, 2 = double)
    #[param(range(0.0..=2.0), default = 1.0, identity = 1.0, step = 0.05)]
    #[param(unit = "\u{d7}", section = "Main", slider = FactorCentered)]
    #[kv("s.saturation")]
    pub factor: f32,
}

impl Default for Saturation {
    fn default() -> Self {
        Self { factor: 1.0 }
    }
}

/// Smart saturation that protects already-saturated colors.
///
/// Boosts chroma of low-saturation pixels more than high-saturation ones,
/// preventing skin tone and sky clipping.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.vibrance", group = Color, role = Filter)]
#[node(label = "Vibrance")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(coalesce = "fused_adjust")]
pub struct Vibrance {
    /// Vibrance boost (0 = off, 1 = full)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub amount: f32,

    /// Protection exponent for already-saturated colors
    #[param(range(0.5..=4.0), default = 2.0, identity = 2.0, step = 0.1)]
    #[param(unit = "", section = "Advanced", slider = Linear)]
    pub protection: f32,
}

impl Default for Vibrance {
    fn default() -> Self {
        Self {
            amount: 0.0,
            protection: 2.0,
        }
    }
}

/// Color temperature adjustment (warm/cool) via Oklab b shift.
///
/// Positive values warm the image (shift toward yellow/orange).
/// Negative values cool it (shift toward blue).
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenfilters.temperature", group = Color, role = Filter)]
#[node(label = "Temperature")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(coalesce = "fused_adjust")]
pub struct Temperature {
    /// Color temperature shift (negative = cool, positive = warm)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub shift: f32,
}

/// Green-magenta tint adjustment via Oklab a shift.
///
/// Positive values shift toward magenta, negative toward green.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenfilters.tint", group = Color, role = Filter)]
#[node(label = "Tint")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(coalesce = "fused_adjust")]
pub struct Tint {
    /// Tint shift (negative = green, positive = magenta)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub shift: f32,
}

/// Per-color hue, saturation, and luminance adjustment
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.hsl_adjust", group = Color, role = Filter)]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(tags("color", "hsl"))]
pub struct HslAdjust {
    /// Hue shift per color range in degrees
    #[param(range(-180.0..=180.0), default = 0.0, identity = 0.0, step = 1.0)]
    #[param(unit = "°", section = "Hue", slider = NotSlider)]
    #[param(labels(
        "Red", "Orange", "Yellow", "Green", "Cyan", "Blue", "Purple", "Magenta"
    ))]
    pub hue: [f32; 8],

    /// Saturation multiplier per color range
    #[param(range(0.0..=3.0), default = 1.0, identity = 1.0, step = 0.05)]
    #[param(unit = "×", section = "Saturation", slider = NotSlider)]
    #[param(labels(
        "Red", "Orange", "Yellow", "Green", "Cyan", "Blue", "Purple", "Magenta"
    ))]
    pub saturation: [f32; 8],

    /// Luminance offset per color range
    #[param(range(-0.5..=0.5), default = 0.0, identity = 0.0, step = 0.01)]
    #[param(section = "Luminance", slider = NotSlider)]
    #[param(labels(
        "Red", "Orange", "Yellow", "Green", "Cyan", "Blue", "Purple", "Magenta"
    ))]
    pub luminance: [f32; 8],
}

impl Default for HslAdjust {
    fn default() -> Self {
        Self {
            hue: [0.0; 8],
            saturation: [1.0; 8],
            luminance: [0.0; 8],
        }
    }
}

/// Three-way split-toning for shadows, midtones, and highlights.
///
/// Applies different color tints to shadows, midtones, and highlights
/// independently. Colors are specified as Oklab a/b offsets.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenfilters.color_grading", group = Color, role = Filter)]
#[node(label = "Color Grading")]
#[node(format(preferred = OklabF32, alpha = Skip))]
pub struct ColorGrading {
    /// Shadow tint: Oklab a offset (green-magenta)
    #[param(range(-0.1..=0.1), default = 0.0, identity = 0.0, step = 0.005)]
    #[param(unit = "", section = "Shadows", slider = Linear)]
    pub shadow_a: f32,

    /// Shadow tint: Oklab b offset (blue-yellow)
    #[param(range(-0.1..=0.1), default = 0.0, identity = 0.0, step = 0.005)]
    #[param(unit = "", section = "Shadows", slider = Linear)]
    pub shadow_b: f32,

    /// Midtone tint: Oklab a offset
    #[param(range(-0.1..=0.1), default = 0.0, identity = 0.0, step = 0.005)]
    #[param(unit = "", section = "Midtones", slider = Linear)]
    pub midtone_a: f32,

    /// Midtone tint: Oklab b offset
    #[param(range(-0.1..=0.1), default = 0.0, identity = 0.0, step = 0.005)]
    #[param(unit = "", section = "Midtones", slider = Linear)]
    pub midtone_b: f32,

    /// Highlight tint: Oklab a offset
    #[param(range(-0.1..=0.1), default = 0.0, identity = 0.0, step = 0.005)]
    #[param(unit = "", section = "Highlights", slider = Linear)]
    pub highlight_a: f32,

    /// Highlight tint: Oklab b offset
    #[param(range(-0.1..=0.1), default = 0.0, identity = 0.0, step = 0.005)]
    #[param(unit = "", section = "Highlights", slider = Linear)]
    pub highlight_b: f32,

    /// Balance: shifts the shadow/highlight boundary
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub balance: f32,
}

/// Camera calibration -- primary color hue and saturation calibration
/// with shadow tint.
///
/// Equivalent to Lightroom's Camera Calibration panel. Adjusts how the
/// camera's RGB primaries map to final color.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.camera_calibration", group = Color, role = Filter)]
#[node(label = "Camera Calibration")]
#[node(format(preferred = OklabF32, alpha = Skip))]
pub struct CameraCalibration {
    /// Red primary hue shift
    #[param(range(-60.0..=60.0), default = 0.0, identity = 0.0, step = 1.0)]
    #[param(unit = "\u{b0}", section = "Red Primary", slider = Linear)]
    pub red_hue: f32,

    /// Red primary saturation scale
    #[param(range(0.0..=3.0), default = 1.0, identity = 1.0, step = 0.05)]
    #[param(unit = "\u{d7}", section = "Red Primary", slider = Linear)]
    pub red_saturation: f32,

    /// Green primary hue shift
    #[param(range(-60.0..=60.0), default = 0.0, identity = 0.0, step = 1.0)]
    #[param(unit = "\u{b0}", section = "Green Primary", slider = Linear)]
    pub green_hue: f32,

    /// Green primary saturation scale
    #[param(range(0.0..=3.0), default = 1.0, identity = 1.0, step = 0.05)]
    #[param(unit = "\u{d7}", section = "Green Primary", slider = Linear)]
    pub green_saturation: f32,

    /// Blue primary hue shift
    #[param(range(-60.0..=60.0), default = 0.0, identity = 0.0, step = 1.0)]
    #[param(unit = "\u{b0}", section = "Blue Primary", slider = Linear)]
    pub blue_hue: f32,

    /// Blue primary saturation scale
    #[param(range(0.0..=3.0), default = 1.0, identity = 1.0, step = 0.05)]
    #[param(unit = "\u{d7}", section = "Blue Primary", slider = Linear)]
    pub blue_saturation: f32,

    /// Shadow tint: green-magenta balance in shadows
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Shadow", slider = Linear)]
    pub shadow_tint: f32,
}

impl Default for CameraCalibration {
    fn default() -> Self {
        Self {
            red_hue: 0.0,
            red_saturation: 1.0,
            green_hue: 0.0,
            green_saturation: 1.0,
            blue_hue: 0.0,
            blue_saturation: 1.0,
            shadow_tint: 0.0,
        }
    }
}

/// ASC CDL (Color Decision List) -- industry-standard per-channel
/// slope/offset/power correction with global saturation.
///
/// Formula per channel: `out = clamp(pow(max(slope * in + offset, 0), power), 0, 1)`
/// Applied in linear RGB space with Oklab round-trip.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.asc_cdl", group = Color, role = Filter)]
#[node(label = "ASC CDL")]
#[node(format(preferred = OklabF32, alpha = Skip))]
pub struct AscCdl {
    /// Red channel gain
    #[param(range(0.0..=4.0), default = 1.0, identity = 1.0, step = 0.01)]
    #[param(unit = "×", section = "Slope", slider = Linear)]
    pub slope_r: f32,

    /// Green channel gain
    #[param(range(0.0..=4.0), default = 1.0, identity = 1.0, step = 0.01)]
    #[param(unit = "×", section = "Slope", slider = Linear)]
    pub slope_g: f32,

    /// Blue channel gain
    #[param(range(0.0..=4.0), default = 1.0, identity = 1.0, step = 0.01)]
    #[param(unit = "×", section = "Slope", slider = Linear)]
    pub slope_b: f32,

    /// Red channel offset (lift)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.005)]
    #[param(unit = "", section = "Offset", slider = Linear)]
    pub offset_r: f32,

    /// Green channel offset (lift)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.005)]
    #[param(unit = "", section = "Offset", slider = Linear)]
    pub offset_g: f32,

    /// Blue channel offset (lift)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.005)]
    #[param(unit = "", section = "Offset", slider = Linear)]
    pub offset_b: f32,

    /// Red channel gamma
    #[param(range(0.1..=4.0), default = 1.0, identity = 1.0, step = 0.01)]
    #[param(unit = "", section = "Power", slider = Linear)]
    pub power_r: f32,

    /// Green channel gamma
    #[param(range(0.1..=4.0), default = 1.0, identity = 1.0, step = 0.01)]
    #[param(unit = "", section = "Power", slider = Linear)]
    pub power_g: f32,

    /// Blue channel gamma
    #[param(range(0.1..=4.0), default = 1.0, identity = 1.0, step = 0.01)]
    #[param(unit = "", section = "Power", slider = Linear)]
    pub power_b: f32,

    /// Global saturation (0 = mono, 1 = unchanged)
    #[param(range(0.0..=4.0), default = 1.0, identity = 1.0, step = 0.05)]
    #[param(unit = "×", section = "Main", slider = Linear)]
    pub saturation: f32,
}

impl Default for AscCdl {
    fn default() -> Self {
        Self {
            slope_r: 1.0,
            slope_g: 1.0,
            slope_b: 1.0,
            offset_r: 0.0,
            offset_g: 0.0,
            offset_b: 0.0,
            power_r: 1.0,
            power_g: 1.0,
            power_b: 1.0,
            saturation: 1.0,
        }
    }
}

/// 3D color lookup table loaded from Adobe .cube format.
///
/// The universal LUT exchange format. Maps linear RGB → linear RGB via
/// trilinear interpolation on a uniform 3D grid. Typical sizes: 17³, 33³, 65³.
///
/// The LUT data itself is loaded via `CubeLut::parse()` or `CubeLut::identity()`.
/// This node exposes only the blend strength parameter.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenfilters.cube_lut", group = Color, role = Filter)]
#[node(label = "Cube LUT")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(tags("color", "lut", "grading"))]
pub struct CubeLut {
    /// Blend strength (0 = bypass, 1 = full LUT)
    #[param(range(0.0..=1.0), default = 1.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub strength: f32,
}

/// DaVinci Resolve-style hue-qualified curves.
///
/// Four independent 1D curves for targeted per-hue and per-luminance control:
/// - Hue vs Saturation: per-hue chroma multiplier
/// - Hue vs Hue: per-hue hue offset
/// - Hue vs Luminance: per-hue luminance offset
/// - Luminance vs Saturation: per-luminance chroma multiplier
///
/// Curves are set programmatically via control points or raw LUTs.
/// Oklab's perceptually uniform hue eliminates the skew artifacts
/// inherent in HSL-based implementations.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenfilters.hue_curves", group = Color, role = Filter)]
#[node(label = "Hue Curves")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(tags("color", "curves", "grading", "hue"))]
pub struct HueCurves {}

/// Film look presets using tensor-compressed 3D LUTs.
///
/// 10 built-in mathematical film emulations, each ~5 KB. Select by
/// preset name; strength blends between original and graded result.
///
/// Presets: bleach_bypass, cross_process, teal_orange, faded_film,
/// golden_hour, cool_chrome, print_film, noir, technicolor, matte.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.film_look", group = Color, role = Filter)]
#[node(label = "Film Look")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(tags("color", "grading", "film", "lut"))]
pub struct FilmLook {
    /// Preset name (e.g. "teal_orange", "bleach_bypass")
    #[param(default = "faded_film", section = "Main")]
    pub preset: String,

    /// Blend strength (0 = bypass, 1 = full effect)
    #[param(range(0.0..=1.0), default = 1.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub strength: f32,
}

impl Default for FilmLook {
    fn default() -> Self {
        Self {
            preset: String::from("faded_film"),
            strength: 1.0,
        }
    }
}

/// Grayscale conversion with per-color luminance weights
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.bw_mixer", group = Color, role = Filter)]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(tags("color", "grayscale", "bw"))]
pub struct BwMixer {
    /// Weight per color range (proportional to chroma)
    #[param(range(0.0..=2.0), default = 1.0, identity = 1.0, step = 0.05)]
    #[param(unit = "×", section = "Main", slider = NotSlider)]
    #[param(labels(
        "Red", "Orange", "Yellow", "Green", "Cyan", "Blue", "Purple", "Magenta"
    ))]
    pub weights: [f32; 8],
}

impl Default for BwMixer {
    fn default() -> Self {
        Self { weights: [1.0; 8] }
    }
}

/// Convert to grayscale by zeroing chroma channels.
///
/// In Oklab, grayscale means a=0, b=0. The perceived luminance is already
/// encoded in the L channel, so there is no information loss.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.grayscale", group = Color, role = Filter)]
#[node(label = "Grayscale")]
#[node(format(preferred = OklabF32, alpha = Skip))]
pub struct Grayscale {
    /// Grayscale algorithm. All produce identical results in Oklab space
    /// (zero chroma), but different luma coefficients when applied in sRGB.
    /// Values: "oklab" (default), "ntsc", "bt709", "flat", "ry"
    #[param(default = "oklab")]
    #[param(section = "Main", label = "Algorithm")]
    #[kv("s.grayscale")]
    pub algorithm: String,
}

impl Default for Grayscale {
    fn default() -> Self {
        Self {
            algorithm: String::from("oklab"),
        }
    }
}

/// Hue rotation in Oklab a/b plane.
///
/// Rotates colors around the hue circle by the specified angle in degrees.
/// Preserves lightness and chroma.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenfilters.hue_rotate", group = Color, role = Filter)]
#[node(label = "Hue Rotate")]
#[node(format(preferred = OklabF32, alpha = Skip))]
pub struct HueRotate {
    /// Rotation angle in degrees
    #[param(range(-180.0..=180.0), default = 0.0, identity = 0.0, step = 5.0)]
    #[param(unit = "\u{b0}", section = "Main", slider = Linear)]
    pub degrees: f32,
}

/// Sepia tone effect in perceptual Oklab space.
///
/// Desaturates the image, then applies a warm brown tint by shifting
/// the a and b channels toward the sepia point.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.sepia", group = Color, role = Filter)]
#[node(label = "Sepia")]
#[node(format(preferred = OklabF32, alpha = Skip))]
pub struct Sepia {
    /// Sepia strength (0 = grayscale, 1 = full sepia)
    #[param(range(0.0..=1.0), default = 1.0, identity = 1.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    #[kv("s.sepia")]
    pub amount: f32,
}

impl Default for Sepia {
    fn default() -> Self {
        Self { amount: 1.0 }
    }
}

// ═══════════════════════════════════════════════════════════════════
// DETAIL
// ═══════════════════════════════════════════════════════════════════

/// Multi-scale local contrast enhancement on L channel.
///
/// Uses a two-band decomposition to isolate the mid-frequency "clarity"
/// band, avoiding both noise amplification and halos.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.clarity", group = Detail, role = Filter)]
#[node(label = "Clarity")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(neighborhood)]
pub struct Clarity {
    /// Fine-scale blur sigma (coarse blur is 4x this)
    #[param(range(1.0..=16.0), default = 4.0, identity = 4.0, step = 0.5)]
    #[param(unit = "px", section = "Main", slider = Linear)]
    pub sigma: f32,

    /// Enhancement amount (positive = enhance, negative = soften)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub amount: f32,
}

impl Default for Clarity {
    fn default() -> Self {
        Self {
            sigma: 4.0,
            amount: 0.0,
        }
    }
}

/// Adaptive local contrast based on local average luminance.
///
/// Unlike clarity, brilliance adjusts each pixel relative to its local
/// average -- lifting shadows and compressing highlights selectively.
/// Produces natural dynamic range compression similar to Apple's
/// Brilliance slider.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.brilliance", group = Detail, role = Filter)]
#[node(label = "Brilliance")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(neighborhood)]
pub struct Brilliance {
    /// Blur sigma for computing local average
    #[param(range(2.0..=50.0), default = 10.0, identity = 10.0, step = 1.0)]
    #[param(unit = "px", section = "Main", slider = Linear)]
    pub sigma: f32,

    /// Overall effect strength
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub amount: f32,

    /// Shadow lift strength
    #[param(range(0.0..=1.0), default = 0.6, identity = 0.6, step = 0.05)]
    #[param(unit = "", section = "Advanced", slider = Linear)]
    pub shadow_strength: f32,

    /// Highlight compression strength
    #[param(range(0.0..=1.0), default = 0.4, identity = 0.4, step = 0.05)]
    #[param(unit = "", section = "Advanced", slider = Linear)]
    pub highlight_strength: f32,
}

impl Default for Brilliance {
    fn default() -> Self {
        Self {
            sigma: 10.0,
            amount: 0.0,
            shadow_strength: 0.6,
            highlight_strength: 0.4,
        }
    }
}

/// Noise-gated sharpening with detail and masking controls.
///
/// Measures local texture energy and only sharpens where there is actual
/// detail to enhance, leaving flat areas (sky, skin) unaffected.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.adaptive_sharpen", group = Detail, role = Filter)]
#[node(label = "Adaptive Sharpen")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(neighborhood)]
pub struct AdaptiveSharpen {
    /// Sharpening strength
    #[param(range(0.0..=2.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "\u{d7}", section = "Main", slider = Linear)]
    pub amount: f32,

    /// Detail extraction scale (smaller = finer detail)
    #[param(range(0.5..=3.0), default = 1.0, identity = 1.0, step = 0.1)]
    #[param(unit = "px", section = "Main", slider = Linear)]
    pub sigma: f32,

    /// Edge-only (0) to full detail (1) sharpening
    #[param(range(0.0..=1.0), default = 0.5, identity = 0.5, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub detail: f32,

    /// Restrict sharpening to stronger edges
    #[param(range(0.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Masking", slider = Linear)]
    pub masking: f32,

    /// Threshold below which detail is treated as noise
    #[param(range(0.001..=0.02), default = 0.005, identity = 0.005, step = 0.001)]
    #[param(unit = "", section = "Advanced", slider = SquareFromSlider)]
    pub noise_floor: f32,
}

impl Default for AdaptiveSharpen {
    fn default() -> Self {
        Self {
            amount: 0.0,
            sigma: 1.0,
            detail: 0.5,
            masking: 0.0,
            noise_floor: 0.005,
        }
    }
}

/// Unsharp mask sharpening on L channel.
///
/// Like clarity but with a smaller sigma for fine detail enhancement.
/// Sharpening in Oklab L avoids color fringing at high-contrast edges.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.sharpen", group = Detail, role = Filter)]
#[node(label = "Sharpen")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(neighborhood)]
pub struct Sharpen {
    /// Blur sigma for detail extraction
    #[param(range(0.5..=3.0), default = 1.0, identity = 1.0, step = 0.1)]
    #[param(unit = "px", section = "Main", slider = Linear)]
    pub sigma: f32,

    /// Sharpening strength
    #[param(range(0.0..=2.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "\u{d7}", section = "Main", slider = Linear)]
    pub amount: f32,
}

impl Default for Sharpen {
    fn default() -> Self {
        Self {
            sigma: 1.0,
            amount: 0.0,
        }
    }
}

/// Wavelet-based luminance and chroma noise reduction.
///
/// Uses an a trous wavelet decomposition with soft thresholding. Chroma
/// denoising uses a higher effective threshold since chroma noise is
/// typically more objectionable.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.noise_reduction", group = Detail, role = Filter)]
#[node(label = "Noise Reduction")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(neighborhood)]
pub struct NoiseReduction {
    /// Luminance noise reduction strength
    #[param(range(0.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = SquareFromSlider)]
    pub luminance: f32,

    /// Chroma noise reduction strength
    #[param(range(0.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = SquareFromSlider)]
    pub chroma: f32,

    /// Luminance detail preservation (higher = keep more detail)
    #[param(range(0.0..=1.0), default = 0.5, identity = 0.5, step = 0.05)]
    #[param(unit = "", section = "Advanced", slider = Linear)]
    pub detail: f32,

    /// Luminance contrast preservation in denoised areas
    #[param(range(0.0..=1.0), default = 0.5, identity = 0.5, step = 0.05)]
    #[param(unit = "", section = "Advanced", slider = Linear)]
    pub luminance_contrast: f32,

    /// Chroma detail preservation (higher = keep more color detail)
    #[param(range(0.0..=1.0), default = 0.5, identity = 0.5, step = 0.05)]
    #[param(unit = "", section = "Advanced", slider = Linear)]
    pub chroma_detail: f32,

    /// Number of wavelet scales
    #[param(range(1..=6), default = 4)]
    #[param(unit = "", section = "Advanced", slider = NotSlider)]
    pub scales: i32,
}

impl Default for NoiseReduction {
    fn default() -> Self {
        Self {
            luminance: 0.0,
            chroma: 0.0,
            detail: 0.5,
            luminance_contrast: 0.5,
            chroma_detail: 0.5,
            scales: 4,
        }
    }
}

/// Fine detail contrast enhancement (smaller scale than clarity).
///
/// Similar to Clarity but targets higher-frequency detail like skin pores,
/// fabric weave, and individual leaves. Mirrors Lightroom's Texture slider.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.texture", group = Detail, role = Filter)]
#[node(label = "Texture")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(neighborhood)]
pub struct Texture {
    /// Fine-scale blur sigma (coarse blur is 2x this)
    #[param(range(0.5..=8.0), default = 1.5, identity = 1.5, step = 0.5)]
    #[param(unit = "px", section = "Main", slider = Linear)]
    pub sigma: f32,

    /// Enhancement amount (positive = sharpen, negative = soften)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub amount: f32,
}

impl Default for Texture {
    fn default() -> Self {
        Self {
            sigma: 1.5,
            amount: 0.0,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// EFFECTS
// ═══════════════════════════════════════════════════════════════════

/// Post-crop vignette: darken or lighten image edges.
///
/// Applies a radial falloff from center to edges. Positive strength darkens
/// edges (classic vignette), negative brightens.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.vignette", group = Effects, role = Filter)]
#[node(label = "Vignette")]
#[node(format(preferred = OklabF32, alpha = Skip))]
pub struct Vignette {
    /// Vignette strength (positive = darken edges, negative = brighten)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub strength: f32,

    /// Distance from center where effect starts (0 = center, 1 = corners)
    #[param(range(0.0..=1.0), default = 0.5, identity = 0.5, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub midpoint: f32,

    /// Transition softness (0 = hard, 1 = very soft)
    #[param(range(0.0..=1.0), default = 0.5, identity = 0.5, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub feather: f32,

    /// Shape (1 = circular, 0 = rectangular)
    #[param(range(0.0..=1.0), default = 1.0, identity = 1.0, step = 0.05)]
    #[param(unit = "", section = "Shape", slider = Linear)]
    pub roundness: f32,
}

impl Default for Vignette {
    fn default() -> Self {
        Self {
            strength: 0.0,
            midpoint: 0.5,
            feather: 0.5,
            roundness: 1.0,
        }
    }
}

/// Soft glow from bright areas via screen blending.
///
/// Extracts pixels above a luminance threshold, blurs them with a large
/// Gaussian kernel, and adds the result back. Produces natural-looking
/// soft glow around bright light sources.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.bloom", group = Effects, role = Filter)]
#[node(label = "Bloom")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(neighborhood)]
pub struct Bloom {
    /// Luminance threshold for bloom contribution
    #[param(range(0.0..=1.0), default = 0.7, identity = 0.7, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub threshold: f32,

    /// Bloom spread (larger = softer, wider glow)
    #[param(range(2.0..=100.0), default = 20.0, identity = 20.0, step = 1.0)]
    #[param(unit = "px", section = "Main", slider = Linear)]
    pub sigma: f32,

    /// Bloom intensity (0 = off, 1 = full)
    #[param(range(0.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub amount: f32,
}

impl Default for Bloom {
    fn default() -> Self {
        Self {
            threshold: 0.7,
            sigma: 20.0,
            amount: 0.0,
        }
    }
}

/// Film grain simulation with luminance-adaptive response.
///
/// Adds synthetic grain to the luminance channel. Grain intensity varies
/// with luminance: stronger in midtones, weaker in deep shadows and bright
/// highlights, like real film.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenfilters.grain", group = Effects, role = Filter)]
#[node(label = "Grain")]
#[node(format(preferred = OklabF32, alpha = Skip))]
pub struct Grain {
    /// Grain intensity (0 = none, 1 = heavy)
    #[param(range(0.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub amount: f32,

    /// Grain spatial frequency (1 = fine, 2+ = coarser)
    #[param(range(1.0..=5.0), default = 1.0, identity = 1.0, step = 0.5)]
    #[param(unit = "px", section = "Main", slider = Linear)]
    pub size: f32,

    /// Random seed for grain pattern
    #[param(range(0..=65535), default = 0)]
    #[param(unit = "", section = "Main", slider = NotSlider)]
    pub seed: i32,
}

/// Spatially-adaptive haze removal using dark channel prior.
///
/// Uses a dark channel prior analog in Oklab space to estimate and
/// remove atmospheric haze. Hazy regions get strong correction while
/// clear regions are barely affected.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenfilters.dehaze", group = Effects, role = Filter)]
#[node(label = "Dehaze")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(coalesce = "fused_adjust")]
#[node(neighborhood)]
pub struct Dehaze {
    /// Dehaze correction strength
    #[param(range(0.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = SquareFromSlider)]
    pub strength: f32,
}

/// Color inversion in Oklab space.
///
/// Inverts lightness (L' = 1.0 - L) and negates chroma (a' = -a, b' = -b).
/// Produces a perceptually correct negative.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.invert", group = Effects, role = Filter)]
#[node(label = "Invert")]
#[node(format(preferred = OklabF32, alpha = Skip))]
pub struct Invert {
    /// Enable/disable. Always true when the node is present.
    /// Exists to enable RIAPI s.invert=true querystring support.
    #[param(default = true)]
    #[param(section = "Main", label = "Enable")]
    #[kv("s.invert")]
    pub enabled: bool,
}

impl Default for Invert {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Alpha channel scaling for transparency adjustment.
///
/// Multiplies all alpha values by a constant factor. Useful for fade effects
/// or global opacity changes. If no alpha channel exists, this is a no-op.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.alpha", group = Effects, role = Filter)]
#[node(label = "Alpha")]
#[node(format(preferred = OklabF32, alpha = ModifyAlpha))]
pub struct Alpha {
    /// Alpha multiplier (0 = fully transparent, 1 = unchanged)
    #[param(range(0.0..=1.0), default = 1.0, identity = 1.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    #[kv("s.alpha")]
    pub factor: f32,
}

impl Default for Alpha {
    fn default() -> Self {
        Self { factor: 1.0 }
    }
}

/// 5x5 color matrix applied in linear RGB space.
///
/// Transforms `[R, G, B, A, 1]` -> `[R', G', B', A', 1]` using a row-major
/// 5x5 matrix (25 elements). The 5th column is the bias/offset. The filter
/// converts Oklab -> linear RGB, applies the matrix, then converts back.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.color_matrix", group = Color, role = Filter)]
#[node(label = "Color Matrix")]
#[node(format(preferred = OklabF32, alpha = Process))]
pub struct ColorMatrix {
    /// Row-major 5x5 color matrix (25 floats)
    #[param(range(-10.0..=10.0), default = 0.0, identity = 0.0, step = 0.01)]
    #[param(unit = "", section = "Main", slider = NotSlider)]
    pub matrix: [f32; 25],
}

impl Default for ColorMatrix {
    fn default() -> Self {
        Self {
            matrix: crate::filters::ColorMatrix::IDENTITY,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// AUTO
// ═══════════════════════════════════════════════════════════════════

/// Automatic exposure correction by normalizing to a target middle grey.
///
/// Measures the geometric mean of L (log-average luminance) and applies
/// exposure correction to bring it to the target. The geometric mean is
/// robust against small bright areas that would bias an arithmetic mean.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.auto_exposure", group = Auto, role = Filter)]
#[node(label = "Auto Exposure")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(tags("auto", "exposure", "normalize"))]
pub struct AutoExposureDef {
    /// Correction strength (0 = off, 1 = full correction to target)
    #[param(range(0.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub strength: f32,

    /// Target middle grey in Oklab L
    #[param(range(0.2..=0.8), default = 0.5, identity = 0.5, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub target: f32,

    /// Maximum correction in stops (prevents extreme adjustments)
    #[param(range(0.5..=5.0), default = 2.0, identity = 2.0, step = 0.5)]
    #[param(unit = "EV", section = "Advanced", slider = Linear)]
    pub max_correction: f32,
}

impl Default for AutoExposureDef {
    fn default() -> Self {
        Self {
            strength: 0.0,
            target: 0.5,
            max_correction: 2.0,
        }
    }
}

/// Auto levels: stretch the luminance histogram to fill [0, 1].
///
/// Scans the L plane to find cutoff points, then remaps luminance so the
/// low cutoff maps to 0 and the high cutoff maps to 1. Equivalent to
/// ImageMagick `-auto-level`, with smart outlier-resistant plateau detection,
/// optional midpoint gamma correction, chroma scaling, and cast removal.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.auto_levels", group = Auto, role = Filter)]
#[node(label = "Auto Levels")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(tags("auto", "levels", "normalize", "histogram", "stretch"))]
pub struct AutoLevelsDef {
    /// Fraction of pixels to clip at the dark end (0 = smart plateau detection)
    #[param(range(0.0..=0.1), default = 0.0, identity = 0.0, step = 0.005)]
    #[param(unit = "", section = "Range", slider = Linear)]
    pub clip_low: f32,

    /// Fraction of pixels to clip at the bright end (0 = smart plateau detection)
    #[param(range(0.0..=0.1), default = 0.0, identity = 0.0, step = 0.005)]
    #[param(unit = "", section = "Range", slider = Linear)]
    pub clip_high: f32,

    /// Move the median luminance to this value via gamma correction (0 = off)
    #[param(range(0.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Tone", slider = Linear)]
    pub target_midpoint: f32,

    /// Scale a/b channels by the same factor as L (raises saturation on stretch)
    #[param(default = false)]
    #[param(section = "Color", label = "Scale Chroma")]
    pub scale_chroma: bool,

    /// Subtract mean(a) and mean(b) to neutralize color cast
    #[param(default = false)]
    #[param(section = "Color", label = "Remove Color Cast")]
    pub remove_cast: bool,

    /// Blend strength (0 = off, 1 = full stretch)
    #[param(range(0.0..=1.0), default = 1.0, identity = 1.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub strength: f32,
}

impl Default for AutoLevelsDef {
    fn default() -> Self {
        Self {
            clip_low: 0.0,
            clip_high: 0.0,
            target_midpoint: 0.0,
            scale_chroma: false,
            remove_cast: false,
            strength: 1.0,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// ADDITIONAL DETAIL
// ═══════════════════════════════════════════════════════════════════

/// Edge-preserving smoothing via guided filter.
///
/// Uses a guided filter (He et al., TPAMI 2013) with L as the guide image.
/// O(1) per pixel regardless of radius. Produces locally-linear output that
/// preserves edges from the luminance channel while smoothing noise in all
/// three channels.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.bilateral", group = Detail, role = Filter)]
#[node(label = "Bilateral Filter")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(neighborhood)]
#[node(tags("smooth", "denoise", "edge-preserving"))]
pub struct BilateralDef {
    /// Smoothing window size (spatial sigma)
    #[param(range(0.5..=20.0), default = 2.0, identity = 2.0, step = 0.5)]
    #[param(unit = "px", section = "Main", slider = Linear)]
    pub spatial_sigma: f32,

    /// Edge preservation parameter (smaller = sharper edges)
    #[param(range(0.001..=0.5), default = 0.1, identity = 0.1, step = 0.01)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub range_sigma: f32,

    /// Blend strength (0 = off, 1 = full smoothing)
    #[param(range(0.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub strength: f32,
}

impl Default for BilateralDef {
    fn default() -> Self {
        Self {
            spatial_sigma: 2.0,
            range_sigma: 0.1,
            strength: 0.0,
        }
    }
}

/// Full-image Gaussian blur across all Oklab channels.
///
/// Unlike the L-only blur used internally by clarity/sharpen, this blurs
/// the entire image (L, a, b, and alpha). Blurring in Oklab avoids the
/// darkening artifacts that sRGB gamma-space blurs produce at color boundaries.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenfilters.blur", group = Detail, role = Filter)]
#[node(label = "Blur")]
#[node(format(preferred = OklabF32, alpha = Process))]
#[node(neighborhood)]
#[node(tags("blur", "smooth", "gaussian"))]
pub struct BlurDef {
    /// Gaussian sigma in pixels (larger = more blur)
    #[param(range(0.0..=100.0), default = 0.0, identity = 0.0, step = 0.5)]
    #[param(unit = "\u{3c3}", section = "Main", slider = Linear)]
    pub sigma: f32,
}

/// Per-channel tone curves applied independently to R, G, B in sRGB space.
///
/// Unlike ToneCurve which operates on Oklab L (preserving color ratios),
/// ChannelCurves enables independent tonal correction of each color channel.
/// Each channel has its own 256-entry LUT mapping sRGB [0,1] to [0,1].
///
/// The node accepts control points as comma-separated "x:y" pairs per channel.
/// Default is identity: "0:0,1:1".
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.channel_curves", group = Color, role = Filter)]
#[node(label = "Channel Curves")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(tags("color", "curves", "channel", "rgb"))]
pub struct ChannelCurvesDef {
    /// Red channel control points as "x:y" pairs, comma-separated
    #[param(default = "0:0,1:1")]
    #[param(section = "Red", label = "Red Curve", slider = NotSlider)]
    pub red_points: String,

    /// Green channel control points as "x:y" pairs, comma-separated
    #[param(default = "0:0,1:1")]
    #[param(section = "Green", label = "Green Curve", slider = NotSlider)]
    pub green_points: String,

    /// Blue channel control points as "x:y" pairs, comma-separated
    #[param(default = "0:0,1:1")]
    #[param(section = "Blue", label = "Blue Curve", slider = NotSlider)]
    pub blue_points: String,
}

impl Default for ChannelCurvesDef {
    fn default() -> Self {
        Self {
            red_points: String::from("0:0,1:1"),
            green_points: String::from("0:0,1:1"),
            blue_points: String::from("0:0,1:1"),
        }
    }
}

/// Lateral chromatic aberration correction.
///
/// Corrects color fringing at image edges caused by lens dispersion.
/// In Oklab, CA manifests as radial displacement of the a (green-red)
/// and b (blue-yellow) planes relative to L. Shifts chroma planes
/// radially to re-align them with luminance.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenfilters.chromatic_aberration", group = Effects, role = Filter)]
#[node(label = "Chromatic Aberration")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(tags("lens", "correction", "fringing"))]
pub struct ChromaticAberrationDef {
    /// Radial shift for the a (green-red) channel
    #[param(range(-0.02..=0.02), default = 0.0, identity = 0.0, step = 0.001)]
    #[param(unit = "", section = "Main", label = "Green-Red Shift", slider = Linear)]
    pub shift_a: f32,

    /// Radial shift for the b (blue-yellow) channel
    #[param(range(-0.02..=0.02), default = 0.0, identity = 0.0, step = 0.001)]
    #[param(unit = "", section = "Main", label = "Blue-Yellow Shift", slider = Linear)]
    pub shift_b: f32,
}

/// Lens vignetting correction (devignette).
///
/// Compensates for the natural light falloff at the edges of a lens.
/// Applies a radial brightness correction that increases toward the corners,
/// based on the cos^4 law of illumination falloff.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.devignette", group = Effects, role = Filter)]
#[node(label = "Devignette")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(tags("lens", "correction", "vignette"))]
pub struct DevignetteDef {
    /// Correction strength (1 = full cos^4 compensation)
    #[param(range(0.0..=2.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub strength: f32,

    /// Falloff exponent (4 = cos^4 law, higher = corners only)
    #[param(range(1.0..=8.0), default = 4.0, identity = 4.0, step = 0.5)]
    #[param(unit = "", section = "Advanced", slider = Linear)]
    pub exponent: f32,
}

impl Default for DevignetteDef {
    fn default() -> Self {
        Self {
            strength: 0.0,
            exponent: 4.0,
        }
    }
}

/// Edge detection on the L (lightness) channel.
///
/// Replaces L with gradient magnitude (Sobel/Laplacian) or binary edges (Canny),
/// normalized to [0, 1]. Chroma channels are zeroed to produce a grayscale
/// edge map.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.edge_detect", group = Detail, role = Filter)]
#[node(label = "Edge Detect")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(neighborhood)]
#[node(tags("edge", "detect", "sobel", "canny"))]
pub struct EdgeDetectDef {
    /// Detection mode (0 = Sobel, 1 = Laplacian, 2 = Canny)
    #[param(range(0..=2), default = 0)]
    #[param(unit = "", section = "Main", slider = NotSlider)]
    pub mode: i32,

    /// Sobel/Laplacian: output scaling. Canny: Gaussian blur sigma.
    #[param(range(0.1..=5.0), default = 1.0, identity = 1.0, step = 0.1)]
    #[param(unit = "\u{d7}", section = "Main", slider = Linear)]
    pub strength: f32,
}

impl Default for EdgeDetectDef {
    fn default() -> Self {
        Self {
            mode: 0,
            strength: 1.0,
        }
    }
}

/// Fused per-pixel adjustment: applies all per-pixel operations in a single
/// pass over the data, avoiding repeated plane traversal.
///
/// Equivalent to chaining Exposure + Contrast + BlackPoint + WhitePoint +
/// Saturation + Temperature + Tint + HighlightsShadows + Dehaze + Vibrance,
/// but runs ~3x faster because it only scans the planes once.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.fused_adjust", group = Tone, role = Filter)]
#[node(label = "Fused Adjust")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(coalesce = "fused_adjust")]
#[node(tags("fused", "adjust", "exposure", "contrast", "saturation"))]
pub struct FusedAdjustDef {
    /// Exposure in stops
    #[param(range(-5.0..=5.0), default = 0.0, identity = 0.0, step = 0.1)]
    #[param(unit = "EV", section = "Tone", slider = Linear)]
    pub exposure: f32,

    /// Contrast (-1 to 1)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Tone", slider = Linear)]
    pub contrast: f32,

    /// Highlights recovery
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Tone", slider = Linear)]
    pub highlights: f32,

    /// Shadows recovery
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Tone", slider = Linear)]
    pub shadows: f32,

    /// Vibrance (smart saturation)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Color", slider = Linear)]
    pub vibrance: f32,

    /// Vibrance protection exponent
    #[param(range(0.5..=4.0), default = 2.0, identity = 2.0, step = 0.1)]
    #[param(unit = "", section = "Advanced", slider = Linear)]
    pub vibrance_protection: f32,

    /// Linear saturation factor
    #[param(range(0.0..=2.0), default = 1.0, identity = 1.0, step = 0.05)]
    #[param(unit = "\u{d7}", section = "Color", slider = FactorCentered)]
    pub saturation: f32,

    /// Temperature shift (negative = cool, positive = warm)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Color", slider = Linear)]
    pub temperature: f32,

    /// Tint shift (negative = green, positive = magenta)
    #[param(range(-1.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Color", slider = Linear)]
    pub tint: f32,

    /// Dehaze strength
    #[param(range(0.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Tone", slider = SquareFromSlider)]
    pub dehaze: f32,

    /// Black point level (0 = no change)
    #[param(range(0.0..=0.5), default = 0.0, identity = 0.0, step = 0.01)]
    #[param(unit = "", section = "Tone", slider = Linear)]
    pub black_point: f32,

    /// White point level (1 = no change)
    #[param(range(0.5..=2.0), default = 1.0, identity = 1.0, step = 0.05)]
    #[param(unit = "", section = "Tone", slider = Linear)]
    pub white_point: f32,
}

impl Default for FusedAdjustDef {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            vibrance: 0.0,
            vibrance_protection: 2.0,
            saturation: 1.0,
            temperature: 0.0,
            tint: 0.0,
            dehaze: 0.0,
            black_point: 0.0,
            white_point: 1.0,
        }
    }
}

/// Hue-selective chroma boost simulating wider color gamuts (P3).
///
/// Selectively boosts chroma in hue regions where Display P3 extends
/// beyond sRGB, producing vivid reds, richer greens, and punchier oranges.
/// Already-saturated colors get less boost (vibrance-style protection).
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.gamut_expand", group = Color, role = Filter)]
#[node(label = "Gamut Expand")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(tags("color", "gamut", "p3", "wide"))]
pub struct GamutExpandDef {
    /// Expansion strength (0 = sRGB, 1 = full P3-like expansion)
    #[param(range(0.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = Linear)]
    pub strength: f32,
}

impl Default for GamutExpandDef {
    fn default() -> Self {
        Self { strength: 0.0 }
    }
}

/// Local tone mapping: compresses dynamic range while preserving local contrast.
///
/// Separates the image into a base layer (large-scale luminance) and detail
/// layer (local texture), compresses the base, and recombines. Core of
/// faux HDR processing from a single exposure.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.local_tone_map", group = ToneRange, role = Filter)]
#[node(label = "Local Tone Map")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(neighborhood)]
#[node(tags("tonemap", "hdr", "local", "dynamic range"))]
pub struct LocalToneMapDef {
    /// Dynamic range compression strength
    #[param(range(0.0..=1.0), default = 0.0, identity = 0.0, step = 0.05)]
    #[param(unit = "", section = "Main", slider = SquareFromSlider)]
    pub compression: f32,

    /// Local detail enhancement factor
    #[param(range(0.5..=3.0), default = 1.0, identity = 1.0, step = 0.1)]
    #[param(unit = "\u{d7}", section = "Main", slider = Linear)]
    pub detail_boost: f32,

    /// Base layer extraction sigma (larger = coarser separation)
    #[param(range(5.0..=100.0), default = 30.0, identity = 30.0, step = 5.0)]
    #[param(unit = "px", section = "Advanced", slider = Linear)]
    pub sigma: f32,
}

impl Default for LocalToneMapDef {
    fn default() -> Self {
        Self {
            compression: 0.0,
            detail_boost: 1.0,
            sigma: 30.0,
        }
    }
}

/// Median filter for impulse noise removal (preserves edges).
///
/// Replaces each pixel with the median of its neighborhood. Unlike Gaussian
/// blur, the median filter preserves edges while removing salt-and-pepper noise.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.median_blur", group = Detail, role = Filter)]
#[node(label = "Median Blur")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(neighborhood)]
#[node(tags("median", "denoise", "impulse", "edge-preserving"))]
pub struct MedianBlurDef {
    /// Neighborhood radius (1 = 3x3, 2 = 5x5, 3 = 7x7)
    #[param(range(1..=5), default = 1)]
    #[param(unit = "px", section = "Main", slider = Linear)]
    pub radius: i32,

    /// Also apply median to color channels (a, b)
    #[param(default = false)]
    #[param(section = "Main", label = "Filter Chroma")]
    pub filter_chroma: bool,
}

impl Default for MedianBlurDef {
    fn default() -> Self {
        Self {
            radius: 1,
            filter_chroma: false,
        }
    }
}

/// Zone-based luminance adjustment with edge-aware masking.
///
/// Divides the luminance range into 9 zones (one per photographic stop
/// from -8 EV to 0 EV) and applies independent exposure compensation
/// to each. A guided filter creates an edge-preserving mask so adjustments
/// don't cause halos at high-contrast boundaries.
///
/// Equivalent to darktable's Tone Equalizer module.
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.tone_equalizer", group = ToneRange, role = Filter)]
#[node(label = "Tone Equalizer")]
#[node(format(preferred = OklabF32, alpha = Skip))]
#[node(neighborhood)]
#[node(tags("tone", "zone", "equalizer", "local"))]
pub struct ToneEqualizerDef {
    /// Exposure compensation per zone in stops (9 zones, dark to bright)
    #[param(range(-4.0..=4.0), default = 0.0, identity = 0.0, step = 0.1)]
    #[param(unit = "EV", section = "Zones", slider = NotSlider)]
    #[param(labels(
        "-8 EV", "-7 EV", "-6 EV", "-5 EV", "-4 EV", "-3 EV", "-2 EV", "-1 EV", "0 EV"
    ))]
    pub zones: [f32; 9],

    /// Guided filter sigma (0 = auto-size from image)
    #[param(range(0.0..=100.0), default = 0.0, identity = 0.0, step = 1.0)]
    #[param(unit = "px", section = "Advanced", slider = Linear)]
    pub smoothing: f32,

    /// Guided filter eps (smaller = sharper edges in mask)
    #[param(range(0.001..=0.1), default = 0.01, identity = 0.01, step = 0.005)]
    #[param(unit = "", section = "Advanced", slider = Linear)]
    pub edge_preservation: f32,
}

impl Default for ToneEqualizerDef {
    fn default() -> Self {
        Self {
            zones: [0.0; 9],
            smoothing: 0.0,
            edge_preservation: 0.01,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// GEOMETRY (requires "experimental" feature)
// ═══════════════════════════════════════════════════════════════════

/// Rotation by arbitrary angle with automatic cardinal fast-path.
///
/// Cardinal angles (0°, 90°, 180°, 270°) use pixel-perfect remapping.
/// All other angles use Robidoux interpolation (4×4, sharp, fast).
///
/// Default mode is **Crop** — cropped to the largest clean rectangle.
/// Use Deskew for documents (white fill, full frame) or Fill for
/// custom backgrounds.
#[cfg(feature = "experimental")]
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenfilters.rotate", group = Geometry, role = Filter)]
#[node(label = "Rotate")]
#[node(format(preferred = OklabF32, alpha = Process))]
#[node(changes_dimensions)]
#[node(tags("rotate", "geometry", "transform", "deskew", "straighten"))]
pub struct RotateDef {
    /// Rotation angle in degrees. Positive = counterclockwise.
    /// 90, 180, 270 use pixel-perfect fast path (no interpolation).
    #[param(range(-360.0..=360.0), default = 0.0, identity = 0.0, step = 0.1)]
    #[param(unit = "°", section = "Main", slider = Linear)]
    pub angle: f32,

    /// Border mode (non-cardinal only).
    /// 0 = Crop (default), 1 = Deskew (white fill), 2 = FillClamp, 3 = FillBlack.
    #[param(range(0..=3), default = 0, identity = 0, step = 1)]
    #[param(section = "Options")]
    pub mode: i32,
}

/// Arbitrary geometric transform via 3×3 projective matrix.
///
/// For advanced use: affine transforms (rotation + scale + shear) and
/// perspective correction (homography). Most users should prefer the
/// Rotate or Deskew nodes for rotation.
#[cfg(feature = "experimental")]
#[derive(Node, Clone, Debug)]
#[node(id = "zenfilters.warp", group = Geometry, role = Filter)]
#[node(label = "Warp")]
#[node(format(preferred = OklabF32, alpha = Process))]
#[node(changes_dimensions)]
#[node(tags("warp", "affine", "perspective", "homography", "geometry", "transform"))]
pub struct WarpDef {
    /// 3×3 transform matrix in row-major order (9 floats).
    /// Maps output coordinates to source coordinates (inverse mapping).
    #[param(range(-1000.0..=1000.0), default = 0.0, identity = 0.0, step = 0.01)]
    #[param(unit = "", section = "Main", slider = NotSlider)]
    pub matrix: [f32; 9],

    /// Background mode: 0 = Clamp, 1 = Black.
    #[param(range(0..=1), default = 0, identity = 0, step = 1)]
    #[param(section = "Options")]
    pub background: i32,

    /// Interpolation: 0 = Bilinear, 1 = Bicubic, 2 = Robidoux (default), 3 = Lanczos3.
    #[param(range(0..=3), default = 2, identity = 2, step = 1)]
    #[param(section = "Options")]
    pub interpolation: i32,
}

#[cfg(feature = "experimental")]
impl Default for WarpDef {
    fn default() -> Self {
        Self {
            matrix: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            background: 0,
            interpolation: 1,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// Registry helper
// ═══════════════════════════════════════════════════════════════════

/// Register all zenfilters nodes with the given registry.
///
/// Alias: [`register_all`] (same function, kept for backwards compatibility).
pub fn register(registry: &mut NodeRegistry) {
    register_all(registry);
}

/// Register all zenfilters nodes with the given registry.
pub fn register_all(registry: &mut NodeRegistry) {
    registry.register(&EXPOSURE_NODE);
    registry.register(&CONTRAST_NODE);
    registry.register(&BLACK_POINT_NODE);
    registry.register(&WHITE_POINT_NODE);
    registry.register(&WHITES_BLACKS_NODE);
    registry.register(&SIGMOID_NODE);
    registry.register(&PARAMETRIC_CURVE_NODE);
    registry.register(&TONE_CURVE_NODE);
    registry.register(&DT_SIGMOID_NODE);
    registry.register(&BASECURVE_TONE_MAP_NODE);
    registry.register(&LEVELS_NODE);
    registry.register(&HIGHLIGHTS_SHADOWS_NODE);
    registry.register(&SHADOW_LIFT_NODE);
    registry.register(&HIGHLIGHT_RECOVERY_NODE);
    registry.register(&SATURATION_NODE);
    registry.register(&VIBRANCE_NODE);
    registry.register(&TEMPERATURE_NODE);
    registry.register(&TINT_NODE);
    registry.register(&HSL_ADJUST_NODE);
    registry.register(&COLOR_GRADING_NODE);
    registry.register(&CAMERA_CALIBRATION_NODE);
    registry.register(&BW_MIXER_NODE);
    registry.register(&GRAYSCALE_NODE);
    registry.register(&HUE_ROTATE_NODE);
    registry.register(&SEPIA_NODE);
    registry.register(&CLARITY_NODE);
    registry.register(&BRILLIANCE_NODE);
    registry.register(&ADAPTIVE_SHARPEN_NODE);
    registry.register(&SHARPEN_NODE);
    registry.register(&NOISE_REDUCTION_NODE);
    registry.register(&TEXTURE_NODE);
    registry.register(&VIGNETTE_NODE);
    registry.register(&BLOOM_NODE);
    registry.register(&GRAIN_NODE);
    registry.register(&DEHAZE_NODE);
    registry.register(&INVERT_NODE);
    registry.register(&ALPHA_NODE);
    registry.register(&COLOR_MATRIX_NODE);
    registry.register(&AUTO_EXPOSURE_DEF_NODE);
    registry.register(&AUTO_LEVELS_DEF_NODE);
    registry.register(&BILATERAL_DEF_NODE);
    registry.register(&BLUR_DEF_NODE);
    registry.register(&CHANNEL_CURVES_DEF_NODE);
    registry.register(&CHROMATIC_ABERRATION_DEF_NODE);
    registry.register(&DEVIGNETTE_DEF_NODE);
    registry.register(&EDGE_DETECT_DEF_NODE);
    registry.register(&FUSED_ADJUST_DEF_NODE);
    registry.register(&GAMUT_EXPAND_DEF_NODE);
    registry.register(&LOCAL_TONE_MAP_DEF_NODE);
    registry.register(&MEDIAN_BLUR_DEF_NODE);
    registry.register(&TONE_EQUALIZER_DEF_NODE);
    #[cfg(feature = "experimental")]
    {
        registry.register(&ROTATE_DEF_NODE);

        registry.register(&WARP_DEF_NODE);
    }
}

/// All zenfilters node definitions.
pub static ALL: &[&dyn NodeDef] = &[
    &EXPOSURE_NODE,
    &CONTRAST_NODE,
    &BLACK_POINT_NODE,
    &WHITE_POINT_NODE,
    &WHITES_BLACKS_NODE,
    &SIGMOID_NODE,
    &PARAMETRIC_CURVE_NODE,
    &TONE_CURVE_NODE,
    &DT_SIGMOID_NODE,
    &BASECURVE_TONE_MAP_NODE,
    &LEVELS_NODE,
    &HIGHLIGHTS_SHADOWS_NODE,
    &SHADOW_LIFT_NODE,
    &HIGHLIGHT_RECOVERY_NODE,
    &SATURATION_NODE,
    &VIBRANCE_NODE,
    &TEMPERATURE_NODE,
    &TINT_NODE,
    &HSL_ADJUST_NODE,
    &COLOR_GRADING_NODE,
    &CAMERA_CALIBRATION_NODE,
    &BW_MIXER_NODE,
    &GRAYSCALE_NODE,
    &HUE_ROTATE_NODE,
    &SEPIA_NODE,
    &CLARITY_NODE,
    &BRILLIANCE_NODE,
    &ADAPTIVE_SHARPEN_NODE,
    &SHARPEN_NODE,
    &NOISE_REDUCTION_NODE,
    &TEXTURE_NODE,
    &VIGNETTE_NODE,
    &BLOOM_NODE,
    &GRAIN_NODE,
    &DEHAZE_NODE,
    &INVERT_NODE,
    &ALPHA_NODE,
    &COLOR_MATRIX_NODE,
    &AUTO_EXPOSURE_DEF_NODE,
    &AUTO_LEVELS_DEF_NODE,
    &BILATERAL_DEF_NODE,
    &BLUR_DEF_NODE,
    &CHANNEL_CURVES_DEF_NODE,
    &CHROMATIC_ABERRATION_DEF_NODE,
    &DEVIGNETTE_DEF_NODE,
    &EDGE_DETECT_DEF_NODE,
    &FUSED_ADJUST_DEF_NODE,
    &GAMUT_EXPAND_DEF_NODE,
    &LOCAL_TONE_MAP_DEF_NODE,
    &MEDIAN_BLUR_DEF_NODE,
    &TONE_EQUALIZER_DEF_NODE,
];

/// Geometry node definitions (requires `experimental` feature).
#[cfg(feature = "experimental")]
pub static GEOMETRY: &[&dyn NodeDef] = &[&ROTATE_DEF_NODE, &WARP_DEF_NODE];

// ═══════════════════════════════════════════════════════════════════
// NodeInstance → Filter bridge
// ═══════════════════════════════════════════════════════════════════

/// Convert a zenfilters `NodeInstance` to a `Box<dyn Filter>`.
///
/// Reads params from the node and constructs the corresponding filter type.
/// Returns `None` if the node's schema_id is not recognized.
pub fn node_to_filter(
    node: &dyn zennode::traits::NodeInstance,
) -> Option<alloc::boxed::Box<dyn crate::Filter>> {
    use crate::filters::*;

    fn f32_param(node: &dyn zennode::traits::NodeInstance, name: &str) -> f32 {
        node.get_param(name)
            .and_then(|p| match p {
                ParamValue::F32(v) => Some(v),
                _ => None,
            })
            .unwrap_or(0.0)
    }

    match node.schema().id {
        // Tone
        "zenfilters.exposure" => Some(alloc::boxed::Box::new(Exposure {
            stops: f32_param(node, "stops"),
        })),
        "zenfilters.contrast" => Some(alloc::boxed::Box::new(Contrast {
            amount: f32_param(node, "amount"),
        })),
        "zenfilters.black_point" => Some(alloc::boxed::Box::new(BlackPoint {
            level: f32_param(node, "level"),
        })),
        "zenfilters.white_point" => Some(alloc::boxed::Box::new(WhitePoint {
            level: f32_param(node, "level"),
            headroom: f32_param(node, "headroom"),
        })),
        "zenfilters.sigmoid" => Some(alloc::boxed::Box::new(Sigmoid {
            contrast: f32_param(node, "contrast"),
            skew: f32_param(node, "skew"),
            chroma_compression: f32_param(node, "chroma_compression"),
        })),
        // Color
        "zenfilters.saturation" => {
            let factor = f32_param(node, "factor");
            let amount = f32_param(node, "amount");
            // translate.rs uses "amount" param, but the filter uses "factor"
            let val = if factor != 0.0 { factor } else { amount + 1.0 };
            Some(alloc::boxed::Box::new(Saturation { factor: val }))
        }
        "zenfilters.vibrance" => Some(alloc::boxed::Box::new(Vibrance {
            amount: f32_param(node, "amount"),
            protection: f32_param(node, "protection"),
        })),
        "zenfilters.temperature" => Some(alloc::boxed::Box::new(Temperature {
            shift: f32_param(node, "amount"),
        })),
        "zenfilters.tint" => Some(alloc::boxed::Box::new(Tint {
            shift: f32_param(node, "amount"),
        })),
        "zenfilters.hue_rotate" => Some(alloc::boxed::Box::new(HueRotate {
            degrees: f32_param(node, "degrees"),
        })),
        "zenfilters.grayscale" => Some(alloc::boxed::Box::new(Grayscale::default())),
        "zenfilters.sepia" => Some(alloc::boxed::Box::new(Sepia {
            amount: f32_param(node, "amount"),
        })),
        "zenfilters.invert" => Some(alloc::boxed::Box::new(Invert)),
        // Detail
        "zenfilters.clarity" => Some(alloc::boxed::Box::new(Clarity {
            amount: f32_param(node, "amount"),
            sigma: f32_param(node, "sigma"),
        })),
        "zenfilters.sharpen" => Some(alloc::boxed::Box::new(Sharpen {
            amount: f32_param(node, "amount"),
            sigma: f32_param(node, "sigma"),
        })),
        "zenfilters.dehaze" => Some(alloc::boxed::Box::new(Dehaze {
            strength: f32_param(node, "strength"),
        })),
        "zenfilters.bloom" => Some(alloc::boxed::Box::new(Bloom {
            amount: f32_param(node, "amount"),
            sigma: f32_param(node, "sigma"),
            threshold: f32_param(node, "threshold"),
        })),
        "zenfilters.grain" => Some(alloc::boxed::Box::new(Grain {
            amount: f32_param(node, "amount"),
            size: f32_param(node, "size"),
            seed: 0,
        })),
        "zenfilters.alpha" => Some(alloc::boxed::Box::new(crate::filters::Alpha {
            factor: f32_param(node, "factor"),
        })),
        "zenfilters.color_matrix" => {
            let matrix = match node.get_param("matrix") {
                Some(ParamValue::F32Array(arr)) if arr.len() == 25 => {
                    let mut m = [0.0f32; 25];
                    m.copy_from_slice(&arr);
                    m
                }
                _ => crate::filters::ColorMatrix::IDENTITY,
            };
            Some(alloc::boxed::Box::new(crate::filters::ColorMatrix {
                matrix,
            }))
        }
        // Auto
        "zenfilters.auto_exposure" => Some(alloc::boxed::Box::new(AutoExposure {
            strength: f32_param(node, "strength"),
            target: f32_param(node, "target"),
            max_correction: f32_param(node, "max_correction"),
        })),
        "zenfilters.auto_levels" => {
            fn bool_param(node: &dyn zennode::traits::NodeInstance, name: &str) -> bool {
                node.get_param(name)
                    .and_then(|p| match p {
                        ParamValue::Bool(v) => Some(v),
                        _ => None,
                    })
                    .unwrap_or(false)
            }
            Some(alloc::boxed::Box::new(AutoLevels {
                clip_low: f32_param(node, "clip_low"),
                clip_high: f32_param(node, "clip_high"),
                target_midpoint: f32_param(node, "target_midpoint"),
                scale_chroma: bool_param(node, "scale_chroma"),
                remove_cast: bool_param(node, "remove_cast"),
                strength: {
                    let v = f32_param(node, "strength");
                    if v > 0.0 { v } else { 1.0 }
                },
            }))
        }
        // Additional Detail
        "zenfilters.bilateral" => Some(alloc::boxed::Box::new(Bilateral {
            spatial_sigma: f32_param(node, "spatial_sigma"),
            range_sigma: f32_param(node, "range_sigma"),
            strength: f32_param(node, "strength"),
        })),
        "zenfilters.blur" => Some(alloc::boxed::Box::new(Blur {
            sigma: f32_param(node, "sigma"),
        })),
        "zenfilters.channel_curves" => {
            fn parse_curve_points(s: &str) -> alloc::vec::Vec<(f32, f32)> {
                s.split(',')
                    .filter_map(|pair| {
                        let mut parts = pair.trim().split(':');
                        let x = parts.next()?.trim().parse::<f32>().ok()?;
                        let y = parts.next()?.trim().parse::<f32>().ok()?;
                        Some((x, y))
                    })
                    .collect()
            }
            fn str_param(node: &dyn zennode::traits::NodeInstance, name: &str) -> alloc::string::String {
                node.get_param(name)
                    .and_then(|p| match p {
                        ParamValue::Str(s) => Some(s),
                        _ => None,
                    })
                    .unwrap_or_else(|| alloc::string::String::from("0:0,1:1"))
            }
            let r_pts = parse_curve_points(&str_param(node, "red_points"));
            let g_pts = parse_curve_points(&str_param(node, "green_points"));
            let b_pts = parse_curve_points(&str_param(node, "blue_points"));
            if r_pts.len() >= 2 && g_pts.len() >= 2 && b_pts.len() >= 2 {
                Some(alloc::boxed::Box::new(ChannelCurves::from_points(&r_pts, &g_pts, &b_pts)))
            } else {
                Some(alloc::boxed::Box::new(ChannelCurves::default()))
            }
        }
        "zenfilters.chromatic_aberration" => Some(alloc::boxed::Box::new(ChromaticAberration {
            shift_a: f32_param(node, "shift_a"),
            shift_b: f32_param(node, "shift_b"),
        })),
        "zenfilters.devignette" => Some(alloc::boxed::Box::new(Devignette {
            strength: f32_param(node, "strength"),
            exponent: {
                let v = f32_param(node, "exponent");
                if v > 0.0 { v } else { 4.0 }
            },
        })),
        "zenfilters.edge_detect" => {
            let mode_int = node.get_param("mode")
                .and_then(|p| match p {
                    ParamValue::I32(v) => Some(v),
                    _ => None,
                })
                .unwrap_or(0);
            let mode = match mode_int {
                1 => EdgeMode::Laplacian,
                2 => EdgeMode::Canny,
                _ => EdgeMode::Sobel,
            };
            Some(alloc::boxed::Box::new(EdgeDetect {
                mode,
                strength: f32_param(node, "strength"),
            }))
        }
        "zenfilters.fused_adjust" => Some(alloc::boxed::Box::new(FusedAdjust {
            exposure: f32_param(node, "exposure"),
            contrast: f32_param(node, "contrast"),
            highlights: f32_param(node, "highlights"),
            shadows: f32_param(node, "shadows"),
            vibrance: f32_param(node, "vibrance"),
            vibrance_protection: {
                let v = f32_param(node, "vibrance_protection");
                if v > 0.0 { v } else { 2.0 }
            },
            saturation: {
                let v = f32_param(node, "saturation");
                if v > 0.0 { v } else { 1.0 }
            },
            temperature: f32_param(node, "temperature"),
            tint: f32_param(node, "tint"),
            dehaze: f32_param(node, "dehaze"),
            black_point: f32_param(node, "black_point"),
            white_point: {
                let v = f32_param(node, "white_point");
                if v > 0.0 { v } else { 1.0 }
            },
        })),
        "zenfilters.gamut_expand" => Some(alloc::boxed::Box::new(GamutExpand {
            strength: f32_param(node, "strength"),
        })),
        "zenfilters.local_tone_map" => Some(alloc::boxed::Box::new(LocalToneMap {
            compression: f32_param(node, "compression"),
            detail_boost: {
                let v = f32_param(node, "detail_boost");
                if v > 0.0 { v } else { 1.0 }
            },
            sigma: {
                let v = f32_param(node, "sigma");
                if v > 0.0 { v } else { 30.0 }
            },
        })),
        "zenfilters.median_blur" => {
            let radius = node.get_param("radius")
                .and_then(|p| match p {
                    ParamValue::I32(v) => Some(v as u32),
                    _ => None,
                })
                .unwrap_or(1);
            let filter_chroma = node.get_param("filter_chroma")
                .and_then(|p| match p {
                    ParamValue::Bool(v) => Some(v),
                    _ => None,
                })
                .unwrap_or(false);
            Some(alloc::boxed::Box::new(MedianBlur { radius, filter_chroma }))
        }
        "zenfilters.tone_equalizer" => {
            let zones = match node.get_param("zones") {
                Some(ParamValue::F32Array(arr)) if arr.len() == 9 => {
                    let mut z = [0.0f32; 9];
                    z.copy_from_slice(&arr);
                    z
                }
                _ => [0.0; 9],
            };
            Some(alloc::boxed::Box::new(ToneEqualizer {
                zones,
                smoothing: f32_param(node, "smoothing"),
                edge_preservation: {
                    let v = f32_param(node, "edge_preservation");
                    if v > 0.0 { v } else { 0.01 }
                },
            }))
        }
        // Geometry (experimental)
        #[cfg(feature = "experimental")]
        "zenfilters.rotate" => {
            let angle = f32_param(node, "angle");
            let mode = match node
                .get_param("mode")
                .and_then(|p| match p {
                    ParamValue::I32(v) => Some(v),
                    _ => None,
                })
                .unwrap_or(0)
            {
                1 => RotateMode::Deskew,
                2 => RotateMode::FillClamp,
                3 => RotateMode::black(),
                _ => RotateMode::Crop,
            };
            Some(alloc::boxed::Box::new(crate::filters::Rotate {
                angle_degrees: angle,
                mode,
            }))
        }
        #[cfg(feature = "experimental")]
        "zenfilters.warp" => {
            let matrix = match node.get_param("matrix") {
                Some(ParamValue::F32Array(arr)) if arr.len() == 9 => {
                    let mut m = [0.0f32; 9];
                    m.copy_from_slice(&arr);
                    m
                }
                _ => [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            };
            let bg = match node
                .get_param("background")
                .and_then(|p| match p {
                    ParamValue::I32(v) => Some(v),
                    _ => None,
                })
                .unwrap_or(0)
            {
                1 => WarpBackground::black(),
                _ => WarpBackground::Clamp,
            };
            let interp = match node
                .get_param("interpolation")
                .and_then(|p| match p {
                    ParamValue::I32(v) => Some(v),
                    _ => None,
                })
                .unwrap_or(1)
            {
                0 => WarpInterpolation::Bilinear,
                1 => WarpInterpolation::Bicubic,
                2 => WarpInterpolation::Robidoux,
                3 => WarpInterpolation::Lanczos3,
                _ => WarpInterpolation::Robidoux,
            };
            {
                let mut warp = crate::filters::Warp::projective(matrix);
                warp.background = bg;
                warp.interpolation = interp;
                Some(alloc::boxed::Box::new(warp))
            }
        }
        _ => None,
    }
}

/// Returns true if the given schema ID is a zenfilters node.
pub fn is_zenfilters_node(schema_id: &str) -> bool {
    schema_id.starts_with("zenfilters.")
}

// ═══════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    #[test]
    fn exposure_node_schema_matches() {
        let schema = EXPOSURE_NODE.schema();
        assert_eq!(schema.id, "zenfilters.exposure");
        assert_eq!(schema.label, "Exposure");
        assert_eq!(schema.group, NodeGroup::Tone);
        assert_eq!(schema.role, NodeRole::Filter);
        assert_eq!(schema.params.len(), 1);
        assert_eq!(schema.params[0].name, "stops");
        match &schema.params[0].kind {
            ParamKind::Float {
                min,
                max,
                default,
                identity,
                step,
            } => {
                assert_eq!(*min, -5.0);
                assert_eq!(*max, 5.0);
                assert_eq!(*default, 0.0);
                assert_eq!(*identity, 0.0);
                assert_eq!(*step, 0.1);
            }
            other => panic!("expected Float, got {other:?}"),
        }
        assert_eq!(schema.params[0].unit, "EV");
        assert_eq!(schema.params[0].slider, SliderMapping::Linear);
    }

    #[test]
    fn contrast_node_schema_matches() {
        let schema = CONTRAST_NODE.schema();
        assert_eq!(schema.id, "zenfilters.contrast");
        assert_eq!(schema.group, NodeGroup::Tone);
        assert_eq!(schema.params.len(), 1);
        assert_eq!(schema.params[0].name, "amount");
        assert_eq!(schema.params[0].slider, SliderMapping::SquareFromSlider);
    }

    #[test]
    fn saturation_node_defaults() {
        let node = Saturation::default();
        assert_eq!(node.factor, 1.0);
        let schema = SATURATION_NODE.schema();
        assert_eq!(schema.id, "zenfilters.saturation");
        assert_eq!(schema.group, NodeGroup::Color);
        assert_eq!(schema.params[0].slider, SliderMapping::FactorCentered);
    }

    #[test]
    fn clarity_node_is_neighborhood() {
        let schema = CLARITY_NODE.schema();
        assert_eq!(schema.id, "zenfilters.clarity");
        assert_eq!(schema.group, NodeGroup::Detail);
        assert_eq!(schema.role, NodeRole::Filter);
        assert!(schema.format.is_neighborhood);
        assert_eq!(schema.params.len(), 2);
    }

    #[test]
    fn vignette_node_post_resize() {
        let schema = VIGNETTE_NODE.schema();
        assert_eq!(schema.id, "zenfilters.vignette");
        assert_eq!(schema.group, NodeGroup::Effects);
        assert_eq!(schema.role, NodeRole::Filter);
        assert_eq!(schema.params.len(), 4);
    }

    #[test]
    fn dt_sigmoid_is_tonemap() {
        let schema = DT_SIGMOID_NODE.schema();
        assert_eq!(schema.id, "zenfilters.dt_sigmoid");
        assert_eq!(schema.group, NodeGroup::ToneMap);
        assert_eq!(schema.role, NodeRole::Filter);
    }

    #[test]
    fn coalesce_groups_correct() {
        // Fused adjust filters should have coalesce group
        let fused = [
            EXPOSURE_NODE.schema(),
            CONTRAST_NODE.schema(),
            BLACK_POINT_NODE.schema(),
            WHITE_POINT_NODE.schema(),
            SATURATION_NODE.schema(),
            VIBRANCE_NODE.schema(),
            TEMPERATURE_NODE.schema(),
            TINT_NODE.schema(),
            DEHAZE_NODE.schema(),
        ];
        for schema in &fused {
            assert!(
                schema.coalesce.is_some(),
                "{} should have coalesce info",
                schema.id
            );
            assert_eq!(
                schema.coalesce.as_ref().unwrap().group,
                "fused_adjust",
                "{} coalesce group mismatch",
                schema.id
            );
        }
    }

    #[test]
    fn register_all_populates_registry() {
        let mut registry = NodeRegistry::new();
        register_all(&mut registry);
        // We register 47 nodes (35 original + 12 new)
        assert!(
            registry.all().len() >= 47,
            "expected at least 47 nodes, got {}",
            registry.all().len()
        );
        // Spot-check lookups
        assert!(registry.get("zenfilters.exposure").is_some());
        assert!(registry.get("zenfilters.invert").is_some());
        assert!(registry.get("zenfilters.vignette").is_some());
    }

    #[test]
    fn node_instance_get_set() {
        use zennode::traits::NodeInstance;
        let mut node = Exposure { stops: 1.5 };
        assert_eq!(node.get_param("stops"), Some(ParamValue::F32(1.5)));
        assert!(node.set_param("stops", ParamValue::F32(-2.0)));
        assert_eq!(node.stops, -2.0);
        assert!(!node.set_param("nonexistent", ParamValue::F32(0.0)));
    }

    #[test]
    fn node_instance_to_params() {
        use zennode::traits::NodeInstance;
        let node = Vibrance {
            amount: 0.3,
            protection: 1.5,
        };
        let params = node.to_params();
        assert_eq!(params.get("amount"), Some(&ParamValue::F32(0.3)));
        assert_eq!(params.get("protection"), Some(&ParamValue::F32(1.5)));
    }

    #[test]
    fn hsl_adjust_schema() {
        let schema = HSL_ADJUST_NODE.schema();
        assert_eq!(schema.id, "zenfilters.hsl_adjust");
        assert_eq!(schema.group, NodeGroup::Color);
        assert_eq!(schema.role, NodeRole::Filter);
        assert_eq!(schema.params.len(), 3);

        // Check hue param
        assert_eq!(schema.params[0].name, "hue");
        assert_eq!(schema.params[0].section, "Hue");
        assert_eq!(schema.params[0].unit, "°");
        assert_eq!(schema.params[0].slider, SliderMapping::NotSlider);
        match &schema.params[0].kind {
            ParamKind::FloatArray {
                len,
                min,
                max,
                default,
                labels,
            } => {
                assert_eq!(*len, 8);
                assert_eq!(*min, -180.0);
                assert_eq!(*max, 180.0);
                assert_eq!(*default, 0.0);
                assert_eq!(labels.len(), 8);
                assert_eq!(labels[0], "Red");
                assert_eq!(labels[7], "Magenta");
            }
            other => panic!("expected FloatArray for hue, got {other:?}"),
        }

        // Check saturation param
        assert_eq!(schema.params[1].name, "saturation");
        assert_eq!(schema.params[1].section, "Saturation");
        match &schema.params[1].kind {
            ParamKind::FloatArray {
                len,
                min,
                max,
                default,
                ..
            } => {
                assert_eq!(*len, 8);
                assert_eq!(*min, 0.0);
                assert_eq!(*max, 3.0);
                assert_eq!(*default, 1.0);
            }
            other => panic!("expected FloatArray for saturation, got {other:?}"),
        }

        // Check luminance param
        assert_eq!(schema.params[2].name, "luminance");
        assert_eq!(schema.params[2].section, "Luminance");
        match &schema.params[2].kind {
            ParamKind::FloatArray {
                len,
                min,
                max,
                default,
                ..
            } => {
                assert_eq!(*len, 8);
                assert_eq!(*min, -0.5);
                assert_eq!(*max, 0.5);
                assert_eq!(*default, 0.0);
            }
            other => panic!("expected FloatArray for luminance, got {other:?}"),
        }

        // Tags
        assert!(schema.tags.contains(&"color"));
        assert!(schema.tags.contains(&"hsl"));
    }

    #[test]
    fn hsl_adjust_identity() {
        use zennode::traits::NodeInstance;
        let node = HslAdjust::default();
        assert!(node.is_identity());

        let mut non_identity = node.clone();
        non_identity.hue[3] = 10.0;
        assert!(!non_identity.is_identity());
    }

    #[test]
    fn hsl_adjust_get_set() {
        use zennode::traits::NodeInstance;
        let mut node = HslAdjust::default();

        // Get returns F32Array
        let val = node.get_param("hue").unwrap();
        match &val {
            ParamValue::F32Array(arr) => assert_eq!(arr.len(), 8),
            other => panic!("expected F32Array, got {other:?}"),
        }

        // Set works
        let new_hue = vec![10.0, 20.0, 30.0, 40.0, -10.0, -20.0, -30.0, -40.0];
        assert!(node.set_param("hue", ParamValue::F32Array(new_hue.clone())));
        assert_eq!(node.hue[0], 10.0);
        assert_eq!(node.hue[7], -40.0);

        // Wrong length fails
        assert!(!node.set_param("hue", ParamValue::F32Array(vec![1.0, 2.0])));
    }

    #[test]
    fn bw_mixer_schema() {
        let schema = BW_MIXER_NODE.schema();
        assert_eq!(schema.id, "zenfilters.bw_mixer");
        assert_eq!(schema.group, NodeGroup::Color);
        assert_eq!(schema.params.len(), 1);
        assert_eq!(schema.params[0].name, "weights");
        match &schema.params[0].kind {
            ParamKind::FloatArray {
                len,
                min,
                max,
                default,
                labels,
            } => {
                assert_eq!(*len, 8);
                assert_eq!(*min, 0.0);
                assert_eq!(*max, 2.0);
                assert_eq!(*default, 1.0);
                assert_eq!(labels[0], "Red");
                assert_eq!(labels[7], "Magenta");
            }
            other => panic!("expected FloatArray, got {other:?}"),
        }
        assert!(schema.tags.contains(&"bw"));
        assert!(schema.tags.contains(&"grayscale"));
    }

    #[test]
    fn bw_mixer_identity() {
        use zennode::traits::NodeInstance;
        let node = BwMixer::default();
        assert!(node.is_identity());

        let mut non_identity = node.clone();
        non_identity.weights[0] = 0.5;
        assert!(!non_identity.is_identity());
    }

    #[test]
    fn basecurve_tonemap_schema() {
        let schema = BASECURVE_TONE_MAP_NODE.schema();
        assert_eq!(schema.id, "zenfilters.basecurve_tonemap");
        assert_eq!(schema.group, NodeGroup::ToneMap);
        assert_eq!(schema.role, NodeRole::Filter);
        assert_eq!(schema.params.len(), 2);

        // preset is a String param
        assert_eq!(schema.params[0].name, "preset");
        assert_eq!(schema.params[0].label, "Preset");
        assert_eq!(schema.params[0].section, "Main");
        match &schema.params[0].kind {
            ParamKind::Str { default } => assert_eq!(*default, ""),
            other => panic!("expected Str for preset, got {other:?}"),
        }

        // chroma_compression is a Float param
        assert_eq!(schema.params[1].name, "chroma_compression");
        match &schema.params[1].kind {
            ParamKind::Float {
                min,
                max,
                default,
                identity,
                step,
            } => {
                assert_eq!(*min, 0.0);
                assert_eq!(*max, 1.0);
                assert_eq!(*default, 0.4);
                assert_eq!(*identity, 0.0);
                assert_eq!(*step, 0.05);
            }
            other => panic!("expected Float for chroma_compression, got {other:?}"),
        }

        assert!(schema.tags.contains(&"tonemap"));
        assert!(schema.tags.contains(&"basecurve"));
    }

    #[test]
    fn basecurve_tonemap_get_set() {
        use zennode::traits::NodeInstance;
        let mut node = BasecurveToneMap::default();

        assert_eq!(
            node.get_param("preset"),
            Some(ParamValue::Str(String::new()))
        );
        assert_eq!(
            node.get_param("chroma_compression"),
            Some(ParamValue::F32(0.4))
        );

        assert!(node.set_param("preset", ParamValue::Str("nikon_d7000".to_string())));
        assert_eq!(node.preset, "nikon_d7000");

        assert!(node.set_param("chroma_compression", ParamValue::F32(0.8)));
        assert_eq!(node.chroma_compression, 0.8);
    }

    #[test]
    fn all_groups_represented() {
        let mut registry = NodeRegistry::new();
        register_all(&mut registry);

        let has = |g: NodeGroup| !registry.by_group(g).is_empty();
        assert!(has(NodeGroup::Tone), "Tone");
        assert!(has(NodeGroup::ToneRange), "ToneRange");
        assert!(has(NodeGroup::ToneMap), "ToneMap");
        assert!(has(NodeGroup::Color), "Color");
        assert!(has(NodeGroup::Detail), "Detail");
        assert!(has(NodeGroup::Effects), "Effects");
    }
}
