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
    /// Tracks the absolute y position of rows consumed from upstream.
    upstream_y: u32,
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
            upstream_y: 0,
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
        self.buf.reset();

        let bpp = self.format.bytes_per_pixel();

        while self.buf.rows_filled() < rows_wanted {
            let strip = self.upstream.next()?;
            let Some(strip) = strip else { break };

            for r in 0..strip.rows() {
                let abs_y = self.upstream_y + r;

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
            self.upstream_y += strip.rows();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::RGBA8_SRGB;
    use crate::sources::materialize::MaterializedSource;

    /// Helper: collect all pixel rows from a Source.
    fn drain_rows(src: &mut dyn Source) -> Vec<Vec<u8>> {
        let bpp = src.format().bytes_per_pixel();
        let w = src.width() as usize;
        let mut rows = Vec::new();
        while let Some(strip) = src.next().unwrap() {
            for r in 0..strip.rows() {
                rows.push(strip.row(r)[..w * bpp].to_vec());
            }
        }
        rows
    }

    #[test]
    fn crop_center_2x2_from_4x4() {
        let w = 4u32;
        let h = 4u32;
        let fmt = RGBA8_SRGB;
        let stride = fmt.aligned_stride(w); // 16

        // Build a 4x4 image where each pixel is (x, y, 0, 255).
        let mut data = vec![0u8; stride * h as usize];
        for y in 0..h {
            for x in 0..w {
                let off = y as usize * stride + x as usize * 4;
                data[off] = x as u8;
                data[off + 1] = y as u8;
                data[off + 2] = 0;
                data[off + 3] = 255;
            }
        }

        let upstream = MaterializedSource::from_data(data, w, h, fmt).with_strip_height(1);

        // Crop center 2x2: x=1, y=1, w=2, h=2.
        let mut crop = CropSource::new(Box::new(upstream), 1, 1, 2, 2).unwrap();
        assert_eq!(crop.width(), 2);
        assert_eq!(crop.height(), 2);

        let rows = drain_rows(&mut crop);
        assert_eq!(rows.len(), 2);

        // Row 0 of crop = original row 1, pixels x=1..3.
        assert_eq!(rows[0], vec![1, 1, 0, 255, 2, 1, 0, 255]);
        // Row 1 of crop = original row 2, pixels x=1..3.
        assert_eq!(rows[1], vec![1, 2, 0, 255, 2, 2, 0, 255]);
    }

    #[test]
    fn crop_full_image_is_identity() {
        let w = 3u32;
        let h = 2u32;
        let fmt = RGBA8_SRGB;
        let stride = fmt.aligned_stride(w); // 12

        let data: Vec<u8> = (0..stride * h as usize).map(|i| i as u8).collect();
        let data_copy = data.clone();

        let upstream = MaterializedSource::from_data(data, w, h, fmt).with_strip_height(1);

        // Crop the entire image.
        let mut crop = CropSource::new(Box::new(upstream), 0, 0, w, h).unwrap();
        assert_eq!(crop.width(), w);
        assert_eq!(crop.height(), h);

        let rows = drain_rows(&mut crop);
        assert_eq!(rows.len(), h as usize);

        // Each row should match the original data.
        for y in 0..h as usize {
            let expected = &data_copy[y * stride..(y + 1) * stride];
            assert_eq!(rows[y], expected, "row {y} mismatch");
        }
    }
}
