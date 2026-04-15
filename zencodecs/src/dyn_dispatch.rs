//! Dynamic codec dispatch for streaming, animation, and push-based decode/encode.
//!
//! # Lifetime design
//!
//! `DynAnimationFrameDecoder` is `'static` — no lifetime parameter — so it works
//! through the dyn dispatch layer with `Cow::Owned` data.
//!
//! `DynStreamingDecoder + 'a` carries a lifetime tied to the decode job, which
//! borrows the config. For pull-based streaming, we use a wrapper around
//! `DynAnimationFrameDecoder`. For true zero-copy streaming, callers should use
//! `push_decode()` which runs to completion with borrowed data.

use alloc::borrow::Cow;
use alloc::boxed::Box;

use crate::codec_id::CodecId;
use crate::config::CodecConfig;
use crate::error::Result;
use crate::limits::to_resource_limits;
use crate::trace::{SelectionStep, SelectionTrace};
use crate::{CodecError, ImageFormat, Limits, StopToken};
use whereat::{ResultAtExt, at, at_crate};
use zencodec::decode::{DecodeJob as _, DecoderConfig as _, DynDecoderConfig, OutputInfo};

/// Wrap a BoxedError from a codec into a CodecError.
fn wrap_boxed(format: ImageFormat, e: zencodec::decode::BoxedError) -> whereat::At<CodecError> {
    at!(CodecError::Codec { format, source: e })
}

fn wrap_enc_boxed(format: ImageFormat, e: zencodec::encode::BoxedError) -> whereat::At<CodecError> {
    at!(CodecError::Codec { format, source: e })
}

// ═══════════════════════════════════════════════════════════════════════════
// Decode parameters
// ═══════════════════════════════════════════════════════════════════════════

/// Parameters for creating a decoder.
pub(crate) struct DecodeParams<'a> {
    pub data: &'a [u8],
    pub codec_config: Option<&'a CodecConfig>,
    pub limits: Option<&'a Limits>,
    pub stop: Option<StopToken>,
    pub preferred: &'a [zenpixels::PixelDescriptor],
    pub decode_policy: Option<zencodec::decode::DecodePolicy>,
    /// When true, codecs that support gain maps will extract and attach
    /// gain map data to the `DecodeOutput` extras.
    pub extract_gain_map: bool,
}

// ═══════════════════════════════════════════════════════════════════════════
// Build a Box<dyn DynDecoderConfig> for a format
// ═══════════════════════════════════════════════════════════════════════════

fn build_dyn_decoder_config(
    format: ImageFormat,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
) -> Result<Box<dyn DynDecoderConfig>> {
    match format {
        #[cfg(feature = "jpeg")]
        ImageFormat::Jpeg => Ok(Box::new(build_jpeg_decoder(codec_config))),
        #[cfg(not(feature = "jpeg"))]
        ImageFormat::Jpeg => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "webp")]
        ImageFormat::WebP => Ok(Box::new(build_webp_decoder(codec_config, limits))),
        #[cfg(not(feature = "webp"))]
        ImageFormat::WebP => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "gif")]
        ImageFormat::Gif => Ok(Box::new(zengif::GifDecoderConfig::new())),
        #[cfg(not(feature = "gif"))]
        ImageFormat::Gif => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "png")]
        ImageFormat::Png => Ok(Box::new(zenpng::PngDecoderConfig::new())),
        #[cfg(not(feature = "png"))]
        ImageFormat::Png => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "avif-decode")]
        ImageFormat::Avif => Ok(Box::new(build_avif_decoder(codec_config))),
        #[cfg(not(feature = "avif-decode"))]
        ImageFormat::Avif => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "jxl-decode")]
        ImageFormat::Jxl => Ok(Box::new(zenjxl::JxlDecoderConfig::new())),
        #[cfg(not(feature = "jxl-decode"))]
        ImageFormat::Jxl => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "heic-decode")]
        ImageFormat::Heic => Ok(Box::new(heic::HeicDecoderConfig::new())),
        #[cfg(not(feature = "heic-decode"))]
        ImageFormat::Heic => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "bitmaps")]
        ImageFormat::Pnm => Ok(Box::new(zenbitmaps::PnmDecoderConfig::new())),
        #[cfg(not(feature = "bitmaps"))]
        ImageFormat::Pnm => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "bitmaps-bmp")]
        ImageFormat::Bmp => Ok(Box::new(zenbitmaps::BmpDecoderConfig::new())),
        #[cfg(not(feature = "bitmaps-bmp"))]
        ImageFormat::Bmp => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "bitmaps")]
        ImageFormat::Farbfeld => Ok(Box::new(zenbitmaps::FarbfeldDecoderConfig::new())),
        #[cfg(not(feature = "bitmaps"))]
        ImageFormat::Farbfeld => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "tiff")]
        ImageFormat::Tiff => Ok(Box::new(zentiff::codec::TiffDecoderCodecConfig::new())),
        #[cfg(not(feature = "tiff"))]
        ImageFormat::Tiff => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "bitmaps-qoi")]
        ImageFormat::Qoi => Ok(Box::new(zenbitmaps::QoiDecoderConfig::new())),
        #[cfg(not(feature = "bitmaps-qoi"))]
        ImageFormat::Qoi => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "bitmaps-tga")]
        ImageFormat::Tga => Ok(Box::new(zenbitmaps::TgaDecoderConfig::new())),
        #[cfg(not(feature = "bitmaps-tga"))]
        ImageFormat::Tga => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "bitmaps-hdr")]
        ImageFormat::Hdr => Ok(Box::new(zenbitmaps::HdrDecoderConfig::new())),
        #[cfg(not(feature = "bitmaps-hdr"))]
        ImageFormat::Hdr => Err(at!(CodecError::UnsupportedFormat(format))),

        // RAW/DNG: Custom format from zenraw
        #[cfg(feature = "raw-decode")]
        ImageFormat::Custom(def) if def.name == "dng" || def.name == "raw" => {
            Ok(Box::new(build_raw_decoder(codec_config)))
        }

        // JPEG 2000: Custom format from zenjp2
        #[cfg(feature = "jp2-decode")]
        ImageFormat::Jp2 => Ok(Box::new(zenjp2::Jp2DecoderConfig::new())),

        // SVG/SVGZ: Custom format from zensvg
        #[cfg(feature = "svg")]
        ImageFormat::Custom(def) if def.name == "svg" => {
            Ok(Box::new(zensvg::SvgDecoderConfig::new()))
        }

        _ => Err(at!(CodecError::UnsupportedFormat(format))),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Push decode — runs to completion, borrows data (concrete types)
// ═══════════════════════════════════════════════════════════════════════════

/// Push-based decode: decoder writes rows into the sink, runs to completion.
///
/// This is the most memory-efficient decode path — zero-copy from input data.
pub(crate) fn dyn_push_decode(
    format: ImageFormat,
    params: &DecodeParams<'_>,
    sink: &mut dyn zencodec::decode::DecodeRowSink,
) -> Result<OutputInfo> {
    // For codecs that return plain errors (zenjpeg, zenbitmaps).
    macro_rules! push_dec {
        ($config:expr) => {{
            let config = $config;
            let mut job = config.job();
            if let Some(lim) = params.limits {
                job = job.with_limits(to_resource_limits(lim));
            }
            if let Some(ref s) = params.stop {
                job = job.with_stop(s.clone());
            }
            if let Some(dp) = params.decode_policy {
                job = job.with_policy(dp);
            }
            if params.extract_gain_map {
                job = job.with_extract_gain_map(true);
            }
            job.push_decoder(Cow::Borrowed(params.data), sink, params.preferred)
                .map_err(|e| at!(CodecError::from_codec(format, e)))
        }};
    }
    // For codecs that return At<E> (instrumented crates: zengif, zenpng, zenwebp, etc.).
    macro_rules! push_dec_at {
        ($config:expr) => {{
            let config = $config;
            let mut job = config.job();
            if let Some(lim) = params.limits {
                job = job.with_limits(to_resource_limits(lim));
            }
            if let Some(ref s) = params.stop {
                job = job.with_stop(s.clone());
            }
            if let Some(dp) = params.decode_policy {
                job = job.with_policy(dp);
            }
            if params.extract_gain_map {
                job = job.with_extract_gain_map(true);
            }
            at_crate!(job.push_decoder(Cow::Borrowed(params.data), sink, params.preferred))
                .map_err_at(|e| CodecError::from_codec(format, e))
        }};
    }

    match format {
        #[cfg(feature = "jpeg")]
        ImageFormat::Jpeg => push_dec!(build_jpeg_decoder(params.codec_config)),
        #[cfg(not(feature = "jpeg"))]
        ImageFormat::Jpeg => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "webp")]
        ImageFormat::WebP => push_dec_at!(build_webp_decoder(params.codec_config, params.limits)),
        #[cfg(not(feature = "webp"))]
        ImageFormat::WebP => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "gif")]
        ImageFormat::Gif => push_dec_at!(zengif::GifDecoderConfig::new()),
        #[cfg(not(feature = "gif"))]
        ImageFormat::Gif => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "png")]
        ImageFormat::Png => push_dec_at!(zenpng::PngDecoderConfig::new()),
        #[cfg(not(feature = "png"))]
        ImageFormat::Png => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "avif-decode")]
        ImageFormat::Avif => push_dec_at!(build_avif_decoder(params.codec_config)),
        #[cfg(not(feature = "avif-decode"))]
        ImageFormat::Avif => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "jxl-decode")]
        ImageFormat::Jxl => push_dec_at!(zenjxl::JxlDecoderConfig::new()),
        #[cfg(not(feature = "jxl-decode"))]
        ImageFormat::Jxl => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "heic-decode")]
        ImageFormat::Heic => push_dec_at!(heic::HeicDecoderConfig::new()),
        #[cfg(not(feature = "heic-decode"))]
        ImageFormat::Heic => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "bitmaps")]
        ImageFormat::Pnm => push_dec!(zenbitmaps::PnmDecoderConfig::new()),
        #[cfg(not(feature = "bitmaps"))]
        ImageFormat::Pnm => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "bitmaps-bmp")]
        ImageFormat::Bmp => push_dec!(zenbitmaps::BmpDecoderConfig::new()),
        #[cfg(not(feature = "bitmaps-bmp"))]
        ImageFormat::Bmp => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "bitmaps")]
        ImageFormat::Farbfeld => push_dec!(zenbitmaps::FarbfeldDecoderConfig::new()),
        #[cfg(not(feature = "bitmaps"))]
        ImageFormat::Farbfeld => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "tiff")]
        ImageFormat::Tiff => push_dec_at!(zentiff::codec::TiffDecoderCodecConfig::new()),
        #[cfg(not(feature = "tiff"))]
        ImageFormat::Tiff => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "bitmaps-qoi")]
        ImageFormat::Qoi => push_dec!(zenbitmaps::QoiDecoderConfig::new()),
        #[cfg(not(feature = "bitmaps-qoi"))]
        ImageFormat::Qoi => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "bitmaps-tga")]
        ImageFormat::Tga => push_dec!(zenbitmaps::TgaDecoderConfig::new()),
        #[cfg(not(feature = "bitmaps-tga"))]
        ImageFormat::Tga => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "bitmaps-hdr")]
        ImageFormat::Hdr => push_dec!(zenbitmaps::HdrDecoderConfig::new()),
        #[cfg(not(feature = "bitmaps-hdr"))]
        ImageFormat::Hdr => Err(at!(CodecError::UnsupportedFormat(format))),

        // RAW/DNG: Custom format from zenraw
        #[cfg(feature = "raw-decode")]
        ImageFormat::Custom(def) if def.name == "dng" || def.name == "raw" => {
            push_dec!(build_raw_decoder(params.codec_config))
        }

        // JPEG 2000: Custom format from zenjp2
        #[cfg(feature = "jp2-decode")]
        ImageFormat::Jp2 => {
            push_dec_at!(zenjp2::Jp2DecoderConfig::new())
        }

        // SVG/SVGZ: Custom format from zensvg
        #[cfg(feature = "svg")]
        ImageFormat::Custom(def) if def.name == "svg" => {
            push_dec!(zensvg::SvgDecoderConfig::new())
        }

        _ => Err(at!(CodecError::UnsupportedFormat(format))),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Full-frame decode — 'static (DynAnimationFrameDecoder has no lifetime param)
// ═══════════════════════════════════════════════════════════════════════════

/// Full-frame decoder for animation. Data is copied to owned ('static).
///
/// `DynAnimationFrameDecoder` is `'static` — no lifetime parameter — so the
/// returned decoder outlives the config and request.
pub(crate) fn dyn_animation_frame_decoder(
    format: ImageFormat,
    params: &DecodeParams<'_>,
) -> Result<Box<dyn zencodec::decode::DynAnimationFrameDecoder>> {
    let config = build_dyn_decoder_config(format, params.codec_config, params.limits)?;
    let mut job = config.dyn_job();
    if let Some(lim) = params.limits {
        job.set_limits(to_resource_limits(lim));
    }
    if let Some(dp) = params.decode_policy {
        job.set_policy(dp);
    }
    if params.extract_gain_map {
        job.set_extract_gain_map(true);
    }
    let data = Cow::Owned(params.data.to_vec());
    job.into_animation_frame_decoder(data, params.preferred)
        .map_err(|e| wrap_boxed(format, e))
}

// ═══════════════════════════════════════════════════════════════════════════
// Streaming decode — 'static (data copied to owned)
// ═══════════════════════════════════════════════════════════════════════════

/// Wrap a concrete `StreamingDecode` into a boxed `DynStreamingDecoder`.
///
/// Local shim — zencodec's `StreamingDecoderShim` is `pub(super)`, so we
/// implement the trait directly for a newtype wrapper.
struct OwnedStreamingDecoderShim<S>(S);

impl<S: zencodec::decode::StreamingDecode + Send> zencodec::decode::DynStreamingDecoder
    for OwnedStreamingDecoderShim<S>
{
    fn next_batch(
        &mut self,
    ) -> core::result::Result<Option<(u32, zenpixels::PixelSlice<'_>)>, zencodec::decode::BoxedError>
    {
        self.0
            .next_batch()
            .map_err(|e| Box::new(e) as zencodec::decode::BoxedError)
    }

    fn info(&self) -> &zencodec::ImageInfo {
        self.0.info()
    }
}

/// Build a streaming decoder that yields scanline batches (pull model).
///
/// The input data is copied into owned storage so the returned decoder is
/// `'static` and can outlive the `DecodeRequest`.
///
/// Not all codecs support streaming decode with owned data. Codecs that
/// require borrowed data (JPEG, PNG) or don't support row-level decode at
/// all (WebP, TIFF, bitmaps) will return an error. Currently GIF, AVIF,
/// and HEIC support this path.
pub(crate) fn dyn_streaming_decoder(
    format: ImageFormat,
    params: &DecodeParams<'_>,
) -> Result<Box<dyn zencodec::decode::DynStreamingDecoder + 'static>> {
    use zencodec::decode::{DecodeJob as _, DecoderConfig as _};

    // For instrumented codecs that return At<E> from streaming_decoder.
    macro_rules! stream_dec {
        ($config:expr) => {{
            let config = $config;
            let mut job = config.job();
            if let Some(lim) = params.limits {
                job = job.with_limits(to_resource_limits(lim));
            }
            if let Some(ref s) = params.stop {
                job = job.with_stop(s.clone());
            }
            if let Some(dp) = params.decode_policy {
                job = job.with_policy(dp);
            }
            if params.extract_gain_map {
                job = job.with_extract_gain_map(true);
            }
            let dec = at_crate!(
                job.streaming_decoder(Cow::Owned(params.data.to_vec()), params.preferred)
            )
            .map_err_at(|e| CodecError::from_codec(format, e))?;
            Ok(Box::new(OwnedStreamingDecoderShim(dec)))
        }};
    }

    match format {
        // Codecs with 'static streaming decoders (owned data supported):
        #[cfg(feature = "gif")]
        ImageFormat::Gif => stream_dec!(zengif::GifDecoderConfig::new()),
        #[cfg(not(feature = "gif"))]
        ImageFormat::Gif => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "avif-decode")]
        ImageFormat::Avif => stream_dec!(build_avif_decoder(params.codec_config)),
        #[cfg(not(feature = "avif-decode"))]
        ImageFormat::Avif => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "heic-decode")]
        ImageFormat::Heic => stream_dec!(heic::HeicDecoderConfig::new()),
        #[cfg(not(feature = "heic-decode"))]
        ImageFormat::Heic => Err(at!(CodecError::UnsupportedFormat(format))),

        // JPEG/PNG: job(self) consumes the config, producing a 'static Job.
        // Combined with Cow::Owned data, the streaming decoder is 'static.
        // Note: zenjpeg returns plain errors (not At<E>), so at!() is used here.
        #[cfg(feature = "jpeg")]
        ImageFormat::Jpeg => {
            let mut job = build_jpeg_decoder(params.codec_config).job();
            if let Some(lim) = params.limits {
                job = job.with_limits(to_resource_limits(lim));
            }
            if let Some(ref s) = params.stop {
                job = job.with_stop(s.clone());
            }
            if let Some(dp) = params.decode_policy {
                job = job.with_policy(dp);
            }
            let dec = job
                .streaming_decoder(Cow::Owned(params.data.to_vec()), params.preferred)
                .map_err(|e| at!(CodecError::from_codec(format, e)))?;
            Ok(Box::new(OwnedStreamingDecoderShim(dec)))
        }
        #[cfg(not(feature = "jpeg"))]
        ImageFormat::Jpeg => Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "png")]
        ImageFormat::Png => {
            let mut job = zenpng::PngDecoderConfig::new().job();
            if let Some(lim) = params.limits {
                job = job.with_limits(to_resource_limits(lim));
            }
            if let Some(ref s) = params.stop {
                job = job.with_stop(s.clone());
            }
            if let Some(dp) = params.decode_policy {
                job = job.with_policy(dp);
            }
            let dec = at_crate!(
                job.streaming_decoder(Cow::Owned(params.data.to_vec()), params.preferred)
            )
            .map_err_at(|e| CodecError::from_codec(format, e))?;
            Ok(Box::new(OwnedStreamingDecoderShim(dec)))
        }
        #[cfg(not(feature = "png"))]
        ImageFormat::Png => Err(at!(CodecError::UnsupportedFormat(format))),

        // Codecs that don't support row-level streaming decode at all.
        _ => Err(at!(CodecError::UnsupportedOperation {
            format,
            detail: "streaming decode not supported for this format; use push_decode()",
        })),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Per-codec config builders
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "jpeg")]
fn build_jpeg_decoder(codec_config: Option<&CodecConfig>) -> zenjpeg::JpegDecoderConfig {
    let mut config = zenjpeg::JpegDecoderConfig::new();
    if let Some(cfg) = codec_config.and_then(|c| c.jpeg_decoder.as_ref()) {
        *config.inner_mut() = cfg.as_ref().clone();
    }
    config
}

#[cfg(feature = "webp")]
fn build_webp_decoder(
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
) -> zenwebp::zencodec::WebpDecoderConfig {
    let mut config = zenwebp::zencodec::WebpDecoderConfig::new();
    if let Some(cfg) = codec_config.and_then(|c| c.webp_decoder.as_ref()) {
        *config.inner_mut() = cfg.as_ref().clone();
    }
    if let Some(lim) = limits {
        config = config.with_limits(to_resource_limits(lim));
    }
    config
}

#[cfg(feature = "avif-decode")]
fn build_avif_decoder(codec_config: Option<&CodecConfig>) -> zenavif::AvifDecoderConfig {
    let mut config = zenavif::AvifDecoderConfig::new();
    if let Some(cfg) = codec_config.and_then(|c| c.avif_decoder.as_ref()) {
        *config.inner_mut() = cfg.as_ref().clone();
    }
    config
}

#[cfg(feature = "raw-decode")]
fn build_raw_decoder(codec_config: Option<&CodecConfig>) -> zenraw::RawDecoderConfig {
    let mut config = zenraw::RawDecoderConfig::new();
    if let Some(cfg) = codec_config.and_then(|c| c.raw_decoder.as_ref()) {
        config = zenraw::RawDecoderConfig::from_config(cfg.as_ref().clone());
    }
    config
}

// ═══════════════════════════════════════════════════════════════════════════
// Encode dispatch for animation
// ═══════════════════════════════════════════════════════════════════════════

use zencodec::encode::{EncodeJob as _, EncoderConfig as _};

/// Parameters for creating an animation encoder.
pub(crate) struct AnimEncodeParams<'a> {
    pub quality: Option<f32>,
    pub effort: Option<u32>,
    pub lossless: bool,
    pub metadata: Option<crate::Metadata>,
    pub codec_config: Option<&'a CodecConfig>,
    pub limits: Option<&'a Limits>,
    pub stop: Option<StopToken>,
    pub encode_policy: Option<zencodec::encode::EncodePolicy>,
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub loop_count: Option<u32>,
}

/// Create a full-frame animation encoder for the specified format.
pub(crate) fn dyn_animation_frame_encoder(
    format: ImageFormat,
    params: AnimEncodeParams<'_>,
) -> Result<Box<dyn zencodec::encode::DynAnimationFrameEncoder>> {
    macro_rules! build_ffe {
        ($config:expr) => {{
            let config = $config;
            let mut job = config.job();
            if let Some(s) = params.stop {
                job = job.with_stop(s);
            }
            if let Some(lim) = params.limits {
                job = job.with_limits(to_resource_limits(lim));
            }
            if let Some(meta) = params.metadata {
                job = job.with_metadata(meta);
            }
            if let Some(ep) = params.encode_policy {
                job = job.with_policy(ep);
            }
            job = job.with_canvas_size(params.canvas_width, params.canvas_height);
            if let Some(lc) = params.loop_count {
                job = job.with_loop_count(Some(lc));
            }
            job.dyn_animation_frame_encoder()
                .map_err(|e| wrap_enc_boxed(format, e))
        }};
    }

    match format {
        #[cfg(feature = "gif")]
        ImageFormat::Gif => {
            let mut config = zengif::GifEncoderConfig::new();
            if let Some(cfg) = params.codec_config.and_then(|c| c.gif_encoder.as_ref()) {
                *config.inner_mut() = cfg.as_ref().clone();
            }
            build_ffe!(config)
        }

        #[cfg(feature = "webp")]
        ImageFormat::WebP => {
            build_ffe!(crate::codecs::webp::build_encoding(
                params.quality,
                params.effort,
                params.lossless,
                params.codec_config,
            ))
        }

        #[cfg(feature = "png")]
        ImageFormat::Png => {
            build_ffe!(crate::codecs::png::build_encoding(
                params.quality,
                params.effort,
                params.lossless,
                params.codec_config,
            ))
        }

        _ => Err(at!(CodecError::UnsupportedOperation {
            format,
            detail: "animation encoding not supported for this format",
        })),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Codec ID resolution (for tracing)
// ═══════════════════════════════════════════════════════════════════════════

/// Get the decoder CodecId for a format.
#[allow(dead_code)]
pub(crate) fn decoder_id_for_format(format: ImageFormat) -> CodecId {
    match format {
        ImageFormat::Jpeg => CodecId::ZenjpegDecode,
        ImageFormat::WebP => CodecId::ZenwebpDecode,
        ImageFormat::Gif => CodecId::ZengifDecode,
        ImageFormat::Png => CodecId::PngDecode,
        ImageFormat::Avif => CodecId::ZenavifDecode,
        ImageFormat::Jxl => CodecId::ZenjxlDecode,
        ImageFormat::Heic => CodecId::HeicDecode,
        ImageFormat::Pnm => CodecId::PnmDecode,
        ImageFormat::Bmp => CodecId::BmpDecode,
        ImageFormat::Farbfeld => CodecId::FarbfeldDecode,
        ImageFormat::Tiff => CodecId::TiffDecode,
        #[cfg(feature = "raw-decode")]
        ImageFormat::Custom(def) if def.name == "dng" || def.name == "raw" => CodecId::ZenrawDecode,
        #[cfg(feature = "svg")]
        ImageFormat::Custom(def) if def.name == "svg" => CodecId::ZensvgDecode,
        _ => CodecId::Custom("unknown"),
    }
}

/// Build a selection trace for a decode operation.
#[allow(dead_code)]
pub(crate) fn trace_decode_selection(format: ImageFormat) -> SelectionTrace {
    let mut trace = SelectionTrace::new();
    let id = decoder_id_for_format(format);
    trace.push(SelectionStep::DecoderChosen {
        id,
        priority: 100,
        reason: "single implementation",
    });
    trace
}
