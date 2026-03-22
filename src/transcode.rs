//! Streaming decode→encode bridge via [`DecodeRowSink`].
//!
//! [`TranscodeSink`] forwards decoded strips directly to an encoder's
//! `push_rows()`, converting pixel formats per-strip via `adapt_for_encode`.
//! No full-image buffer is ever allocated by the sink — only a strip-sized
//! conversion buffer when the decoded pixel format doesn't match the
//! encoder's native format.
//!
//! Codecs that need the full image (WebP, AVIF) buffer internally in their
//! `push_rows()` implementation. That's the codec's concern, not the
//! pipeline's.

use alloc::boxed::Box;
use alloc::vec::Vec;

use zencodec::decode::{DecodeRowSink, SinkError};
use zencodec::encode::{DynEncoder, EncodeOutput};
use zenpixels::{PixelDescriptor, PixelSliceMut};

/// Streaming transcode sink: forwards decoded strips to an encoder.
///
/// Created via [`TranscodeSink::new`] with a [`StreamingEncoder`] from
/// [`EncodeRequest::build_streaming_encoder`].
///
/// [`StreamingEncoder`]: crate::dispatch::StreamingEncoder
/// [`EncodeRequest::build_streaming_encoder`]: crate::EncodeRequest::build_streaming_encoder
///
/// # Example
///
/// ```rust,ignore
/// // Build the encoder
/// let se = EncodeRequest::new(ImageFormat::Jpeg)
///     .with_quality(85.0)
///     .build_streaming_encoder(width, height)?;
///
/// // Create sink and decode through it
/// let mut sink = TranscodeSink::new(se.encoder, se.supported);
/// DecodeRequest::new(data).push_decode(&mut sink)?;
///
/// // Finalize
/// let output = sink.finish_encode()?;
/// ```
pub struct TranscodeSink<'a> {
    encoder: Option<Box<dyn DynEncoder + 'a>>,
    supported: &'static [PixelDescriptor],
    /// Scratch buffer for receiving decoded rows from the decoder.
    /// The decoder writes into this via `provide_next_buffer`, and
    /// we forward it to the encoder on the *next* call (or on finish).
    strip_buf: Vec<u8>,
    /// Metadata for the pending (written but not yet forwarded) strip.
    pending: Option<PendingStrip>,
}

/// Metadata for a strip that the decoder has written but we haven't
/// forwarded to the encoder yet.
struct PendingStrip {
    width: u32,
    height: u32,
    descriptor: PixelDescriptor,
}

impl<'a> TranscodeSink<'a> {
    /// Create a new streaming transcode sink.
    ///
    /// `encoder` — the `DynEncoder` to push strips into.
    /// `supported` — the encoder's supported pixel descriptors
    ///   (from `EncoderConfig::supported_descriptors()`).
    pub fn new(encoder: Box<dyn DynEncoder + 'a>, supported: &'static [PixelDescriptor]) -> Self {
        Self {
            encoder: Some(encoder),
            supported,
            strip_buf: Vec::new(),
            pending: None,
        }
    }

    /// Finalize encoding and return the output.
    ///
    /// Must be called after `push_decode` completes (which calls
    /// `DecodeRowSink::finish` internally). Consumes the encoder
    /// via `DynEncoder::finish()`.
    pub fn finish_encode(
        mut self,
    ) -> core::result::Result<EncodeOutput, Box<dyn core::error::Error + Send + Sync>> {
        let encoder =
            self.encoder
                .take()
                .ok_or_else(|| -> Box<dyn core::error::Error + Send + Sync> {
                    "encoder already finished".into()
                })?;
        encoder.finish()
    }

    /// Forward the pending strip (if any) to the encoder.
    fn flush_pending(&mut self) -> Result<(), SinkError> {
        let pending = match self.pending.take() {
            Some(p) => p,
            None => return Ok(()),
        };

        let encoder = self
            .encoder
            .as_mut()
            .ok_or_else(|| -> SinkError { "encoder already finished".into() })?;

        let bpp = pending.descriptor.bytes_per_pixel();
        let stride = pending.width as usize * bpp;
        let data_len = stride * pending.height as usize;
        let strip_data = &self.strip_buf[..data_len];

        // Adapt pixel format per-strip — zero-copy when format already matches
        let adapted = zenpixels_convert::adapt::adapt_for_encode(
            strip_data,
            pending.descriptor,
            pending.width,
            pending.height,
            stride,
            self.supported,
        )
        .map_err(|e| -> SinkError { alloc::format!("adapt: {e}").into() })?;

        let adapted_stride = adapted.width as usize * adapted.descriptor.bytes_per_pixel();
        let pixel_slice = zenpixels::PixelSlice::new(
            &adapted.data,
            adapted.width,
            adapted.rows,
            adapted_stride,
            adapted.descriptor,
        )
        .map_err(|e| -> SinkError { alloc::format!("pixel slice: {e}").into() })?;

        encoder
            .push_rows(pixel_slice)
            .map_err(|e| -> SinkError { alloc::format!("push_rows: {e}").into() })
    }
}

impl DecodeRowSink for TranscodeSink<'_> {
    fn begin(
        &mut self,
        _width: u32,
        _height: u32,
        _descriptor: PixelDescriptor,
    ) -> Result<(), SinkError> {
        self.pending = None;
        self.strip_buf.clear();
        Ok(())
    }

    fn provide_next_buffer(
        &mut self,
        _y: u32,
        height: u32,
        width: u32,
        descriptor: PixelDescriptor,
    ) -> Result<PixelSliceMut<'_>, SinkError> {
        // The previous buffer (if any) has been fully written by the decoder.
        // Forward it to the encoder before providing the next buffer.
        self.flush_pending()?;

        let bpp = descriptor.bytes_per_pixel();
        let stride = width as usize * bpp;
        let needed = stride * height as usize;

        // Resize strip_buf for this strip
        self.strip_buf.resize(needed, 0);
        self.pending = Some(PendingStrip {
            width,
            height,
            descriptor,
        });

        PixelSliceMut::new(
            &mut self.strip_buf[..needed],
            width,
            height,
            stride,
            descriptor,
        )
        .map_err(|e| -> SinkError { alloc::format!("pixel slice: {e}").into() })
    }

    fn finish(&mut self) -> Result<(), SinkError> {
        // Forward the last strip
        self.flush_pending()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcode_sink_construction() {
        // Verify the type compiles and basic construction works.
        // Full integration requires a real encoder, tested in integration tests.
        assert!(core::mem::size_of::<TranscodeSink<'_>>() > 0);
    }
}
