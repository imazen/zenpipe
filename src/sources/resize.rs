use alloc::boxed::Box;
use alloc::format;
use alloc::string::ToString;

use crate::Source;
#[allow(unused_imports)]
use whereat::at;

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
/// Upstream must produce `RGBA8_SRGB` or `RGBA8_LINEAR` (both 4 bytes/pixel;
/// the format must match the descriptor the resizer was configured with).
/// For sRGB the resizer handles sRGB→linear→premul conversion internally;
/// for Linear it resamples the raw values (zenpipe#41 gain-map sidecars).
/// Output carries the same format as the input.
pub struct ResizeSource {
    upstream: Box<dyn Source>,
    resizer: zenresize::StreamingResize,
    out_width: u32,
    out_height: u32,
    strip_height: u32,
    buf: StripBuf,
    format: PixelFormat,
    y: u32,
    pending: PendingRows<u8>,
    input_exhausted: bool,
    finished: bool,
}

/// Rows of an upstream strip the resizer could not take yet.
///
/// The ring buffer is sized for a 16-row batch, but the [`Source`] contract
/// does not bound upstream strip height (a decoder or a materialized frame may
/// hand over the whole image at once). Rows are pushed one at a time with
/// output drained in between; when the output strip is full while the resizer
/// still holds undrained output, the rest of the upstream strip is copied here
/// and pushed on the next pull.
struct PendingRows<T> {
    data: alloc::vec::Vec<T>,
    row_len: usize,
    next: usize,
}

impl<T: Copy> PendingRows<T> {
    fn new() -> Self {
        Self {
            data: alloc::vec::Vec::new(),
            row_len: 0,
            next: 0,
        }
    }

    fn peek(&self) -> Option<&[T]> {
        let start = self.next * self.row_len;
        if self.row_len == 0 || start >= self.data.len() {
            return None;
        }
        Some(&self.data[start..start + self.row_len])
    }

    fn advance(&mut self) {
        self.next += 1;
        if self.next * self.row_len >= self.data.len() {
            self.data.clear();
            self.next = 0;
        }
    }

    fn stash<'a>(&mut self, rows: impl Iterator<Item = &'a [T]>)
    where
        T: 'a,
    {
        self.data.clear();
        self.next = 0;
        for row in rows {
            self.row_len = row.len();
            self.data.extend_from_slice(row);
        }
    }
}

/// Drain ready RGBA8 output rows into `buf`; `true` when the resizer holds
/// no undrained output, so the next input row cannot overflow its ring.
fn drain_u8(
    resizer: &mut zenresize::StreamingResize,
    buf: &mut StripBuf,
    strip_height: u32,
) -> bool {
    while buf.rows_filled() < strip_height {
        match resizer.next_output_row() {
            Some(row) => {
                buf.push_row(row);
            }
            None => break,
        }
    }
    resizer.output_rows_available() == 0
}

/// [`drain_u8`] for the f32 path.
fn drain_f32(
    resizer: &mut zenresize::StreamingResize,
    buf: &mut StripBuf,
    strip_height: u32,
) -> bool {
    while buf.rows_filled() < strip_height {
        match resizer.next_output_row_f32() {
            Some(row) => {
                buf.push_row(bytemuck::cast_slice(row));
            }
            None => break,
        }
    }
    resizer.output_rows_available() == 0
}

/// Working formats [`ResizeSource`] accepts: 4-byte/pixel RGBA, sRGB or
/// Linear transfer. The transfer tag tells zenresize whether to gamma-decode
/// (sRGB) or pass values through raw (Linear).
fn accepted_format(fmt: PixelFormat) -> bool {
    fmt == format::RGBA8_SRGB || fmt == format::RGBA8_LINEAR
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
    ) -> crate::PipeResult<Self> {
        if !accepted_format(upstream.format()) {
            return Err(at!(PipeError::FormatMismatch {
                expected: format::RGBA8_SRGB,
                got: upstream.format(),
            }));
        }
        if upstream.width() != config.in_width || upstream.height() != config.in_height {
            return Err(at!(PipeError::DimensionMismatch(format!(
                "upstream {}x{} != config {}x{}",
                upstream.width(),
                upstream.height(),
                config.in_width,
                config.in_height,
            ))));
        }

        let out_w = config.total_output_width();
        let out_h = config.total_output_height();
        let sh = strip_height.min(out_h);
        // batch_hint sizes the ring buffer for push_upstream_strip():
        // we push an entire upstream strip at once, so the ring buffer
        // must hold at least that many extra rows beyond the filter taps.
        let resizer = zenresize::StreamingResize::with_batch_hint(config, strip_height);

        let fmt = upstream.format();
        Ok(Self {
            upstream,
            resizer,
            out_width: out_w,
            out_height: out_h,
            strip_height: sh,
            buf: StripBuf::new(out_w, sh, fmt),
            format: fmt,
            y: 0,
            pending: PendingRows::new(),
            input_exhausted: false,
            finished: false,
        })
    }

    /// Create from a pre-built [`StreamingResize`](zenresize::StreamingResize).
    ///
    /// Used by the Layout node to leverage zenresize's built-in crop,
    /// padding, and orientation — all in one streaming pass.
    ///
    /// Upstream must produce `RGBA8_SRGB` or `RGBA8_LINEAR`, matching the
    /// descriptor the resizer was configured with.
    pub fn from_streaming(
        upstream: Box<dyn Source>,
        resizer: zenresize::StreamingResize,
        strip_height: u32,
    ) -> crate::PipeResult<Self> {
        if !accepted_format(upstream.format()) {
            return Err(at!(PipeError::FormatMismatch {
                expected: format::RGBA8_SRGB,
                got: upstream.format(),
            }));
        }
        // output_row_len is in bytes; RGBA8 = 4 bytes/pixel
        let out_w = (resizer.output_row_len() / 4) as u32;
        let out_h = resizer.total_output_height();
        let sh = strip_height.min(out_h);
        let fmt = upstream.format();
        Ok(Self {
            upstream,
            resizer,
            out_width: out_w,
            out_height: out_h,
            strip_height: sh,
            buf: StripBuf::new(out_w, sh, fmt),
            format: fmt,
            y: 0,
            pending: PendingRows::new(),
            input_exhausted: false,
            finished: false,
        })
    }

    /// Push input rows into the resizer: rows left over from the previous
    /// upstream strip first, else a new upstream strip. Rows go straight from
    /// the upstream strip; only rows that cannot be pushed before the output
    /// strip fills are copied (see [`PendingRows`]).
    fn push_upstream_strip(&mut self) -> crate::PipeResult<bool> {
        if self.pending.peek().is_some() {
            while let Some(row) = self.pending.peek() {
                if !drain_u8(&mut self.resizer, &mut self.buf, self.strip_height) {
                    break;
                }
                self.resizer
                    .push_row(row)
                    .map_err(|e| at!(PipeError::Resize(e.to_string())))?;
                self.pending.advance();
            }
            return Ok(true);
        }
        if self.input_exhausted {
            return Ok(false);
        }

        let strip = self.upstream.next()?;
        let Some(strip) = strip else {
            self.input_exhausted = true;
            return Ok(false);
        };

        for r in 0..strip.rows() {
            if !drain_u8(&mut self.resizer, &mut self.buf, self.strip_height) {
                self.pending.stash((r..strip.rows()).map(|r| strip.row(r)));
                break;
            }
            self.resizer
                .push_row(strip.row(r))
                .map_err(|e| at!(PipeError::Resize(e.to_string())))?;
        }

        Ok(true)
    }

    /// Drain available output rows into the buffer.
    fn drain_output(&mut self) {
        drain_u8(&mut self.resizer, &mut self.buf, self.strip_height);
    }
}

impl Source for ResizeSource {
    fn next(&mut self) -> crate::PipeResult<Option<Strip<'_>>> {
        if self.y >= self.out_height {
            return Ok(None);
        }

        let rows_wanted = self.strip_height.min(self.out_height - self.y);
        self.buf
            .reconfigure(self.out_width, rows_wanted, self.format);
        self.buf.reset();

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
        self.format
    }
    fn color_context(&self) -> Option<alloc::sync::Arc<zenpixels::ColorContext>> {
        self.upstream.color_context()
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
    pending: PendingRows<f32>,
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
    ) -> crate::PipeResult<Self> {
        if upstream.format() != format::RGBAF32_LINEAR {
            return Err(at!(PipeError::FormatMismatch {
                expected: format::RGBAF32_LINEAR,
                got: upstream.format(),
            }));
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
            pending: PendingRows::new(),
            input_exhausted: false,
            finished: false,
        })
    }

    /// Push input rows into the resizer, as [`ResizeSource`] does. Rows go
    /// straight from the upstream strip (strip.data borrows self.upstream
    /// while self.resizer is a disjoint field); only rows that cannot be
    /// pushed before the output strip fills are copied.
    fn push_upstream_strip(&mut self) -> crate::PipeResult<bool> {
        if self.pending.peek().is_some() {
            while let Some(row) = self.pending.peek() {
                if !drain_f32(&mut self.resizer, &mut self.buf, self.strip_height) {
                    break;
                }
                self.resizer
                    .push_row_f32(row)
                    .map_err(|e| at!(PipeError::Resize(e.to_string())))?;
                self.pending.advance();
            }
            return Ok(true);
        }
        if self.input_exhausted {
            return Ok(false);
        }

        let strip = self.upstream.next()?;
        let Some(strip) = strip else {
            self.input_exhausted = true;
            return Ok(false);
        };

        for r in 0..strip.rows() {
            if !drain_f32(&mut self.resizer, &mut self.buf, self.strip_height) {
                self.pending
                    .stash((r..strip.rows()).map(|r| bytemuck::cast_slice(strip.row(r))));
                break;
            }
            self.resizer
                .push_row_f32(bytemuck::cast_slice(strip.row(r)))
                .map_err(|e| at!(PipeError::Resize(e.to_string())))?;
        }

        Ok(true)
    }

    fn drain_output(&mut self) {
        drain_f32(&mut self.resizer, &mut self.buf, self.strip_height);
    }
}

impl Source for ResizeF32Source {
    fn next(&mut self) -> crate::PipeResult<Option<Strip<'_>>> {
        if self.y >= self.out_height {
            return Ok(None);
        }

        let rows_wanted = self.strip_height.min(self.out_height - self.y);
        self.buf
            .reconfigure(self.out_width, rows_wanted, format::RGBAF32_LINEAR);
        self.buf.reset();

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
