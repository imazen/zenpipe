/// Strategy for handling out-of-gamut colors after gather.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub enum GamutMapping {
    /// Clamp negative RGB values to 0. Fast, may shift hue slightly
    /// for aggressively boosted colors. Sufficient for most adjustments.
    #[default]
    Clip,

    /// Iteratively reduce Oklch chroma until RGB values are in gamut.
    /// Preserves hue at the cost of reduced saturation. Use for
    /// aggressive saturation boosts where hue accuracy matters.
    ChromaReduce {
        /// Maximum bisection iterations. 8-12 is typical.
        max_iterations: u32,
    },

    /// Soft chroma compression using a precomputed gamut boundary LUT.
    /// Smoothly compresses chroma near the boundary using a rational
    /// knee function that preserves hue and lightness.
    SoftCompress {
        /// Fraction of max chroma where compression starts (0.0–1.0).
        /// Below this threshold, colors pass through unchanged.
        /// Typical value: 0.9 (start compressing at 90% of boundary).
        knee: f32,
    },
}
