//! Dynamic encoder dispatch.
//!
//! Provides [`build_encoder`] factory that creates a type-erased encoder closure
//! for any supported format. Each codec's `Encoder` trait impl handles pixel
//! format dispatch internally.

use crate::config::CodecConfig;
use crate::{CodecError, ImageFormat, Limits, MetadataView, Stop};
use alloc::boxed::Box;
use zc::encode::EncodeOutput;
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

/// Build a type-erased encoder from a config-building closure.
///
/// The closure receives `EncodeParams` and returns the concrete `EncoderConfig`.
/// Config construction happens inside the returned closure so the config's
/// lifetime doesn't escape the function.
pub(crate) fn build_from_config<'a, C, F>(
    build_config: F,
    params: EncodeParams<'a>,
) -> BuiltEncoder<'a>
where
    C: zc::encode::EncoderConfig + 'a,
    F: FnOnce(&EncodeParams<'a>) -> C + 'a,
    for<'b> <C::Job<'b> as zc::encode::EncodeJob<'b>>::Enc: zc::encode::Encoder,
{
    BuiltEncoder {
        encoder: Box::new(move |pixels| {
            use zc::encode::{EncodeJob as _, Encoder as _};
            let config = build_config(&params);
            let mut job = config.job();
            if let Some(lim) = params.limits {
                job = job.with_limits(crate::limits::to_resource_limits(lim));
            }
            if let Some(meta) = params.metadata {
                job = job.with_metadata(meta);
            }
            if let Some(s) = params.stop {
                job = job.with_stop(s);
            }
            let format = C::format();
            let enc = job
                .encoder()
                .map_err(|e| CodecError::from_codec(format, e))?;
            enc.encode(pixels)
                .map_err(|e| CodecError::from_codec(format, e))
        }),
        supported: C::supported_descriptors(),
    }
}

// ===========================================================================
// Object-safe encoder config — zero-generics codec-agnostic encoding
// ===========================================================================

/// Object-safe encoder configuration.
///
/// Blanket-implemented for all [`EncoderConfig`](zc::encode::EncoderConfig)
/// types whose encoder implements [`Encoder`](zc::encode::Encoder).
/// Enables fully codec-agnostic code with no generic parameters:
///
/// ```rust,ignore
/// fn save(enc: &dyn AnyEncoder, img: ImgRef<Rgba<u8>>) -> Result<Vec<u8>, CodecError> {
///     let output = enc.encode_srgba8_imgref(img, true)?;
///     Ok(output.into_data())
/// }
///
/// let jpeg = JpegEncoderConfig::new().with_generic_quality(85.0);
/// let webp = WebpEncoderConfig::lossy();
/// save(&jpeg, img.as_ref())?;
/// save(&webp, img.as_ref())?;
/// ```
pub trait AnyEncoder: Send + Sync {
    /// The image format this encoder produces.
    fn format(&self) -> ImageFormat;

    /// Pixel formats this encoder accepts natively.
    fn supported_descriptors(&self) -> &'static [PixelDescriptor];

    /// Encode type-erased pixels.
    fn encode_pixels(
        &self,
        pixels: PixelSlice<'_>,
        metadata: Option<&MetadataView<'_>>,
        limits: Option<&Limits>,
        stop: Option<&dyn Stop>,
    ) -> Result<EncodeOutput, CodecError>;

    /// Encode sRGB RGBA8 pixels from an `ImgRef`.
    ///
    /// `ignore_alpha = true` treats alpha as padding (codecs may use RGB paths).
    /// `ignore_alpha = false` preserves straight alpha.
    fn encode_srgba8_imgref(
        &self,
        img: imgref::ImgRef<'_, rgb::Rgba<u8>>,
        ignore_alpha: bool,
    ) -> Result<EncodeOutput, CodecError> {
        let typed: PixelSlice<'_, rgb::Rgba<u8>> = PixelSlice::from(img);
        let pixels: PixelSlice<'_> = if ignore_alpha {
            typed
                .with_descriptor(
                    PixelDescriptor::RGBA8_SRGB.with_alpha(Some(zenpixels::AlphaMode::Undefined)),
                )
                .erase()
        } else {
            typed.erase()
        };
        self.encode_pixels(pixels, None, None, None)
    }
}

impl<C> AnyEncoder for C
where
    C: zc::encode::EncoderConfig,
    for<'a> <C::Job<'a> as zc::encode::EncodeJob<'a>>::Enc: zc::encode::Encoder,
{
    fn format(&self) -> ImageFormat {
        C::format()
    }

    fn supported_descriptors(&self) -> &'static [PixelDescriptor] {
        C::supported_descriptors()
    }

    fn encode_pixels(
        &self,
        pixels: PixelSlice<'_>,
        metadata: Option<&MetadataView<'_>>,
        limits: Option<&Limits>,
        stop: Option<&dyn Stop>,
    ) -> Result<EncodeOutput, CodecError> {
        use zc::encode::{EncodeJob as _, Encoder as _};

        // Negotiate pixel format — convert input to something the encoder supports
        let pixel_data = pixels.contiguous_bytes();
        let adapted = zenpixels_convert::adapt::adapt_for_encode(
            &pixel_data,
            pixels.descriptor(),
            pixels.width(),
            pixels.rows(),
            pixels.width() as usize * pixels.descriptor().bytes_per_pixel(),
            C::supported_descriptors(),
        )
        .map_err(|e| CodecError::InvalidInput(alloc::format!("pixel format negotiation: {e}")))?;

        let adapted_stride = adapted.width as usize * adapted.descriptor.bytes_per_pixel();
        let adapted_pixels = PixelSlice::new(
            &adapted.data,
            adapted.width,
            adapted.rows,
            adapted_stride,
            adapted.descriptor,
        )
        .map_err(|e| CodecError::InvalidInput(alloc::format!("pixel slice: {e}")))?;

        let mut job = self.job();
        if let Some(m) = metadata {
            job = job.with_metadata(m);
        }
        if let Some(l) = limits {
            job = job.with_limits(crate::limits::to_resource_limits(l));
        }
        if let Some(s) = stop {
            job = job.with_stop(s);
        }
        let format = C::format();
        let enc = job
            .encoder()
            .map_err(|e| CodecError::from_codec(format, e))?;
        enc.encode(adapted_pixels)
            .map_err(|e| CodecError::from_codec(format, e))
    }
}

/// Build a type-erased encoder for the specified format.
///
/// Each codec arm delegates to its `build_trait_encoder` which builds
/// the codec-specific config, creates the encode job, and returns
/// a closure that calls `Encoder::encode(pixels)` via the trait.
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

        #[cfg(feature = "bitmaps")]
        ImageFormat::Pnm => Ok(crate::codecs::pnm::build_trait_encoder(params)),
        #[cfg(not(feature = "bitmaps"))]
        ImageFormat::Pnm => Err(CodecError::UnsupportedFormat(format)),

        #[cfg(feature = "bitmaps-bmp")]
        ImageFormat::Bmp => Ok(crate::codecs::bmp::build_trait_encoder(params)),
        #[cfg(not(feature = "bitmaps-bmp"))]
        ImageFormat::Bmp => Err(CodecError::UnsupportedFormat(format)),

        #[cfg(feature = "bitmaps")]
        ImageFormat::Farbfeld => Ok(crate::codecs::farbfeld::build_trait_encoder(params)),
        #[cfg(not(feature = "bitmaps"))]
        ImageFormat::Farbfeld => Err(CodecError::UnsupportedFormat(format)),

        _ => Err(CodecError::UnsupportedFormat(format)),
    }
}
