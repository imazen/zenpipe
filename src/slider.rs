/// Slider-to-parameter mappings for ergonomic UI integration.
///
/// Photo editing sliders should feel perceptually linear: equal slider
/// movements should produce equal perceived changes. Many internal parameters
/// have non-linear response curves (power functions, inverse relationships)
/// that make raw values feel uneven on a slider.
///
/// This module provides mapping functions that:
/// - Take a 0.0–1.0 slider position (or -1.0 to +1.0 for bipolar)
/// - Return the internal parameter value
/// - Concentrate the "useful range" in the first half of the slider
///
/// # Design
///
/// Each mapping uses a simple power curve: `value = slider^gamma * range + offset`.
/// Gamma < 1 compresses the useful range into the first half (most common).
/// Gamma > 1 expands it (rare — only for parameters that are too sensitive at low values).
///
/// The `*_inv` functions go from parameter value back to slider position (for UI state).

/// Contrast: raw range [-1, +1], but power curve response is expansive.
/// Sqrt mapping puts the "useful" range (0–0.5 internal) in the first 70% of slider.
pub fn contrast_to_slider(amount: f32) -> f32 {
    // Internal [-1, +1] → slider [-1, +1] via square (inverse of sqrt)
    amount.abs().sqrt() * amount.signum()
}

pub fn contrast_from_slider(slider: f32) -> f32 {
    // Slider [-1, +1] → internal [-1, +1] via square
    slider.abs().powi(2) * slider.signum()
}

/// Dehaze: raw range [0, 1], but inverse transmission creates aggressive
/// effect at high strength. Sqrt mapping concentrates useful range in first half.
pub fn dehaze_to_slider(strength: f32) -> f32 {
    strength.sqrt()
}

pub fn dehaze_from_slider(slider: f32) -> f32 {
    slider * slider
}

/// Local tone map compression: raw range [0, 1], gamma-based compression
/// is sensitive at high values. Sqrt mapping.
pub fn ltm_compression_to_slider(compression: f32) -> f32 {
    compression.sqrt()
}

pub fn ltm_compression_from_slider(slider: f32) -> f32 {
    slider * slider
}

/// Noise reduction strength: BayesShrink threshold scaling is compressive
/// (small values already remove a lot of noise). Sqrt mapping.
pub fn nr_strength_to_slider(strength: f32) -> f32 {
    strength.sqrt()
}

pub fn nr_strength_from_slider(slider: f32) -> f32 {
    slider * slider
}

/// Saturation: internal factor where 1.0 = identity.
/// Slider: 0.0 = grayscale, 0.5 = identity, 1.0 = double saturation.
/// This makes 0.5 the "center" position for a natural slider feel.
pub fn saturation_to_slider(factor: f32) -> f32 {
    // factor 0..2 → slider 0..1 (linear, center at 0.5 = factor 1.0)
    (factor * 0.5).clamp(0.0, 1.0)
}

pub fn saturation_from_slider(slider: f32) -> f32 {
    // slider 0..1 → factor 0..2
    slider * 2.0
}

/// Bilateral range_sigma: eps = range_sigma², so perceptual effect
/// is quadratic. Linear slider maps to sqrt of internal value.
pub fn bilateral_range_to_slider(range_sigma: f32) -> f32 {
    // range_sigma 0..0.3 → slider 0..1
    (range_sigma / 0.3).clamp(0.0, 1.0)
}

pub fn bilateral_range_from_slider(slider: f32) -> f32 {
    // slider 0..1 → range_sigma 0..0.3
    slider * 0.3
}

/// Adaptive sharpen noise floor: very sensitive, tiny range.
/// Map to intuitive 0–1 slider with sqrt for ergonomic control.
pub fn sharpen_noise_floor_to_slider(noise_floor: f32) -> f32 {
    // noise_floor 0.001..0.02 → slider 0..1 via sqrt mapping
    ((noise_floor - 0.001) / 0.019).clamp(0.0, 1.0).sqrt()
}

pub fn sharpen_noise_floor_from_slider(slider: f32) -> f32 {
    // slider 0..1 → noise_floor 0.001..0.02 via square mapping
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
        // Slider 0.5 should map to internal ~0.25 (the useful range)
        let internal = contrast_from_slider(0.5);
        assert!(
            internal < 0.3,
            "slider 0.5 should map to modest contrast: {internal}"
        );
        assert!(
            internal > 0.2,
            "slider 0.5 should have visible effect: {internal}"
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
        // Slider 0.5 should map to internal 0.25 (modest dehaze)
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
