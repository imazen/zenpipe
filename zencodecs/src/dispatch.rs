//! Dynamic encoder dispatch.
//!
//! Provides [`build_encoder`] factory that creates a type-erased encoder closure
//! for any supported format. Each codec's `Encoder` trait impl handles pixel
//! format dispatch internally.

use crate::config::CodecConfig;
use crate::error::Result;
use crate::macros::dispatch_format;
use crate::{CodecError, ImageFormat, Limits, Metadata, SharedPulse};
use alloc::boxed::Box;
use whereat::at;
use zencodec::encode::EncodeOutput;
use zenpixels::{PixelDescriptor, PixelSlice};

/// Encoding parameters extracted from [`EncodeRequest`](crate::EncodeRequest).
// Every field is read by some encoder; with only decoders compiled the request
// still carries them, unread.
#[cfg_attr(
    not(any(
        feature = "jpeg",
        feature = "webp",
        feature = "gif",
        feature = "png",
        feature = "avif-encode",
        feature = "jxl-encode",
        feature = "bitmaps",
        feature = "bitmaps-bmp",
        feature = "bitmaps-qoi",
        feature = "bitmaps-tga",
        feature = "bitmaps-hdr",
        feature = "tiff"
    )),
    allow(dead_code)
)]
pub(crate) struct EncodeParams<'a> {
    // Read by the encoders that map quality and effort; GIF, the bitmap
    // formats and TIFF take neither.
    #[cfg_attr(
        not(any(
            feature = "jpeg",
            feature = "webp",
            feature = "png",
            feature = "avif-encode",
            feature = "jxl-encode"
        )),
        allow(dead_code)
    )]
    pub quality: Option<f32>,
    #[cfg_attr(
        not(any(
            feature = "jpeg",
            feature = "webp",
            feature = "png",
            feature = "avif-encode",
            feature = "jxl-encode"
        )),
        allow(dead_code)
    )]
    pub effort: Option<u32>,
    #[allow(dead_code)] // Not every compiled codec has a lossless configuration.
    pub lossless: bool,
    pub prefer_streaming: bool,
    pub metadata: Option<Metadata>,
    /// Retention policy applied to `metadata` at the codec boundary via
    /// [`zencodec::encode::EncodeJob::with_metadata_policy`].
    pub metadata_policy: zencodec::MetadataPolicy,
    // The bitmap formats and TIFF have no per-codec configuration.
    #[cfg_attr(
        not(any(
            feature = "jpeg",
            feature = "webp",
            feature = "png",
            feature = "avif-encode",
            feature = "jxl-encode",
            feature = "gif"
        )),
        allow(dead_code)
    )]
    pub codec_config: Option<&'a CodecConfig>,
    pub limits: Option<&'a Limits>,
    pub pulse: SharedPulse,
    pub encode_policy: Option<zencodec::encode::EncodePolicy>,
    pub orientation_mode: Option<zencodec::encode::EncodeOrientation>,
    #[cfg(feature = "tonemap")]
    pub tone_map_policy: crate::ToneMapPolicy,
    /// Explicit color signal to emit (e.g. a source `source_color` resolved from
    /// an ICC profile that the pixel descriptor's color-space enum can't carry).
    /// Currently honored by the PNG encoder (cICP chunk). `None` lets the encoder
    /// derive color from the pixel descriptor as before.
    #[allow(dead_code)] // Consumed by the optional PNG arm.
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
///
/// Only the precomputed gain-map path uses it: that path encodes the caller's
/// pixels and gain map unrotated. Ordinary encodes bake orientation into the
/// pixels instead (`crate::orient_fit`).
#[cfg(any(test, feature = "jpeg-ultrahdr", feature = "checked-transcode"))]
#[allow(deprecated)] // Preserve the existing serializer on this compatibility path.
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
            Some(zenmeta::exif::serialize(&e))
        }
        Some(blob) => match Exif::parse(blob) {
            // Tag present: the policy layer reconciles it to the field.
            Some(e) if e.orientation().is_some() => None,
            Some(mut e) => {
                e.set_orientation(meta.orientation);
                Some(zenmeta::exif::serialize(&e))
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
    /// Read only by the gain-map encodes; the closure negotiates its own.
    #[cfg_attr(not(feature = "jpeg-ultrahdr"), allow(dead_code))]
    pub supported: &'static [PixelDescriptor],
}

/// Build a type-erased encoder from a config-building closure.
///
/// The closure receives `EncodeParams` and returns the concrete `EncoderConfig`.
/// Config construction happens inside the returned closure so the config's
/// lifetime doesn't escape the function.
#[cfg(any(
    feature = "jpeg",
    feature = "webp",
    feature = "gif",
    feature = "png",
    feature = "avif-encode",
    feature = "jxl-encode",
    feature = "bitmaps",
    feature = "bitmaps-bmp",
    feature = "bitmaps-qoi",
    feature = "bitmaps-tga",
    feature = "bitmaps-hdr",
    feature = "tiff"
))]
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
            let format = C::format();
            // Re-encodes write upright pixels and orientation 1.
            let (orientation, upright) = crate::orient_fit::upright(params.metadata.as_ref());
            let strict = params
                .encode_policy
                .is_some_and(|p| p.resolve_strict(false));
            // Check the original colour description before pixel negotiation
            // can replace it with the converted samples' sRGB description.
            if strict {
                crate::color_fit::plan(
                    upright.as_ref(),
                    <C as zencodec::encode::EncoderConfig>::capabilities(),
                )
                .check_strict(true)
                .map_err(|detail| at!(CodecError::UnsupportedOperation { format, detail }))?;
            }
            let oriented = crate::orient_fit::orient(&pixels, orientation);
            let pixels = match &oriented {
                Some(b) => b.as_slice(),
                None => pixels,
            };
            crate::row_contract::check_dimensions(format, pixels.width(), pixels.rows())
                .map_err(|e| at!(CodecError::InvalidInput(alloc::format!("{e}"))))?;
            // HDR pixels into a target that carries neither HDR nor CICP are
            // tone-mapped to sRGB first (see `crate::tonemap`); the fitting
            // below then sees sRGB pixels.
            #[cfg(feature = "tonemap")]
            let toned = crate::tonemap::one_shot(
                &pixels,
                upright.as_ref(),
                <C as zencodec::encode::EncoderConfig>::capabilities(),
                params.tone_map_policy,
                strict,
            )
            .map_err(|e| {
                at!(CodecError::InvalidInput(alloc::format!(
                    "tone mapping HDR for {format:?}: {e}"
                )))
            })?;
            #[cfg(feature = "tonemap")]
            let (pixels, upright) = match &toned {
                Some((b, m)) => (b.as_slice(), m.clone()),
                None => (pixels, upright),
            };
            // A pixel-format change that changes the colour space converts
            // from the described colour and re-describes the result.
            let fitted = crate::color_fit::fit_to_supported(
                &pixels,
                upright.as_ref(),
                C::supported_descriptors(),
            )
            .map_err(|e| {
                at!(CodecError::InvalidInput(alloc::format!(
                    "converting to a pixel format {format:?} accepts: {e}"
                )))
            })?;
            let (pixels, upright) = match &fitted {
                Some((b, m)) => (b.as_slice(), Some(m.clone())),
                None => (pixels, upright),
            };
            // Fit the colour description to what this format carries,
            // converting the pixels when it can't carry theirs.
            let fit = crate::color_fit::plan(
                upright.as_ref(),
                <C as zencodec::encode::EncoderConfig>::capabilities(),
            )
            .check_strict(strict)
            .map_err(|detail| at!(CodecError::UnsupportedOperation { format, detail }))?;
            let (metadata, converted) = match fit {
                crate::color_fit::ColorFit::Keep => (upright.clone(), None),
                crate::color_fit::ColorFit::Metadata(m)
                | crate::color_fit::ColorFit::Dropped(m) => (Some(m), None),
                crate::color_fit::ColorFit::ConvertToSrgb { from, metadata } => {
                    let buf = crate::color_fit::convert_to_srgb(&pixels, &from).map_err(|e| {
                        at!(CodecError::InvalidInput(alloc::format!(
                            "converting to sRGB for {format:?}, which cannot carry the source colour: {e}"
                        )))
                    })?;
                    (Some(metadata), Some(buf))
                }
            };
            let pixels = match &converted {
                Some(b) => b.as_slice(),
                None => pixels,
            };
            // Negotiate the cheapest pixel format the encoder accepts;
            // zero-copy when it already accepts these pixels.
            let bytes = pixels.contiguous_bytes();
            let adapted = zenpixels_convert::adapt::adapt_for_encode_cow(
                &bytes,
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
            let pixels = adapted.as_slice();
            let mut config = build_config(&params);
            if params.prefer_streaming
                && !params
                    .codec_config
                    .is_some_and(|c| c.has_encode_override(C::format()))
            {
                config = config.with_prefer_streaming(true);
            }
            let mut job = config.job();
            job = job.with_pulse(params.pulse);
            if let Some(lim) = params.limits {
                job = job.with_limits(crate::limits::to_resource_limits(lim));
            }
            if let Some(meta) = metadata {
                job = job.with_metadata_policy(
                    crate::color_fit::exif_color_space(meta),
                    params.metadata_policy,
                );
            }
            let format = C::format();
            if let Some(mode) = params.orientation_mode {
                job.try_set_orientation_mode(mode)
                    .map_err(|e| at!(CodecError::from_codec(format, e)))?;
            }
            if let Some(ep) = params.encode_policy {
                // Refuse a policy the codec cannot enforce instead of ignoring it.
                job.try_set_policy(ep)
                    .map_err(|e| at!(CodecError::from_codec(format, e)))?;
            }
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
        pulse: SharedPulse,
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
        self.encode_pixels(pixels, None, None, SharedPulse::default())
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
        pulse: SharedPulse,
    ) -> Result<EncodeOutput> {
        use zencodec::encode::{EncodeJob as _, Encoder as _};

        // Re-encodes write upright pixels and orientation 1.
        let (orientation, metadata) = crate::orient_fit::upright(metadata.as_ref());
        let oriented = crate::orient_fit::orient(&pixels, orientation);
        let pixels = match &oriented {
            Some(b) => b.as_slice(),
            None => pixels,
        };
        // A pixel-format change that changes the colour space converts from
        // the described colour and re-describes the result.
        let fitted = crate::color_fit::fit_to_supported(
            &pixels,
            metadata.as_ref(),
            C::supported_descriptors(),
        )
        .map_err(|e| {
            at!(CodecError::InvalidInput(alloc::format!(
                "converting to a pixel format {:?} accepts: {e}",
                C::format()
            )))
        })?;
        let (pixels, metadata) = match &fitted {
            Some((b, m)) => (b.as_slice(), Some(m.clone())),
            None => (pixels, metadata),
        };

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
        job = job.with_pulse(pulse);
        if let Some(m) = metadata {
            // Type-erased convenience path carries no policy; embed verbatim.
            job = job.with_metadata_policy(
                crate::color_fit::exif_color_space(m),
                zencodec::MetadataPolicy::PreserveExact,
            );
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
    /// Pixel formats accepted by this encoder and its conversion adapters.
    /// Pass to `adapt_for_encode_cow` to pick the cheapest conversion. A tone-map
    /// adapter advertises HDR inputs so rendering happens before SDR negotiation.
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
#[cfg(any(
    feature = "jpeg",
    feature = "webp",
    feature = "gif",
    feature = "png",
    feature = "avif-encode",
    feature = "jxl-encode",
    feature = "bitmaps",
    feature = "bitmaps-bmp",
    feature = "bitmaps-qoi",
    feature = "bitmaps-tga",
    feature = "bitmaps-hdr",
    feature = "tiff"
))]
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
    let mut config = build_config(&params);
    if params.prefer_streaming
        && !params
            .codec_config
            .is_some_and(|c| c.has_encode_override(C::format()))
    {
        config = config.with_prefer_streaming(true);
    }
    let mut job = config.job();
    job = job.with_pulse(params.pulse);
    if let Some(lim) = params.limits {
        job = job.with_limits(crate::limits::to_resource_limits(lim));
    }
    // Re-encodes write upright pixels and orientation 1; rows are collected
    // and oriented at `finish` when a rotation or flip is needed.
    let (orientation, upright) = crate::orient_fit::upright(params.metadata.as_ref());
    let strict = params
        .encode_policy
        .is_some_and(|p| p.resolve_strict(false));
    if strict {
        crate::color_fit::plan(upright.as_ref(), C::capabilities())
            .check_strict(true)
            .map_err(|detail| {
                at!(CodecError::UnsupportedOperation {
                    format: C::format(),
                    detail
                })
            })?;
    }
    // HDR strips into a target that carries neither HDR nor CICP are
    // tone-mapped to sRGB (see `crate::tonemap`). A strip can't be measured
    // up front, so the content peak comes from the metadata or the default.
    #[cfg(feature = "tonemap")]
    let tone_map = crate::tonemap::plan(
        upright.as_ref(),
        <C as zencodec::encode::EncoderConfig>::capabilities(),
        params.tone_map_policy,
        strict,
    );
    #[cfg(feature = "tonemap")]
    let upright = match &tone_map {
        Some(plan) => plan.metadata.clone(),
        None => upright,
    };
    // Fit the colour description to what this format carries; the pixels
    // are converted strip by strip when it can't carry theirs.
    let fit = crate::color_fit::plan(
        upright.as_ref(),
        <C as zencodec::encode::EncoderConfig>::capabilities(),
    )
    .check_strict(strict)
    .map_err(|detail| {
        at!(CodecError::UnsupportedOperation {
            format: C::format(),
            detail
        })
    })?;
    let (metadata, convert_from) = match fit {
        crate::color_fit::ColorFit::Keep => (upright.clone(), None),
        crate::color_fit::ColorFit::Metadata(m) | crate::color_fit::ColorFit::Dropped(m) => {
            (Some(m), None)
        }
        crate::color_fit::ColorFit::ConvertToSrgb { from, metadata } => {
            (Some(metadata), Some(from))
        }
    };
    if let Some(meta) = metadata {
        job = job.with_metadata_policy(
            crate::color_fit::exif_color_space(meta),
            params.metadata_policy,
        );
    }
    let format = C::format();
    if let Some(mode) = params.orientation_mode {
        job.try_set_orientation_mode(mode)
            .map_err(|e| at!(CodecError::from_codec(format, e)))?;
    }
    if let Some(ep) = params.encode_policy {
        // Refuse a policy the codec cannot enforce instead of ignoring it.
        job.try_set_policy(ep)
            .map_err(|e| at!(CodecError::from_codec(format, e)))?;
    }
    let encoder = job
        .dyn_encoder()
        .map_err(|e| at!(CodecError::Codec { format, source: e }))?;
    let encoder: Box<dyn zencodec::encode::DynEncoder + Send> = match convert_from {
        Some(from) => Box::new(ColorConvertingEncoder {
            inner: encoder,
            from,
        }),
        None => encoder,
    };
    // A codec without row-level encode still streams its input: rows are
    // collected and its one-shot encode runs at `finish`, so a streaming
    // transcode gives the same bytes as the one-shot one.
    // An orientation to apply needs the whole image, so it buffers too.
    let encoder: Box<dyn zencodec::encode::DynEncoder + Send> =
        if orientation != zencodec::Orientation::Identity {
            Box::new(
                BufferedRowEncoder::new(encoder, params.limits.copied().unwrap_or_default())
                    .with_orientation(orientation),
            )
        } else if <C as zencodec::encode::EncoderConfig>::capabilities().push_rows() {
            encoder
        } else {
            Box::new(BufferedRowEncoder::new(
                encoder,
                params.limits.copied().unwrap_or_default(),
            ))
        };
    #[cfg(feature = "tonemap")]
    let supported = if tone_map.is_some() {
        crate::tonemap::STREAMING_INPUTS
    } else {
        C::supported_descriptors()
    };
    #[cfg(not(feature = "tonemap"))]
    let supported = C::supported_descriptors();
    #[cfg(feature = "tonemap")]
    let encoder: Box<dyn zencodec::encode::DynEncoder + Send> = match tone_map {
        Some(plan) => Box::new(ToneMappingEncoder {
            inner: encoder,
            plan,
            supported: C::supported_descriptors(),
        }),
        None => encoder,
    };
    let encoder = Box::new(crate::row_contract::RowEncoder::new(
        encoder,
        format,
        params.limits.copied().unwrap_or_else(Limits::none),
        orientation,
    ));
    Ok(StreamingEncoder {
        encoder,
        supported,
        format,
    })
}

/// A configured encoder job, before any encoder exists.
///
/// Retain it across planning and policy, then hand it to a planned row driver
/// (`zencodec_io::row_encode::encode_job` through an erased-job adapter). The
/// job carries the request's quality, effort, limits, pulse and metadata.
pub struct ConfiguredJob {
    /// The erased job; `plan_encode` and `into_encoder` are available on it.
    pub job: Box<dyn zencodec::encode::DynEncodeJob>,
    /// Pixel formats the encoder accepts natively.
    pub supported: &'static [PixelDescriptor],
    /// The resolved output format.
    pub format: ImageFormat,
}

/// The configuration [`build_streaming_from_config`] builds, kept as a job.
///
/// A required encode policy is applied with the checked setter: a codec that
/// cannot honor it refuses here. (The streaming path keeps its legacy
/// best-effort setter, which may ignore such a policy.)
#[cfg(any(feature = "jpeg", feature = "png"))]
pub(crate) fn build_job_from_config<C, F>(
    build_config: F,
    params: EncodeParams<'_>,
) -> Result<ConfiguredJob>
where
    C: zencodec::encode::EncoderConfig + 'static,
    F: FnOnce(&EncodeParams<'_>) -> C,
    <C::Job as zencodec::encode::EncodeJob>::Enc: zencodec::encode::Encoder + Send,
    <C::Job as zencodec::encode::EncodeJob>::AnimationFrameEnc:
        zencodec::encode::AnimationFrameEncoder,
{
    use zencodec::encode::DynEncoderConfig as _;
    let config = build_config(&params);
    let format = C::format();
    let mut job = config.dyn_job();
    job.set_pulse(params.pulse);
    if let Some(lim) = params.limits {
        job.set_limits(crate::limits::to_resource_limits(lim));
    }
    if let Some(meta) = params.metadata {
        job.set_metadata_policy(meta, params.metadata_policy);
    }
    if let Some(mode) = params.orientation_mode {
        job.try_set_orientation_mode(mode)
            .map_err(|source| at!(CodecError::Codec { format, source }))?;
    }
    if let Some(policy) = params.encode_policy {
        job.try_set_policy(policy)
            .map_err(|source| at!(CodecError::Codec { format, source }))?;
    }
    Ok(ConfiguredJob {
        job,
        supported: C::supported_descriptors(),
        format,
    })
}

/// Build a configured job for the specified format. Planned row encoding is
/// qualified for JPEG and PNG; other formats refuse instead of falling back.
pub(crate) fn build_job(format: ImageFormat, params: EncodeParams<'_>) -> Result<ConfiguredJob> {
    let _ = &params;
    match format {
        #[cfg(feature = "jpeg")]
        ImageFormat::Jpeg => crate::codecs::jpeg::build_job(params),
        #[cfg(feature = "png")]
        ImageFormat::Png => crate::codecs::png::build_job(params),
        _ => Err(at!(CodecError::UnsupportedOperation {
            format,
            detail: "planned row encoding is qualified for JPEG and PNG only",
        })),
    }
}

/// Converts each strip to sRGB before the codec sees it, for a target that
/// cannot carry the source colour (see `crate::color_fit`). The conversion is
/// per pixel, so strip by strip gives what a whole-image conversion gives.
#[cfg(any(
    feature = "jpeg",
    feature = "webp",
    feature = "gif",
    feature = "png",
    feature = "avif-encode",
    feature = "jxl-encode",
    feature = "bitmaps",
    feature = "tiff",
))]
struct ColorConvertingEncoder {
    inner: Box<dyn zencodec::encode::DynEncoder + Send>,
    from: crate::color_fit::Source,
}

#[cfg(any(
    feature = "jpeg",
    feature = "webp",
    feature = "gif",
    feature = "png",
    feature = "avif-encode",
    feature = "jxl-encode",
    feature = "bitmaps",
    feature = "tiff",
))]
impl ColorConvertingEncoder {
    fn convert(
        &self,
        pixels: PixelSlice<'_>,
    ) -> core::result::Result<zenpixels::PixelBuffer, zencodec::encode::BoxedError> {
        crate::color_fit::convert_to_srgb(&pixels, &self.from).map_err(|e| e.into())
    }
}

#[cfg(any(
    feature = "jpeg",
    feature = "webp",
    feature = "gif",
    feature = "png",
    feature = "avif-encode",
    feature = "jxl-encode",
    feature = "bitmaps",
    feature = "tiff",
))]
impl zencodec::encode::DynEncoder for ColorConvertingEncoder {
    fn preferred_strip_height(&self) -> u32 {
        self.inner.preferred_strip_height()
    }

    fn encode(
        self: Box<Self>,
        pixels: PixelSlice<'_>,
    ) -> core::result::Result<EncodeOutput, zencodec::encode::BoxedError> {
        let buf = self.convert(pixels)?;
        self.inner.encode(buf.as_slice())
    }

    fn encode_srgba8(
        self: Box<Self>,
        data: &mut [u8],
        _make_opaque: bool,
        width: u32,
        height: u32,
        stride_pixels: u32,
    ) -> core::result::Result<EncodeOutput, zencodec::encode::BoxedError> {
        let pixels = PixelSlice::new(
            data,
            width,
            height,
            stride_pixels as usize * 4,
            PixelDescriptor::RGBA8_SRGB,
        )
        .map_err(|e| -> zencodec::encode::BoxedError { alloc::format!("{e:?}").into() })?;
        let buf = self.convert(pixels)?;
        self.inner.encode(buf.as_slice())
    }

    fn begin(
        &mut self,
        width: u32,
        height: u32,
        descriptor: PixelDescriptor,
    ) -> core::result::Result<zencodec::encode::OutputTiming, zencodec::encode::BoxedError> {
        // Declare the descriptor the converted strips will carry: convert
        // one pixel through the same function.
        let one = alloc::vec![0u8; descriptor.bytes_per_pixel()];
        let px = PixelSlice::new(&one, 1, 1, one.len(), descriptor)
            .map_err(|e| -> zencodec::encode::BoxedError { alloc::format!("{e:?}").into() })?;
        let target = self.convert(px)?.descriptor();
        self.inner.begin(width, height, target)
    }

    fn pending_output(&self) -> &[u8] {
        self.inner.pending_output()
    }

    fn consume_output(&mut self, n: usize) {
        self.inner.consume_output(n)
    }

    fn take_output(&mut self, into: &mut alloc::vec::Vec<u8>) {
        self.inner.take_output(into)
    }

    fn push_rows(
        &mut self,
        rows: PixelSlice<'_>,
    ) -> core::result::Result<(), zencodec::encode::BoxedError> {
        let buf = self.convert(rows)?;
        self.inner.push_rows(buf.as_slice())
    }

    fn finish(self: Box<Self>) -> core::result::Result<EncodeOutput, zencodec::encode::BoxedError> {
        self.inner.finish()
    }

    fn encode_from(
        self: Box<Self>,
        _source: &mut dyn FnMut(u32, zenpixels::PixelSliceMut<'_>) -> usize,
    ) -> core::result::Result<EncodeOutput, zencodec::encode::BoxedError> {
        Err(
            "encode_from is not available when the pixels need colour conversion; use push_rows"
                .into(),
        )
    }
}

/// Tone-maps each HDR strip to sRGB before the codec sees it, for a target
/// that carries neither HDR nor CICP (see `crate::tonemap`). Tone mapping is
/// per pixel, so strip by strip gives what a whole-image tone map with the
/// same content peak gives. Strips that aren't HDR pass through unchanged.
#[cfg(feature = "tonemap")]
struct ToneMappingEncoder {
    inner: Box<dyn zencodec::encode::DynEncoder + Send>,
    plan: crate::tonemap::ToneMapPlan,
    supported: &'static [PixelDescriptor],
}

/// Render before adapting to the native codec's formats. Both operations
/// retain the original row stride until an actual conversion is needed.
#[cfg(feature = "tonemap")]
fn with_tone_mapped<R>(
    plan: &crate::tonemap::ToneMapPlan,
    pixels: PixelSlice<'_>,
    supported: &[PixelDescriptor],
    consume: impl FnOnce(PixelSlice<'_>) -> core::result::Result<R, zencodec::encode::BoxedError>,
) -> core::result::Result<R, zencodec::encode::BoxedError> {
    let toned = match plan.source_for(pixels.descriptor()) {
        Some(source) => {
            let source = source?;
            Some(crate::tonemap::tone_map_to_srgb8(
                &pixels,
                source,
                plan.peak(source),
            )?)
        }
        None => None,
    };
    let pixels = toned.as_ref().map_or(pixels, |b| b.as_slice());
    let bytes = pixels.contiguous_bytes();
    let adapted = zenpixels_convert::adapt::adapt_for_encode_cow(
        &bytes,
        pixels.descriptor(),
        pixels.width(),
        pixels.rows(),
        pixels.descriptor().aligned_stride(pixels.width()),
        supported,
    )
    .map_err(|e| -> zencodec::encode::BoxedError { alloc::format!("{e}").into() })?;
    consume(adapted.as_slice())
}

#[cfg(feature = "tonemap")]
impl zencodec::encode::DynEncoder for ToneMappingEncoder {
    fn preferred_strip_height(&self) -> u32 {
        self.inner.preferred_strip_height()
    }

    fn encode(
        self: Box<Self>,
        pixels: PixelSlice<'_>,
    ) -> core::result::Result<EncodeOutput, zencodec::encode::BoxedError> {
        let Self {
            inner,
            plan,
            supported,
        } = *self;
        with_tone_mapped(&plan, pixels, supported, |p| inner.encode(p))
    }

    fn encode_srgba8(
        self: Box<Self>,
        data: &mut [u8],
        make_opaque: bool,
        width: u32,
        height: u32,
        stride_pixels: u32,
    ) -> core::result::Result<EncodeOutput, zencodec::encode::BoxedError> {
        // Metadata remains authoritative; U8 HDR samples must be refused.
        if let Some(source) = self.plan.source_for(PixelDescriptor::RGBA8_SRGB) {
            source?;
            return Err("tone mapping reads f32 or U16 samples, not U8".into());
        }
        let stride = (stride_pixels as usize)
            .checked_mul(4)
            .ok_or("sRGBA8 stride overflows")?;
        // Validate before touching caller memory, including the valid case
        // where the last row omits trailing padding.
        PixelSlice::new(data, width, height, stride, PixelDescriptor::RGBA8_SRGB)
            .map_err(|e| -> zencodec::encode::BoxedError { alloc::format!("{e:?}").into() })?;
        if make_opaque {
            for y in 0..height as usize {
                for p in data[y * stride..y * stride + width as usize * 4]
                    .as_chunks_mut::<4>()
                    .0
                {
                    p[3] = 255;
                }
            }
        }
        // Native codecs need not accept AlphaMode::Undefined. Materializing
        // opaque alpha honors the request while keeping a supported descriptor.
        let pixels = PixelSlice::new(data, width, height, stride, PixelDescriptor::RGBA8_SRGB)
            .map_err(|e| -> zencodec::encode::BoxedError { alloc::format!("{e:?}").into() })?;
        let Self {
            inner,
            plan,
            supported,
        } = *self;
        with_tone_mapped(&plan, pixels, supported, |p| inner.encode(p))
    }

    fn begin(
        &mut self,
        width: u32,
        height: u32,
        descriptor: PixelDescriptor,
    ) -> core::result::Result<zencodec::encode::OutputTiming, zencodec::encode::BoxedError> {
        let one = alloc::vec![0u8; descriptor.bytes_per_pixel()];
        let px = PixelSlice::new(&one, 1, 1, one.len(), descriptor)
            .map_err(|e| -> zencodec::encode::BoxedError { alloc::format!("{e:?}").into() })?;
        with_tone_mapped(&self.plan, px, self.supported, |p| {
            self.inner.begin(width, height, p.descriptor())
        })
    }

    fn pending_output(&self) -> &[u8] {
        self.inner.pending_output()
    }
    fn consume_output(&mut self, n: usize) {
        self.inner.consume_output(n)
    }
    fn take_output(&mut self, into: &mut alloc::vec::Vec<u8>) {
        self.inner.take_output(into)
    }

    fn push_rows(
        &mut self,
        rows: PixelSlice<'_>,
    ) -> core::result::Result<(), zencodec::encode::BoxedError> {
        with_tone_mapped(&self.plan, rows, self.supported, |p| {
            self.inner.push_rows(p)
        })
    }

    fn finish(self: Box<Self>) -> core::result::Result<EncodeOutput, zencodec::encode::BoxedError> {
        self.inner.finish()
    }

    fn encode_from(
        self: Box<Self>,
        _source: &mut dyn FnMut(u32, zenpixels::PixelSliceMut<'_>) -> usize,
    ) -> core::result::Result<EncodeOutput, zencodec::encode::BoxedError> {
        Err("encode_from is not available when HDR pixels need tone mapping; use push_rows".into())
    }
}

/// Row-level encode over a codec that only encodes whole images.
///
/// `begin` and `push_rows` collect tightly packed rows (allocated
/// fallibly); `finish` hands them to the codec's one-shot `encode`. Output
/// arrives at `finish` ([`OutputTiming::AT_FINISH`]).
///
/// [`OutputTiming::AT_FINISH`]: zencodec::encode::OutputTiming::AT_FINISH
#[cfg(any(
    feature = "jpeg",
    feature = "webp",
    feature = "gif",
    feature = "png",
    feature = "avif-encode",
    feature = "jxl-encode",
    feature = "bitmaps",
    feature = "tiff",
))]
struct BufferedRowEncoder {
    inner: Box<dyn zencodec::encode::DynEncoder + Send>,
    limits: Limits,
    failed: bool,
    /// Applied to the collected image at `finish` (re-encodes are written
    /// upright; see `crate::orient_fit`).
    orientation: zencodec::Orientation,
    /// Width, declared height (if `begin` ran) and descriptor.
    image: Option<(u32, Option<u32>, PixelDescriptor)>,
    rows: alloc::vec::Vec<u8>,
    filled: u32,
}

#[cfg(any(
    feature = "jpeg",
    feature = "webp",
    feature = "gif",
    feature = "png",
    feature = "avif-encode",
    feature = "jxl-encode",
    feature = "bitmaps",
    feature = "tiff",
))]
impl BufferedRowEncoder {
    fn new(inner: Box<dyn zencodec::encode::DynEncoder + Send>, limits: Limits) -> Self {
        Self {
            inner,
            limits,
            failed: false,
            orientation: zencodec::Orientation::Identity,
            image: None,
            rows: alloc::vec::Vec::new(),
            filled: 0,
        }
    }

    fn with_orientation(mut self, orientation: zencodec::Orientation) -> Self {
        self.orientation = orientation;
        self
    }

    fn row_bytes(
        width: u32,
        descriptor: PixelDescriptor,
    ) -> core::result::Result<usize, zencodec::encode::BoxedError> {
        (width as usize)
            .checked_mul(descriptor.bytes_per_pixel())
            .ok_or_else(|| "row size overflows usize".into())
    }
}

#[cfg(any(
    feature = "jpeg",
    feature = "webp",
    feature = "gif",
    feature = "png",
    feature = "avif-encode",
    feature = "jxl-encode",
    feature = "bitmaps",
    feature = "tiff",
))]
impl zencodec::encode::DynEncoder for BufferedRowEncoder {
    fn preferred_strip_height(&self) -> u32 {
        self.inner.preferred_strip_height()
    }

    fn encode(
        self: Box<Self>,
        pixels: PixelSlice<'_>,
    ) -> core::result::Result<EncodeOutput, zencodec::encode::BoxedError> {
        if self.image.is_some() || self.failed {
            return Err("one-shot encode after row encoding started".into());
        }
        self.inner.encode(pixels)
    }

    fn encode_srgba8(
        self: Box<Self>,
        data: &mut [u8],
        make_opaque: bool,
        width: u32,
        height: u32,
        stride_pixels: u32,
    ) -> core::result::Result<EncodeOutput, zencodec::encode::BoxedError> {
        if self.image.is_some() || self.failed {
            return Err("one-shot encode after row encoding started".into());
        }
        self.inner
            .encode_srgba8(data, make_opaque, width, height, stride_pixels)
    }

    fn begin(
        &mut self,
        width: u32,
        height: u32,
        descriptor: PixelDescriptor,
    ) -> core::result::Result<zencodec::encode::OutputTiming, zencodec::encode::BoxedError> {
        if self.image.is_some() || self.failed {
            return Err("begin after an earlier declaration or rows".into());
        }
        let (out_width, out_height) = self.orientation.output_dimensions(width, height);
        self.limits.check_dimensions(out_width, out_height)?;
        let total = Self::row_bytes(width, descriptor)?
            .checked_mul(height as usize)
            .ok_or("image size overflows usize")?;
        self.limits.check_memory(total as u64)?;
        self.rows
            .try_reserve_exact(total)
            .map_err(|_| -> zencodec::encode::BoxedError {
                "row buffer allocation failed".into()
            })?;
        self.image = Some((width, Some(height), descriptor));
        Ok(zencodec::encode::OutputTiming::AT_FINISH)
    }

    fn push_rows(
        &mut self,
        rows: PixelSlice<'_>,
    ) -> core::result::Result<(), zencodec::encode::BoxedError> {
        if self.failed {
            return Err("row encoding already failed".into());
        }
        let result = (|| {
            let (width, height, descriptor) =
                *self
                    .image
                    .get_or_insert((rows.width(), None, rows.descriptor()));
            if rows.width() != width || rows.descriptor() != descriptor {
                return Err("rows differ in width or pixel format from the image".into());
            }
            let filled = self
                .filled
                .checked_add(rows.rows())
                .ok_or("row count overflows u32")?;
            if height.is_some_and(|h| filled > h) {
                return Err("more rows than the declared height".into());
            }
            let (out_width, out_height) = self.orientation.output_dimensions(width, filled);
            self.limits.check_dimensions(out_width, out_height)?;
            let row_bytes = Self::row_bytes(width, descriptor)?;
            let add = row_bytes
                .checked_mul(rows.rows() as usize)
                .ok_or("strip size overflows usize")?;
            let total = self
                .rows
                .len()
                .checked_add(add)
                .ok_or("image size overflows usize")?;
            self.limits.check_memory(total as u64)?;
            self.rows
                .try_reserve(add)
                .map_err(|_| -> zencodec::encode::BoxedError {
                    "row buffer allocation failed".into()
                })?;
            for y in 0..rows.rows() {
                self.rows.extend_from_slice(&rows.row(y)[..row_bytes]);
            }
            self.filled = filled;
            Ok(())
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn finish(self: Box<Self>) -> core::result::Result<EncodeOutput, zencodec::encode::BoxedError> {
        let this = *self;
        if this.failed {
            return Err("row encoding already failed".into());
        }
        let (width, height, descriptor) = this.image.ok_or("finish before any rows")?;
        if height.is_some_and(|h| h != this.filled) {
            return Err("fewer rows than the declared height".into());
        }
        let row_bytes = Self::row_bytes(width, descriptor)?;
        let pixels = PixelSlice::new(&this.rows, width, this.filled, row_bytes, descriptor)
            .map_err(|e| -> zencodec::encode::BoxedError { alloc::format!("{e:?}").into() })?;
        let output = match crate::orient_fit::orient(&pixels, this.orientation) {
            Some(oriented) => this.inner.encode(oriented.as_slice()),
            None => this.inner.encode(pixels),
        }?;
        this.limits.check_output_size(output.len() as u64)?;
        Ok(output)
    }

    fn encode_from(
        self: Box<Self>,
        source: &mut dyn FnMut(u32, zenpixels::PixelSliceMut<'_>) -> usize,
    ) -> core::result::Result<EncodeOutput, zencodec::encode::BoxedError> {
        if self.image.is_some() || self.failed {
            return Err("pull encode after row encoding started".into());
        }
        self.inner.encode_from(source)
    }
}

/// Build a streaming encoder for the specified format.
pub(crate) fn build_streaming_encoder(
    format: ImageFormat,
    params: EncodeParams<'_>,
) -> Result<StreamingEncoder> {
    // Only the encoder arms below consume `params`.
    #[cfg(not(any(
        feature = "jpeg",
        feature = "webp",
        feature = "gif",
        feature = "png",
        feature = "avif-encode",
        feature = "jxl-encode",
        feature = "bitmaps",
        feature = "bitmaps-bmp",
        feature = "bitmaps-qoi",
        feature = "bitmaps-tga",
        feature = "bitmaps-hdr",
        feature = "tiff"
    )))]
    let _ = params;
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
    // Only the encoder arms below consume `params`.
    #[cfg(not(any(
        feature = "jpeg",
        feature = "webp",
        feature = "gif",
        feature = "png",
        feature = "avif-encode",
        feature = "jxl-encode",
        feature = "bitmaps",
        feature = "bitmaps-bmp",
        feature = "bitmaps-qoi",
        feature = "bitmaps-tga",
        feature = "bitmaps-hdr",
        feature = "tiff"
    )))]
    let _ = params;
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
#[allow(deprecated)] // Exercise the existing compatibility serializer.
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
