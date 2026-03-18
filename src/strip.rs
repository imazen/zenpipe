use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use zenpixels::buffer::BufferError;
use zenpixels::color::ColorContext;

use crate::PixelFormat;
use crate::error::PipeError;
use crate::format::PixelFormatExt;

/// A strip of pixel rows — a borrowed view wrapping [`zenpixels::PixelSlice`].
///
/// Carries pixel data, format descriptor, ICC/CICP color context, and the
/// y-offset of this strip within the full image. All metadata flows
/// automatically through the pipeline.
pub struct Strip<'a> {
    /// The pixel data for this strip (borrowed, validated, carries ColorContext).
    pub pixels: zenpixels::PixelSlice<'a>,
    /// Y offset of this strip within the full image.
    pub y: u32,
}

impl<'a> Strip<'a> {
    /// Create a strip from raw parts. Validates the pixel data.
    pub fn new(
        data: &'a [u8],
        width: u32,
        height: u32,
        stride: usize,
        format: PixelFormat,
        y: u32,
    ) -> Result<Self, PipeError> {
        let pixels = zenpixels::PixelSlice::new(data, width, height, stride, format)
            .map_err(|e| PipeError::Op(alloc::format!("strip construction: {e}")))?;
        Ok(Self { pixels, y })
    }

    /// Width in pixels.
    #[inline]
    pub fn width(&self) -> u32 {
        self.pixels.width()
    }

    /// Number of rows in this strip.
    #[inline]
    pub fn height(&self) -> u32 {
        self.pixels.rows()
    }

    /// Bytes per row.
    #[inline]
    pub fn stride(&self) -> usize {
        self.pixels.stride()
    }

    /// Pixel format descriptor.
    #[inline]
    pub fn format(&self) -> PixelFormat {
        self.pixels.descriptor()
    }

    /// Raw pixel data (all rows, may include stride padding).
    #[inline]
    pub fn data(&self) -> &[u8] {
        self.pixels.as_strided_bytes()
    }

    /// Get a single row by index (0-based within this strip, no padding).
    #[inline]
    pub fn row(&self, r: u32) -> &[u8] {
        self.pixels.row(r)
    }

    /// Color context (ICC profile + CICP), if attached.
    #[inline]
    pub fn color_context(&self) -> Option<&Arc<ColorContext>> {
        self.pixels.color_context()
    }

    /// Return a new strip with the given color context attached.
    pub fn with_color_context(mut self, ctx: Arc<ColorContext>) -> Self {
        self.pixels = self.pixels.with_color_context(ctx);
        self
    }
}

/// Owned buffer for accumulating strip data.
///
/// Pre-allocated and reused across strips to avoid per-strip allocation.
/// Produces [`Strip`] views via [`as_strip()`](Self::as_strip).
pub struct StripBuf {
    data: Vec<u8>,
    width: u32,
    format: PixelFormat,
    color: Option<Arc<ColorContext>>,
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
            color: None,
            capacity_rows: max_rows,
            rows_filled: 0,
            y_offset: 0,
        }
    }

    /// Set the color context for strips produced by this buffer.
    pub fn set_color_context(&mut self, ctx: Option<Arc<ColorContext>>) {
        self.color = ctx;
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

    /// View the filled portion as a [`Strip`].
    ///
    /// # Panics
    ///
    /// Panics if the buffer data fails PixelSlice validation (should not
    /// happen if the buffer was constructed correctly).
    pub fn as_strip(&self) -> Strip<'_> {
        let stride = self.stride();
        let data = &self.data[..self.rows_filled as usize * stride];
        let mut pixels =
            zenpixels::PixelSlice::new(data, self.width, self.rows_filled, stride, self.format)
                .expect("StripBuf data should always be valid for PixelSlice");
        if let Some(ref ctx) = self.color {
            pixels = pixels.with_color_context(Arc::clone(ctx));
        }
        Strip {
            pixels,
            y: self.y_offset,
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

/// Convert a [`BufferError`] to a [`PipeError`].
impl From<whereat::At<BufferError>> for PipeError {
    fn from(e: whereat::At<BufferError>) -> Self {
        PipeError::Op(alloc::format!("pixel buffer: {e}"))
    }
}
