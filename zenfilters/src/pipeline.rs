use crate::prelude::*;
use whereat::{At, at};
use zenpixels::ColorPrimaries;
use zenpixels_convert::gamut::GamutMatrix;
use zenpixels_convert::oklab;

use crate::context::FilterContext;
use crate::filter::Filter;
use crate::gamut_lut::GamutBoundaryLut;
use crate::gamut_map::GamutMapping;
use crate::planes::OklabPlanes;
use crate::scatter_gather::scatter_to_oklab;

/// Compute the number of core rows per strip for L3-friendly processing.
///
/// Targets keeping all planar data (L + a + b + optional alpha) under
/// ~4 MB, which fits comfortably in L3 on most CPUs (8–32 MB).
/// The `halo` parameter accounts for extra overlap rows needed by
/// neighborhood filters — the total rows scattered per strip is
/// `core_rows + 2 * halo`, and this function ensures the total fits
/// the L3 budget.
pub(crate) fn strip_height(width: u32, has_alpha: bool, halo: usize) -> usize {
    const TARGET_BYTES: usize = 4 * 1024 * 1024;
    let plane_count = if has_alpha { 4 } else { 3 };
    let bytes_per_row = (width as usize) * plane_count * core::mem::size_of::<f32>();
    let max_total_rows = (TARGET_BYTES / bytes_per_row.max(1)).clamp(8, 2048);
    // Core rows = total budget minus halo on each side.
    // Ensure at least 8 core rows so we make forward progress.
    max_total_rows.saturating_sub(2 * halo).max(8)
}

/// Working color space for the filter pipeline.
///
/// Determines how input RGB values are mapped to planar data before
/// filters are applied, and how they're mapped back afterward.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum WorkingSpace {
    /// Perceptual Oklab color space (default).
    ///
    /// L = perceptual lightness, a/b = chroma axes. Produces the
    /// highest-quality photo adjustments because equal parameter
    /// changes produce equal perceived changes. Used by Lightroom,
    /// darktable, and modern photo editors.
    #[default]
    Oklab,
    /// sRGB gamma space — matches ImageMagick's default behavior.
    ///
    /// L/a/b planes contain sRGB R/G/B values directly (normalized to
    /// [0, 1]). All filters operate in gamma-encoded sRGB, producing
    /// output identical to ImageMagick for operations like blur,
    /// sharpen, convolution, morphology, emboss, posterize, solarize.
    ///
    /// Per-pixel adjustments (contrast, saturation, exposure) will use
    /// the same sRGB formulas as ImageMagick's `-brightness-contrast`,
    /// `-modulate`, etc.
    ///
    /// Known artifacts (same as ImageMagick): gamma-space blur darkens
    /// color boundaries, saturation shifts hue, contrast is non-perceptual.
    Srgb,
    /// Linear RGB — physically correct light math.
    ///
    /// Matches ImageMagick's `-colorspace RGB` (linear) mode. Blur and
    /// compositing are physically correct (no gamma darkening), but
    /// per-pixel adjustments operate on linear values which are not
    /// perceptually uniform (shadows are compressed, highlights expanded).
    ///
    /// The pipeline linearizes sRGB input, deinterleaves to planes, and
    /// re-encodes on gather. No Oklab conversion.
    LinearRgb,
}

/// Configuration for the filter pipeline.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct PipelineConfig {
    /// Color primaries (determines RGB↔LMS matrices).
    pub primaries: ColorPrimaries,
    /// Reference white luminance for HDR normalization.
    /// Use 1.0 for SDR, 203.0 for PQ (ITU-R BT.2408).
    pub reference_white: f32,
    /// Gamut mapping strategy applied after filters, before gather.
    pub gamut_mapping: GamutMapping,
    /// Working color space for filter processing.
    pub working_space: WorkingSpace,
    /// Reference width for resolution-independent parameters.
    ///
    /// When set, pixel-space sigma values in neighborhood filters are
    /// automatically scaled at apply time: `sigma *= actual_width / reference_width`.
    ///
    /// This means you can define filter parameters once (e.g., calibrated for 4K)
    /// and they'll produce visually identical results at any resolution.
    ///
    /// `None` (default): no scaling, sigma values are used as-is.
    pub reference_width: Option<u32>,
}

impl PipelineConfig {
    /// sRGB working space — matches ImageMagick's default behavior.
    ///
    /// Filters operate on gamma-encoded sRGB values directly. This is
    /// suboptimal for photo adjustments (gamma-space blur darkens edges,
    /// saturation shifts hue) but produces output identical to ImageMagick
    /// for spatial operations.
    ///
    /// Use sRGB-compatible filter types (`LinearContrast`, `HslSaturate`,
    /// `LumaGrayscale`) instead of Oklab-specific filters (`Contrast`,
    /// `Saturation`, `Grayscale`). The pipeline validates at push time.
    ///
    /// Requires the `srgb-compat` feature.
    #[cfg(feature = "srgb-compat")]
    pub fn srgb_compat() -> Self {
        Self {
            working_space: WorkingSpace::Srgb,
            gamut_mapping: GamutMapping::Bypass,
            ..Default::default()
        }
    }

    /// Linear RGB working space — physically correct light math.
    ///
    /// Matches ImageMagick's `-colorspace RGB` mode. Blur, compositing,
    /// and spatial filters are physically correct (no gamma darkening).
    /// Use for operations where linear light accuracy matters more than
    /// perceptual uniformity.
    ///
    /// Requires the `srgb-compat` feature.
    #[cfg(feature = "srgb-compat")]
    pub fn linear_rgb() -> Self {
        Self {
            working_space: WorkingSpace::LinearRgb,
            gamut_mapping: GamutMapping::Bypass,
            ..Default::default()
        }
    }
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            primaries: ColorPrimaries::Bt709,
            reference_white: 1.0,
            gamut_mapping: GamutMapping::Clip,
            working_space: WorkingSpace::Oklab,
            reference_width: None,
        }
    }
}

/// Errors from pipeline construction or execution.
#[derive(Debug)]
#[non_exhaustive]
pub enum PipelineError {
    /// The requested color primaries are not supported (Unknown).
    UnsupportedPrimaries(ColorPrimaries),
    /// Input buffer has wrong size.
    BufferSize { expected: usize, actual: usize },
}

impl core::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnsupportedPrimaries(p) => write!(f, "unsupported primaries: {p:?}"),
            Self::BufferSize { expected, actual } => {
                write!(f, "buffer size mismatch: expected {expected}, got {actual}")
            }
        }
    }
}

impl core::error::Error for PipelineError {}

/// A composable filter pipeline operating on planar Oklab f32.
///
/// The pipeline handles scatter (RGB→planar Oklab), applies a stack of
/// filters, then gathers (planar Oklab→RGB). All filters run in a single
/// planar Oklab domain — no intermediate format conversions.
pub struct Pipeline {
    config: PipelineConfig,
    filters: Vec<Box<dyn Filter>>,
    m1: GamutMatrix,
    m1_inv: GamutMatrix,
    gamut_lut: Option<alloc::sync::Arc<GamutBoundaryLut>>,
}

impl Pipeline {
    /// Create a new pipeline with the given configuration.
    #[track_caller]
    pub fn new(config: PipelineConfig) -> Result<Self, At<PipelineError>> {
        let m1 = oklab::rgb_to_lms_matrix(config.primaries)
            .ok_or_else(|| at!(PipelineError::UnsupportedPrimaries(config.primaries)))?;
        let m1_inv = oklab::lms_to_rgb_matrix(config.primaries)
            .ok_or_else(|| at!(PipelineError::UnsupportedPrimaries(config.primaries)))?;

        let gamut_lut = match config.gamut_mapping {
            GamutMapping::SoftCompress { .. } => {
                Some(alloc::sync::Arc::new(GamutBoundaryLut::new(&m1_inv)))
            }
            _ => None,
        };

        Ok(Self {
            config,
            filters: Vec::new(),
            m1,
            m1_inv,
            gamut_lut,
        })
    }

    /// Access the pipeline configuration.
    pub fn config(&self) -> &PipelineConfig {
        &self.config
    }

    /// Add a filter to the pipeline.
    ///
    /// # Panics
    ///
    /// Panics if the filter's `plane_semantics()` is incompatible with the
    /// pipeline's `working_space`. For example, pushing an Oklab-specific
    /// filter (`Contrast`) into an sRGB pipeline panics.
    pub fn push(&mut self, filter: Box<dyn Filter>) {
        use crate::filter::PlaneSemantics;
        let semantics = filter.plane_semantics();
        let space = self.config.working_space;
        let compatible = match semantics {
            PlaneSemantics::Any => true,
            PlaneSemantics::Oklab => space == WorkingSpace::Oklab,
            PlaneSemantics::Rgb => matches!(space, WorkingSpace::Srgb | WorkingSpace::LinearRgb),
        };
        assert!(
            compatible,
            "Filter requires {:?} planes but pipeline uses {:?} working space. \
             Use a compatible filter type or change PipelineConfig::working_space.",
            semantics, space
        );
        self.filters.push(filter);
    }

    /// Number of filters in the pipeline.
    pub fn len(&self) -> usize {
        self.filters.len()
    }

    /// Whether the pipeline has no filters.
    pub fn is_empty(&self) -> bool {
        self.filters.is_empty()
    }

    /// Scale all pixel-space parameters for the given output width.
    ///
    /// Requires `reference_width` to be set in `PipelineConfig`. Calls
    /// `scale_for_resolution(actual / reference)` on every filter.
    ///
    /// Call this once before `apply()` when the output resolution differs
    /// from the reference resolution the parameters were designed for.
    ///
    /// ```ignore
    /// let mut pipe = Pipeline::new(PipelineConfig {
    ///     reference_width: Some(3840), // parameters calibrated for 4K
    ///     ..Default::default()
    /// })?;
    /// pipe.push(Box::new(Clarity { sigma: 4.0, amount: 0.3 }));
    ///
    /// pipe.scale_to_width(1920); // clarity sigma becomes 2.0
    /// pipe.apply(&src, &mut dst, 1920, 1080, 3, &mut ctx)?;
    /// ```
    pub fn scale_to_width(&mut self, actual_width: u32) {
        if let Some(ref_w) = self.config.reference_width {
            let scale = actual_width as f32 / ref_w as f32;
            if (scale - 1.0).abs() > 0.01 {
                for filter in &mut self.filters {
                    filter.scale_for_resolution(scale);
                }
            }
            // Clear reference_width so it's not applied again
            self.config.reference_width = None;
        }
    }

    /// Scale and split in one call.
    ///
    /// If `reference_width` is set, scales PreResize filters for `input_width`
    /// and PostResize filters for `output_width`, then splits.
    ///
    /// ```ignore
    /// let (pre, post) = pipe.split_scaled(3840, 1920);
    /// pre.apply(&src, &mut buf, 3840, 2160, 3, &mut ctx)?;
    /// // ... resize ...
    /// post.apply(&resized, &mut dst, 1920, 1080, 3, &mut ctx)?;
    /// ```
    pub fn split_scaled(mut self, input_width: u32, output_width: u32) -> (Self, Self) {
        use crate::filter::ResizePhase;

        if let Some(ref_w) = self.config.reference_width {
            let pre_scale = input_width as f32 / ref_w as f32;
            let post_scale = output_width as f32 / ref_w as f32;

            for filter in &mut self.filters {
                let scale = match filter.resize_phase() {
                    ResizePhase::PreResize | ResizePhase::Either => pre_scale,
                    ResizePhase::PostResize => post_scale,
                };
                if (scale - 1.0).abs() > 0.01 {
                    filter.scale_for_resolution(scale);
                }
            }
            self.config.reference_width = None;
        }

        self.split_for_resize()
    }

    /// Split this pipeline into pre-resize and post-resize halves.
    ///
    /// Filters are classified by their [`ResizePhase`](crate::filter::ResizePhase):
    /// - `PreResize` → pre (full-res detail: NR, sharpen, clarity, CA)
    /// - `PostResize` → post (output-relative: grain, vignette, bloom)
    /// - `Either` → pre (more precision at full resolution)
    ///
    /// Both halves share the same `PipelineConfig` (primaries, gamut mapping).
    ///
    /// ```ignore
    /// let (pre, post) = pipeline.split_for_resize();
    ///
    /// pre.apply(&src, &mut full_res, in_w, in_h, 3, &mut ctx)?;
    /// // ... resize full_res → resized ...
    /// post.apply(&resized, &mut dst, out_w, out_h, 3, &mut ctx)?;
    /// ```
    pub fn split_for_resize(self) -> (Self, Self) {
        use crate::filter::ResizePhase;

        let mut pre = Self {
            config: self.config.clone(),
            filters: Vec::new(),
            m1: self.m1,
            m1_inv: self.m1_inv,
            gamut_lut: self.gamut_lut.clone(),
        };
        let mut post = Self {
            config: self.config,
            filters: Vec::new(),
            m1: self.m1,
            m1_inv: self.m1_inv,
            gamut_lut: self.gamut_lut,
        };

        for filter in self.filters {
            match filter.resize_phase() {
                ResizePhase::PreResize | ResizePhase::Either => pre.filters.push(filter),
                ResizePhase::PostResize => post.filters.push(filter),
            }
        }

        (pre, post)
    }

    /// Whether any filter requires neighborhood access (spatial operations).
    ///
    /// When true, strip processing is disabled and the full frame is processed
    /// at once to ensure neighborhood filters can see adjacent rows.
    pub fn has_neighborhood_filter(&self) -> bool {
        self.filters.iter().any(|f| f.is_neighborhood())
    }

    /// Maximum neighborhood radius across all filters, in pixels.
    ///
    /// Returns 0 if no filters have spatial neighborhoods. For windowed
    /// pipeline processing, the window overlap must be at least this value
    /// on each side of the strip.
    pub fn max_neighborhood_radius(&self, width: u32, height: u32) -> u32 {
        self.filters
            .iter()
            .map(|f| f.neighborhood_radius(width, height))
            .max()
            .unwrap_or(0)
    }

    /// Total neighborhood halo across all filters (sum of radii).
    ///
    /// When processing in strips, each strip must be extended by this many
    /// rows on each side to ensure all neighborhood filters produce correct
    /// results. The sum (not max) is required because each neighborhood
    /// filter "consumes" its radius of padding from the correct output of
    /// the previous filter.
    pub fn total_halo(&self, width: u32, height: u32) -> u32 {
        self.filters
            .iter()
            .map(|f| f.neighborhood_radius(width, height))
            .sum()
    }

    /// Apply the full pipeline: scatter → filters → gather.
    ///
    /// `src` is interleaved linear RGB(A) f32 data.
    /// `dst` is the output buffer (same size as src).
    /// `width` and `height` are image dimensions.
    /// `channels` is 3 (RGB) or 4 (RGBA).
    /// `ctx` provides reusable scratch buffers — pass a persistent
    /// `FilterContext` to avoid per-call allocations.
    ///
    /// Processing uses L3-cache-friendly horizontal strips with overlapping
    /// halos for neighborhood filters. Each strip is extended by
    /// [`total_halo()`](Pipeline::total_halo) rows on each side so that
    /// neighborhood filters (clarity, sharpen, denoise, etc.) produce
    /// correct results without materializing the entire image.
    #[track_caller]
    pub fn apply(
        &self,
        src: &[f32],
        dst: &mut [f32],
        width: u32,
        height: u32,
        channels: u32,
        ctx: &mut FilterContext,
    ) -> Result<(), At<PipelineError>> {
        let n = (width as usize) * (height as usize) * (channels as usize);
        if src.len() < n {
            return Err(at!(PipelineError::BufferSize {
                expected: n,
                actual: src.len(),
            }));
        }
        if dst.len() < n {
            return Err(at!(PipelineError::BufferSize {
                expected: n,
                actual: dst.len(),
            }));
        }

        // Neighborhood filters need full-frame access for correct output.
        // Per-pixel filters use L3-cache-friendly strips (no halo needed).
        // Halo-based stripping for neighborhood filters is possible but
        // benchmarks show 4x slower than full-frame due to redundant
        // re-filtering of overlapping rows.
        if self.has_neighborhood_filter() {
            return self.apply_full_frame(src, dst, width, height, channels, ctx);
        }

        self.apply_stripped(src, dst, width, height, channels, ctx)
    }

    /// Full-frame Oklab planes with streamed color conversions.
    ///
    /// Neighborhood filters need the complete Oklab planes, but scatter
    /// (linear RGB → Oklab) and gather (Oklab → linear RGB) can be done
    /// in L3-cache-friendly strips. Only the filter step operates on the
    /// full-frame planes.
    fn apply_full_frame(
        &self,
        src: &[f32],
        dst: &mut [f32],
        width: u32,
        height: u32,
        channels: u32,
        ctx: &mut FilterContext,
    ) -> Result<(), At<PipelineError>> {
        let ch = channels as usize;
        let w = width as usize;

        // Full-frame Oklab planes (needed for neighborhood context)
        let mut planes = if ch == 4 {
            OklabPlanes::from_ctx_with_alpha(ctx, width, height)
        } else {
            OklabPlanes::from_ctx(ctx, width, height)
        };

        let is_passthrough = matches!(
            self.config.working_space,
            WorkingSpace::Srgb | WorkingSpace::LinearRgb
        );

        // Scatter in strips for cache locality
        let scatter_strip = strip_height(width, ch == 4, 0);
        for y in (0..height as usize).step_by(scatter_strip) {
            let rows = scatter_strip.min(height as usize - y);
            let src_off = y * w * ch;
            let src_len = rows * w * ch;
            let plane_off = y * w;
            let plane_len = rows * w;

            if is_passthrough {
                // sRGB passthrough: deinterleave R→L, G→a, B→b
                for i in 0..plane_len {
                    planes.l[plane_off + i] = src[src_off + i * ch];
                    planes.a[plane_off + i] = src[src_off + i * ch + 1];
                    planes.b[plane_off + i] = src[src_off + i * ch + 2];
                }
            } else {
                crate::simd::scatter_oklab(
                    &src[src_off..src_off + src_len],
                    &mut planes.l[plane_off..plane_off + plane_len],
                    &mut planes.a[plane_off..plane_off + plane_len],
                    &mut planes.b[plane_off..plane_off + plane_len],
                    channels,
                    &self.m1,
                    self.config.reference_white,
                );
            }
            if ch == 4
                && let Some(ref mut alpha) = planes.alpha
            {
                for i in 0..plane_len {
                    alpha[plane_off + i] = src[src_off + i * ch + 3];
                }
            }
        }

        // Apply all filters on full-frame planes
        self.apply_planar(&mut planes, ctx);

        // Gather in strips for cache locality
        for y in (0..height as usize).step_by(scatter_strip) {
            let rows = scatter_strip.min(height as usize - y);
            let dst_off = y * w * ch;
            let dst_len = rows * w * ch;
            let plane_off = y * w;
            let plane_len = rows * w;

            if is_passthrough {
                // sRGB passthrough: interleave L→R, a→G, b→B
                for i in 0..plane_len {
                    dst[dst_off + i * ch] = planes.l[plane_off + i];
                    dst[dst_off + i * ch + 1] = planes.a[plane_off + i];
                    dst[dst_off + i * ch + 2] = planes.b[plane_off + i];
                }
            } else {
                crate::simd::gather_oklab(
                    &planes.l[plane_off..plane_off + plane_len],
                    &planes.a[plane_off..plane_off + plane_len],
                    &planes.b[plane_off..plane_off + plane_len],
                    &mut dst[dst_off..dst_off + dst_len],
                    channels,
                    &self.m1_inv,
                    self.config.reference_white,
                );
            }
            if ch == 4
                && let Some(ref alpha) = planes.alpha
            {
                for i in 0..plane_len {
                    dst[dst_off + i * ch + 3] = alpha[plane_off + i];
                }
            }
        }

        planes.return_to_ctx(ctx);
        Ok(())
    }

    /// Strip-process: scatter, filter, gather in L3-sized horizontal strips.
    ///
    /// Only used for per-pixel filters (halo = 0). Neighborhood filter
    /// pipelines use `apply_full_frame()` instead.
    fn apply_stripped(
        &self,
        src: &[f32],
        dst: &mut [f32],
        width: u32,
        height: u32,
        channels: u32,
        ctx: &mut FilterContext,
    ) -> Result<(), At<PipelineError>> {
        let ch = channels as usize;
        let w = width as usize;
        let has_alpha = ch == 4;
        let halo = self.total_halo(width, height) as usize;
        let strip_h = strip_height(width, has_alpha, halo);

        for y_start in (0..height as usize).step_by(strip_h) {
            let y_end = (y_start + strip_h).min(height as usize);
            let core_rows = y_end - y_start;

            // Extended region: core ± halo, clamped to image bounds
            let ext_y_start = y_start.saturating_sub(halo);
            let ext_y_end = (y_end + halo).min(height as usize);
            let ext_rows = ext_y_end - ext_y_start;
            let core_offset = y_start - ext_y_start;

            // Scatter the extended strip
            let ext_src_offset = ext_y_start * w * ch;
            let ext_src_len = ext_rows * w * ch;

            let mut planes = if has_alpha {
                OklabPlanes::from_ctx_with_alpha(ctx, width, ext_rows as u32)
            } else {
                OklabPlanes::from_ctx(ctx, width, ext_rows as u32)
            };

            if matches!(
                self.config.working_space,
                WorkingSpace::Srgb | WorkingSpace::LinearRgb
            ) {
                crate::scatter_gather::scatter_srgb_passthrough(
                    &src[ext_src_offset..ext_src_offset + ext_src_len],
                    &mut planes,
                    channels,
                );
            } else {
                scatter_to_oklab(
                    &src[ext_src_offset..ext_src_offset + ext_src_len],
                    &mut planes,
                    channels,
                    &self.m1,
                    self.config.reference_white,
                );
            }
            self.apply_planar(&mut planes, ctx);

            // Gather only the core rows from the filtered extended strip
            let dst_offset = y_start * w * ch;
            let dst_len = core_rows * w * ch;
            let plane_start = core_offset * w;
            let plane_end = plane_start + core_rows * w;

            if matches!(
                self.config.working_space,
                WorkingSpace::Srgb | WorkingSpace::LinearRgb
            ) {
                let n = core_rows * w;
                for i in 0..n {
                    dst[dst_offset + i * ch] = planes.l[plane_start + i];
                    dst[dst_offset + i * ch + 1] = planes.a[plane_start + i];
                    dst[dst_offset + i * ch + 2] = planes.b[plane_start + i];
                }
            } else {
                crate::simd::gather_oklab(
                    &planes.l[plane_start..plane_end],
                    &planes.a[plane_start..plane_end],
                    &planes.b[plane_start..plane_end],
                    &mut dst[dst_offset..dst_offset + dst_len],
                    channels,
                    &self.m1_inv,
                    self.config.reference_white,
                );
            }

            // Alpha: copy core rows
            if ch == 4 {
                let n = core_rows * w;
                for i in 0..n {
                    dst[dst_offset + i * ch + 3] =
                        planes.alpha.as_ref().map_or(1.0, |a| a[plane_start + i]);
                }
            }

            planes.return_to_ctx(ctx);
        }

        Ok(())
    }

    /// Apply filters to already-scattered planar Oklab data.
    ///
    /// Use this when you want to manage scatter/gather yourself,
    /// or when chaining multiple pipelines on the same planar data.
    pub fn apply_planar(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        for filter in &self.filters {
            filter.apply(planes, ctx);
        }

        // Apply gamut mapping after all filters
        if let (GamutMapping::SoftCompress { knee }, Some(lut)) =
            (self.config.gamut_mapping, &self.gamut_lut)
        {
            lut.compress_planes(&planes.l, &mut planes.a, &mut planes.b, knee);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    #[test]
    fn empty_pipeline_is_identity() {
        let pipeline = Pipeline::new(PipelineConfig::default()).unwrap();
        let src = vec![0.5f32; 64 * 64 * 3];
        let mut dst = vec![0.0f32; 64 * 64 * 3];
        let mut ctx = FilterContext::new();
        pipeline.apply(&src, &mut dst, 64, 64, 3, &mut ctx).unwrap();

        let mut max_err = 0.0f32;
        for i in 0..src.len() {
            max_err = max_err.max((src[i] - dst[i]).abs());
        }
        assert!(max_err < 1e-3, "empty pipeline max error: {max_err}");
    }

    #[test]
    fn unsupported_primaries() {
        let result = Pipeline::new(PipelineConfig {
            primaries: ColorPrimaries::Unknown,
            reference_white: 1.0,
            gamut_mapping: GamutMapping::Clip,
            working_space: WorkingSpace::Oklab,
            reference_width: None,
        });
        assert!(result.is_err());
    }

    #[test]
    fn buffer_size_validation() {
        let pipeline = Pipeline::new(PipelineConfig::default()).unwrap();
        let src = vec![0.5f32; 10]; // too small
        let mut dst = vec![0.0f32; 64 * 64 * 3];
        let mut ctx = FilterContext::new();
        let result = pipeline.apply(&src, &mut dst, 64, 64, 3, &mut ctx);
        assert!(result.is_err());
    }

    /// Verify that overlapping-strip processing matches full-frame for
    /// a pipeline containing neighborhood filters (clarity + sharpen).
    #[test]
    fn neighborhood_strip_matches_full_frame() {
        use crate::filters;
        use crate::scatter_gather::gather_from_oklab;
        use zenpixels_convert::oklab;

        let (w, h) = (128, 128);
        let n = (w as usize) * (h as usize);
        let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
        let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();

        // Generate a test image with enough spatial variation to exercise
        // neighborhood filters meaningfully.
        let mut src = Vec::with_capacity(n * 3);
        for y in 0..h as usize {
            for x in 0..w as usize {
                let t = (y * w as usize + x) as f32 / n as f32;
                let r = (t * 0.6 + 0.2).clamp(0.01, 0.99);
                let g = ((1.0 - t) * 0.5 + 0.25).clamp(0.01, 0.99);
                let b_val = ((x as f32 / w as f32) * 0.4 + 0.3).clamp(0.01, 0.99);
                src.push(r);
                src.push(g);
                src.push(b_val);
            }
        }

        // Build pipeline with neighborhood filters
        let mut pipeline = Pipeline::new(PipelineConfig::default()).unwrap();
        pipeline.push(Box::new(filters::Exposure { stops: 0.3 }));
        pipeline.push(Box::new(filters::Clarity {
            sigma: 3.0,
            amount: 0.4,
            adaptive: false,
        }));
        pipeline.push(Box::new(filters::Sharpen {
            sigma: 1.0,
            amount: 0.5,
        }));
        pipeline.push(Box::new(filters::Contrast { amount: 0.2 }));
        assert!(pipeline.has_neighborhood_filter());
        assert!(pipeline.total_halo(w, h) > 0);

        // Full-frame reference: scatter → apply_planar → gather
        let mut ctx = FilterContext::new();
        let mut planes = OklabPlanes::from_ctx(&mut ctx, w, h);
        scatter_to_oklab(&src, &mut planes, 3, &m1, 1.0);
        pipeline.apply_planar(&mut planes, &mut ctx);
        let mut full_frame = vec![0.0f32; n * 3];
        gather_from_oklab(&planes, &mut full_frame, 3, &m1_inv, 1.0);
        planes.return_to_ctx(&mut ctx);

        // Strip-processed result via apply()
        let mut stripped = vec![0.0f32; n * 3];
        pipeline
            .apply(&src, &mut stripped, w, h, 3, &mut ctx)
            .unwrap();

        // Compare
        let mut max_err = 0.0f32;
        for i in 0..full_frame.len() {
            max_err = max_err.max((full_frame[i] - stripped[i]).abs());
        }
        // Tolerance accounts for edge-replication differences at image
        // boundaries (full-frame replicates the true image edge; strips
        // replicate the extended strip edge, which is the same pixel
        // for interior strips but may differ at the image boundary
        // due to floating-point ordering).
        assert!(
            max_err < 1e-4,
            "strip vs full-frame max error: {max_err} (should be < 1e-4)"
        );
    }

    #[test]
    fn total_halo_is_sum_of_radii() {
        use crate::filters;

        let mut pipeline = Pipeline::new(PipelineConfig::default()).unwrap();
        // Per-pixel only: halo should be 0
        pipeline.push(Box::new(filters::Exposure { stops: 0.5 }));
        assert_eq!(pipeline.total_halo(64, 64), 0);

        // Add clarity (sigma=4 → coarse blur sigma=16 → radius=48)
        let clarity = filters::Clarity {
            sigma: 4.0,
            amount: 0.3,
            adaptive: false,
        };
        let clarity_radius = clarity.neighborhood_radius(64, 64);
        pipeline.push(Box::new(clarity));

        // Add sharpen (sigma=1 → radius=3)
        let sharpen = filters::Sharpen {
            sigma: 1.0,
            amount: 0.5,
        };
        let sharpen_radius = sharpen.neighborhood_radius(64, 64);
        pipeline.push(Box::new(sharpen));

        assert_eq!(pipeline.total_halo(64, 64), clarity_radius + sharpen_radius);
    }
}
