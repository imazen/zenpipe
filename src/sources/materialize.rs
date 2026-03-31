use alloc::boxed::Box;
use alloc::vec;

use crate::Source;
#[allow(unused_imports)]
use whereat::at;

use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::limits::Limits;
use crate::strip::Strip;

/// Fully materializes an upstream source, then replays it as strips.
///
/// Used as a barrier for operations that can't stream (transpose,
/// rotate90, full-image blur). Pulls all rows from upstream into
/// a contiguous buffer, optionally transforms the buffer, then
/// yields strips from the result.
///
/// This is the one place where full materialization happens.
/// Everything upstream and downstream of a `MaterializedSource`
/// still streams.
pub struct MaterializedSource {
    data: alloc::vec::Vec<u8>,
    width: u32,
    height: u32,
    format: PixelFormat,
    strip_height: u32,
    y: u32,
}

impl MaterializedSource {
    /// Drain all strips from `upstream` into memory, checking resource limits.
    pub fn from_source_checked(
        upstream: Box<dyn Source>,
        limits: &Limits,
    ) -> crate::PipeResult<Self> {
        limits.check(upstream.width(), upstream.height(), upstream.format())?;
        Self::from_source(upstream)
    }

    /// Drain all strips from `upstream` into memory.
    pub fn from_source(mut upstream: Box<dyn Source>) -> crate::PipeResult<Self> {
        let width = upstream.width();
        let height = upstream.height();
        let format = upstream.format();
        let row_bytes = format.aligned_stride(width);
        let mut data = vec![0u8; row_bytes * height as usize];
        let mut y = 0u32;

        while let Some(strip) = upstream.next()? {
            for r in 0..strip.rows() {
                let dst_start = (y + r) as usize * row_bytes;
                let src_row = strip.row(r);
                data[dst_start..dst_start + row_bytes].copy_from_slice(&src_row[..row_bytes]);
            }
            y += strip.rows();
        }

        Ok(Self {
            data,
            width,
            height,
            format,
            strip_height: 16.min(height),
            y: 0,
        })
    }

    /// Drain upstream, then apply a transformation to the full buffer.
    ///
    /// The transform receives `(data, width, height, format)` and may
    /// modify the data in-place or change dimensions (e.g., transpose).
    pub fn from_source_with_transform(
        upstream: Box<dyn Source>,
        transform: impl FnOnce(&mut alloc::vec::Vec<u8>, &mut u32, &mut u32, &mut PixelFormat),
    ) -> crate::PipeResult<Self> {
        let mut mat = Self::from_source(upstream)?;
        transform(
            &mut mat.data,
            &mut mat.width,
            &mut mat.height,
            &mut mat.format,
        );
        Ok(mat)
    }

    /// Create from pre-existing data.
    pub fn from_data(
        data: alloc::vec::Vec<u8>,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Self {
        Self {
            data,
            width,
            height,
            format,
            strip_height: 16.min(height),
            y: 0,
        }
    }

    /// Set output strip height.
    pub fn with_strip_height(mut self, h: u32) -> Self {
        self.strip_height = h.min(self.height);
        self
    }

    /// Raw pixel data (row-major, `stride × height` bytes).
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Mutable raw pixel data (row-major, `stride × height` bytes).
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Stride (bytes per row, may include padding).
    pub fn stride(&self) -> usize {
        self.format.aligned_stride(self.width)
    }

    /// Read a single row by index.
    pub fn row(&self, y: u32) -> &[u8] {
        let stride = self.stride();
        let start = y as usize * stride;
        &self.data[start..start + stride]
    }
}

impl Source for MaterializedSource {
    fn next(&mut self) -> crate::PipeResult<Option<Strip<'_>>> {
        use crate::strip::BufferResultExt as _;
        if self.y >= self.height {
            return Ok(None);
        }

        let rows = self.strip_height.min(self.height - self.y);
        let stride = self.format.aligned_stride(self.width);
        let start = self.y as usize * stride;
        let end = start + rows as usize * stride;

        self.y += rows;

        Ok(Some(
            Strip::new(
                &self.data[start..end],
                self.width,
                rows,
                stride,
                self.format,
            )
            .pipe_err()?,
        ))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{RGB8_SRGB, RGBA8_SRGB};

    /// Helper: collect all rows from a Source into a flat Vec.
    fn drain_all_rows(src: &mut dyn Source) -> Vec<Vec<u8>> {
        let mut rows = Vec::new();
        let bpp = src.format().bytes_per_pixel();
        let w = src.width() as usize;
        while let Some(strip) = src.next().unwrap() {
            for r in 0..strip.rows() {
                let row = strip.row(r);
                rows.push(row[..w * bpp].to_vec());
            }
        }
        rows
    }

    #[test]
    fn from_data_dimensions() {
        let w = 3u32;
        let h = 2u32;
        let fmt = RGBA8_SRGB;
        let stride = fmt.aligned_stride(w); // 12
        let data = vec![0u8; stride * h as usize];
        let src = MaterializedSource::from_data(data, w, h, fmt);

        assert_eq!(src.width(), w);
        assert_eq!(src.height(), h);
        assert_eq!(src.format(), fmt);
    }

    #[test]
    fn from_data_iterate_rows() {
        let w = 2u32;
        let h = 3u32;
        let fmt = RGBA8_SRGB;
        let stride = fmt.aligned_stride(w); // 8

        // Fill with distinct per-row patterns.
        let mut data = vec![0u8; stride * h as usize];
        for y in 0..h as usize {
            for x in 0..stride {
                data[y * stride + x] = (y * 10 + x) as u8;
            }
        }

        let mut src = MaterializedSource::from_data(data.clone(), w, h, fmt).with_strip_height(1); // one row per strip

        let rows = drain_all_rows(&mut src);
        assert_eq!(rows.len(), h as usize);

        for y in 0..h as usize {
            let expected: Vec<u8> = (0..stride).map(|x| (y * 10 + x) as u8).collect();
            assert_eq!(rows[y], expected, "row {y} mismatch");
        }
    }

    #[test]
    fn data_returns_full_buffer() {
        let w = 4u32;
        let h = 2u32;
        let fmt = RGB8_SRGB;
        let stride = fmt.aligned_stride(w); // 12
        let data: Vec<u8> = (0..stride * h as usize).map(|i| i as u8).collect();
        let src = MaterializedSource::from_data(data.clone(), w, h, fmt);

        assert_eq!(src.data(), &data[..]);
    }

    #[test]
    fn stride_correct() {
        let w = 5u32;
        let h = 1u32;
        let fmt = RGBA8_SRGB;
        let src = MaterializedSource::from_data(vec![0u8; fmt.aligned_stride(w)], w, h, fmt);

        assert_eq!(src.stride(), w as usize * 4); // RGBA8 = 4 bpp
    }

    #[test]
    fn with_strip_height_changes_strip_size() {
        let w = 2u32;
        let h = 8u32;
        let fmt = RGBA8_SRGB;
        let stride = fmt.aligned_stride(w);
        let data = vec![0u8; stride * h as usize];

        let mut src = MaterializedSource::from_data(data, w, h, fmt).with_strip_height(3);

        // Should get strips of 3, 3, 2 rows.
        let s1 = src.next().unwrap().unwrap();
        assert_eq!(s1.rows(), 3);

        let s2 = src.next().unwrap().unwrap();
        assert_eq!(s2.rows(), 3);

        let s3 = src.next().unwrap().unwrap();
        assert_eq!(s3.rows(), 2);

        assert!(src.next().unwrap().is_none());
    }
}
