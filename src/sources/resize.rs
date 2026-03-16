use alloc::boxed::Box;
use alloc::format;
use alloc::string::ToString;

use crate::Source;
use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::strip::{StripBuf, StripRef};

/// Streaming resize source wrapping [`zenresize::StreamingResize`].
///
/// Pulls strips from upstream, feeds rows into the resizer's ring buffer,
/// and accumulates output rows into output strips. The resize kernel's
/// ring buffer means only ~21 input rows (Lanczos3 worst case) are held
/// in memory at any time.
///
/// # Format requirements
///
/// Upstream must produce [`Rgba8`](PixelFormat::Rgba8). The resizer
/// handles sRGB→linear→premul conversion internally. Output is also
/// [`Rgba8`](PixelFormat::Rgba8).
pub struct ResizeSource {
    upstream: Box<dyn Source>,
    resizer: zenresize::StreamingResize,
    out_width: u32,
    out_height: u32,
    strip_height: u32,
    buf: StripBuf,
    y: u32,
    input_exhausted: bool,
    finished: bool,
    /// Leftover rows from the last upstream strip that haven't been pushed yet.
    pending_strip: Option<PendingStrip>,
}

/// Tracks remaining rows from an upstream strip.
struct PendingStrip {
    data: alloc::vec::Vec<u8>,
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

impl ResizeSource {
    /// Create a streaming resize source.
    ///
    /// `config` must have matching `in_width`/`in_height` to the upstream source.
    /// `strip_height` controls the output strip batch size.
    pub fn new(
        upstream: Box<dyn Source>,
        config: &zenresize::ResizeConfig,
        strip_height: u32,
    ) -> Result<Self, PipeError> {
        if upstream.format() != PixelFormat::Rgba8 {
            return Err(PipeError::FormatMismatch {
                expected: PixelFormat::Rgba8,
                got: upstream.format(),
            });
        }
        if upstream.width() != config.in_width || upstream.height() != config.in_height {
            return Err(PipeError::DimensionMismatch(format!(
                "upstream {}x{} != config {}x{}",
                upstream.width(),
                upstream.height(),
                config.in_width,
                config.in_height,
            )));
        }

        let out_w = config.total_output_width();
        let out_h = config.total_output_height();
        let sh = strip_height.min(out_h);
        let resizer = zenresize::StreamingResize::new(config);

        Ok(Self {
            upstream,
            resizer,
            out_width: out_w,
            out_height: out_h,
            strip_height: sh,
            buf: StripBuf::new(out_w, sh, PixelFormat::Rgba8),
            y: 0,
            input_exhausted: false,
            finished: false,
            pending_strip: None,
        })
    }

    /// Create from a pre-built [`StreamingResize`](zenresize::StreamingResize).
    ///
    /// Used by the Layout node to leverage zenresize's built-in crop,
    /// padding, and orientation — all in one streaming pass.
    ///
    /// Upstream must produce [`Rgba8`](PixelFormat::Rgba8).
    pub fn from_streaming(
        upstream: Box<dyn Source>,
        resizer: zenresize::StreamingResize,
        strip_height: u32,
    ) -> Result<Self, PipeError> {
        if upstream.format() != PixelFormat::Rgba8 {
            return Err(PipeError::FormatMismatch {
                expected: PixelFormat::Rgba8,
                got: upstream.format(),
            });
        }
        // output_row_len is in bytes; RGBA8 = 4 bytes/pixel
        let out_w = (resizer.output_row_len() / 4) as u32;
        let out_h = resizer.total_output_height();
        let sh = strip_height.min(out_h);
        Ok(Self {
            upstream,
            resizer,
            out_width: out_w,
            out_height: out_h,
            strip_height: sh,
            buf: StripBuf::new(out_w, sh, PixelFormat::Rgba8),
            y: 0,
            input_exhausted: false,
            finished: false,
            pending_strip: None,
        })
    }

    /// Push one input row from the pending strip or upstream.
    /// Returns Ok(true) if a row was pushed, Ok(false) if input exhausted.
    fn push_one_row(&mut self) -> Result<bool, PipeError> {
        // Try pending strip first
        if let Some(ref mut pending) = self.pending_strip {
            if pending.remaining() > 0 {
                let row = pending.row(pending.next_row);
                self.resizer
                    .push_row(row)
                    .map_err(|e| PipeError::Resize(e.to_string()))?;
                pending.next_row += 1;
                if pending.remaining() == 0 {
                    self.pending_strip = None;
                }
                return Ok(true);
            }
            self.pending_strip = None;
        }

        // Pull from upstream
        if self.input_exhausted {
            return Ok(false);
        }

        let strip = self
            .upstream
            .next()
            .map_err(|e| PipeError::Op(e.to_string()))?;
        let Some(strip) = strip else {
            self.input_exhausted = true;
            return Ok(false);
        };

        // Save the strip and push first row
        let mut pending = PendingStrip {
            data: strip.data.to_vec(),
            stride: strip.stride,
            total_rows: strip.height,
            next_row: 0,
        };

        let row = pending.row(0);
        self.resizer
            .push_row(row)
            .map_err(|e| PipeError::Resize(e.to_string()))?;
        pending.next_row = 1;

        if pending.remaining() > 0 {
            self.pending_strip = Some(pending);
        }

        Ok(true)
    }

    /// Drain available output rows into the buffer.
    fn drain_output(&mut self) {
        while self.buf.rows_filled() < self.strip_height {
            if let Some(row) = self.resizer.next_output_row() {
                self.buf.push_row(row);
            } else {
                break;
            }
        }
    }
}

impl Source for ResizeSource {
    fn next(&mut self) -> Result<Option<StripRef<'_>>, PipeError> {
        if self.y >= self.out_height {
            return Ok(None);
        }

        let rows_wanted = self.strip_height.min(self.out_height - self.y);
        self.buf
            .reconfigure(self.out_width, rows_wanted, PixelFormat::Rgba8);
        self.buf.reset(self.y);

        // Feed input rows and drain output until we have enough
        loop {
            self.drain_output();
            if self.buf.rows_filled() >= rows_wanted {
                break;
            }

            if self.input_exhausted && !self.finished {
                // Signal end-of-input and drain remaining
                self.resizer.finish();
                self.finished = true;
                self.drain_output();
                break;
            }

            if self.finished {
                break;
            }

            // Push more input
            let pushed = self.push_one_row()?;
            if !pushed {
                // Input exhausted — finish and drain
                self.resizer.finish();
                self.finished = true;
                self.drain_output();
                break;
            }
        }

        if self.buf.rows_filled() == 0 {
            return Ok(None);
        }

        self.y += self.buf.rows_filled();
        Ok(Some(self.buf.as_ref()))
    }

    fn width(&self) -> u32 {
        self.out_width
    }
    fn height(&self) -> u32 {
        self.out_height
    }
    fn format(&self) -> PixelFormat {
        PixelFormat::Rgba8
    }
}

// =============================================================================
// ResizeF32Source — f32 path, eliminates sRGB roundtrip for downstream f32
// =============================================================================

/// Streaming resize in f32 linear space — no sRGB conversions.
///
/// Uses [`StreamingResize`]'s f32 internal path: input rows are pushed
/// via [`push_row_f32`](zenresize::StreamingResize::push_row_f32) and
/// output rows are pulled via [`next_output_row_f32`](zenresize::StreamingResize::next_output_row_f32).
///
/// Input/output format: [`Rgbaf32Linear`](PixelFormat::Rgbaf32Linear).
///
/// Use this instead of [`ResizeSource`] when downstream needs f32 data
/// (e.g., filters) to avoid an sRGB encode→decode roundtrip that wastes
/// ~15% of pipeline instructions.
pub struct ResizeF32Source {
    upstream: Box<dyn Source>,
    resizer: zenresize::StreamingResize,
    out_width: u32,
    out_height: u32,
    strip_height: u32,
    buf: StripBuf,
    y: u32,
    input_exhausted: bool,
    finished: bool,
    pending_strip: Option<PendingStrip>,
}

impl ResizeF32Source {
    /// Create a streaming f32 resize source.
    ///
    /// Upstream must produce [`Rgbaf32Linear`](PixelFormat::Rgbaf32Linear).
    /// The config must use an f32 linear pixel descriptor.
    pub fn new(
        upstream: Box<dyn Source>,
        config: &zenresize::ResizeConfig,
        strip_height: u32,
    ) -> Result<Self, PipeError> {
        if upstream.format() != PixelFormat::Rgbaf32Linear {
            return Err(PipeError::FormatMismatch {
                expected: PixelFormat::Rgbaf32Linear,
                got: upstream.format(),
            });
        }

        let out_w = config.total_output_width();
        let out_h = config.total_output_height();
        let sh = strip_height.min(out_h);
        let resizer = zenresize::StreamingResize::new(config);

        Ok(Self {
            upstream,
            resizer,
            out_width: out_w,
            out_height: out_h,
            strip_height: sh,
            buf: StripBuf::new(out_w, sh, PixelFormat::Rgbaf32Linear),
            y: 0,
            input_exhausted: false,
            finished: false,
            pending_strip: None,
        })
    }

    fn push_one_row(&mut self) -> Result<bool, PipeError> {
        if let Some(ref mut pending) = self.pending_strip {
            if pending.remaining() > 0 {
                let row = pending.row(pending.next_row);
                let row_f32: &[f32] = bytemuck::cast_slice(row);
                self.resizer
                    .push_row_f32(row_f32)
                    .map_err(|e| PipeError::Resize(e.to_string()))?;
                pending.next_row += 1;
                if pending.remaining() == 0 {
                    self.pending_strip = None;
                }
                return Ok(true);
            }
            self.pending_strip = None;
        }

        if self.input_exhausted {
            return Ok(false);
        }

        let strip = self
            .upstream
            .next()
            .map_err(|e| PipeError::Op(e.to_string()))?;
        let Some(strip) = strip else {
            self.input_exhausted = true;
            return Ok(false);
        };

        let mut pending = PendingStrip {
            data: strip.data.to_vec(),
            stride: strip.stride,
            total_rows: strip.height,
            next_row: 0,
        };

        let row = pending.row(0);
        let row_f32: &[f32] = bytemuck::cast_slice(row);
        self.resizer
            .push_row_f32(row_f32)
            .map_err(|e| PipeError::Resize(e.to_string()))?;
        pending.next_row = 1;

        if pending.remaining() > 0 {
            self.pending_strip = Some(pending);
        }

        Ok(true)
    }

    fn drain_output(&mut self) {
        while self.buf.rows_filled() < self.strip_height {
            if let Some(row_f32) = self.resizer.next_output_row_f32() {
                let row_u8: &[u8] = bytemuck::cast_slice(row_f32);
                self.buf.push_row(row_u8);
            } else {
                break;
            }
        }
    }
}

impl Source for ResizeF32Source {
    fn next(&mut self) -> Result<Option<StripRef<'_>>, PipeError> {
        if self.y >= self.out_height {
            return Ok(None);
        }

        let rows_wanted = self.strip_height.min(self.out_height - self.y);
        self.buf
            .reconfigure(self.out_width, rows_wanted, PixelFormat::Rgbaf32Linear);
        self.buf.reset(self.y);

        loop {
            self.drain_output();
            if self.buf.rows_filled() >= rows_wanted {
                break;
            }

            if self.input_exhausted && !self.finished {
                self.resizer.finish();
                self.finished = true;
                self.drain_output();
                break;
            }

            if self.finished {
                break;
            }

            let pushed = self.push_one_row()?;
            if !pushed {
                self.resizer.finish();
                self.finished = true;
                self.drain_output();
                break;
            }
        }

        if self.buf.rows_filled() == 0 {
            return Ok(None);
        }

        self.y += self.buf.rows_filled();
        Ok(Some(self.buf.as_ref()))
    }

    fn width(&self) -> u32 {
        self.out_width
    }
    fn height(&self) -> u32 {
        self.out_height
    }
    fn format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32Linear
    }
}
