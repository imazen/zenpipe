use alloc::vec;

use zenpixels::ColorOrigin;
use zenpixels::buffer::BufferError;

use crate::PixelFormat;
use crate::error::PipeError;
use crate::format::aligned_stride;

/// Type alias — strips are [`zenpixels::PixelView`] values.
///
/// Color metadata (ICC profiles, CICP codes) flows through the pipeline
/// via `PixelView::origin()`.
pub type Strip<'a> = zenpixels::PixelView<'a>;

/// Owned buffer for accumulating strip rows.
///
/// Pre-allocated and reused across strips to avoid per-strip allocation.
/// Produces [`Strip`] views via [`as_strip()`](Self::as_strip).
pub struct StripBuf {
    data: alloc::vec::Vec<u8>,
    width: u32,
    stride: usize,
    format: PixelFormat,
    origin: Option<ColorOrigin>,
    rows_filled: u32,
    capacity_rows: u32,
}

impl StripBuf {
    /// Create a new strip buffer with space for `max_rows` rows.
    ///
    /// Panics if `stride * max_rows` overflows `usize`. Use
    /// [`try_new`](Self::try_new) for fallible construction; the panic
    /// path remains for backwards compatibility with infallible callers,
    /// but new code should prefer the fallible variant.
    pub fn new(width: u32, max_rows: u32, format: PixelFormat) -> Self {
        Self::try_new(width, max_rows, format)
            .unwrap_or_else(|e| panic!("StripBuf::new: dimension overflow: {}", e.error()))
    }

    /// Fallible constructor. Returns `Err(LimitExceeded)` if
    /// `stride * max_rows` overflows `usize` (reachable on 32-bit / wasm32).
    pub fn try_new(width: u32, max_rows: u32, format: PixelFormat) -> crate::PipeResult<Self> {
        let stride = aligned_stride(&format, width);
        let total = crate::limits::checked_stride_buffer(stride, max_rows)?;
        Ok(Self {
            data: vec![0u8; total],
            width,
            stride,
            format,
            origin: None,
            rows_filled: 0,
            capacity_rows: max_rows,
        })
    }

    /// Set the color origin for strips produced by this buffer.
    pub fn set_color_origin(&mut self, origin: Option<ColorOrigin>) {
        self.origin = origin;
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

    /// View the filled portion as a [`Strip`] (PixelView).
    pub fn as_strip(&self) -> Strip<'_> {
        let end = self.rows_filled as usize * self.stride;
        let mut strip = Strip::new(
            &self.data[..end],
            self.width,
            self.rows_filled,
            self.stride,
            &self.format,
        )
        .expect("StripBuf::as_strip: invalid dimensions");
        if let Some(ref origin) = self.origin {
            strip = strip.with_origin(origin);
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
    ///
    /// Panics if `stride * max_rows` overflows `usize`. Prefer
    /// [`try_reconfigure`](Self::try_reconfigure) on untrusted input.
    pub fn reconfigure(&mut self, width: u32, max_rows: u32, format: PixelFormat) {
        self.try_reconfigure(width, max_rows, format)
            .unwrap_or_else(|e| panic!("StripBuf::reconfigure: dimension overflow: {}", e.error()))
    }

    /// Fallible variant of [`reconfigure`](Self::reconfigure).
    pub fn try_reconfigure(
        &mut self,
        width: u32,
        max_rows: u32,
        format: PixelFormat,
    ) -> crate::PipeResult<()> {
        let stride = aligned_stride(&format, width);
        if self.width != width || self.capacity_rows != max_rows || self.format != format {
            let needed = crate::limits::checked_stride_buffer(stride, max_rows)?;
            self.width = width;
            self.stride = stride;
            self.format = format;
            self.capacity_rows = max_rows;
            if self.data.len() < needed {
                self.data.resize(needed, 0);
            }
        }
        self.rows_filled = 0;
        Ok(())
    }
}

/// Extension trait to convert `Result<T, At<BufferError>>` into `Result<T, At<PipeError>>`.
pub(crate) trait BufferResultExt<T> {
    fn pipe_err(self) -> Result<T, whereat::At<PipeError>>;
}

impl<T> BufferResultExt<T> for Result<T, whereat::At<BufferError>> {
    fn pipe_err(self) -> Result<T, whereat::At<PipeError>> {
        self.map_err(|e| e.map_error(|be| PipeError::Op(alloc::format!("pixel buffer: {be}"))))
    }
}
