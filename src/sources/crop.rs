use alloc::boxed::Box;

use crate::Source;
use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::strip::{Strip, StripBuf};

/// Crops a region from an upstream source without materializing.
///
/// Skips rows before the crop region, extracts the x-range from each
/// row within the region, and stops after the region ends.
pub struct CropSource {
    upstream: Box<dyn Source>,
    x: u32,
    y: u32,
    crop_w: u32,
    crop_h: u32,
    format: PixelFormat,
    strip_height: u32,
    buf: StripBuf,
    out_y: u32,
}

impl CropSource {
    /// Crop a rectangle `(x, y, w, h)` from the upstream source.
    pub fn new(
        upstream: Box<dyn Source>,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    ) -> Result<Self, PipeError> {
        let fmt = upstream.format();
        let uw = upstream.width();
        let uh = upstream.height();

        if x + w > uw || y + h > uh {
            return Err(PipeError::DimensionMismatch(alloc::format!(
                "crop ({x},{y},{w},{h}) exceeds source ({uw},{uh})"
            )));
        }

        let sh = 16u32.min(h);
        Ok(Self {
            upstream,
            x,
            y,
            crop_w: w,
            crop_h: h,
            format: fmt,
            strip_height: sh,
            buf: StripBuf::new(w, sh, fmt),
            out_y: 0,
        })
    }

    /// Extract the x-range from one upstream row.
    fn extract_row_into(row: &[u8], x: u32, crop_w: u32, bpp: usize) -> &[u8] {
        let x_start = x as usize * bpp;
        let x_end = x_start + crop_w as usize * bpp;
        &row[x_start..x_end]
    }
}

impl Source for CropSource {
    fn next(&mut self) -> Result<Option<Strip<'_>>, PipeError> {
        if self.out_y >= self.crop_h {
            return Ok(None);
        }

        let rows_wanted = self.strip_height.min(self.crop_h - self.out_y);
        self.buf.reconfigure(self.crop_w, rows_wanted, self.format);
        self.buf.reset(self.out_y);

        let bpp = self.format.bytes_per_pixel();

        while self.buf.rows_filled() < rows_wanted {
            let strip = self.upstream.next()?;
            let Some(strip) = strip else { break };

            for r in 0..strip.height() {
                let abs_y = strip.y + r;

                // Skip rows before crop region
                if abs_y < self.y {
                    continue;
                }
                // Stop after crop region
                if abs_y >= self.y + self.crop_h {
                    self.out_y += self.buf.rows_filled();
                    return if self.buf.rows_filled() > 0 {
                        Ok(Some(self.buf.as_strip()))
                    } else {
                        Ok(None)
                    };
                }

                if self.buf.rows_filled() >= rows_wanted {
                    break;
                }

                let src_row = strip.row(r);
                let cropped = Self::extract_row_into(src_row, self.x, self.crop_w, bpp);
                self.buf.push_row(cropped);
            }
        }

        if self.buf.rows_filled() == 0 {
            return Ok(None);
        }

        self.out_y += self.buf.rows_filled();
        Ok(Some(self.buf.as_strip()))
    }

    fn width(&self) -> u32 {
        self.crop_w
    }
    fn height(&self) -> u32 {
        self.crop_h
    }
    fn format(&self) -> PixelFormat {
        self.format
    }
}
