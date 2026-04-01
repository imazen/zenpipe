use alloc::boxed::Box;

use crate::Source;
#[allow(unused_imports)]
use whereat::at;

use crate::format::PixelFormat;
use crate::strip::{Strip, StripBuf};

/// A source that pulls rows from a callback function.
///
/// Wraps any row-producing function (decoder, buffer reader, test data)
/// as a [`Source`]. The callback receives a mutable row buffer and returns
/// `true` while rows remain.
///
/// # Example
///
/// ```ignore
/// let mut row_idx = 0u32;
/// let source = CallbackSource::new(256, 256, PixelFormat::Rgba8, 16, move |buf| {
///     if row_idx >= 256 { return Ok(false); }
///     buf.fill(128); // gray
///     row_idx += 1;
///     Ok(true)
/// });
/// ```
type RowCallback = Box<dyn FnMut(&mut [u8]) -> crate::PipeResult<bool> + Send>;

pub struct CallbackSource {
    callback: RowCallback,
    width: u32,
    height: u32,
    format: PixelFormat,
    strip_height: u32,
    buf: StripBuf,
    y: u32,
    exhausted: bool,
}

impl CallbackSource {
    /// Create a source from a row-producing callback.
    ///
    /// The callback is called once per row. It receives a buffer of
    /// `width * format.bytes_per_pixel()` bytes to fill. Return `Ok(true)`
    /// while rows remain, `Ok(false)` when done.
    pub fn new(
        width: u32,
        height: u32,
        format: PixelFormat,
        strip_height: u32,
        callback: impl FnMut(&mut [u8]) -> crate::PipeResult<bool> + Send + 'static,
    ) -> Self {
        let sh = strip_height.min(height);
        Self {
            callback: Box::new(callback),
            width,
            height,
            format,
            strip_height: sh,
            buf: StripBuf::new(width, sh, format),
            y: 0,
            exhausted: false,
        }
    }

    /// Create a source from pre-existing pixel data (row-major, tightly packed).
    pub fn from_data(
        data: &[u8],
        width: u32,
        height: u32,
        format: PixelFormat,
        strip_height: u32,
    ) -> Self {
        let row_bytes = format.aligned_stride(width);
        let data = data.to_vec();
        let mut offset = 0usize;
        Self::new(width, height, format, strip_height, move |buf| {
            if offset >= data.len() {
                return Ok(false);
            }
            let end = (offset + row_bytes).min(data.len());
            buf[..end - offset].copy_from_slice(&data[offset..end]);
            offset = end;
            Ok(true)
        })
    }
}

impl Source for CallbackSource {
    fn next(&mut self) -> crate::PipeResult<Option<Strip<'_>>> {
        if self.exhausted || self.y >= self.height {
            return Ok(None);
        }

        let rows_wanted = self.strip_height.min(self.height - self.y);
        self.buf.reset();

        for _ in 0..rows_wanted {
            let row_bytes = self.format.aligned_stride(self.width);
            let mut row_buf = alloc::vec![0u8; row_bytes];
            let has_more = (self.callback)(&mut row_buf)?;
            self.buf.push_row(&row_buf);
            if !has_more {
                self.exhausted = true;
                break;
            }
        }

        if self.buf.rows_filled() == 0 {
            return Ok(None);
        }

        self.y += self.buf.rows_filled();
        Ok(Some(self.buf.as_strip()))
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
