use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use crate::Source;
#[allow(unused_imports)]
use whereat::at;

use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::limits::checked_buffer_size;
use crate::strip::{Strip, StripBuf};

/// How the padding area of an expanded canvas is filled (zenpipe#23).
///
/// Mirrors sharp's `extendWith` / libvips `embed` extend modes. Every mode
/// is streaming; the non-solid modes buffer only what the vertical padding
/// needs (see [`ExpandCanvasSource::with_fill`] for the exact bounds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasFill {
    /// Solid color, RGBA bytes (sharp `'background'`). The default.
    Solid([u8; 4]),
    /// Replicate the nearest content edge pixel — clamp (sharp `'copy'`,
    /// vips `VIPS_EXTEND_COPY`).
    Replicate,
    /// Reflect the content at its edges with the edge pixel repeated
    /// (`abc|cba|abc…`; sharp `'mirror'`, vips `VIPS_EXTEND_MIRROR`).
    Mirror,
    /// Tile the content (`abc|abc|abc…`; sharp `'repeat'`,
    /// vips `VIPS_EXTEND_REPEAT`).
    Repeat,
}

impl CanvasFill {
    /// Parse a mode name: `solid` | `background` | `replicate` | `copy` |
    /// `clamp` | `mirror` | `reflect` | `repeat` | `tile`. `Solid` carries
    /// the given color.
    pub fn from_name(name: &str, solid_color: [u8; 4]) -> Option<Self> {
        let mut buf = [0u8; 12];
        let bytes = name.trim().as_bytes();
        if bytes.len() > buf.len() {
            return None;
        }
        for (d, b) in buf.iter_mut().zip(bytes) {
            *d = b.to_ascii_lowercase();
        }
        Some(match &buf[..bytes.len()] {
            b"solid" | b"background" | b"color" | b"" => CanvasFill::Solid(solid_color),
            b"replicate" | b"copy" | b"clamp" | b"edge" => CanvasFill::Replicate,
            b"mirror" | b"reflect" => CanvasFill::Mirror,
            b"repeat" | b"tile" => CanvasFill::Repeat,
            _ => return None,
        })
    }

    /// Map a coordinate relative to the content origin (`t < 0` = before the
    /// content, `t >= len` = after it) onto a content index in `0..len`.
    /// `None` for `Solid` (padding is background, not content).
    fn map_index(self, t: i64, len: u32) -> Option<u32> {
        if len == 0 {
            return None;
        }
        let len_i = len as i64;
        Some(match self {
            CanvasFill::Solid(_) => return None,
            CanvasFill::Replicate => t.clamp(0, len_i - 1) as u32,
            CanvasFill::Mirror => {
                let period = 2 * len_i;
                let p = t.rem_euclid(period);
                (if p >= len_i { period - 1 - p } else { p }) as u32
            }
            CanvasFill::Repeat => t.rem_euclid(len_i) as u32,
        })
    }
}

/// Streaming canvas expansion — places upstream on a larger canvas with padding.
///
/// Emits solid-color rows for padding above/below the content, and pads
/// columns left/right for rows containing the image. No materialization.
/// Other fill modes ([`CanvasFill`]) buffer a bounded number of content
/// rows — see [`with_fill`](Self::with_fill).
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
    /// Padding fill mode (zenpipe#23). `Solid` keeps the original fast path.
    fill: CanvasFill,
    /// Leading visible content rows (packed, `content_w * bpp`) buffered
    /// before the first canvas row is emitted; `prefix_needed` says how many.
    prefix: Vec<Vec<u8>>,
    prefix_needed: u32,
    /// Trailing visible content rows kept for the padding below.
    ring: VecDeque<Vec<u8>>,
    ring_cap: u32,
    /// Visible content rows emitted so far.
    content_emitted: u32,
    /// Scratch canvas row for the non-solid fill path.
    row_scratch: Vec<u8>,
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
    ///
    /// Returns `Err(LimitExceeded)` if `canvas_w * bpp` overflows `usize`.
    /// `place_x`/`place_y` of [`i32::MIN`] are accepted via
    /// [`unsigned_abs`](i32::unsigned_abs) (the previous `(-place_x) as u32`
    /// would have wrapped).
    pub fn new(
        upstream: Box<dyn Source>,
        canvas_w: u32,
        canvas_h: u32,
        place_x: i32,
        place_y: i32,
        bg_pixel: [u8; 4],
    ) -> crate::PipeResult<Self> {
        let fmt = upstream.format();
        let src_w = upstream.width();
        let src_h = upstream.height();

        // unsigned_abs handles i32::MIN correctly; -i32::MIN as u32 overflows.
        let skip_x = if place_x < 0 {
            place_x.unsigned_abs()
        } else {
            0
        };
        let skip_y = if place_y < 0 {
            place_y.unsigned_abs()
        } else {
            0
        };
        let dst_x = if place_x >= 0 { place_x as u32 } else { 0 };
        let dst_y = if place_y >= 0 { place_y as u32 } else { 0 };

        let content_w = src_w
            .saturating_sub(skip_x)
            .min(canvas_w.saturating_sub(dst_x));
        let content_h = src_h
            .saturating_sub(skip_y)
            .min(canvas_h.saturating_sub(dst_y));

        // Pre-build a full background row.
        // Branch on bpp: chunks_exact_mut(4) is only correct for 4-byte
        // pixels. For other bpp, replicate the appropriate prefix of the
        // 4-byte bg_pixel (or zero-fill for non-RGBA layouts).
        let bpp = fmt.bytes_per_pixel();
        let row_len = checked_buffer_size(canvas_w, 1, bpp).map_err(|e| {
            at!(PipeError::LimitExceeded(alloc::format!(
                "ExpandCanvas bg_row size overflow: canvas_w={canvas_w} bpp={bpp}: {e}",
                e = e.error()
            )))
        })?;
        let mut bg_row = vec![0u8; row_len];
        match bpp {
            4 => {
                for chunk in bg_row.chunks_exact_mut(4) {
                    chunk.copy_from_slice(&bg_pixel);
                }
            }
            3 => {
                for chunk in bg_row.chunks_exact_mut(3) {
                    chunk.copy_from_slice(&bg_pixel[..3]);
                }
            }
            2 => {
                for chunk in bg_row.chunks_exact_mut(2) {
                    chunk.copy_from_slice(&bg_pixel[..2]);
                }
            }
            1 => bg_row.fill(bg_pixel[0]),
            // Wider pixels (U16/F32 multi-channel) — leave as zero rather
            // than reinterpret a u8 4-tuple. Caller should not depend on a
            // specific non-zero background for these formats.
            _ => {}
        }

        let sh = 16u32.min(canvas_h);
        let buf = StripBuf::try_new(canvas_w, sh, fmt)?;
        Ok(Self {
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
            buf,
            out_y: 0,
            pending: None,
            upstream_rows_consumed: 0,
            upstream_exhausted: false,
            fill: CanvasFill::Solid(bg_pixel),
            prefix: Vec::new(),
            prefix_needed: 0,
            ring: VecDeque::new(),
            ring_cap: 0,
            content_emitted: 0,
            row_scratch: Vec::new(),
        })
    }

    /// Choose how the padding is filled (zenpipe#23).
    ///
    /// Everything stays streaming. Buffering (rows of `content_w × bpp`):
    /// - `Solid`: none.
    /// - `Replicate`: 1 row above (the first content row), 1 row below.
    /// - `Mirror`: `min(top, content_h)` leading rows + `min(bottom,
    ///   content_h)` trailing rows.
    /// - `Repeat`: rows above the content are the *last* content rows, so
    ///   `top > 0` buffers the whole visible content (`content_h` rows);
    ///   with `top == 0` only `min(bottom, content_h)` leading rows.
    ///
    /// The fill applies to the visible content region (after any negative
    /// placement crop). Empty content falls back to the solid background.
    pub fn with_fill(mut self, fill: CanvasFill) -> Self {
        if let CanvasFill::Solid(px) = fill {
            // Rebuild the background row for the new color.
            let bpp = self.format.bytes_per_pixel();
            match bpp {
                4 => self
                    .bg_row
                    .chunks_exact_mut(4)
                    .for_each(|c| c.copy_from_slice(&px)),
                3 => self
                    .bg_row
                    .chunks_exact_mut(3)
                    .for_each(|c| c.copy_from_slice(&px[..3])),
                2 => self
                    .bg_row
                    .chunks_exact_mut(2)
                    .for_each(|c| c.copy_from_slice(&px[..2])),
                1 => self.bg_row.fill(px[0]),
                _ => {}
            }
        }
        self.fill = fill;
        let top = self.place_y;
        let bottom = self
            .canvas_h
            .saturating_sub(self.place_y)
            .saturating_sub(self.content_h);
        let h = self.content_h;
        let (prefix_needed, ring_cap) = match fill {
            CanvasFill::Solid(_) => (0, 0),
            CanvasFill::Replicate => (u32::from(top > 0), u32::from(bottom > 0)),
            CanvasFill::Mirror => (top.min(h), bottom.min(h)),
            CanvasFill::Repeat => (if top > 0 { h } else { bottom.min(h) }, 0),
        };
        self.prefix_needed = prefix_needed;
        self.ring_cap = ring_cap;
        self.prefix = Vec::new();
        self.ring = VecDeque::new();
        self.row_scratch = if matches!(fill, CanvasFill::Solid(_)) {
            Vec::new()
        } else {
            vec![0u8; self.bg_row.len()]
        };
        self
    }

    /// Current fill mode.
    pub fn fill(&self) -> CanvasFill {
        self.fill
    }

    /// Buffer the leading visible content rows the fill mode needs before
    /// the first canvas row can be emitted.
    fn fill_prefix(&mut self) -> crate::PipeResult<()> {
        let bpp = self.format.bytes_per_pixel();
        let src_start = self.skip_x as usize * bpp;
        let src_end = src_start + self.content_w as usize * bpp;
        while (self.prefix.len() as u32) < self.prefix_needed {
            if self.next_upstream_row()?.is_none() {
                break;
            }
            let row = match self.pending {
                Some(ref p) if p.remaining() > 0 => p.row(p.next_row)[src_start..src_end].to_vec(),
                _ => break,
            };
            self.prefix.push(row);
            self.consume_pending_row();
        }
        Ok(())
    }

    /// Fill the left/right padding of `row_scratch` from the content segment
    /// already placed at `place_x`. Solid fill (or empty content) leaves the
    /// background bytes in place.
    fn fill_horizontal(&mut self) {
        if self.content_w == 0 {
            return;
        }
        let bpp = self.format.bytes_per_pixel();
        let content_w = self.content_w;
        let place_x = self.place_x as usize;
        let mut px = [0u8; 16];
        let px = &mut px[..bpp];
        let mut fill_range = |scratch: &mut [u8], xs: core::ops::Range<usize>| {
            for x in xs {
                let t = x as i64 - place_x as i64;
                let Some(sx) = self.fill.map_index(t, content_w) else {
                    return;
                };
                let src = (place_x + sx as usize) * bpp;
                px.copy_from_slice(&scratch[src..src + bpp]);
                scratch[x * bpp..x * bpp + bpp].copy_from_slice(px);
            }
        };
        let right_start = place_x + content_w as usize;
        let canvas_w = self.canvas_w as usize;
        let mut scratch = core::mem::take(&mut self.row_scratch);
        fill_range(&mut scratch, 0..place_x);
        fill_range(&mut scratch, right_start..canvas_w);
        self.row_scratch = scratch;
    }

    /// Emit one canvas row for the non-solid fill path.
    fn emit_filled_row(&mut self, canvas_y: u32) -> crate::PipeResult<()> {
        let bpp = self.format.bytes_per_pixel();
        let cw = self.content_w as usize * bpp;
        let dst_start = self.place_x as usize * bpp;
        let content_y_start = self.place_y;
        let content_y_end = self.place_y.saturating_add(self.content_h);
        self.row_scratch.copy_from_slice(&self.bg_row);

        if canvas_y >= content_y_start && canvas_y < content_y_end {
            let ci = canvas_y - content_y_start;
            let from_prefix = (ci as usize) < self.prefix.len();
            let mut placed = false;
            if from_prefix {
                self.row_scratch[dst_start..dst_start + cw]
                    .copy_from_slice(&self.prefix[ci as usize]);
                placed = true;
            } else if self.next_upstream_row()?.is_some() {
                if let Some(ref p) = self.pending
                    && p.remaining() > 0
                {
                    let src_start = self.skip_x as usize * bpp;
                    self.row_scratch[dst_start..dst_start + cw]
                        .copy_from_slice(&p.row(p.next_row)[src_start..src_start + cw]);
                    placed = true;
                }
                self.consume_pending_row();
            }
            if placed {
                if self.ring_cap > 0 {
                    let mut v = if self.ring.len() as u32 >= self.ring_cap {
                        self.ring.pop_front().unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    v.clear();
                    v.extend_from_slice(&self.row_scratch[dst_start..dst_start + cw]);
                    self.ring.push_back(v);
                }
                self.fill_horizontal();
            }
            self.content_emitted += 1;
        } else if self.content_w > 0 && self.content_h > 0 {
            // Padding row above/below: pick the content row the mode maps to.
            let t = canvas_y as i64 - content_y_start as i64;
            if let Some(idx) = self.fill.map_index(t, self.content_h) {
                let ring_base = self.content_emitted.saturating_sub(self.ring.len() as u32);
                let row: Option<&[u8]> = if (idx as usize) < self.prefix.len() {
                    Some(&self.prefix[idx as usize])
                } else if idx >= ring_base && ((idx - ring_base) as usize) < self.ring.len() {
                    Some(&self.ring[(idx - ring_base) as usize])
                } else {
                    None
                };
                if let Some(row) = row {
                    self.row_scratch[dst_start..dst_start + cw].copy_from_slice(row);
                    self.fill_horizontal();
                }
            }
        }
        self.buf.push_row(&self.row_scratch);
        Ok(())
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

        if !matches!(self.fill, CanvasFill::Solid(_)) {
            if self.out_y == 0 {
                self.fill_prefix()?;
            }
            for r in 0..rows_wanted {
                let canvas_y = self.out_y.saturating_add(r);
                self.emit_filled_row(canvas_y)?;
            }
            if self.buf.rows_filled() == 0 {
                return Ok(None);
            }
            self.out_y += self.buf.rows_filled();
            return Ok(Some(self.buf.as_strip()));
        }

        let content_y_start = self.place_y;
        // place_y + content_h validated bounded by canvas_h via construction
        // (content_h = saturating_sub of canvas_h - place_y), so saturating_add
        // here is safe and never produces an out-of-canvas value.
        let content_y_end = self.place_y.saturating_add(self.content_h);
        let bpp = self.format.bytes_per_pixel();

        for r in 0..rows_wanted {
            let canvas_y = self.out_y.saturating_add(r);

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
