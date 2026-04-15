//! Slider-to-parameter mappings for ergonomic UI integration.
//!
//! Photo editing sliders should feel perceptually linear: equal slider
//! movements should produce equal perceived changes. Many internal parameters
//! have non-linear response curves that make raw values feel uneven.
//!
//! # Parameter Reference
//!
//! Every user-facing filter parameter, its recommended slider range, identity
//! point, linearity classification, and whether a mapping function is needed.
//!
//! ## Already Perceptually Linear (use raw values directly)
//!
//! | Filter | Parameter | Range | Identity | Notes |
//! |--------|-----------|-------|----------|-------|
//! | Exposure | `stops` | -3..+3 | 0.0 | Photographic stops = perceptual unit |
//! | Temperature | `shift` | -1..+1 | 0.0 | Oklab b-axis is perceptually uniform |
//! | Tint | `shift` | -1..+1 | 0.0 | Oklab a-axis is perceptually uniform |
//! | Highlights/Shadows | `highlights` | -1..+1 | 0.0 | Linear strength × quadratic mask |
//! | Highlights/Shadows | `shadows` | -1..+1 | 0.0 | Same |
//! | Whites/Blacks | `whites` | -1..+1 | 0.0 | Smoothstep-weighted, headroom-limited |
//! | Whites/Blacks | `blacks` | -1..+1 | 0.0 | Same |
//! | Clarity | `amount` | -2..+2 | 0.0 | Linear unsharp band boost |
//! | Texture | `amount` | -2..+2 | 0.0 | Same as clarity, finer scale |
//! | Sharpen | `amount` | 0..2 | 0.0 | Linear USM |
//! | AdaptiveSharpen | `amount` | 0..2 | 0.0 | Linear gated USM |
//! | AdaptiveSharpen | `detail` | 0..1 | 0.5 | Edges-only (0) to full detail (1) |
//! | AdaptiveSharpen | `masking` | 0..1 | 0.0 | Edge restriction strength |
//! | Vibrance | `amount` | 0..1 | 0.0 | Linear for desaturated colors |
//! | Brilliance | `amount` | 0..1 | 0.0 | Linear correction strength |
//! | Black Point | `level` | 0..0.3 | 0.0 | Linear tonal remap |
//! | Vignette | `strength` | -1..+1 | 0.0 | Linear falloff strength |
//! | Vignette | `midpoint` | 0..1 | 0.5 | Radial start position |
//! | Vignette | `feather` | 0..1 | 0.5 | Transition width |
//! | Vignette | `roundness` | 0..1 | 1.0 | Circle (1) to rectangle (0) |
//! | Sepia | `amount` | 0..1 | 0.0 | Blend toward sepia tone |
//! | Grain | `amount` | 0..1 | 0.0 | Noise amplitude |
//! | Grain | `size` | 1..4 | 1.5 | Noise grain size |
//! | HueRotate | `degrees` | -180..+180 | 0.0 | Angular rotation |
//! | GamutExpand | `strength` | 0..1 | 0.0 | P3 chroma expansion |
//! | HighlightRecovery | `strength` | 0..1 | 0.0 | Soft-knee compression |
//! | ShadowLift | `strength` | 0..1 | 0.0 | Toe lift |
//! | AutoExposure | `strength` | 0..1 | 0.0 | Blend toward auto |
//! | Devignette | `strength` | 0..1 | 0.0 | Radial brightening |
//! | Alpha | `factor` | 0..1 | 1.0 | Opacity scaling |
//!
//! ## Need Perceptual Remapping (use `from_slider` / mapping functions)
//!
//! | Filter | Parameter | Raw Range | Slider Range | Mapping | `from_slider` |
//! |--------|-----------|-----------|-------------|---------|--------------|
//! | Contrast | `amount` | -1..+1 | -1..+1 | `slider²×sign` | `Contrast::from_slider` |
//! | Saturation | `factor` | 0..2 | 0..1 | `slider×2` (0.5=identity) | `Saturation::from_slider` |
//! | Dehaze | `strength` | 0..1 | 0..1 | `slider²` | `Dehaze::from_slider` |
//! | LocalToneMap | `compression` | 0..1 | 0..1 | `slider²` | `LocalToneMap::from_slider` |
//! | NoiseReduction | `luminance` | 0..1 | 0..1 | `slider²` | `NoiseReduction::from_slider` |
//! | NoiseReduction | `chroma` | 0..1 | 0..1 | `slider²` | `NoiseReduction::from_slider` |
//! | AdaptiveSharpen | `noise_floor` | 0.001..0.02 | 0..1 | `0.001+slider²×0.019` | `AdaptiveSharpen::from_sliders` |
//!
//! ## Expert/Specialized Parameters (not typically slider-bound)
//!
//! | Filter | Parameter | Range | Notes |
//! |--------|-----------|-------|-------|
//! | Bilateral | `spatial_sigma` | 2..8 | Blur radius — expose as "Smoothing Radius" |
//! | Bilateral | `range_sigma` | 0.01..0.3 | Edge sensitivity — Low/Med/High presets |
//! | Bilateral | `strength` | 0..1 | Blend — linear, slider-friendly |
//! | Clarity/Texture | `sigma` | 1..8 | Detail scale — expose as "Scale" |
//! | Brilliance | `sigma` | 5..40 | Local average window — usually auto-sized |
//! | Brilliance | `shadow_strength` | 0..1 | Fine control — usually kept at default |
//! | Brilliance | `highlight_strength` | 0..1 | Fine control — usually kept at default |
//! | LocalToneMap | `detail_boost` | 1..3 | Linear — slider-friendly as "Detail" |
//! | LocalToneMap | `sigma` | 10..60 | Usually proportional to image size |
//! | NoiseReduction | `detail` | 0..1 | Fine detail preservation |
//! | NoiseReduction | `luminance_contrast` | 0..1 | Contrast preservation |
//! | NoiseReduction | `chroma_detail` | 0..1 | Color detail preservation |
//! | NoiseReduction | `scales` | 1..6 | Wavelet depth — rarely changed |
//! | Vibrance | `protection` | 1..3 | Saturation protection curve — rarely changed |
//! | WhitePoint | `level` | 0.7..1.5 | Highlight remapping |
//! | WhitePoint | `headroom` | 0..0.5 | Soft-clip headroom |
//! | Sigmoid | `contrast` | 0.5..3 | Film contrast — already perceptual |
//! | Sigmoid | `skew` | -1..+1 | Shadow/highlight bias |
//! | ColorGrading | `shadow/mid/high_a/b` | -0.1..+0.1 | Oklab tint offsets |
//! | ColorGrading | `balance` | -1..+1 | Zone crossover shift |
//! | HslAdjust | `hue[8]` | -30..+30 | Per-hue rotation (degrees) |
//! | HslAdjust | `saturation[8]` | -1..+1 | Per-hue saturation shift |
//! | HslAdjust | `luminance[8]` | -0.3..+0.3 | Per-hue luminance shift |
//! | BwMixer | `weights[8]` | 0..3 | Per-color luminance weight |
//! | CameraCalibration | `*_hue` | -30..+30 | Primary hue shift (degrees) |
//! | CameraCalibration | `*_saturation` | -1..+1 | Primary saturation shift |
//! | CameraCalibration | `shadow_tint` | -1..+1 | Green-magenta in shadows |
//! | ChromaticAberration | `shift_a/b` | -0.01..+0.01 | Radial chroma shift |
//! | Devignette | `exponent` | 2..6 | Falloff curve (4 = cos⁴ law) |

// ─── Mapping functions ───────────────────────────────────────────────

/// Contrast: raw range [-1, +1], power curve response is expansive.
/// Sqrt mapping puts the useful range (0–0.25 internal) in the first 70% of slider.
pub fn contrast_to_slider(amount: f32) -> f32 {
    amount.abs().sqrt() * amount.signum()
}

pub fn contrast_from_slider(slider: f32) -> f32 {
    slider.abs().powi(2) * slider.signum()
}

/// Dehaze: raw range [0, 1], inverse transmission creates aggressive
/// effect at high strength. Sqrt mapping concentrates useful range in first half.
pub fn dehaze_to_slider(strength: f32) -> f32 {
    strength.sqrt()
}

pub fn dehaze_from_slider(slider: f32) -> f32 {
    slider * slider
}

/// Local tone map compression: raw range [0, 2], gamma-based compression
/// is sensitive at high values. Sqrt mapping scales from [0,1] slider to
/// [0, 2] internal so that slider=0.25 → 0.125 (visible on low-DR images).
pub fn ltm_compression_to_slider(compression: f32) -> f32 {
    (compression * 0.5).sqrt()
}

pub fn ltm_compression_from_slider(slider: f32) -> f32 {
    slider * slider * 2.0
}

/// Noise reduction strength: BayesShrink threshold scaling is compressive.
pub fn nr_strength_to_slider(strength: f32) -> f32 {
    strength.sqrt()
}

pub fn nr_strength_from_slider(slider: f32) -> f32 {
    slider * slider
}

/// Saturation: internal factor where 1.0 = identity.
/// Slider: 0.0 = grayscale, 0.5 = identity, 1.0 = double saturation.
pub fn saturation_to_slider(factor: f32) -> f32 {
    (factor * 0.5).clamp(0.0, 1.0)
}

pub fn saturation_from_slider(slider: f32) -> f32 {
    slider * 2.0
}

/// Bilateral range_sigma: maps 0..1 slider to 0..0.3 range_sigma.
pub fn bilateral_range_to_slider(range_sigma: f32) -> f32 {
    (range_sigma / 0.3).clamp(0.0, 1.0)
}

pub fn bilateral_range_from_slider(slider: f32) -> f32 {
    slider * 0.3
}

/// Adaptive sharpen noise floor: very sensitive, tiny range.
/// Sqrt mapping for ergonomic 0–1 slider control.
pub fn sharpen_noise_floor_to_slider(noise_floor: f32) -> f32 {
    ((noise_floor - 0.001) / 0.019).clamp(0.0, 1.0).sqrt()
}

pub fn sharpen_noise_floor_from_slider(slider: f32) -> f32 {
    0.001 + slider * slider * 0.019
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_roundtrip() {
        for &v in &[-0.8, -0.3, 0.0, 0.3, 0.8] {
            let slider = contrast_to_slider(v);
            let back = contrast_from_slider(slider);
            assert!(
                (v - back).abs() < 1e-5,
                "contrast roundtrip: {v} → {slider} → {back}"
            );
        }
    }

    #[test]
    fn contrast_first_half_covers_useful_range() {
        let internal = contrast_from_slider(0.5);
        assert!(
            internal < 0.3 && internal > 0.2,
            "slider 0.5 should map to moderate contrast: {internal}"
        );
    }

    #[test]
    fn saturation_center_is_identity() {
        let factor = saturation_from_slider(0.5);
        assert!(
            (factor - 1.0).abs() < 1e-5,
            "slider center should be identity: {factor}"
        );
    }

    #[test]
    fn dehaze_roundtrip() {
        for &v in &[0.0, 0.1, 0.5, 0.9, 1.0] {
            let slider = dehaze_to_slider(v);
            let back = dehaze_from_slider(slider);
            assert!(
                (v - back).abs() < 1e-5,
                "dehaze roundtrip: {v} → {slider} → {back}"
            );
        }
    }

    #[test]
    fn dehaze_first_half_covers_useful_range() {
        let internal = dehaze_from_slider(0.5);
        assert!(
            (internal - 0.25).abs() < 1e-5,
            "slider 0.5 should be 0.25 internal: {internal}"
        );
    }

    #[test]
    fn nr_roundtrip() {
        for &v in &[0.0, 0.1, 0.5, 1.0] {
            let slider = nr_strength_to_slider(v);
            let back = nr_strength_from_slider(slider);
            assert!(
                (v - back).abs() < 1e-4,
                "NR roundtrip: {v} → {slider} → {back}"
            );
        }
    }

    #[test]
    fn noise_floor_roundtrip() {
        for &v in &[0.001, 0.005, 0.01, 0.02] {
            let slider = sharpen_noise_floor_to_slider(v);
            let back = sharpen_noise_floor_from_slider(slider);
            assert!(
                (v - back).abs() < 1e-4,
                "noise_floor roundtrip: {v} → {slider} → {back}"
            );
        }
    }
}
