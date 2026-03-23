//! # zencodecs
//!
//! Unified image codec abstraction over multiple format-specific encoders and decoders.
//!
//! ## Overview
//!
//! zencodecs is a thin dispatch layer that provides:
//! - **Format detection** from magic bytes or file extensions
//! - **Codec dispatch** to format-specific implementations
//! - **Typed pixel buffers** via `imgref::ImgVec` and `rgb` crate types
//! - **Runtime codec registry** for enabling/disabling formats
//! - **Unified error handling** across all codecs
//!
//! Each codec is feature-gated. Enable only what you need:
//!
//! ```toml
//! [dependencies]
//! zencodecs = { version = "0.1", features = ["jpeg", "webp", "png"] }
//! ```
//!
//! ## Usage Examples
//!
//! ### Detect and Decode
//!
//! ```no_run
//! use zencodecs::{ImageFormat, DecodeRequest};
//!
//! let data: &[u8] = &[]; // your image bytes
//! let decoded = DecodeRequest::new(data).decode_full_frame()?;
//! println!("{}x{} {:?}", decoded.width(), decoded.height(), decoded.pixels());
//! # Ok::<(), whereat::At<zencodecs::CodecError>>(())
//! ```
//!
//! ### Encode to Different Format
//!
//! ```no_run
//! use zencodecs::{EncodeRequest, ImageFormat};
//! use zencodecs::pixel::{ImgVec, Rgba};
//!
//! let pixels = ImgVec::new(vec![Rgba { r: 0u8, g: 0, b: 0, a: 255 }; 100*100], 100, 100);
//! let webp = EncodeRequest::new(ImageFormat::WebP)
//!     .with_quality(85.0)
//!     .encode_full_frame_rgba8(pixels.as_ref())?;
//! println!("Encoded {} bytes", webp.len());
//! # Ok::<(), whereat::At<zencodecs::CodecError>>(())
//! ```
//!
//! ### Probe Image Metadata
//!
//! ```no_run
//! use zencodecs::from_bytes;
//!
//! let data: &[u8] = &[]; // your image bytes
//! let info = from_bytes(data)?;
//! println!("{}x{} {:?}", info.width, info.height, info.format);
//! # Ok::<(), whereat::At<zencodecs::CodecError>>(())
//! ```
//!
//! ### Control Available Codecs
//!
//! ```no_run
//! use zencodecs::{CodecRegistry, ImageFormat, DecodeRequest};
//!
//! let registry = CodecRegistry::none()
//!     .with_decode(ImageFormat::Jpeg, true)
//!     .with_decode(ImageFormat::Png, true);
//!
//! let data: &[u8] = &[]; // your image bytes
//! let decoded = DecodeRequest::new(data)
//!     .with_registry(&registry)
//!     .decode_full_frame()?;
//! # Ok::<(), whereat::At<zencodecs::CodecError>>(())
//! ```
//!
//! ## What This Crate Does NOT Do
//!
//! - **No image processing**: No resize, crop, rotate. Use `zenimage` or similar.
//! - **No color management**: No ICC profile application (yet).
//! - **No streaming**: One-shot decode/encode only (streaming planned).
//! - **No animation**: First frame only for animated formats (animation planned).

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

whereat::define_at_crate_info!();

pub mod codec_id;
mod codecs;
pub mod config;
pub mod decision;
mod decode;
pub mod depthmap;
mod dispatch;
mod dyn_dispatch;
mod encode;
mod error;
pub mod exif;
mod format_set;
pub mod gainmap;
mod info;
pub mod intent;
mod limits;
pub mod pixel;
pub mod policy;
pub mod quality;
mod registry;
#[cfg(feature = "riapi")]
pub mod riapi_parse;
pub mod select;
pub mod trace;
pub mod transcode;
#[cfg(feature = "zenode")]
pub mod zenode_defs;

// Re-exports
pub use codec_id::CodecId;
pub use decision::FormatDecision;
pub use decode::{DecodeOutput, DecodeRequest};
pub use dispatch::{AnyEncoder, StreamingEncoder};
pub use encode::{EncodeOutput, EncodeRequest};
pub use error::{CodecError, Result};
pub use format_set::FormatSet;
pub use info::ImageInfo;
pub use info::{decode_info, decode_info_with_config};
pub use info::{from_bytes, from_bytes_format, from_bytes_with_registry};
pub use intent::{BoolKeep, CodecIntent, FormatChoice, PerCodecHints};
pub use limits::{Limits, Stop};
pub use policy::CodecPolicy;
pub use quality::{QualityIntent, QualityProfile};
pub use registry::CodecRegistry;
#[cfg(feature = "riapi")]
pub use riapi_parse::{CodecEngine, parse_codec_keys};
pub use select::ImageFacts;
pub use select::select_format_from_intent;
pub use trace::SelectionTrace;
pub use transcode::{
    SupplementPolicy, SupplementSet, TranscodeOptions, TranscodeOutput, TranscodeSink,
};
pub use zencodec::ImageFormat;
pub use zencodec::Metadata;

// Gain map types (format-agnostic)
#[cfg(feature = "jpeg-ultrahdr")]
pub use gainmap::{DecodedGainMap, GainMap, GainMapMetadata, GainMapSource};
pub use zencodec::gainmap::{GainMapDirection, GainMapInfo, GainMapParams, GainMapPresence};

// Depth map types (format-agnostic)
pub use depthmap::{
    DecodedDepthMap, DepthFormat, DepthImage, DepthMapMetadata, DepthMeasureType, DepthPixelFormat,
    DepthSource, DepthUnits,
};

// zencodec trait re-exports
pub use zencodec::decode::{
    DecodeJob, DecodeRowSink, DecoderConfig, DynFullFrameDecoder, DynStreamingDecoder, OutputInfo,
};
pub use zencodec::encode::{DynEncoder, DynFullFrameEncoder, EncodeJob, EncoderConfig};
pub use zencodec::{FullFrame, OwnedFullFrame};

// Pixel conversion extension traits
pub use zenpixels_convert::PixelBufferConvertExt;
pub use zenpixels_convert::PixelBufferConvertTypedExt;

#[cfg(feature = "png")]
pub use zenpng::{PngDecodeJob, PngDecoderConfig, PngEncodeJob, PngEncoderConfig};

#[cfg(feature = "webp")]
pub use zenwebp::{WebpDecodeJob, WebpDecoderConfig, WebpEncodeJob, WebpEncoderConfig};

#[cfg(feature = "gif")]
pub use zengif::{GifDecodeJob, GifDecoderConfig, GifEncodeJob, GifEncoderConfig};

#[cfg(feature = "jpeg")]
pub use zenjpeg::{JpegDecodeJob, JpegDecoderConfig, JpegEncodeJob, JpegEncoderConfig};

#[cfg(feature = "jpeg-ultrahdr")]
pub use zenjpeg::ultrahdr::UltraHdrExtras;

#[cfg(feature = "jxl-decode")]
pub use zenjxl::{JxlDecodeJob, JxlDecoderConfig};

#[cfg(feature = "jxl-encode")]
pub use zenjxl::{JxlEncodeJob, JxlEncoderConfig};

#[cfg(feature = "heic-decode")]
pub use heic_decoder::{HeicDecodeJob, HeicDecoderConfig};

#[cfg(feature = "bitmaps")]
pub use zenbitmaps::{
    FarbfeldDecodeJob, FarbfeldDecoderConfig, FarbfeldEncodeJob, FarbfeldEncoderConfig,
    PnmDecodeJob, PnmDecoderConfig, PnmEncodeJob, PnmEncoderConfig,
};

#[cfg(feature = "bitmaps-bmp")]
pub use zenbitmaps::{BmpDecodeJob, BmpDecoderConfig, BmpEncodeJob, BmpEncoderConfig};

#[cfg(feature = "raw-decode")]
pub use zenraw::{RawDecodeConfig, RawDecoderConfig};

// ═══════════════════════════════════════════════════════════════════════
// Top-level convenience functions (streaming-first API)
// ═══════════════════════════════════════════════════════════════════════

/// Primary decode API: push decoded rows into a sink. Zero materialization.
///
/// The decoder writes rows into the sink as they become available.
/// For formats that support row-based streaming (JPEG, PNG), this
/// avoids allocating the full image buffer. For formats that require
/// full-image decode (WebP, AVIF), the codec buffers internally.
///
/// This is the preferred decode path. Use [`DecodeRequest::decode_full_frame()`]
/// only when you genuinely need all pixels in memory.
///
/// # Example
///
/// ```rust,ignore
/// use zencodecs::{push_decode, CodecRegistry};
///
/// let mut sink = /* your DecodeRowSink impl */;
/// let info = zencodecs::push_decode(&data, &mut sink, &CodecRegistry::all())?;
/// println!("Decoded {}x{}", info.width(), info.height());
/// ```
pub fn push_decode(
    data: &[u8],
    sink: &mut dyn zencodec::decode::DecodeRowSink,
    registry: &CodecRegistry,
) -> error::Result<zencodec::decode::OutputInfo> {
    DecodeRequest::new(data)
        .with_registry(registry)
        .push_decode(sink)
}

/// Primary encode API: build a streaming encoder.
///
/// Returns a [`StreamingEncoder`] containing a `DynEncoder` that accepts
/// rows via `push_rows()` and produces encoded bytes on `finish()`.
///
/// The caller is responsible for pixel format conversion per-strip via
/// [`zenpixels_convert::adapt::adapt_for_encode`]. The `StreamingEncoder`
/// provides the encoder's `supported` pixel descriptors for negotiation.
///
/// # Lifetime issue (current status)
///
/// The streaming encoder is currently stubbed due to a lifetime GAT issue.
/// Fix path: have each codec produce a `Box<dyn DynEncoder + 'static>`
/// directly via a factory method that takes owned config, bypassing the
/// GAT borrow chain. See [`dispatch::build_streaming_encoder`] for details.
///
/// # Example
///
/// ```rust,ignore
/// use zencodecs::{streaming_encoder, CodecRegistry, FormatDecision, ImageFormat};
/// use zencodecs::quality::QualityIntent;
///
/// let decision = FormatDecision {
///     format: ImageFormat::Jpeg,
///     quality: QualityIntent::from_quality(85.0),
///     ..Default::default()
/// };
///
/// let se = zencodecs::streaming_encoder(
///     ImageFormat::Jpeg,
///     &decision,
///     1920, 1080,
///     None,
///     &CodecRegistry::all(),
/// )?;
/// // se.encoder.push_rows(strip)?;
/// // let output = se.encoder.finish()?;
/// ```
pub fn streaming_encoder<'a>(
    format: ImageFormat,
    decision: &FormatDecision,
    width: u32,
    height: u32,
    metadata: Option<&'a zencodec::Metadata>,
    registry: &'a CodecRegistry,
) -> error::Result<StreamingEncoder<'a>> {
    let mut request = EncodeRequest::new(format)
        .with_quality(decision.quality.quality)
        .with_registry(registry);

    if decision.lossless {
        request = request.with_lossless(true);
    }
    if let Some(effort) = decision.quality.effort {
        request = request.with_effort(effort);
    }
    if let Some(meta) = metadata {
        request = request.with_metadata(meta);
    }

    request.build_streaming_encoder(width, height)
}

/// Probe image metadata without decoding pixels.
///
/// Detects format from magic bytes and dispatches to the appropriate
/// codec's header parser. Returns dimensions, format, alpha, animation,
/// supplement inventory, and embedded metadata.
///
/// This is a convenience wrapper around [`from_bytes_with_registry`].
///
/// # Example
///
/// ```no_run
/// use zencodecs::{probe, CodecRegistry};
///
/// let data: &[u8] = &[]; // your image bytes
/// let info = zencodecs::probe(data, &CodecRegistry::all())?;
/// println!("{}x{} {:?}", info.width, info.height, info.format);
/// # Ok::<(), whereat::At<zencodecs::CodecError>>(())
/// ```
pub fn probe(data: &[u8], registry: &CodecRegistry) -> error::Result<zencodec::ImageInfo> {
    from_bytes_with_registry(data, registry)
}

/// Transcode an image: decode and re-encode to a different format/quality.
///
/// This is a convenience wrapper around [`transcode::transcode`].
/// See that function for full documentation.
///
/// # Example
///
/// ```rust,ignore
/// use zencodecs::{transcode, TranscodeOptions, FormatDecision, CodecRegistry};
///
/// let output = zencodecs::transcode(
///     &jpeg_bytes,
///     &decision,
///     &TranscodeOptions::default(),
///     &CodecRegistry::all(),
/// )?;
/// ```
pub use transcode::transcode;
