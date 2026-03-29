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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::RGBA8_SRGB;
    use crate::sources::materialize::MaterializedSource;

    #[test]
    fn horizontal_flip_4x1_rgba8() {
        let w = 4u32;
        let h = 1u32;
        let fmt = RGBA8_SRGB;

        // 4 pixels: A=red, B=green, C=blue, D=white.
        #[rustfmt::skip]
        let data: Vec<u8> = vec![
            255, 0,   0,   255,  // A: red
            0,   255, 0,   255,  // B: green
            0,   0,   255, 255,  // C: blue
            255, 255, 255, 255,  // D: white
        ];

        let upstream = MaterializedSource::from_data(data, w, h, fmt)
            .with_strip_height(h);

        let mut flip = FlipHSource::new(Box::new(upstream));
        assert_eq!(flip.width(), w);
        assert_eq!(flip.height(), h);
        assert_eq!(flip.format(), fmt);

        let strip = flip.next().unwrap().unwrap();
        assert_eq!(strip.rows(), 1);

        let row = strip.row(0);
        let bpp = fmt.bytes_per_pixel();

        // After flip, order should be D, C, B, A.
        // D: white
        assert_eq!(&row[0 * bpp..1 * bpp], &[255, 255, 255, 255]);
        // C: blue
        assert_eq!(&row[1 * bpp..2 * bpp], &[0, 0, 255, 255]);
        // B: green
        assert_eq!(&row[2 * bpp..3 * bpp], &[0, 255, 0, 255]);
        // A: red
        assert_eq!(&row[3 * bpp..4 * bpp], &[255, 0, 0, 255]);

        assert!(flip.next().unwrap().is_none());
    }
}
