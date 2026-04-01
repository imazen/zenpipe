use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use crate::Source;
#[allow(unused_imports)]
use whereat::at;

use crate::format::PixelFormat;
use crate::strip::{Strip, StripBuf};

/// Streaming canvas expansion — places upstream on a larger canvas with padding.
///
/// Emits solid-color rows for padding above/below the content, and pads
/// columns left/right for rows containing the image. No materialization.
pub struct ExpandCanvasSource {
    upstream: Box<dyn Source>,
    canvas_w: u32,
    canvas_h: u32,
    /// Where content starts on canvas (clamped to >= 0).
    place_x: u32,
    place_y: u32,
    /// How many source columns/rows to skip (if placement was negative).
    skip_x: u32,
    skip_y: u32,
    /// Content region dimensions on canvas.
    content_w: u32,
    content_h: u32,
    /// Pre-built background row (reused, no per-row allocation).
    bg_row: Vec<u8>,
    format: PixelFormat,
    strip_height: u32,
    buf: StripBuf,
    out_y: u32,
    /// Buffered upstream strip for row-by-row consumption.
    pending: Option<PendingStrip>,
    /// Total upstream rows consumed (including skipped).
    upstream_rows_consumed: u32,
    upstream_exhausted: bool,
}

struct PendingStrip {
    data: Vec<u8>,
    stride: usize,
    height: u32,
    next_row: u32,
}

impl PendingStrip {
    fn row(&self, r: u32) -> &[u8] {
        let start = r as usize * self.stride;
        &self.data[start..start + self.stride]
    }

    fn remaining(&self) -> u32 {
        self.height - self.next_row
    }
}

impl ExpandCanvasSource {
    /// Place upstream content on a `canvas_w × canvas_h` canvas at offset
    /// `(place_x, place_y)`. Negative offsets crop the content; positive
    /// offsets add padding filled with `bg_pixel`.
    pub fn new(
        upstream: Box<dyn Source>,
        canvas_w: u32,
        canvas_h: u32,
        place_x: i32,
        place_y: i32,
        bg_pixel: [u8; 4],
    ) -> Self {
        let fmt = upstream.format();
        let src_w = upstream.width();
        let src_h = upstream.height();

        let skip_x = if place_x < 0 { (-place_x) as u32 } else { 0 };
        let skip_y = if place_y < 0 { (-place_y) as u32 } else { 0 };
        let dst_x = if place_x >= 0 { place_x as u32 } else { 0 };
        let dst_y = if place_y >= 0 { place_y as u32 } else { 0 };

        let content_w = src_w
            .saturating_sub(skip_x)
            .min(canvas_w.saturating_sub(dst_x));
        let content_h = src_h
            .saturating_sub(skip_y)
            .min(canvas_h.saturating_sub(dst_y));

        // Pre-build a full background row
        let bpp = fmt.bytes_per_pixel();
        let row_len = canvas_w as usize * bpp;
        let mut bg_row = vec![0u8; row_len];
        for chunk in bg_row.chunks_exact_mut(4) {
            chunk.copy_from_slice(&bg_pixel);
        }

        let sh = 16u32.min(canvas_h);
        Self {
            upstream,
            canvas_w,
            canvas_h,
            place_x: dst_x,
            place_y: dst_y,
            skip_x,
            skip_y,
            content_w,
            content_h,
            bg_row,
            format: fmt,
            strip_height: sh,
            buf: StripBuf::new(canvas_w, sh, fmt),
            out_y: 0,
            pending: None,
            upstream_rows_consumed: 0,
            upstream_exhausted: false,
        }
    }

    /// Pull the next upstream row, refilling the pending strip if needed.
    fn next_upstream_row(&mut self) -> crate::PipeResult<Option<()>> {
        // If pending strip has rows, use it
        if let Some(ref p) = self.pending
            && p.remaining() > 0
        {
            return Ok(Some(()));
        }
        self.pending = None;

        if self.upstream_exhausted {
            return Ok(None);
        }

        match self.upstream.next()? {
            Some(strip) => {
                self.pending = Some(PendingStrip {
                    data: strip.as_strided_bytes().to_vec(),
                    stride: strip.stride(),
                    height: strip.rows(),
                    next_row: 0,
                });
                Ok(Some(()))
            }
            None => {
                self.upstream_exhausted = true;
                Ok(None)
            }
        }
    }

    /// Consume one row from the pending strip.
    fn consume_pending_row(&mut self) -> Option<()> {
        if let Some(ref mut p) = self.pending
            && p.remaining() > 0
        {
            p.next_row += 1;
            self.upstream_rows_consumed += 1;
            return Some(());
        }
        None
    }

    /// Skip upstream rows that fall before the visible content region.
    fn skip_leading_rows(&mut self) -> crate::PipeResult<()> {
        while self.upstream_rows_consumed < self.skip_y {
            if self.next_upstream_row()?.is_none() {
                break;
            }
            self.consume_pending_row();
        }
        Ok(())
    }
}

impl Source for ExpandCanvasSource {
    fn next(&mut self) -> crate::PipeResult<Option<Strip<'_>>> {
        if self.out_y >= self.canvas_h {
            return Ok(None);
        }

        // Skip upstream rows before visible region (once)
        self.skip_leading_rows()?;

        let rows_wanted = self.strip_height.min(self.canvas_h - self.out_y);
        self.buf
            .reconfigure(self.canvas_w, rows_wanted, self.format);
        self.buf.reset();

        let content_y_start = self.place_y;
        let content_y_end = self.place_y + self.content_h;
        let bpp = self.format.bytes_per_pixel();

        for r in 0..rows_wanted {
            let canvas_y = self.out_y + r;

            if canvas_y >= content_y_start && canvas_y < content_y_end {
                // Content row: start with bg, blit content pixels
                self.buf.push_row(&self.bg_row);

                // Try to get an upstream row
                let got_row = self.next_upstream_row()?.is_some();
                if got_row {
                    // Access pending strip directly to avoid borrow conflict with buf
                    if let Some(ref p) = self.pending
                        && p.remaining() > 0
                    {
                        let src_row = p.row(p.next_row);
                        let src_start = self.skip_x as usize * bpp;
                        let src_end = src_start + self.content_w as usize * bpp;
                        let dst_start = self.place_x as usize * bpp;
                        let dst_end = dst_start + self.content_w as usize * bpp;
                        let dst_row = self.buf.row_mut(r);
                        dst_row[dst_start..dst_end].copy_from_slice(&src_row[src_start..src_end]);
                    }
                    self.consume_pending_row();
                }
            } else {
                // Pure padding row
                self.buf.push_row(&self.bg_row);
            }
        }

        if self.buf.rows_filled() == 0 {
            return Ok(None);
        }

        self.out_y += self.buf.rows_filled();
        Ok(Some(self.buf.as_strip()))
    }

    fn width(&self) -> u32 {
        self.canvas_w
    }
    fn height(&self) -> u32 {
        self.canvas_h
    }
    fn format(&self) -> PixelFormat {
        self.format
    }
}
