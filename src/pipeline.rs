use whereat::{At, at};
use zenpixels::ColorPrimaries;
use zenpixels_convert::gamut::GamutMatrix;
use zenpixels_convert::oklab;

use crate::context::FilterContext;
use crate::filter::Filter;
use crate::gamut_lut::GamutBoundaryLut;
use crate::gamut_map::GamutMapping;
use crate::planes::OklabPlanes;
use crate::scatter_gather::{gather_from_oklab, scatter_to_oklab};

/// Compute the number of rows per strip for L3-friendly processing.
///
/// Targets keeping all planar data (L + a + b + optional alpha) under
/// ~4 MB, which fits comfortably in L3 on most CPUs (8–32 MB).
pub(crate) fn strip_height(width: u32, has_alpha: bool) -> usize {
    const TARGET_BYTES: usize = 4 * 1024 * 1024;
    let plane_count = if has_alpha { 4 } else { 3 };
    let bytes_per_row = (width as usize) * plane_count * core::mem::size_of::<f32>();
    (TARGET_BYTES / bytes_per_row.max(1)).clamp(8, 2048)
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

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            primaries: ColorPrimaries::Bt709,
            reference_white: 1.0,
            gamut_mapping: GamutMapping::Clip,
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

impl std::error::Error for PipelineError {}

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

    /// Add a filter to the pipeline.
    pub fn push(&mut self, filter: Box<dyn Filter>) {
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

    /// Apply the full pipeline: scatter → filters → gather.
    ///
    /// `src` is interleaved linear RGB(A) f32 data.
    /// `dst` is the output buffer (same size as src).
    /// `width` and `height` are image dimensions.
    /// `channels` is 3 (RGB) or 4 (RGBA).
    /// `ctx` provides reusable scratch buffers — pass a persistent
    /// `FilterContext` to avoid per-call allocations.
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

        // Strip processing: keep planar data in L3 cache by processing
        // horizontal strips instead of the full frame.
        // Neighborhood filters need full-frame access, so fall back then.
        if !self.has_neighborhood_filter() {
            return self.apply_stripped(src, dst, width, height, channels, ctx);
        }

        // Full-frame fallback for pipelines with neighborhood filters
        let mut planes = if channels == 4 {
            OklabPlanes::from_ctx_with_alpha(ctx, width, height)
        } else {
            OklabPlanes::from_ctx(ctx, width, height)
        };
        scatter_to_oklab(
            src,
            &mut planes,
            channels,
            &self.m1,
            self.config.reference_white,
        );
        self.apply_planar(&mut planes, ctx);
        gather_from_oklab(
            &planes,
            dst,
            channels,
            &self.m1_inv,
            self.config.reference_white,
        );
        planes.return_to_ctx(ctx);

        Ok(())
    }

    /// Strip-process: scatter, filter, gather in L3-sized horizontal strips.
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
        let strip_h = strip_height(width, has_alpha);

        for y_start in (0..height as usize).step_by(strip_h) {
            let y_end = (y_start + strip_h).min(height as usize);
            let strip_rows = (y_end - y_start) as u32;
            let offset = y_start * w * ch;
            let len = (strip_rows as usize) * w * ch;

            let mut planes = if has_alpha {
                OklabPlanes::from_ctx_with_alpha(ctx, width, strip_rows)
            } else {
                OklabPlanes::from_ctx(ctx, width, strip_rows)
            };

            scatter_to_oklab(
                &src[offset..offset + len],
                &mut planes,
                channels,
                &self.m1,
                self.config.reference_white,
            );
            self.apply_planar(&mut planes, ctx);
            gather_from_oklab(
                &planes,
                &mut dst[offset..offset + len],
                channels,
                &self.m1_inv,
                self.config.reference_white,
            );

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
}
