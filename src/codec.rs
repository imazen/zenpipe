//! Bridge between zencodec's `Dyn*` traits and zenpipe's `Source`/`Sink` model.
//!
//! Provides [`DecoderSource`] (wraps a streaming decoder as a [`Source`]) and
//! [`EncoderSink`] (wraps an encoder as a [`Sink`]).

use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::vec::Vec;

use zencodec::decode::DynStreamingDecoder;
use zencodec::encode::DynEncoder;
use zenpixels::PixelDescriptor;

use crate::Source;
#[allow(unused_imports)]
use whereat::at;

use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::strip::{Strip, StripBuf};

// =============================================================================
// DecoderSource
// =============================================================================

/// Wraps a zencodec [`DynStreamingDecoder`] as a zenpipe [`Source`].
///
/// Pulls scanline batches from the decoder and yields them as [`Strip`]s.
/// The output pixel format is discovered by eagerly decoding the first batch
/// during construction — `format()` is accurate from the start.
pub struct DecoderSource<'a> {
    decoder: Box<dyn DynStreamingDecoder + 'a>,
    width: u32,
    height: u32,
    format: PixelFormat,
    buf: StripBuf,
    /// First batch, eagerly decoded during `new()` to discover the format.
    /// Served on the first `next()` call, then `None`.
    first_batch: Option<(u32, Vec<u8>, u32)>, // (rows, data, row_bytes)
    y: u32,
}

impl<'a> DecoderSource<'a> {
    /// Create a `DecoderSource` from a streaming decoder.
    ///
    /// Eagerly decodes the first batch to discover the output pixel format.
    /// This ensures `format()`, `width()`, and `height()` are all accurate
    /// before any `next()` call — required by `build_pipeline` which reads
    /// format during graph compilation.
    pub fn new(mut decoder: Box<dyn DynStreamingDecoder + 'a>) -> crate::PipeResult<Self> {
        let info = decoder.info();
        let w = info.width;
        let h = info.height;

        // Eagerly decode first batch to discover output pixel format.
        let first = decoder
            .next_batch()
            .map_err(|e| at!(PipeError::Op(e.to_string())))?;

        let (format, first_batch) = match first {
            Some((_batch_y, pixels)) => {
                let fmt = pixels.descriptor();
                let rows = pixels.rows();
                let bpp = fmt.bytes_per_pixel();
                let row_bytes = w as usize * bpp;

                // Copy first batch data for replay on first next() call.
                let mut data = Vec::with_capacity(rows as usize * row_bytes);
                for r in 0..rows {
                    let row = pixels.row(r);
                    data.extend_from_slice(&row[..row_bytes]);
                }
                (fmt, Some((rows, data, row_bytes as u32)))
            }
            None => {
                // Empty image — use a sensible default.
                (crate::format::RGBA8_SRGB, None)
            }
        };

        let sh = 16u32.min(h);

        Ok(Self {
            decoder,
            width: w,
            height: h,
            format,
            buf: StripBuf::new(w, sh, format),
            first_batch,
            y: 0,
        })
    }

    /// Create a `DecoderSource` with an explicit pixel format (no eager decode).
    ///
    /// Use when you know the decoder's output format ahead of time.
    pub fn with_format(
        decoder: Box<dyn DynStreamingDecoder + 'a>,
        format: PixelFormat,
    ) -> crate::PipeResult<Self> {
        let info = decoder.info();
        let w = info.width;
        let h = info.height;
        let sh = 16u32.min(h);

        Ok(Self {
            decoder,
            width: w,
            height: h,
            format,
            buf: StripBuf::new(w, sh, format),
            first_batch: None,
            y: 0,
        })
    }
}

impl Source for DecoderSource<'_> {
    fn next(&mut self) -> crate::PipeResult<Option<Strip<'_>>> {
        if self.y >= self.height {
            return Ok(None);
        }

        // Serve the eagerly-decoded first batch if available.
        if let Some((rows, data, row_bytes)) = self.first_batch.take() {
            self.buf.reconfigure(self.width, rows, self.format);
            self.buf.reset();
            let rb = row_bytes as usize;
            for r in 0..rows as usize {
                self.buf.push_row(&data[r * rb..(r + 1) * rb]);
            }
            self.y += rows;
            return Ok(Some(self.buf.as_strip()));
        }

        // Pull subsequent batches from the decoder.
        let batch = self
            .decoder
            .next_batch()
            .map_err(|e| at!(PipeError::Op(e.to_string())))?;

        let Some((_batch_y, pixels)) = batch else {
            return Ok(None);
        };

        let rows = pixels.rows();
        let bpp = self.format.bytes_per_pixel();
        let row_bytes = self.width as usize * bpp;

        self.buf.reconfigure(self.width, rows, self.format);
        self.buf.reset();

        for r in 0..rows {
            let src_row = pixels.row(r);
            self.buf.push_row(&src_row[..row_bytes]);
        }

        self.y += rows;
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

// =============================================================================
// EncoderSink
// =============================================================================

/// Wraps a zencodec [`DynEncoder`] as a zenpipe [`Sink`].
///
/// Receives pixel strips, converts them to [`PixelSlice`]s, and pushes
/// rows into the encoder. Call [`take_output()`](EncoderSink::take_output)
/// after [`finish()`](crate::Sink::finish) to retrieve the encoded bytes.
pub struct EncoderSink<'a> {
    encoder: Option<Box<dyn DynEncoder + Send + 'a>>,
    output: Option<zencodec::encode::EncodeOutput>,
    descriptor: PixelDescriptor,
}

impl<'a> EncoderSink<'a> {
    /// Create an `EncoderSink` from a dyn encoder.
    ///
    /// `format` specifies the pixel format that will be pushed into the encoder.
    /// The caller must ensure strips match this format.
    pub fn new(encoder: Box<dyn DynEncoder + Send + 'a>, format: PixelFormat) -> Self {
        Self {
            encoder: Some(encoder),
            output: None,
            descriptor: format,
        }
    }

    /// Take the encoded output after [`finish()`](crate::Sink::finish).
    ///
    /// Returns `None` if `finish()` hasn't been called or if the output
    /// was already taken.
    pub fn take_output(&mut self) -> Option<zencodec::encode::EncodeOutput> {
        self.output.take()
    }

    /// Suggested strip height for the encoder (e.g., JPEG MCU height).
    pub fn preferred_strip_height(&self) -> u32 {
        self.encoder
            .as_ref()
            .map_or(0, |e| e.preferred_strip_height())
    }
}

impl crate::Sink for EncoderSink<'_> {
    fn consume(&mut self, strip: &Strip<'_>) -> crate::PipeResult<()> {
        let encoder = self
            .encoder
            .as_mut()
            .ok_or_else(|| at!(PipeError::Op("encoder already finished".to_string())))?;

        let pixels = zenpixels::PixelSlice::new(
            strip.as_strided_bytes(),
            strip.width(),
            strip.rows(),
            strip.stride(),
            self.descriptor,
        )
        .map_err(|e| {
            at!(PipeError::Op(alloc::format!(
                "PixelSlice construction failed: {e}"
            )))
        })?;

        encoder
            .push_rows(pixels)
            .map_err(|e| at!(PipeError::Op(e.to_string())))
    }

    fn finish(&mut self) -> crate::PipeResult<()> {
        let encoder = self
            .encoder
            .take()
            .ok_or_else(|| at!(PipeError::Op("encoder already finished".to_string())))?;

        let output = encoder
            .finish()
            .map_err(|e| at!(PipeError::Op(e.to_string())))?;

        self.output = Some(output);
        Ok(())
    }
}
