use zenpixels::ColorPrimaries;
use zenpixels_convert::gamut::GamutMatrix;
use zenpixels_convert::oklab;

use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::scatter_gather::{gather_from_oklab, scatter_to_oklab};

/// Configuration for the filter pipeline.
#[derive(Clone, Debug)]
pub struct PipelineConfig {
    /// Color primaries (determines RGB↔LMS matrices).
    pub primaries: ColorPrimaries,
    /// Reference white luminance for HDR normalization.
    /// Use 1.0 for SDR, 203.0 for PQ (ITU-R BT.2408).
    pub reference_white: f32,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            primaries: ColorPrimaries::Bt709,
            reference_white: 1.0,
        }
    }
}

/// Errors from pipeline construction or execution.
#[derive(Debug)]
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
}

impl Pipeline {
    /// Create a new pipeline with the given configuration.
    pub fn new(config: PipelineConfig) -> Result<Self, PipelineError> {
        let m1 = oklab::rgb_to_lms_matrix(config.primaries)
            .ok_or(PipelineError::UnsupportedPrimaries(config.primaries))?;
        let m1_inv = oklab::lms_to_rgb_matrix(config.primaries)
            .ok_or(PipelineError::UnsupportedPrimaries(config.primaries))?;

        Ok(Self {
            config,
            filters: Vec::new(),
            m1,
            m1_inv,
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

    /// Apply the full pipeline: scatter → filters → gather.
    ///
    /// `src` is interleaved linear RGB(A) f32 data.
    /// `dst` is the output buffer (same size as src).
    /// `width` and `height` are image dimensions.
    /// `channels` is 3 (RGB) or 4 (RGBA).
    pub fn apply(
        &self,
        src: &[f32],
        dst: &mut [f32],
        width: u32,
        height: u32,
        channels: u32,
    ) -> Result<(), PipelineError> {
        let n = (width as usize) * (height as usize) * (channels as usize);
        if src.len() < n {
            return Err(PipelineError::BufferSize {
                expected: n,
                actual: src.len(),
            });
        }
        if dst.len() < n {
            return Err(PipelineError::BufferSize {
                expected: n,
                actual: dst.len(),
            });
        }

        // Scatter to planar Oklab
        let mut planes = if channels == 4 {
            OklabPlanes::with_alpha(width, height)
        } else {
            OklabPlanes::new(width, height)
        };
        scatter_to_oklab(
            src,
            &mut planes,
            channels,
            &self.m1,
            self.config.reference_white,
        );

        // Apply all filters
        for filter in &self.filters {
            filter.apply(&mut planes);
        }

        // Gather back to interleaved RGB
        gather_from_oklab(
            &planes,
            dst,
            channels,
            &self.m1_inv,
            self.config.reference_white,
        );

        Ok(())
    }

    /// Apply filters to already-scattered planar Oklab data.
    ///
    /// Use this when you want to manage scatter/gather yourself,
    /// or when chaining multiple pipelines on the same planar data.
    pub fn apply_planar(&self, planes: &mut OklabPlanes) {
        for filter in &self.filters {
            filter.apply(planes);
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
        pipeline.apply(&src, &mut dst, 64, 64, 3).unwrap();

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
        });
        assert!(result.is_err());
    }

    #[test]
    fn buffer_size_validation() {
        let pipeline = Pipeline::new(PipelineConfig::default()).unwrap();
        let src = vec![0.5f32; 10]; // too small
        let mut dst = vec![0.0f32; 64 * 64 * 3];
        let result = pipeline.apply(&src, &mut dst, 64, 64, 3);
        assert!(result.is_err());
    }
}
