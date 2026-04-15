use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::planes::OklabPlanes;

/// What a filter assumes about the values in the planes.
///
/// Used by the pipeline to validate filter/working-space compatibility
/// at push time. Pushing an Oklab-specific filter into an sRGB pipeline
/// panics with a descriptive error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaneSemantics {
    /// Filter is pure math on plane values — works in any color space.
    /// Examples: blur, sharpen, convolve, morphology, posterize.
    Any,
    /// Filter assumes planes contain Oklab L/a/b data.
    /// Examples: Contrast (power curve on L), Saturation (chroma scale on a/b).
    Oklab,
    /// Filter assumes planes contain RGB data (sRGB, linear, etc.).
    /// Examples: LinearContrast, LumaGrayscale, HslSaturate.
    Rgb,
}

/// A photo filter that operates on planar Oklab f32 data.
///
/// Filters modify `OklabPlanes` in-place. The pipeline guarantees that
/// planes are in the correct format (f32 Oklab) before calling `apply`.
///
/// Filters are infallible — any validation (e.g., parameter clamping)
/// happens at construction time, not at apply time.
///
/// The `ctx` parameter provides a pool of reusable scratch buffers.
/// Neighborhood filters should use `ctx.take_f32()` and `ctx.return_f32()`
/// for temporary planes instead of allocating fresh vectors each call.
pub trait Filter: Send + Sync {
    /// Which planes this filter reads and writes.
    fn channel_access(&self) -> ChannelAccess;

    /// Whether this filter needs neighborhood access (reads adjacent pixels).
    ///
    /// Per-pixel filters return false. Neighborhood filters (clarity,
    /// brilliance, bilateral) return true.
    fn is_neighborhood(&self) -> bool {
        false
    }

    /// Maximum spatial radius this filter needs, in pixels.
    ///
    /// For Gaussian-based filters: `ceil(3 * sigma)`. For wavelet filters:
    /// the maximum tap distance across all scales. For per-pixel filters: 0.
    ///
    /// Used by windowed pipeline processors to determine how many rows of
    /// overlap context to buffer around each processing strip.
    ///
    /// For filters whose radius depends on image dimensions (e.g., Dehaze),
    /// return the radius for the given `width` and `height`.
    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        0
    }

    /// Compatibility tag for conflict detection.
    ///
    /// Used by [`filter_compat::validate_pipeline`] to check for mutually
    /// exclusive filters, ordering violations, and range conflicts.
    fn tag(&self) -> crate::filter_compat::FilterTag {
        crate::filter_compat::FilterTag::Other
    }

    /// When this filter should run relative to a resize operation.
    ///
    /// Filters return their preferred phase so a resize-aware pipeline
    /// can split the filter stack into pre-resize and post-resize groups.
    fn resize_phase(&self) -> ResizePhase {
        ResizePhase::Either
    }

    /// Scale pixel-space parameters for a different resolution.
    ///
    /// `scale` = actual_width / reference_width.
    /// Example: parameters designed for 4K, running at 1080p → scale = 0.5.
    ///
    /// Default: no-op. Filters with pixel-space sigma override this.
    fn scale_for_resolution(&mut self, _scale: f32) {}

    /// What this filter assumes about the values in the planes.
    ///
    /// The pipeline validates this at push time: an `Oklab`-semantic filter
    /// cannot be pushed into an sRGB pipeline (and vice versa). `Any` filters
    /// work in all pipelines.
    ///
    /// Default: `PlaneSemantics::Oklab` — most zenfilters are Oklab-native.
    /// Override to `Any` for generic spatial filters, or `Rgb` for sRGB-compat filters.
    fn plane_semantics(&self) -> PlaneSemantics {
        PlaneSemantics::Oklab
    }

    /// Apply the filter in-place to the given planes.
    ///
    /// `ctx` provides reusable scratch buffers — neighborhood filters should
    /// borrow temporary planes from `ctx` instead of allocating.
    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext);
}

/// When a filter should run relative to a resize operation.
///
/// A resize-aware pipeline uses this to split filters into
/// pre-resize and post-resize groups automatically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ResizePhase {
    /// Must run at full resolution before any downscale.
    ///
    /// Filters that operate at sub-pixel precision or whose sigma is
    /// calibrated for full-resolution detail:
    /// - Chromatic aberration (sub-pixel chroma shifts)
    /// - Noise reduction (catches full-res noise before downscale averages it)
    /// - Adaptive sharpen (preserves fine detail before blur)
    /// - Clarity/texture (sigma is in absolute pixels)
    PreResize,

    /// Must run after resize, relative to output dimensions.
    ///
    /// Spatial effects whose coordinates are relative to the output frame:
    /// - Grain (grain size relative to output pixels)
    /// - Vignette (falloff relative to output frame)
    /// - Bloom (glow relative to output viewing size)
    PostResize,

    /// Can run before or after resize with no quality difference.
    ///
    /// Per-pixel filters with no spatial dependency:
    /// - Exposure, contrast, saturation, vibrance, temperature, tint
    /// - Tone curves, color grading, levels
    /// - All FusedAdjust operations
    Either,
}
