//! Bridge between zencodec's `Dyn*` traits and zenpipe's `Source`/`Sink` model.
//!
//! Provides [`DecoderSource`] (wraps a streaming decoder as a [`Source`]) and
//! [`EncoderSink`] (wraps an encoder as a [`Sink`]).
//!
//! # Codec-specific parameters
//!
//! All codec configuration goes through zencodec's `Dyn*` traits. For
//! codec-specific options, use `extensions()` → `Any` downcast on the
//! job before creating the decoder/encoder:
//!
//! ```rust,ignore
//! let mut job = decoder_config.dyn_job();
//! if let Some(ext) = job.extensions_mut() {
//!     if let Some(jpeg) = ext.downcast_mut::<JpegDecodeExtensions>() {
//!         jpeg.fancy_upsampling = true;
//!     }
//! }
//! let streaming = job.into_streaming_decoder(data, &[PixelDescriptor::RGBA8_SRGB])?;
//! let source = DecoderSource::new(streaming, PixelDescriptor::RGBA8_SRGB)?;
//! ```

use alloc::boxed::Box;
use alloc::string::ToString;

use zencodec::decode::DynStreamingDecoder;
use zencodec::encode::DynEncoder;
use zenpixels::PixelDescriptor;

use crate::Source;
use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::strip::{Strip, StripBuf};

// =============================================================================
// DecoderSource
// =============================================================================

/// Wraps a zencodec [`DynStreamingDecoder`] as a zenpipe [`Source`].
///
/// Pulls scanline batches from the decoder and yields them as [`Strip`]s.
/// The decoder's pixel data is copied into an internal buffer so the strip
/// lifetime is tied to this source, not the decoder.
pub struct DecoderSource<'a> {
    decoder: Box<dyn DynStreamingDecoder + Send + 'a>,
    width: u32,
    height: u32,
    format: PixelFormat,
    buf: StripBuf,
    y: u32,
}

impl<'a> DecoderSource<'a> {
    /// Create a `DecoderSource` from a streaming decoder.
    ///
    /// `format` is the expected output pixel format — it must match the
    /// `preferred` descriptors used when creating the streaming decoder.
    /// Width and height are read from the decoder's [`ImageInfo`].
    pub fn new(
        decoder: Box<dyn DynStreamingDecoder + Send + 'a>,
        format: PixelFormat,
    ) -> Result<Self, PipeError> {
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
            y: 0,
        })
    }
}

impl Source for DecoderSource<'_> {
    fn next(&mut self) -> Result<Option<Strip<'_>>, PipeError> {
        if self.y >= self.height {
            return Ok(None);
        }

        let batch = self
            .decoder
            .next_batch()
            .map_err(|e| PipeError::Op(e.to_string()))?;

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
    fn consume(&mut self, strip: &Strip<'_>) -> Result<(), PipeError> {
        let encoder = self
            .encoder
            .as_mut()
            .ok_or_else(|| PipeError::Op("encoder already finished".to_string()))?;

        let pixels = zenpixels::PixelSlice::new(
            strip.as_strided_bytes(),
            strip.width(),
            strip.rows(),
            strip.stride(),
            self.descriptor,
        )
        .map_err(|e| PipeError::Op(alloc::format!("PixelSlice construction failed: {e}")))?;

        encoder
            .push_rows(pixels)
            .map_err(|e| PipeError::Op(e.to_string()))
    }

    fn finish(&mut self) -> Result<(), PipeError> {
        let encoder = self
            .encoder
            .take()
            .ok_or_else(|| PipeError::Op("encoder already finished".to_string()))?;

        let output = encoder.finish().map_err(|e| PipeError::Op(e.to_string()))?;

        self.output = Some(output);
        Ok(())
    }
}
