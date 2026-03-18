use alloc::boxed::Box;

use crate::Source;
use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::strip::{Strip, StripBuf};

/// Streaming horizontal flip — reverses pixel order within each row.
///
/// No materialization needed; each strip is flipped independently.
pub struct FlipHSource {
    upstream: Box<dyn Source>,
    buf: StripBuf,
}

impl FlipHSource {
    pub fn new(upstream: Box<dyn Source>) -> Self {
        let w = upstream.width();
        let h = upstream.height();
        let fmt = upstream.format();
        let sh = 16u32.min(h);
        Self {
            upstream,
            buf: StripBuf::new(w, sh, fmt),
        }
    }
}

impl Source for FlipHSource {
    fn next(&mut self) -> Result<Option<Strip<'_>>, PipeError> {
        let strip = self.upstream.next()?;
        let Some(strip) = strip else {
            return Ok(None);
        };

        let bpp = strip.descriptor().bytes_per_pixel();
        let width = strip.width() as usize;
        self.buf
            .reconfigure(strip.width(), strip.rows(), strip.descriptor());
        self.buf.reset();

        for r in 0..strip.rows() {
            let src_row = strip.row(r);
            self.buf.push_row(src_row);
            let dst_row = self.buf.row_mut(r);
            // Reverse pixel order in-place
            for x in 0..width / 2 {
                let a = x * bpp;
                let b = (width - 1 - x) * bpp;
                for c in 0..bpp {
                    dst_row.swap(a + c, b + c);
                }
            }
        }

        Ok(Some(self.buf.as_strip()))
    }

    fn width(&self) -> u32 {
        self.upstream.width()
    }
    fn height(&self) -> u32 {
        self.upstream.height()
    }
    fn format(&self) -> PixelFormat {
        self.upstream.format()
    }
}
