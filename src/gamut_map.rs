/// Strategy for handling out-of-gamut colors after gather.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
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
}
