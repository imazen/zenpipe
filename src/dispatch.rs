//! Dynamic encoder dispatch.
//!
//! Provides [`build_encoder`] factory that creates a type-erased encoder closure
//! for any supported format. Each codec's `Encoder` trait impl handles pixel
//! format dispatch internally.

use crate::config::CodecConfig;
use crate::{CodecError, ImageFormat, Limits, MetadataView, Stop};
use alloc::boxed::Box;
use zencodec_types::EncodeOutput;
use zenpixels::{PixelDescriptor, PixelSlice};

/// Encoding parameters extracted from [`EncodeRequest`](crate::EncodeRequest).
pub(crate) struct EncodeParams<'a> {
    pub quality: Option<f32>,
    pub effort: Option<u32>,
    pub lossless: bool,
    pub metadata: Option<&'a MetadataView<'a>>,
    pub codec_config: Option<&'a CodecConfig>,
    pub limits: Option<&'a Limits>,
    pub stop: Option<&'a dyn Stop>,
}

/// Type-erased one-shot encode closure.
pub(crate) type EncodeFn<'a> =
    Box<dyn FnOnce(PixelSlice<'_>) -> Result<EncodeOutput, CodecError> + 'a>;

/// A built encoder: a closure that encodes pixels + its supported descriptors.
pub(crate) struct BuiltEncoder<'a> {
    pub encoder: EncodeFn<'a>,
    pub supported: &'static [PixelDescriptor],
}

/// Build a type-erased encoder for the specified format.
///
/// Each codec arm creates a closure that:
/// 1. Builds the codec-specific config from quality/effort/lossless/codec_config
/// 2. Creates the encode job and applies limits/metadata/stop
/// 3. Calls `Encoder::encode(pixels)` via the trait
pub(crate) fn build_encoder<'a>(
    format: ImageFormat,
    params: EncodeParams<'a>,
) -> Result<BuiltEncoder<'a>, CodecError> {
    match format {
        #[cfg(feature = "jpeg")]
        ImageFormat::Jpeg => Ok(crate::codecs::jpeg::build_trait_encoder(params)),
        #[cfg(not(feature = "jpeg"))]
        ImageFormat::Jpeg => Err(CodecError::UnsupportedFormat(format)),

        #[cfg(feature = "webp")]
        ImageFormat::WebP => Ok(crate::codecs::webp::build_trait_encoder(params)),
        #[cfg(not(feature = "webp"))]
        ImageFormat::WebP => Err(CodecError::UnsupportedFormat(format)),

        #[cfg(feature = "gif")]
        ImageFormat::Gif => Ok(crate::codecs::gif::build_trait_encoder(params)),
        #[cfg(not(feature = "gif"))]
        ImageFormat::Gif => Err(CodecError::UnsupportedFormat(format)),

        #[cfg(feature = "png")]
        ImageFormat::Png => Ok(crate::codecs::png::build_trait_encoder(params)),
        #[cfg(not(feature = "png"))]
        ImageFormat::Png => Err(CodecError::UnsupportedFormat(format)),

        #[cfg(feature = "avif-encode")]
        ImageFormat::Avif => Ok(crate::codecs::avif_enc::build_trait_encoder(params)),
        #[cfg(not(feature = "avif-encode"))]
        ImageFormat::Avif => Err(CodecError::UnsupportedFormat(format)),

        #[cfg(feature = "jxl-encode")]
        ImageFormat::Jxl => Ok(crate::codecs::jxl_enc::build_trait_encoder(params)),
        #[cfg(not(feature = "jxl-encode"))]
        ImageFormat::Jxl => Err(CodecError::UnsupportedFormat(format)),

        _ => Err(CodecError::UnsupportedFormat(format)),
    }
}
