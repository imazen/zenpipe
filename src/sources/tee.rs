//! Fan-out support: one upstream source feeding multiple downstream consumers.
//!
//! [`TeeSource`] materializes its upstream once, then produces [`TeeCursor`]
//! instances that each independently replay the buffered data as strips.
//!
//! # Memory
//!
//! The full upstream image is materialized into memory. This is the minimum
//! cost for fan-out — streaming requires each consumer to advance independently,
//! and a pull-based pipeline can't rewind an upstream source.
//!
//! # Example
//!
//! ```ignore
//! let tee = TeeSource::new(upstream_source)?;
//! let cursor_a = tee.cursor(); // feeds into resize pipeline
//! let cursor_b = tee.cursor(); // feeds into thumbnail pipeline
//! ```

use alloc::sync::Arc;
use alloc::vec;

use crate::Source;
use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::strip::StripRef;

/// Shared buffer holding the fully materialized upstream image.
struct SharedBuffer {
    data: Arc<Vec<u8>>,
    width: u32,
    height: u32,
    format: PixelFormat,
    stride: usize,
}

/// A fan-out source that materializes upstream once and produces cursors.
///
/// Each [`TeeCursor`] independently replays the buffered image data as
/// strips. The shared buffer is reference-counted — it stays alive as
/// long as any cursor references it.
pub struct TeeSource {
    buf: SharedBuffer,
}

impl TeeSource {
    /// Materialize the upstream source, checking resource limits first.
    pub fn new_checked(
        upstream: Box<dyn Source>,
        limits: &crate::limits::Limits,
    ) -> Result<Self, PipeError> {
        limits.check(upstream.width(), upstream.height(), upstream.format())?;
        Self::new(upstream)
    }

    /// Materialize the upstream source and prepare for fan-out.
    pub fn new(mut upstream: Box<dyn Source>) -> Result<Self, PipeError> {
        let width = upstream.width();
        let height = upstream.height();
        let format = upstream.format();
        let stride = width as usize * format.bytes_per_pixel();

        let mut data = vec![0u8; stride * height as usize];
        let mut y = 0usize;

        while let Some(strip) = upstream.next()? {
            for r in 0..strip.height {
                let dst_start = (y + r as usize) * stride;
                let src_row = strip.row(r);
                data[dst_start..dst_start + stride].copy_from_slice(&src_row[..stride]);
            }
            y += strip.height as usize;
        }

        Ok(Self {
            buf: SharedBuffer {
                data: Arc::new(data),
                width,
                height,
                format,
                stride,
            },
        })
    }

    /// Width of the materialized image.
    pub fn width(&self) -> u32 {
        self.buf.width
    }

    /// Height of the materialized image.
    pub fn height(&self) -> u32 {
        self.buf.height
    }

    /// Pixel format of the materialized image.
    pub fn format(&self) -> PixelFormat {
        self.buf.format
    }

    /// Create a new cursor that replays the materialized data as strips.
    ///
    /// Each cursor is independent — it has its own y position and strip
    /// height. Cursors can be used as `Box<dyn Source>` in pipeline graphs.
    pub fn cursor(&self) -> TeeCursor {
        self.cursor_with_strip_height(16)
    }

    /// Create a cursor with a specific output strip height.
    pub fn cursor_with_strip_height(&self, strip_height: u32) -> TeeCursor {
        TeeCursor {
            data: Arc::clone(&self.buf.data),
            width: self.buf.width,
            height: self.buf.height,
            format: self.buf.format,
            stride: self.buf.stride,
            strip_height: strip_height.min(self.buf.height),
            y: 0,
        }
    }
}

/// An independent cursor over a [`TeeSource`]'s materialized buffer.
///
/// Implements [`Source`] — each call to [`next`](Source::next) yields
/// the next strip of rows from the shared buffer. Multiple cursors
/// over the same `TeeSource` share the underlying allocation via `Arc`.
pub struct TeeCursor {
    data: Arc<Vec<u8>>,
    width: u32,
    height: u32,
    format: PixelFormat,
    stride: usize,
    strip_height: u32,
    y: u32,
}

impl Source for TeeCursor {
    fn next(&mut self) -> Result<Option<StripRef<'_>>, PipeError> {
        if self.y >= self.height {
            return Ok(None);
        }

        let rows = self.strip_height.min(self.height - self.y);
        let start = self.y as usize * self.stride;
        let end = start + rows as usize * self.stride;

        let y = self.y;
        self.y += rows;

        Ok(Some(StripRef {
            data: &self.data[start..end],
            width: self.width,
            height: rows,
            stride: self.stride,
            y,
            format: self.format,
        }))
    }

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn format(&self) -> PixelFormat {
        self.format
    }
}

// TeeCursor is Send because Arc<Vec<u8>> is Send.
// The Source trait already requires Send, so this is verified by the compiler.
