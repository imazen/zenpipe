//! Streaming filter source wrapping [`zenfilters::Pipeline`].
//!
//! Applies a zenfilters pipeline to each strip pulled from upstream.
//! Only supports per-pixel filter pipelines (no neighborhood filters).
//! For pipelines containing neighborhood filters, use
//! [`MaterializedSource`](super::MaterializedSource) to materialize first.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::Source;
use crate::error::PipeError;
use crate::format::{self, PixelFormat, PixelFormatExt};
use crate::strip::Strip;

/// Applies a [`zenfilters::Pipeline`] strip-by-strip.
///
/// Input and output format is [`Rgbaf32Linear`](format::RGBAF32_LINEAR)
/// (interleaved RGBA f32, straight alpha, linear light). The graph compiler
/// inserts format conversions automatically.
///
/// # Panics
///
/// Construction panics if the pipeline contains neighborhood filters
/// (use materialized path instead).
pub struct FilterSource {
    upstream: Box<dyn Source>,
    pipeline: zenfilters::Pipeline,
    ctx: zenfilters::FilterContext,
    /// Output buffer for pipeline.apply().
    dst_buf: Vec<f32>,
    width: u32,
    height: u32,
}

impl FilterSource {
    /// Wrap an upstream source with a per-pixel filter pipeline.
    ///
    /// Upstream must produce [`Rgbaf32Linear`](format::RGBAF32_LINEAR).
    pub fn new(
        upstream: Box<dyn Source>,
        pipeline: zenfilters::Pipeline,
    ) -> Result<Self, PipeError> {
        if upstream.format() != format::RGBAF32_LINEAR {
            return Err(PipeError::FormatMismatch {
                expected: format::RGBAF32_LINEAR,
                got: upstream.format(),
            });
        }
        assert!(
            !pipeline.has_neighborhood_filter(),
            "FilterSource only supports per-pixel pipelines; \
             use MaterializedSource for neighborhood filters"
        );
        let w = upstream.width();
        let h = upstream.height();
        Ok(Self {
            upstream,
            pipeline,
            ctx: zenfilters::FilterContext::new(),
            dst_buf: Vec::new(),
            width: w,
            height: h,
        })
    }
}

impl Source for FilterSource {
    fn next(&mut self) -> Result<Option<Strip<'_>>, PipeError> {
        let strip = self.upstream.next()?;
        let Some(strip) = strip else {
            return Ok(None);
        };

        let w = strip.width();
        let h = strip.rows();
        let pixels = w as usize * h as usize;

        // Apply filter pipeline directly from upstream strip data.
        // strip.as_strided_bytes() borrows self.upstream; pipeline/dst_buf/ctx are disjoint fields.
        let in_f32: &[f32] = bytemuck::cast_slice(strip.as_strided_bytes());
        self.dst_buf.resize(pixels * 4, 0.0);
        self.pipeline
            .apply(in_f32, &mut self.dst_buf, w, h, 4, &mut self.ctx)
            .map_err(|e| PipeError::Op(e.to_string()))?;

        let stride = format::RGBAF32_LINEAR.row_bytes(w);
        let data: &[u8] = bytemuck::cast_slice(&self.dst_buf);

        Ok(Some(Strip::new(
            data,
            w,
            h,
            stride,
            format::RGBAF32_LINEAR,
        )?))
    }

    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
    fn format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR
    }
}
