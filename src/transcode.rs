//! Transcode API and streaming decode→encode bridge.
//!
//! ## Transcode function
//!
//! [`transcode()`] provides a high-level one-call transcode: decode the input,
//! re-encode to the target format specified by a [`FormatDecision`], and return
//! the encoded bytes. Metadata (EXIF, ICC, XMP) is roundtripped by default.
//!
//! ## TranscodeSink
//!
//! [`TranscodeSink`] is the low-level streaming bridge. It implements
//! [`DecodeRowSink`] and forwards decoded strips directly to an encoder's
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

use crate::decision::FormatDecision;
use crate::error::Result;
use crate::{CodecError, CodecRegistry, ImageFormat};
use whereat::at;

// ═══════════════════════════════════════════════════════════════════════
// TranscodeOptions, SupplementPolicy, SupplementSet
// ═══════════════════════════════════════════════════════════════════════

/// Options controlling a transcode operation.
///
/// Controls metadata roundtrip, supplement handling, and alpha compositing.
#[derive(Clone, Debug)]
pub struct TranscodeOptions<'a> {
    /// Metadata to embed in the output (EXIF, ICC, XMP).
    ///
    /// - `None` (default): extract metadata from the source and roundtrip it.
    /// - `Some(meta)`: use the provided metadata instead of the source's.
    pub metadata: Option<&'a zencodec::Metadata>,

    /// How to handle container supplements (gain maps, depth maps, etc.)
    /// during transcode.
    pub supplements: SupplementPolicy,

    /// Matte color for alpha compositing when encoding to a format without
    /// alpha (e.g., RGBA source → JPEG output).
    ///
    /// `None` defaults to white `[255, 255, 255]`.
    pub matte: Option<[u8; 3]>,
}

impl Default for TranscodeOptions<'_> {
    fn default() -> Self {
        Self {
            metadata: None,
            supplements: SupplementPolicy::default(),
            matte: None,
        }
    }
}

/// What to do with container supplements (gain maps, depth maps, etc.)
/// during transcode.
#[derive(Clone, Copy, Debug, Default)]
pub enum SupplementPolicy {
    /// Roundtrip all supplements the target format supports.
    ///
    /// Gain maps, depth maps, and auxiliary images are extracted from the
    /// source container and re-embedded in the output container.
    /// Supplements that the target format can't represent are silently dropped.
    #[default]
    Preserve,

    /// Strip all supplements. Output contains only the primary image + metadata.
    Strip,

    /// Preserve only specific supplement types.
    Only(SupplementSet),
}

/// Bitflag set of supplement types.
///
/// Used with [`SupplementPolicy::Only`] to selectively preserve supplements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SupplementSet(u32);

impl SupplementSet {
    /// UltraHDR / ISO 21496-1 gain map.
    pub const GAIN_MAP: Self = Self(1);
    /// Depth / disparity map.
    pub const DEPTH_MAP: Self = Self(2);
    /// Embedded thumbnail.
    pub const THUMBNAIL: Self = Self(4);

    /// Check whether a specific supplement type is in this set.
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// Combine two supplement sets (union).
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Check whether this set is empty.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl core::ops::BitOr for SupplementSet {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// TranscodeOutput
// ═══════════════════════════════════════════════════════════════════════

/// The result of a [`transcode()`] operation.
#[derive(Clone, Debug)]
pub struct TranscodeOutput {
    /// The encoded image bytes.
    pub data: Vec<u8>,
    /// The output format.
    pub format: ImageFormat,
    /// The MIME type of the output format.
    pub mime_type: &'static str,
}

// ═══════════════════════════════════════════════════════════════════════
// transcode() — high-level one-call transcode
// ═══════════════════════════════════════════════════════════════════════

/// Transcode an image: decode from `data`, re-encode to the format and
/// quality specified by `decision`.
///
/// This is the primary transcode entry point. Metadata (EXIF, ICC, XMP)
/// is roundtripped from the source unless overridden in `opts`.
///
/// # Current implementation
///
/// Uses the one-shot decode + encode path internally: decodes the full
/// image, then re-encodes it. This materializes the entire image in memory.
///
/// # TODO
///
/// Wire through [`TranscodeSink`] for true zero-materialization streaming.
/// The API shape is stable; only the internals will change.
///
/// # Example
///
/// ```rust,ignore
/// use zencodecs::{transcode, TranscodeOptions, FormatDecision, CodecRegistry};
/// use zencodecs::quality::QualityIntent;
/// use zencodecs::ImageFormat;
///
/// let decision = FormatDecision {
///     format: ImageFormat::WebP,
///     quality: QualityIntent::from_quality(80.0),
///     lossless: false,
///     hints: Default::default(),
///     matte: None,
///     trace: Vec::new(),
/// };
///
/// let output = zencodecs::transcode(
///     &jpeg_bytes,
///     &decision,
///     &TranscodeOptions::default(),
///     &CodecRegistry::all(),
/// )?;
/// assert_eq!(output.format, ImageFormat::WebP);
/// ```
pub fn transcode(
    data: &[u8],
    decision: &FormatDecision,
    opts: &TranscodeOptions<'_>,
    registry: &CodecRegistry,
) -> Result<TranscodeOutput> {
    // Step 1: Decode the source image (full materialization for now)
    let decoded = crate::DecodeRequest::new(data)
        .with_registry(registry)
        .decode_full_frame()?;

    // Step 2: Determine metadata to embed
    let source_metadata;
    let metadata = match opts.metadata {
        Some(m) => m,
        None => {
            // Roundtrip metadata from source via probe
            match crate::info::from_bytes_with_registry(data, registry) {
                Ok(info) => {
                    source_metadata = info.metadata();
                    &source_metadata
                }
                Err(_) => {
                    // No metadata to roundtrip — proceed without it
                    source_metadata = zencodec::Metadata::none();
                    &source_metadata
                }
            }
        }
    };

    // Step 3: Encode to the target format
    let format = decision.format;
    if !registry.can_encode(format) {
        return Err(at!(CodecError::DisabledFormat(format)));
    }

    // Build the encode request from the decision
    let mut request = crate::EncodeRequest::new(format)
        .with_quality(decision.quality.quality)
        .with_metadata(metadata)
        .with_registry(registry);

    if decision.lossless {
        request = request.with_lossless(true);
    }
    if let Some(effort) = decision.quality.effort {
        request = request.with_effort(effort);
    }

    // Convert decoded pixels to the appropriate encoding call
    use zenpixels_convert::PixelBufferConvertTypedExt as _;
    let buffer = decoded.into_buffer();
    let rgb8 = buffer.to_rgb8();

    let encode_output = request.encode_full_frame_rgb8(rgb8.as_imgref())?;

    Ok(TranscodeOutput {
        data: encode_output.into_vec(),
        format,
        mime_type: format.mime_type(),
    })
}

// ═══════════════════════════════════════════════════════════════════════
// TranscodeSink — streaming decode→encode bridge
// ═══════════════════════════════════════════════════════════════════════

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
    fn flush_pending(&mut self) -> core::result::Result<(), SinkError> {
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
    ) -> core::result::Result<(), SinkError> {
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
    ) -> core::result::Result<PixelSliceMut<'_>, SinkError> {
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

    fn finish(&mut self) -> core::result::Result<(), SinkError> {
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

    #[test]
    fn supplement_set_operations() {
        let set = SupplementSet::GAIN_MAP | SupplementSet::DEPTH_MAP;
        assert!(set.contains(SupplementSet::GAIN_MAP));
        assert!(set.contains(SupplementSet::DEPTH_MAP));
        assert!(!set.contains(SupplementSet::THUMBNAIL));

        let empty = SupplementSet(0);
        assert!(empty.is_empty());
        assert!(!set.is_empty());

        assert_eq!(
            SupplementSet::GAIN_MAP.union(SupplementSet::THUMBNAIL),
            SupplementSet::GAIN_MAP | SupplementSet::THUMBNAIL
        );
    }

    #[test]
    fn transcode_options_default() {
        let opts = TranscodeOptions::default();
        assert!(opts.metadata.is_none());
        assert!(opts.matte.is_none());
        assert!(matches!(opts.supplements, SupplementPolicy::Preserve));
    }

    /// Round-trip: encode a tiny JPEG, transcode to WebP, verify output.
    #[cfg(all(feature = "jpeg", feature = "webp"))]
    #[test]
    fn transcode_jpeg_to_webp() {
        use crate::quality::QualityIntent;

        // Create a small test image
        let img = imgref::ImgVec::new(
            alloc::vec![
                rgb::Rgb {
                    r: 128u8,
                    g: 64,
                    b: 32
                };
                10 * 10
            ],
            10,
            10,
        );

        // Encode to JPEG first
        let jpeg_output = crate::EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality(75.0)
            .encode_full_frame_rgb8(img.as_ref())
            .unwrap();
        assert!(!jpeg_output.data().is_empty());

        // Now transcode JPEG → WebP
        let decision = FormatDecision {
            format: ImageFormat::WebP,
            quality: QualityIntent::from_quality(80.0),
            lossless: false,
            hints: Default::default(),
            matte: None,
            trace: alloc::vec::Vec::new(),
        };

        let output = transcode(
            jpeg_output.data(),
            &decision,
            &TranscodeOptions::default(),
            &CodecRegistry::all(),
        )
        .unwrap();

        assert_eq!(output.format, ImageFormat::WebP);
        assert_eq!(output.mime_type, "image/webp");
        assert!(!output.data.is_empty());

        // Verify we can decode the transcoded output
        let decoded = crate::DecodeRequest::new(&output.data)
            .decode_full_frame()
            .unwrap();
        assert_eq!(decoded.width(), 10);
        assert_eq!(decoded.height(), 10);
    }

    /// Round-trip: encode a tiny image, transcode keeping the same format.
    #[cfg(feature = "jpeg")]
    #[test]
    fn transcode_jpeg_to_jpeg() {
        use crate::quality::QualityIntent;

        let img = imgref::ImgVec::new(
            alloc::vec![
                rgb::Rgb {
                    r: 200u8,
                    g: 100,
                    b: 50
                };
                8 * 8
            ],
            8,
            8,
        );

        let jpeg_output = crate::EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality(90.0)
            .encode_full_frame_rgb8(img.as_ref())
            .unwrap();

        let decision = FormatDecision {
            format: ImageFormat::Jpeg,
            quality: QualityIntent::from_quality(70.0),
            lossless: false,
            hints: Default::default(),
            matte: None,
            trace: alloc::vec::Vec::new(),
        };

        let output = transcode(
            jpeg_output.data(),
            &decision,
            &TranscodeOptions::default(),
            &CodecRegistry::all(),
        )
        .unwrap();

        assert_eq!(output.format, ImageFormat::Jpeg);
        assert_eq!(output.mime_type, "image/jpeg");
        assert!(!output.data.is_empty());

        // Lower quality should produce fewer bytes
        assert!(
            output.data.len() <= jpeg_output.data().len(),
            "q70 ({}) should be <= q90 ({})",
            output.data.len(),
            jpeg_output.data().len(),
        );
    }
}
