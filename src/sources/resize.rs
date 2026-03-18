use alloc::boxed::Box;
use alloc::format;
use alloc::string::ToString;

use crate::Source;
use crate::error::PipeError;
use crate::format::{self, PixelFormat};
use crate::strip::{Strip, StripBuf};

/// Streaming resize source wrapping [`zenresize::StreamingResize`].
///
/// Pulls strips from upstream, feeds rows into the resizer's ring buffer,
/// and accumulates output rows into output strips. The resize kernel's
/// ring buffer means only ~21 input rows (Lanczos3 worst case) are held
/// in memory at any time.
///
/// # Format requirements
///
/// Upstream must produce `RGBA8_SRGB`. The resizer handles sRGB→linear→premul
/// conversion internally. Output is also `RGBA8_SRGB`.
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
        if upstream.format() != format::RGBA8_SRGB {
            return Err(PipeError::FormatMismatch {
                expected: format::RGBA8_SRGB,
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
        // batch_hint sizes the ring buffer for push_upstream_strip():
        // we push an entire upstream strip at once, so the ring buffer
        // must hold at least that many extra rows beyond the filter taps.
        let resizer = zenresize::StreamingResize::with_batch_hint(config, strip_height);

        Ok(Self {
            upstream,
            resizer,
            out_width: out_w,
            out_height: out_h,
            strip_height: sh,
            buf: StripBuf::new(out_w, sh, format::RGBA8_SRGB),
            y: 0,
            input_exhausted: false,
            finished: false,
        })
    }

    /// Create from a pre-built [`StreamingResize`](zenresize::StreamingResize).
    ///
    /// Used by the Layout node to leverage zenresize's built-in crop,
    /// padding, and orientation — all in one streaming pass.
    ///
    /// Upstream must produce `RGBA8_SRGB`.
    pub fn from_streaming(
        upstream: Box<dyn Source>,
        resizer: zenresize::StreamingResize,
        strip_height: u32,
    ) -> Result<Self, PipeError> {
        if upstream.format() != format::RGBA8_SRGB {
            return Err(PipeError::FormatMismatch {
                expected: format::RGBA8_SRGB,
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
            buf: StripBuf::new(out_w, sh, format::RGBA8_SRGB),
            y: 0,
            input_exhausted: false,
            finished: false,
        })
    }

    /// Pull a strip from upstream and push all its rows directly into the
    /// resizer. No intermediate copy.
    fn push_upstream_strip(&mut self) -> Result<bool, PipeError> {
        if self.input_exhausted {
            return Ok(false);
        }

        let strip = self.upstream.next()?;
        let Some(strip) = strip else {
            self.input_exhausted = true;
            return Ok(false);
        };

        for r in 0..strip.height() {
            let row = strip.row(r);
            self.resizer
                .push_row(row)
                .map_err(|e| PipeError::Resize(e.to_string()))?;
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
    fn next(&mut self) -> Result<Option<Strip<'_>>, PipeError> {
        if self.y >= self.out_height {
            return Ok(None);
        }

        let rows_wanted = self.strip_height.min(self.out_height - self.y);
        self.buf
            .reconfigure(self.out_width, rows_wanted, format::RGBA8_SRGB);
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
            let pushed = self.push_upstream_strip()?;
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
        Ok(Some(self.buf.as_strip()))
    }

    fn width(&self) -> u32 {
        self.out_width
    }
    fn height(&self) -> u32 {
        self.out_height
    }
    fn format(&self) -> PixelFormat {
        format::RGBA8_SRGB
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
/// Input/output format: `RGBAF32_LINEAR`.
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
}

impl ResizeF32Source {
    /// Create a streaming f32 resize source.
    ///
    /// Upstream must produce `RGBAF32_LINEAR`.
    /// The config must use an f32 linear pixel descriptor.
    pub fn new(
        upstream: Box<dyn Source>,
        config: &zenresize::ResizeConfig,
        strip_height: u32,
    ) -> Result<Self, PipeError> {
        if upstream.format() != format::RGBAF32_LINEAR {
            return Err(PipeError::FormatMismatch {
                expected: format::RGBAF32_LINEAR,
                got: upstream.format(),
            });
        }

        let out_w = config.total_output_width();
        let out_h = config.total_output_height();
        let sh = strip_height.min(out_h);
        let resizer = zenresize::StreamingResize::with_batch_hint(config, strip_height);

        Ok(Self {
            upstream,
            resizer,
            out_width: out_w,
            out_height: out_h,
            strip_height: sh,
            buf: StripBuf::new(out_w, sh, format::RGBAF32_LINEAR),
            y: 0,
            input_exhausted: false,
            finished: false,
        })
    }

    /// Pull a strip from upstream and push all its rows directly into the
    /// resizer. No intermediate copy — strip.data borrows self.upstream
    /// while self.resizer is a disjoint field.
    fn push_upstream_strip(&mut self) -> Result<bool, PipeError> {
        if self.input_exhausted {
            return Ok(false);
        }

        let strip = self.upstream.next()?;
        let Some(strip) = strip else {
            self.input_exhausted = true;
            return Ok(false);
        };

        // Push all rows directly from the upstream strip into the resizer.
        // NLL allows this: strip.data borrows *self.upstream, push_row_f32
        // borrows self.resizer — disjoint fields.
        for r in 0..strip.height() {
            let row = strip.row(r);
            let row_f32: &[f32] = bytemuck::cast_slice(row);
            self.resizer
                .push_row_f32(row_f32)
                .map_err(|e| PipeError::Resize(e.to_string()))?;
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
    fn next(&mut self) -> Result<Option<Strip<'_>>, PipeError> {
        if self.y >= self.out_height {
            return Ok(None);
        }

        let rows_wanted = self.strip_height.min(self.out_height - self.y);
        self.buf
            .reconfigure(self.out_width, rows_wanted, format::RGBAF32_LINEAR);
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

            // Push entire upstream strip, then drain output.
            let pushed = self.push_upstream_strip()?;
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
        Ok(Some(self.buf.as_strip()))
    }

    fn width(&self) -> u32 {
        self.out_width
    }
    fn height(&self) -> u32 {
        self.out_height
    }
    fn format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR
    }
}
