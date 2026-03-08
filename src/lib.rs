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
//! let decoded = DecodeRequest::new(data).decode()?;
//! println!("{}x{} {:?}", decoded.width(), decoded.height(), decoded.pixels());
//! # Ok::<(), zencodecs::CodecError>(())
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
//!     .encode_rgba8(pixels.as_ref())?;
//! println!("Encoded {} bytes", webp.len());
//! # Ok::<(), zencodecs::CodecError>(())
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
//! # Ok::<(), zencodecs::CodecError>(())
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
//!     .decode()?;
//! # Ok::<(), zencodecs::CodecError>(())
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

mod codecs;
pub mod config;
mod decode;
mod dispatch;
mod encode;
mod error;
mod info;
mod limits;
#[cfg(feature = "pipeline")]
pub mod pipeline;
pub mod pixel;
mod registry;

// Re-exports
pub use decode::{DecodeOutput, DecodeRequest};
pub use dispatch::AnyEncoder;
pub use encode::{EncodeOutput, EncodeRequest};
pub use error::CodecError;
pub use info::ImageInfo;
pub use info::{decode_info, decode_info_with_config};
pub use info::{from_bytes, from_bytes_format, from_bytes_with_registry};
pub use limits::{Limits, Stop};
pub use registry::CodecRegistry;
pub use zc::ImageFormat;
pub use zc::MetadataView;

// zencodec-types trait re-exports
pub use zc::decode::{DecodeJob, DecoderConfig};
pub use zc::encode::{EncodeJob, EncoderConfig};

// Pixel conversion extension trait (provides to_rgb8(), to_rgba8(), etc.)
pub use zenpixels_convert::PixelBufferConvertExt;

#[cfg(feature = "png")]
pub use zenpng::{PngDecodeJob, PngDecoderConfig, PngEncodeJob, PngEncoderConfig};

#[cfg(feature = "webp")]
pub use zenwebp::{WebpDecodeJob, WebpDecoderConfig, WebpEncodeJob, WebpEncoderConfig};

#[cfg(feature = "gif")]
pub use zengif::{GifDecodeJob, GifDecoderConfig, GifEncodeJob, GifEncoderConfig};

#[cfg(feature = "jpeg")]
pub use zenjpeg::{JpegDecodeJob, JpegDecoderConfig, JpegEncodeJob, JpegEncoderConfig};

#[cfg(feature = "jpeg-ultrahdr")]
pub use zenjpeg::ultrahdr::{GainMap, GainMapMetadata, UltraHdrExtras};

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
