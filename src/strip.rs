use alloc::vec;
use alloc::vec::Vec;

use crate::PixelFormat;
use crate::format::PixelFormatExt;

/// Borrowed view of a horizontal strip of pixels.
///
/// Returned by [`Source::next`](crate::Source::next). Valid until the
/// next call to the source.
pub struct StripRef<'a> {
    /// Raw pixel data (row-major, tightly packed).
    pub data: &'a [u8],
    /// Width in pixels.
    pub width: u32,
    /// Number of rows in this strip.
    pub height: u32,
    /// Bytes per row (== width * format.bytes_per_pixel()).
    pub stride: usize,
    /// Y offset of this strip within the full image.
    pub y: u32,
    /// Pixel format.
    pub format: PixelFormat,
}

impl<'a> StripRef<'a> {
    /// Get a single row by index (0-based within this strip).
    #[inline]
    pub fn row(&self, r: u32) -> &[u8] {
        let start = r as usize * self.stride;
        &self.data[start..start + self.stride]
    }
}

/// Owned buffer for accumulating strip data.
///
/// Pre-allocated and reused across strips to avoid per-strip allocation.
pub struct StripBuf {
    data: Vec<u8>,
    width: u32,
    format: PixelFormat,
    capacity_rows: u32,
    rows_filled: u32,
    y_offset: u32,
}

impl StripBuf {
    /// Create a new strip buffer with space for `max_rows` rows.
    pub fn new(width: u32, max_rows: u32, format: PixelFormat) -> Self {
        let total = format.row_bytes(width) * max_rows as usize;
        Self {
            data: vec![0u8; total],
            width,
            format,
            capacity_rows: max_rows,
            rows_filled: 0,
            y_offset: 0,
        }
    }

    /// Reset for a new strip starting at the given y offset.
    pub fn reset(&mut self, y_offset: u32) {
        self.rows_filled = 0;
        self.y_offset = y_offset;
    }

    /// Append a row of pixel data. Returns false if buffer is full.
    pub fn push_row(&mut self, row: &[u8]) -> bool {
        if self.rows_filled >= self.capacity_rows {
            return false;
        }
        let stride = self.stride();
        let start = self.rows_filled as usize * stride;
        self.data[start..start + stride].copy_from_slice(&row[..stride]);
        self.rows_filled += 1;
        true
    }

    /// Number of rows currently in the buffer.
    #[inline]
    pub fn rows_filled(&self) -> u32 {
        self.rows_filled
    }

    /// Maximum rows this buffer can hold.
    #[inline]
    pub fn capacity_rows(&self) -> u32 {
        self.capacity_rows
    }

    /// Bytes per row.
    #[inline]
    pub fn stride(&self) -> usize {
        self.format.row_bytes(self.width)
    }

    /// Get a single row by index (0-based).
    #[inline]
    pub fn row(&self, r: u32) -> &[u8] {
        let stride = self.stride();
        let start = r as usize * stride;
        &self.data[start..start + stride]
    }

    /// Get a mutable single row by index (0-based).
    #[inline]
    pub fn row_mut(&mut self, r: u32) -> &mut [u8] {
        let stride = self.stride();
        let start = r as usize * stride;
        &mut self.data[start..start + stride]
    }

    /// View the filled portion as a StripRef.
    pub fn as_ref(&self) -> StripRef<'_> {
        let stride = self.stride();
        StripRef {
            data: &self.data[..self.rows_filled as usize * stride],
            width: self.width,
            height: self.rows_filled,
            stride,
            y: self.y_offset,
            format: self.format,
        }
    }

    /// Mutable access to all filled data.
    pub fn filled_data_mut(&mut self) -> &mut [u8] {
        let end = self.rows_filled as usize * self.stride();
        &mut self.data[..end]
    }

    /// All filled data.
    pub fn filled_data(&self) -> &[u8] {
        let end = self.rows_filled as usize * self.stride();
        &self.data[..end]
    }

    /// Resize buffer for a different format/width (reuses allocation if possible).
    pub fn reconfigure(&mut self, width: u32, max_rows: u32, format: PixelFormat) {
        self.width = width;
        self.format = format;
        self.capacity_rows = max_rows;
        self.rows_filled = 0;
        self.y_offset = 0;
        let total = format.row_bytes(width) * max_rows as usize;
        self.data.resize(total, 0);
    }
}
