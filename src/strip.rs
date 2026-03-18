use alloc::sync::Arc;
use alloc::vec;

use zenpixels::buffer::BufferError;
use zenpixels::color::ColorContext;

use crate::PixelFormat;
use crate::error::PipeError;
use crate::format::PixelFormatExt;

/// Type alias — strips are [`zenpixels::PixelSlice`] values.
///
/// Color metadata (ICC profiles, CICP codes) flows automatically
/// through the pipeline via `PixelSlice::color_context()`.
pub type Strip<'a> = zenpixels::PixelSlice<'a>;

/// Owned buffer for accumulating strip rows.
///
/// Pre-allocated and reused across strips to avoid per-strip allocation.
/// Produces [`Strip`] views via [`as_strip()`](Self::as_strip).
pub struct StripBuf {
    data: alloc::vec::Vec<u8>,
    width: u32,
    stride: usize,
    format: PixelFormat,
    color: Option<Arc<ColorContext>>,
    rows_filled: u32,
    capacity_rows: u32,
}

impl StripBuf {
    /// Create a new strip buffer with space for `max_rows` rows.
    pub fn new(width: u32, max_rows: u32, format: PixelFormat) -> Self {
        let stride = format.row_bytes(width);
        Self {
            data: vec![0u8; stride * max_rows as usize],
            width,
            stride,
            format,
            color: None,
            rows_filled: 0,
            capacity_rows: max_rows,
        }
    }

    /// Set the color context for strips produced by this buffer.
    pub fn set_color_context(&mut self, ctx: Option<Arc<ColorContext>>) {
        self.color = ctx;
    }

    /// Reset for a new strip (clear filled count).
    pub fn reset(&mut self) {
        self.rows_filled = 0;
    }

    /// Append a row of pixel data. Returns false if buffer is full.
    pub fn push_row(&mut self, row: &[u8]) -> bool {
        if self.rows_filled >= self.capacity_rows {
            return false;
        }
        let start = self.rows_filled as usize * self.stride;
        let len = self.stride.min(row.len());
        self.data[start..start + len].copy_from_slice(&row[..len]);
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
        self.stride
    }

    /// Get a single row by index (0-based).
    #[inline]
    pub fn row(&self, r: u32) -> &[u8] {
        let start = r as usize * self.stride;
        &self.data[start..start + self.stride]
    }

    /// Get a mutable single row by index (0-based).
    #[inline]
    pub fn row_mut(&mut self, r: u32) -> &mut [u8] {
        let start = r as usize * self.stride;
        &mut self.data[start..start + self.stride]
    }

    /// View the filled portion as a [`Strip`] (PixelSlice).
    pub fn as_strip(&self) -> Strip<'_> {
        let end = self.rows_filled as usize * self.stride;
        let mut strip = Strip::new(
            &self.data[..end],
            self.width,
            self.rows_filled,
            self.stride,
            self.format,
        )
        .expect("StripBuf::as_strip: invalid dimensions");
        if let Some(ref ctx) = self.color {
            strip = strip.with_color_context(Arc::clone(ctx));
        }
        strip
    }

    /// Mutable access to all filled data.
    pub fn filled_data_mut(&mut self) -> &mut [u8] {
        let end = self.rows_filled as usize * self.stride;
        &mut self.data[..end]
    }

    /// All filled data.
    pub fn filled_data(&self) -> &[u8] {
        let end = self.rows_filled as usize * self.stride;
        &self.data[..end]
    }

    /// Resize buffer for a different format/width (reallocates if needed).
    pub fn reconfigure(&mut self, width: u32, max_rows: u32, format: PixelFormat) {
        let stride = format.row_bytes(width);
        if self.width != width || self.capacity_rows != max_rows || self.format != format {
            self.width = width;
            self.stride = stride;
            self.format = format;
            self.capacity_rows = max_rows;
            let needed = stride * max_rows as usize;
            if self.data.len() < needed {
                self.data.resize(needed, 0);
            }
        }
        self.rows_filled = 0;
    }
}

/// Convert a [`BufferError`] to a [`PipeError`].
impl From<whereat::At<BufferError>> for PipeError {
    fn from(e: whereat::At<BufferError>) -> Self {
        PipeError::Op(alloc::format!("pixel buffer: {e}"))
    }
}
