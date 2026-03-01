//! Dynamic encoder dispatch.
//!
//! Provides [`DynEncoder`] trait and [`build_encoder`] factory to eliminate
//! the O(formats × pixel_types) match explosion in encode.rs.
//!
//! Each codec implements `DynEncoder` once, handling pixel type dispatch
//! internally. The `build_encoder` function does a single match on
//! [`ImageFormat`] to select the right codec.

use crate::config::CodecConfig;
use crate::{CodecError, ImageFormat, Limits, MetadataView, Stop};
use alloc::boxed::Box;
use zencodec_types::{EncodeOutput, PixelDescriptor};

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

/// Type-erased encoder. Each codec implements this once.
///
/// The caller ensures that `data` passed to [`encode_pixels`](DynEncoder::encode_pixels)
/// has a descriptor matching one of [`supported_descriptors`](DynEncoder::supported_descriptors).
/// Use [`zenpixels::adapt::adapt_for_encode`] to negotiate and convert beforehand.
pub(crate) trait DynEncoder {
    /// Which format this encoder produces.
    fn format(&self) -> ImageFormat;

    /// Pixel formats this encoder accepts natively (in preference order).
    fn supported_descriptors(&self) -> &'static [PixelDescriptor];

    /// Encode pixel data.
    ///
    /// - `data`: raw pixel bytes, `rows * stride` bytes minimum
    /// - `descriptor`: pixel format of `data`
    /// - `width`: pixels per row
    /// - `height`: number of rows
    /// - `stride`: byte distance between row starts
    fn encode_pixels(
        self: Box<Self>,
        data: &[u8],
        descriptor: PixelDescriptor,
        width: u32,
        height: u32,
        stride: usize,
    ) -> Result<EncodeOutput, CodecError>;
}

/// Build a [`DynEncoder`] for the specified format.
///
/// Single match on `ImageFormat` — replaces the 9× format matches in the
/// old `encode_format_*` methods.
pub(crate) fn build_encoder<'a>(
    format: ImageFormat,
    params: EncodeParams<'a>,
) -> Result<Box<dyn DynEncoder + 'a>, CodecError> {
    match format {
        #[cfg(feature = "jpeg")]
        ImageFormat::Jpeg => Ok(Box::new(crate::codecs::jpeg::build_dyn_encoder(params))),
        #[cfg(not(feature = "jpeg"))]
        ImageFormat::Jpeg => Err(CodecError::UnsupportedFormat(format)),

        #[cfg(feature = "webp")]
        ImageFormat::WebP => Ok(Box::new(crate::codecs::webp::build_dyn_encoder(params))),
        #[cfg(not(feature = "webp"))]
        ImageFormat::WebP => Err(CodecError::UnsupportedFormat(format)),

        #[cfg(feature = "gif")]
        ImageFormat::Gif => Ok(Box::new(crate::codecs::gif::build_dyn_encoder(params))),
        #[cfg(not(feature = "gif"))]
        ImageFormat::Gif => Err(CodecError::UnsupportedFormat(format)),

        #[cfg(feature = "png")]
        ImageFormat::Png => Ok(Box::new(crate::codecs::png::build_dyn_encoder(params))),
        #[cfg(not(feature = "png"))]
        ImageFormat::Png => Err(CodecError::UnsupportedFormat(format)),

        #[cfg(feature = "avif-encode")]
        ImageFormat::Avif => Ok(Box::new(crate::codecs::avif_enc::build_dyn_encoder(params))),
        #[cfg(not(feature = "avif-encode"))]
        ImageFormat::Avif => Err(CodecError::UnsupportedFormat(format)),

        #[cfg(feature = "jxl-encode")]
        ImageFormat::Jxl => Ok(Box::new(crate::codecs::jxl_enc::build_dyn_encoder(params))),
        #[cfg(not(feature = "jxl-encode"))]
        ImageFormat::Jxl => Err(CodecError::UnsupportedFormat(format)),

        _ => Err(CodecError::UnsupportedFormat(format)),
    }
}
