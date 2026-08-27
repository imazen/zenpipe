//! Dynamic encoder dispatch.
//!
//! Provides [`build_encoder`] factory that creates a type-erased encoder closure
//! for any supported format. Each codec's `Encoder` trait impl handles pixel
//! format dispatch internally.

use crate::config::CodecConfig;
use crate::error::Result;
use crate::macros::dispatch_format;
use crate::{CodecError, ImageFormat, Limits, Metadata, StopToken};
use alloc::boxed::Box;
use whereat::at;
use zencodec::encode::EncodeOutput;
use zenpixels::{PixelDescriptor, PixelSlice};

/// Encoding parameters extracted from [`EncodeRequest`](crate::EncodeRequest).
pub(crate) struct EncodeParams<'a> {
    pub quality: Option<f32>,
    pub effort: Option<u32>,
    pub lossless: bool,
    pub metadata: Option<Metadata>,
    /// Retention policy applied to `metadata` at the codec boundary via
    /// [`zencodec::encode::EncodeJob::with_metadata_policy`].
    pub metadata_policy: zencodec::MetadataPolicy,
    pub codec_config: Option<&'a CodecConfig>,
    pub limits: Option<&'a Limits>,
    pub stop: Option<StopToken>,
    pub encode_policy: Option<zencodec::encode::EncodePolicy>,
    /// Explicit color signal to emit (e.g. a source `source_color` resolved from
    /// an ICC profile that the pixel descriptor's color-space enum can't carry).
    /// Currently honored by the PNG encoder (cICP chunk). `None` lets the encoder
    /// derive color from the pixel descriptor as before.
    pub cicp: Option<zencodec::Cicp>,
}

/// Fold `Metadata::orientation` into the EXIF blob for formats whose only
/// orientation carrier is EXIF (zenpipe#36, gap 1).
///
/// JPEG, PNG, and WebP have no native orientation field; their decoders
/// normalize the EXIF Orientation tag into `info.orientation`, so a caller
/// who sets `Metadata::with_orientation(..)` with no EXIF blob would see the
/// field silently dropped. When the field is non-identity: no blob → author
/// a minimal TIFF with just the Orientation tag; a blob without the tag →
/// insert it; a blob that already carries the tag → untouched (the codec
/// boundary's `MetadataPolicy` reconciles it to the field). Formats with a
/// native carrier (AVIF irot/imir, JXL codestream orientation — where EXIF
/// orientation is *not* authoritative) are left alone.
pub(crate) fn fold_orientation_into_exif(meta: Metadata, format: ImageFormat) -> Metadata {
    use zencodec::exif::{Exif, TextEncoding};
    if meta.orientation == zencodec::Orientation::Identity
        || !matches!(
            format,
            ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP
        )
    {
        return meta;
    }
    let authored = match meta.exif.as_deref() {
        None => {
            let mut e = Exif::new(TextEncoding::Ascii);
            e.set_orientation(meta.orientation);
            Some(e.to_bytes())
        }
        Some(blob) => match Exif::parse(blob) {
            // Tag present: the policy layer reconciles it to the field.
            Some(e) if e.orientation().is_some() => None,
            Some(mut e) => {
                e.set_orientation(meta.orientation);
                Some(e.to_bytes())
            }
            // Malformed blob: leave it to the codec's own handling.
            None => None,
        },
    };
    match authored {
        Some(bytes) => meta.with_exif(bytes),
        None => meta,
    }
}

/// Type-erased one-shot encode closure.
pub(crate) type EncodeFn<'a> = Box<dyn FnOnce(PixelSlice<'_>) -> Result<EncodeOutput> + 'a>;

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
    C: zencodec::encode::EncoderConfig + 'a,
    F: FnOnce(&EncodeParams<'a>) -> C + 'a,
    <C::Job as zencodec::encode::EncodeJob>::Enc: zencodec::encode::Encoder + Send,
{
    BuiltEncoder {
        encoder: Box::new(move |pixels| {
            use zencodec::encode::{EncodeJob as _, Encoder as _};
            let config = build_config(&params);
            let mut job = config.job();
            if let Some(s) = params.stop {
                job = job.with_stop(s);
            }
            if let Some(lim) = params.limits {
                job = job.with_limits(crate::limits::to_resource_limits(lim));
            }
            if let Some(meta) = params.metadata {
                job = job.with_metadata_policy(meta, params.metadata_policy);
            }
            if let Some(ep) = params.encode_policy {
                job = job.with_policy(ep);
            }
            let format = C::format();
            let enc = job
                .encoder()
                .map_err(|e| at!(CodecError::from_codec(format, e)))?;
            enc.encode(pixels)
                .map_err(|e| at!(CodecError::from_codec(format, e)))
        }),
        supported: C::supported_descriptors(),
    }
}

// ===========================================================================
// Object-safe encoder config -- zero-generics codec-agnostic encoding
// ===========================================================================

/// Object-safe encoder configuration.
///
/// Blanket-implemented for all [`EncoderConfig`](zencodec::encode::EncoderConfig)
/// types whose encoder implements [`Encoder`](zencodec::encode::Encoder).
/// Enables fully codec-agnostic code with no generic parameters:
///
/// ```rust,ignore
/// fn save(enc: &dyn AnyEncoder, img: ImgRef<Rgba<u8>>) -> Result<Vec<u8>, At<CodecError>> {
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
        metadata: Option<Metadata>,
        limits: Option<&Limits>,
        stop: Option<StopToken>,
    ) -> Result<EncodeOutput>;

    /// Encode sRGB RGBA8 pixels from an `ImgRef`.
    ///
    /// `ignore_alpha = true` treats alpha as padding (codecs may use RGB paths).
    /// `ignore_alpha = false` preserves straight alpha.
    fn encode_srgba8_imgref(
        &self,
        img: imgref::ImgRef<'_, rgb::Rgba<u8>>,
        ignore_alpha: bool,
    ) -> Result<EncodeOutput> {
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
    C: zencodec::encode::EncoderConfig,
    <C::Job as zencodec::encode::EncodeJob>::Enc: zencodec::encode::Encoder + Send,
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
        metadata: Option<Metadata>,
        limits: Option<&Limits>,
        stop: Option<StopToken>,
    ) -> Result<EncodeOutput> {
        use zencodec::encode::{EncodeJob as _, Encoder as _};

        // Negotiate pixel format -- convert input to something the encoder supports
        let pixel_data = pixels.contiguous_bytes();
        let adapted = zenpixels_convert::adapt::adapt_for_encode_cow(
            &pixel_data,
            pixels.descriptor(),
            pixels.width(),
            pixels.rows(),
            pixels.descriptor().aligned_stride(pixels.width()),
            C::supported_descriptors(),
        )
        .map_err(|e| {
            at!(CodecError::InvalidInput(alloc::format!(
                "pixel format negotiation: {e}"
            )))
        })?;
        let adapted_pixels = adapted.as_slice();

        let mut job = self.clone().job();
        if let Some(s) = stop {
            job = job.with_stop(s);
        }
        if let Some(m) = metadata {
            // Type-erased convenience path carries no policy; embed verbatim
            // (PreserveExact also reconciles a stale EXIF orientation tag).
            let m = fold_orientation_into_exif(m, C::format());
            job = job.with_metadata_policy(m, zencodec::MetadataPolicy::PreserveExact);
        }
        if let Some(l) = limits {
            job = job.with_limits(crate::limits::to_resource_limits(l));
        }
        let format = C::format();
        let enc = job
            .encoder()
            .map_err(|e| at!(CodecError::from_codec(format, e)))?;
        enc.encode(adapted_pixels)
            .map_err(|e| at!(CodecError::from_codec(format, e)))
    }
}

/// A streaming encoder: a `DynEncoder` + its supported pixel descriptors.
///
/// The caller pushes strips via [`DynEncoder::push_rows()`] and finalizes
/// with [`DynEncoder::finish()`]. Use [`adapt_for_encode_cow`] per-strip
/// to convert pixel formats without materializing the full image.
///
/// All codec encoders are `'static` (they clone/Arc their config), so this
/// type has no lifetime parameter.
///
/// [`adapt_for_encode_cow`]: zenpixels_convert::adapt::adapt_for_encode_cow
pub struct StreamingEncoder {
    /// The type-erased encoder. Call `push_rows()` per strip, `finish()` when done.
    ///
    /// `DynEncoder: Send` (zencodec 0.1.4+), so this can be moved across threads
    /// or used in `zenpipe::codec::EncoderSink`.
    pub encoder: Box<dyn zencodec::encode::DynEncoder + Send>,
    /// Pixel formats this encoder accepts natively (from codec's `supported_descriptors()`).
    /// Pass to `adapt_for_encode_cow` to pick the cheapest conversion.
    pub supported: &'static [PixelDescriptor],
    /// The resolved output format.
    pub format: ImageFormat,
}

/// Build a `DynEncoder` from a config-building closure.
///
/// Like [`build_from_config`] but returns the live encoder object
/// instead of a one-shot closure. The encoder supports both
/// `push_rows()` (streaming) and `encode()` (one-shot).
///
/// Works because `EncoderConfig::job(self)` consumes the config.
/// The encoder returned by `dyn_encoder()` is `'static` — all codec
/// encoders own their data (clone/Arc configs).
///
/// Cancellation is checked once before building the encoder.
/// For streaming encode the caller controls pacing and can check
/// the stop token between `push_rows()` calls.
pub(crate) fn build_streaming_from_config<C, F>(
    build_config: F,
    params: EncodeParams<'_>,
) -> Result<StreamingEncoder>
where
    C: zencodec::encode::EncoderConfig + 'static,
    F: FnOnce(&EncodeParams<'_>) -> C,
    <C::Job as zencodec::encode::EncodeJob>::Enc: zencodec::encode::Encoder + Send,
{
    use zencodec::encode::EncodeJob as _;
    let config = build_config(&params);
    let mut job = config.job();
    if let Some(s) = params.stop {
        job = job.with_stop(s);
    }
    if let Some(lim) = params.limits {
        job = job.with_limits(crate::limits::to_resource_limits(lim));
    }
    if let Some(meta) = params.metadata {
        job = job.with_metadata_policy(meta, params.metadata_policy);
    }
    if let Some(ep) = params.encode_policy {
        job = job.with_policy(ep);
    }
    let format = C::format();
    let encoder = job
        .dyn_encoder()
        .map_err(|e| at!(CodecError::Codec { format, source: e }))?;
    Ok(StreamingEncoder {
        encoder,
        supported: C::supported_descriptors(),
        format,
    })
}

/// Build a streaming encoder for the specified format.
pub(crate) fn build_streaming_encoder(
    format: ImageFormat,
    params: EncodeParams<'_>,
) -> Result<StreamingEncoder> {
    dispatch_format! {
        format, unsupported = Err(at!(CodecError::UnsupportedFormat(format)));
        Jpeg => "jpeg" => crate::codecs::jpeg::build_streaming(params),
        WebP => "webp" => crate::codecs::webp::build_streaming(params),
        Gif => "gif" => crate::codecs::gif::build_streaming(params),
        Png => "png" => crate::codecs::png::build_streaming(params),
        Avif => "avif-encode" => crate::codecs::avif_enc::build_streaming(params),
        Jxl => "jxl-encode" => crate::codecs::jxl_enc::build_streaming(params),
        Pnm => "bitmaps" => crate::codecs::pnm::build_streaming(params),
        Bmp => "bitmaps-bmp" => crate::codecs::bmp::build_streaming(params),
        Farbfeld => "bitmaps" => crate::codecs::farbfeld::build_streaming(params),
        Tiff => "tiff" => crate::codecs::tiff::build_streaming(params),
        Qoi => "bitmaps-qoi" => crate::codecs::qoi::build_streaming(params),
        Tga => "bitmaps-tga" => crate::codecs::tga::build_streaming(params),
        Hdr => "bitmaps-hdr" => crate::codecs::hdr::build_streaming(params);
        _ => Err(at!(CodecError::UnsupportedFormat(format))),
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
) -> Result<BuiltEncoder<'a>> {
    dispatch_format! {
        format, unsupported = Err(at!(CodecError::UnsupportedFormat(format)));
        Jpeg => "jpeg" => Ok(crate::codecs::jpeg::build_trait_encoder(params)),
        WebP => "webp" => Ok(crate::codecs::webp::build_trait_encoder(params)),
        Gif => "gif" => Ok(crate::codecs::gif::build_trait_encoder(params)),
        Png => "png" => Ok(crate::codecs::png::build_trait_encoder(params)),
        Avif => "avif-encode" => Ok(crate::codecs::avif_enc::build_trait_encoder(params)),
        Jxl => "jxl-encode" => Ok(crate::codecs::jxl_enc::build_trait_encoder(params)),
        Pnm => "bitmaps" => Ok(crate::codecs::pnm::build_trait_encoder(params)),
        Bmp => "bitmaps-bmp" => Ok(crate::codecs::bmp::build_trait_encoder(params)),
        Farbfeld => "bitmaps" => Ok(crate::codecs::farbfeld::build_trait_encoder(params)),
        Tiff => "tiff" => Ok(crate::codecs::tiff::build_trait_encoder(params)),
        Qoi => "bitmaps-qoi" => Ok(crate::codecs::qoi::build_trait_encoder(params)),
        Tga => "bitmaps-tga" => Ok(crate::codecs::tga::build_trait_encoder(params)),
        Hdr => "bitmaps-hdr" => Ok(crate::codecs::hdr::build_trait_encoder(params));
        _ => Err(at!(CodecError::UnsupportedFormat(format))),
    }
}

#[cfg(test)]
mod fold_tests {
    use super::*;
    use zencodec::Orientation;
    use zencodec::exif::Exif;

    fn tag_of(meta: &Metadata) -> Option<Orientation> {
        meta.exif
            .as_deref()
            .and_then(Exif::parse)
            .and_then(|e| e.orientation())
    }

    #[test]
    fn authors_a_blob_when_none_exists() {
        let m = Metadata::none().with_orientation(Orientation::Rotate90);
        let out = fold_orientation_into_exif(m, ImageFormat::Jpeg);
        assert_eq!(tag_of(&out), Some(Orientation::Rotate90));
        assert_eq!(out.orientation, Orientation::Rotate90);
    }

    #[test]
    fn inserts_the_tag_into_a_blob_that_lacks_it_and_keeps_other_fields() {
        let mut e = Exif::new(zencodec::exif::TextEncoding::Ascii);
        e.set_copyright("(c) test");
        let m = Metadata::none()
            .with_exif(e.to_bytes())
            .with_orientation(Orientation::Rotate270);
        let out = fold_orientation_into_exif(m, ImageFormat::Png);
        assert_eq!(tag_of(&out), Some(Orientation::Rotate270));
        let parsed = Exif::parse(out.exif.as_deref().unwrap()).unwrap();
        assert_eq!(parsed.copyright().as_deref(), Some("(c) test"));
    }

    #[test]
    fn leaves_identity_native_carriers_and_existing_tags_alone() {
        // Identity: nothing to emit.
        let m = Metadata::none();
        assert!(
            fold_orientation_into_exif(m, ImageFormat::WebP)
                .exif
                .is_none()
        );
        // AVIF carries orientation natively (irot/imir) — no EXIF authored.
        let m = Metadata::none().with_orientation(Orientation::Rotate90);
        assert!(
            fold_orientation_into_exif(m, ImageFormat::Avif)
                .exif
                .is_none()
        );
        // Existing tag: blob byte-identical (reconciliation is the policy's job).
        let mut e = Exif::new(zencodec::exif::TextEncoding::Ascii);
        e.set_orientation(Orientation::Rotate180);
        let blob = e.to_bytes();
        let m = Metadata::none()
            .with_exif(blob.clone())
            .with_orientation(Orientation::Rotate90);
        let out = fold_orientation_into_exif(m, ImageFormat::Jpeg);
        assert_eq!(out.exif.as_deref(), Some(blob.as_slice()));
    }
}
