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
use crate::format::PixelFormat;
use crate::strip::StripRef;

/// Applies a [`zenfilters::Pipeline`] strip-by-strip.
///
/// Input and output format is [`Rgbaf32Linear`](PixelFormat::Rgbaf32Linear)
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
    /// Copy of upstream strip data (f32), needed to break borrow chain.
    src_buf: Vec<f32>,
    /// Output buffer for pipeline.apply().
    dst_buf: Vec<f32>,
    width: u32,
    height: u32,
}

impl FilterSource {
    /// Wrap an upstream source with a per-pixel filter pipeline.
    ///
    /// Upstream must produce [`Rgbaf32Linear`](PixelFormat::Rgbaf32Linear).
    pub fn new(
        upstream: Box<dyn Source>,
        pipeline: zenfilters::Pipeline,
    ) -> Result<Self, PipeError> {
        if upstream.format() != PixelFormat::Rgbaf32Linear {
            return Err(PipeError::FormatMismatch {
                expected: PixelFormat::Rgbaf32Linear,
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
            src_buf: Vec::new(),
            dst_buf: Vec::new(),
            width: w,
            height: h,
        })
    }
}

impl Source for FilterSource {
    fn next(&mut self) -> Result<Option<StripRef<'_>>, PipeError> {
        let strip = self.upstream.next()?;
        let Some(strip) = strip else {
            return Ok(None);
        };

        let w = strip.width;
        let h = strip.height;
        let y = strip.y;
        let pixels = w as usize * h as usize;

        // Copy upstream f32 data to break the borrow on self.upstream
        let in_f32: &[f32] = bytemuck::cast_slice(strip.data);
        self.src_buf.resize(pixels * 4, 0.0);
        self.src_buf.copy_from_slice(in_f32);

        // Apply filter pipeline
        self.dst_buf.resize(pixels * 4, 0.0);
        self.pipeline
            .apply(&self.src_buf, &mut self.dst_buf, w, h, 4, &mut self.ctx)
            .map_err(|e| PipeError::Op(e.to_string()))?;

        let stride = PixelFormat::Rgbaf32Linear.row_bytes(w);
        let data: &[u8] = bytemuck::cast_slice(&self.dst_buf);

        Ok(Some(StripRef {
            data,
            width: w,
            height: h,
            stride,
            y,
            format: PixelFormat::Rgbaf32Linear,
        }))
    }

    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
    fn format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32Linear
    }
}
