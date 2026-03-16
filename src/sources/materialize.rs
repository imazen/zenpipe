use alloc::boxed::Box;
use alloc::vec;

use crate::Source;
use crate::error::PipeError;
use crate::format::{PixelFormat, PixelFormatExt};
use crate::strip::StripRef;

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
    /// Drain all strips from `upstream` into memory.
    pub fn from_source(mut upstream: Box<dyn Source>) -> Result<Self, PipeError> {
        let width = upstream.width();
        let height = upstream.height();
        let format = upstream.format();
        let row_bytes = format.row_bytes(width);
        let mut data = vec![0u8; row_bytes * height as usize];
        let mut y = 0u32;

        while let Some(strip) = upstream.next()? {
            for r in 0..strip.height {
                let dst_start = (y + r) as usize * row_bytes;
                let src_row = strip.row(r);
                data[dst_start..dst_start + row_bytes].copy_from_slice(&src_row[..row_bytes]);
            }
            y += strip.height;
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
    ) -> Result<Self, PipeError> {
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
}

impl Source for MaterializedSource {
    fn next(&mut self) -> Result<Option<StripRef<'_>>, PipeError> {
        if self.y >= self.height {
            return Ok(None);
        }

        let rows = self.strip_height.min(self.height - self.y);
        let stride = self.format.row_bytes(self.width);
        let start = self.y as usize * stride;
        let end = start + rows as usize * stride;

        let y = self.y;
        self.y += rows;

        Ok(Some(StripRef {
            data: &self.data[start..end],
            width: self.width,
            height: rows,
            stride,
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
