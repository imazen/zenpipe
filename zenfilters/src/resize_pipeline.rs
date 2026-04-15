//! Sigma scaling helpers for resize-aware filter application.
//!
//! When filters designed for one resolution are applied at another (e.g.,
//! after downscale), sigma values need adjustment so they target the same
//! perceptual frequency band:
//!
//! ```ignore
//! let scale = output_width as f32 / input_width as f32; // e.g., 0.5 for 2× downscale
//! clarity.sigma *= scale;
//! noise_reduction.scales = adjusted_scales(noise_reduction.scales, scale);
//! ```
//!
//! The [`scale_sigma`] function handles the common case.

/// Adjust a pixel-space sigma for a different resolution.
///
/// `scale` is output/input (0.5 for 2× downscale, 2.0 for 2× upscale).
/// Returns the adjusted sigma that targets the same perceptual frequency.
#[inline]
pub fn scale_sigma(sigma: f32, scale: f32) -> f32 {
    (sigma * scale).max(0.5)
}

/// Compute optimal wavelet scales for noise reduction at a given resolution scale.
///
/// At 2× downscale, the finest wavelet scale (1–2px noise) is already averaged
/// out, so we can skip it. Returns the adjusted scale count.
#[inline]
pub fn adjusted_nr_scales(original_scales: u32, scale: f32) -> u32 {
    if scale >= 1.0 {
        return original_scales; // upscale or same: keep all scales
    }
    // Each 2× downscale eliminates ~1 scale of noise
    let skip = (-scale.log2()).floor() as u32;
    original_scales.saturating_sub(skip).max(1)
}
