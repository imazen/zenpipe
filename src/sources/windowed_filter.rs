//! Windowed filter source — applies neighborhood filters via sliding window
//! materialization instead of full-frame materialization.
//!
//! For each output strip, materializes a window of `strip_height + 2 * overlap`
//! rows, applies the filter pipeline, and yields only the center `strip_height`
//! rows. The overlap rows provide context for correct neighborhood filter output.
//!
//! Memory usage: `O(window_rows * width)` instead of `O(height * width)`.
//! For a 4K image with overlap=128 and strip_height=64: ~15% of full
//! materialization memory.

use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use crate::Source;
use crate::error::PipeError;
use crate::format::{self, PixelFormat};
use crate::strip::{Strip, StripBuf};

/// Default overlap in rows. Covers Sharpen (3), Bilateral (6),
/// Clarity (48), Brilliance (30), and most LocalToneMap (90).
/// Users should increase for filters with larger radii (e.g., Dehaze).
pub const DEFAULT_OVERLAP: u32 = 128;

/// Applies a [`zenfilters::Pipeline`] via sliding window materialization.
///
/// Instead of materializing the entire image, buffers a window of
/// `strip_height + 2 * overlap` rows. Each window is processed by the
/// filter pipeline, and only the center `strip_height` rows are emitted.
/// The `overlap` rows on each side provide neighborhood context for
/// correct filter output.
///
/// Input and output format is [`Rgbaf32Linear`](format::RGBAF32_LINEAR).
pub struct WindowedFilterSource {
    upstream: Box<dyn Source>,
    pipeline: zenfilters::Pipeline,
    ctx: zenfilters::FilterContext,
    overlap: u32,
    strip_height: u32,
    width: u32,
    total_height: u32,

    /// Accumulated rows from upstream (interleaved RGBA f32).
    /// Indexed as `row_buf[local_row * row_len .. (local_row+1) * row_len]`.
    row_buf: Vec<f32>,
    /// Row length in f32 values (width * 4).
    row_len: usize,
    /// Global y of the first row currently in row_buf.
    buf_start_y: u32,
    /// Number of valid rows in row_buf.
    buf_rows: u32,

    /// Scratch for filter output.
    out_f32: Vec<f32>,

    /// Output strip buffer (u8 view of f32 data).
    out_strip: StripBuf,

    /// Current output y position.
    output_y: u32,

    /// Next upstream row to consume.
    upstream_y: u32,
    upstream_done: bool,

    /// Pending upstream strip data.
    pending: Option<PendingStrip>,
}

struct PendingStrip {
    data: Vec<u8>,
    stride: usize,
    total_rows: u32,
    next_row: u32,
}

impl PendingStrip {
    fn row(&self, r: u32) -> &[u8] {
        let start = r as usize * self.stride;
        &self.data[start..start + self.stride]
    }

    fn remaining(&self) -> u32 {
        self.total_rows - self.next_row
    }
}

impl WindowedFilterSource {
    /// Create a windowed filter source.
    ///
    /// `overlap` is the number of rows of context on each side of the output
    /// strip. Must be ≥ the maximum neighborhood radius of any filter in the
    /// pipeline. Use [`DEFAULT_OVERLAP`] (128) for common filters.
    ///
    /// Upstream must produce [`Rgbaf32Linear`](format::RGBAF32_LINEAR).
    pub fn new(
        upstream: Box<dyn Source>,
        pipeline: zenfilters::Pipeline,
        overlap: u32,
    ) -> Result<Self, PipeError> {
        if upstream.format() != format::RGBAF32_LINEAR {
            return Err(PipeError::FormatMismatch {
                expected: format::RGBAF32_LINEAR,
                got: upstream.format(),
            });
        }

        let width = upstream.width();
        let height = upstream.height();
        let row_len = width as usize * 4; // RGBA f32

        // Scale strip height to target ~75% utilization:
        // utilization = strip / (strip + 2*overlap). For 75%: strip = 6*overlap.
        // Minimum 64, capped to image height. Large strips amortize overlap cost.
        let strip_height = if overlap > 0 {
            (overlap * 6).max(64).min(height)
        } else {
            64.min(height)
        };

        // Max window = strip_height + 2*overlap, capped to image height
        let max_window = (strip_height + 2 * overlap).min(height);

        Ok(Self {
            upstream,
            pipeline,
            ctx: zenfilters::FilterContext::new(),
            overlap,
            strip_height,
            width,
            total_height: height,
            row_buf: vec![0.0f32; row_len * max_window as usize],
            row_len,
            buf_start_y: 0,
            buf_rows: 0,
            out_f32: vec![0.0f32; row_len * max_window as usize],
            out_strip: StripBuf::new(width, strip_height, format::RGBAF32_LINEAR),
            output_y: 0,
            upstream_y: 0,
            upstream_done: false,
            pending: None,
        })
    }

    /// Pull one row from upstream into the row buffer at the given local index.
    fn pull_row_into(&mut self, local_idx: u32) -> Result<bool, PipeError> {
        let row_f32 = self.pull_upstream_row_f32()?;
        let Some(row_f32) = row_f32 else {
            return Ok(false);
        };
        let dst_start = local_idx as usize * self.row_len;
        self.row_buf[dst_start..dst_start + self.row_len].copy_from_slice(&row_f32[..self.row_len]);
        Ok(true)
    }

    /// Pull one row of f32 data from upstream.
    fn pull_upstream_row_f32(&mut self) -> Result<Option<Vec<f32>>, PipeError> {
        // Try pending strip
        if let Some(ref mut pending) = self.pending {
            if pending.remaining() > 0 {
                let row_u8 = pending.row(pending.next_row);
                let row_f32: &[f32] = bytemuck::cast_slice(row_u8);
                let result = row_f32[..self.row_len].to_vec();
                pending.next_row += 1;
                self.upstream_y += 1;
                if pending.remaining() == 0 {
                    self.pending = None;
                }
                return Ok(Some(result));
            }
            self.pending = None;
        }

        if self.upstream_done {
            return Ok(None);
        }

        let strip = self.upstream.next()?;
        let Some(strip) = strip else {
            self.upstream_done = true;
            return Ok(None);
        };

        let mut pending = PendingStrip {
            data: strip.as_strided_bytes().to_vec(),
            stride: strip.stride(),
            total_rows: strip.rows(),
            next_row: 0,
        };

        let row_u8 = pending.row(0);
        let row_f32: &[f32] = bytemuck::cast_slice(row_u8);
        let result = row_f32[..self.row_len].to_vec();
        pending.next_row = 1;
        self.upstream_y += 1;

        if pending.remaining() > 0 {
            self.pending = Some(pending);
        }

        Ok(Some(result))
    }

    /// Prepare the window for output rows starting at `output_y`.
    /// Returns the window height and the offset of the first output row within the window.
    fn prepare_window(&mut self) -> Result<(u32, u32), PipeError> {
        let y = self.output_y;

        // Window bounds (global coordinates)
        let window_top = y.saturating_sub(self.overlap);
        let window_bottom = (y + self.strip_height + self.overlap).min(self.total_height);
        let window_height = window_bottom - window_top;
        let output_offset = y - window_top; // offset of first output row in window

        // Discard rows before window_top
        if window_top > self.buf_start_y && self.buf_rows > 0 {
            let discard = (window_top - self.buf_start_y).min(self.buf_rows);
            if discard > 0 && discard < self.buf_rows {
                // Shift remaining rows to front
                let keep_start = discard as usize * self.row_len;
                let keep_len = (self.buf_rows - discard) as usize * self.row_len;
                self.row_buf
                    .copy_within(keep_start..keep_start + keep_len, 0);
            }
            self.buf_start_y += discard;
            self.buf_rows = self.buf_rows.saturating_sub(discard);
        }

        // Fill window: pull rows from upstream until we have enough
        let rows_needed = window_bottom - self.buf_start_y;
        while self.buf_rows < rows_needed {
            if !self.pull_row_into(self.buf_rows)? {
                break;
            }
            self.buf_rows += 1;
        }

        Ok((window_height.min(self.buf_rows), output_offset))
    }
}

impl Source for WindowedFilterSource {
    fn next(&mut self) -> Result<Option<Strip<'_>>, PipeError> {
        if self.output_y >= self.total_height {
            return Ok(None);
        }

        let output_rows = self.strip_height.min(self.total_height - self.output_y);
        let (window_height, output_offset) = self.prepare_window()?;

        if window_height == 0 {
            return Ok(None);
        }

        // Apply filter pipeline to the window
        let window_pixels = self.width as usize * window_height as usize;
        let window_f32_len = window_pixels * 4;

        self.out_f32.resize(window_f32_len, 0.0);
        self.pipeline
            .apply(
                &self.row_buf[..window_f32_len],
                &mut self.out_f32[..window_f32_len],
                self.width,
                window_height,
                4,
                &mut self.ctx,
            )
            .map_err(|e| PipeError::Op(e.to_string()))?;

        // Extract center rows into output strip
        let actual_rows = output_rows.min(window_height.saturating_sub(output_offset));
        self.out_strip
            .reconfigure(self.width, actual_rows, format::RGBAF32_LINEAR);
        self.out_strip.reset();

        for r in 0..actual_rows {
            let src_row = (output_offset + r) as usize;
            let src_start = src_row * self.row_len;
            let src_f32 = &self.out_f32[src_start..src_start + self.row_len];
            let src_u8: &[u8] = bytemuck::cast_slice(src_f32);
            self.out_strip.push_row(src_u8);
        }

        self.output_y += actual_rows;

        if self.out_strip.rows_filled() == 0 {
            return Ok(None);
        }

        Ok(Some(self.out_strip.as_strip()))
    }

    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.total_height
    }
    fn format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR
    }
}
