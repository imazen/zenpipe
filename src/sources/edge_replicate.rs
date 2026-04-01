//! Streaming edge replication — extends content to canvas dimensions by
//! replicating the rightmost pixel of each row and repeating the last
//! content row. Used for MCU alignment padding in JPEG encoding.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use crate::Source;
#[allow(unused_imports)]
use whereat::at;

use crate::format::PixelFormat;
use crate::strip::{Strip, StripBuf};

/// Expands upstream content to canvas dimensions via edge replication.
///
/// For rows within the content area: replicates the rightmost content
/// pixel to fill remaining columns. For rows past the content height:
/// repeats the last content row (already edge-replicated).
///
/// This produces better encoder output than solid-color padding because
/// there are no sharp transitions at the content boundary.
pub struct EdgeReplicateSource {
    upstream: Box<dyn Source>,
    content_w: u32,
    content_h: u32,
    canvas_w: u32,
    canvas_h: u32,
    format: PixelFormat,
    bpp: usize,
    /// Last content row (edge-replicated), for repeating past content_h.
    last_row: Vec<u8>,
    buf: StripBuf,
    y: u32,
    /// Pending strip from upstream (consumed row by row).
    pending: Option<PendingStrip>,
    upstream_done: bool,
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

impl EdgeReplicateSource {
    /// Create an edge replication wrapper.
    ///
    /// `content_w × content_h` is the actual content region (top-left corner).
    /// `canvas_w × canvas_h` is the output size. Canvas must be ≥ content.
    pub fn new(
        upstream: Box<dyn Source>,
        content_w: u32,
        content_h: u32,
        canvas_w: u32,
        canvas_h: u32,
    ) -> Self {
        let format = upstream.format();
        let bpp = format.bytes_per_pixel();
        let row_bytes = canvas_w as usize * bpp;
        Self {
            upstream,
            content_w,
            content_h,
            canvas_w,
            canvas_h,
            format,
            bpp,
            last_row: vec![0u8; row_bytes],
            buf: StripBuf::new(canvas_w, 16.min(canvas_h), format),
            y: 0,
            pending: None,
            upstream_done: false,
        }
    }

    /// Pull next row from upstream (content area only).
    fn pull_content_row(&mut self) -> crate::PipeResult<Option<Vec<u8>>> {
        // Try pending strip first
        if let Some(ref mut pending) = self.pending {
            if pending.remaining() > 0 {
                let row = pending.row(pending.next_row).to_vec();
                pending.next_row += 1;
                if pending.remaining() == 0 {
                    self.pending = None;
                }
                return Ok(Some(row));
            }
            self.pending = None;
        }

        if self.upstream_done {
            return Ok(None);
        }

        // Pull next strip
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

        let row = pending.row(0).to_vec();
        pending.next_row = 1;
        if pending.remaining() > 0 {
            self.pending = Some(pending);
        }
        Ok(Some(row))
    }

    /// Replicate the rightmost content pixel across remaining columns.
    fn replicate_right_edge(&self, row: &mut [u8]) {
        let content_bytes = self.content_w as usize * self.bpp;
        let canvas_bytes = self.canvas_w as usize * self.bpp;

        if content_bytes >= canvas_bytes || content_bytes == 0 {
            return;
        }

        // Copy the rightmost content pixel
        let last_px_start = (self.content_w as usize - 1) * self.bpp;
        let pixel: Vec<u8> = row[last_px_start..last_px_start + self.bpp].to_vec();

        // Replicate across remaining columns
        for x in self.content_w as usize..self.canvas_w as usize {
            let dst = x * self.bpp;
            row[dst..dst + self.bpp].copy_from_slice(&pixel);
        }
    }
}

impl Source for EdgeReplicateSource {
    fn next(&mut self) -> crate::PipeResult<Option<Strip<'_>>> {
        if self.y >= self.canvas_h {
            return Ok(None);
        }

        let rows_wanted = 16.min(self.canvas_h - self.y);
        self.buf
            .reconfigure(self.canvas_w, rows_wanted, self.format);
        self.buf.reset();

        for _ in 0..rows_wanted {
            if self.y < self.content_h {
                // Content row — pull from upstream and replicate right edge
                if let Some(mut row) = self.pull_content_row()? {
                    // Extend row to canvas width if needed
                    let canvas_bytes = self.canvas_w as usize * self.bpp;
                    row.resize(canvas_bytes, 0);
                    self.replicate_right_edge(&mut row);
                    // Save as last content row
                    self.last_row.copy_from_slice(&row[..canvas_bytes]);
                    self.buf.push_row(&row);
                } else {
                    // Upstream exhausted early — use last_row
                    self.buf.push_row(&self.last_row);
                }
            } else {
                // Past content: repeat last content row
                self.buf.push_row(&self.last_row);
            }
            self.y += 1;
        }

        if self.buf.rows_filled() == 0 {
            return Ok(None);
        }

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
