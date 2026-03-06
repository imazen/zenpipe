//! WebP codec adapter using zenwebp.
//!
//! Probe, decode, and encode all use the trait interface.

use crate::config::CodecConfig;
use crate::limits::to_resource_limits;
use crate::pixel::Rgba;
use crate::{CodecError, DecodeOutput, ImageFormat, ImageInfo, Limits, Stop};
use alloc::boxed::Box;
use zencodec_types::{
    Decode as _, DecodeJob as _, DecoderConfig as _, EncodeJob as _, EncoderConfig as _,
};
use zenpixels::PixelSliceMut;

/// Probe WebP metadata without decoding pixels.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo, CodecError> {
    zenwebp::WebpDecoderConfig::new()
        .probe_header(data)
        .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))
}

/// Decode WebP to pixels.
pub(crate) fn decode(
    data: &[u8],
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<DecodeOutput, CodecError> {
    let mut dec = zenwebp::WebpDecoderConfig::new();
    if let Some(cfg) = codec_config.and_then(|c| c.webp_decoder.as_ref()) {
        *dec.inner_mut() = cfg.as_ref().clone();
    }
    // Set limits on both config (native limits) and job (pre-flight checks)
    if let Some(lim) = limits {
        let rl = to_resource_limits(lim);
        dec = dec.with_limits(rl);
    }
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.decoder(data, &[])
        .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))?
        .decode()
        .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))
}

/// Decode WebP directly into a caller-provided RGBA8 buffer.
pub(crate) fn decode_into_rgba8(
    data: &[u8],
    dst: imgref::ImgRefMut<'_, Rgba<u8>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<ImageInfo, CodecError> {
    let mut dec = zenwebp::WebpDecoderConfig::new();
    if let Some(cfg) = codec_config.and_then(|c| c.webp_decoder.as_ref()) {
        *dec.inner_mut() = cfg.as_ref().clone();
    }
    // Set limits on both config (native limits) and job (pre-flight checks)
    if let Some(lim) = limits {
        let rl = to_resource_limits(lim);
        dec = dec.with_limits(rl);
    }
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.decoder(data, &[])
        .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))?
        .decode_into(data, PixelSliceMut::from(dst))
        .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))
}

/// Build a WebpEncoderConfig from quality/lossless/codec_config.
///
/// Uses `EncoderConfig` trait methods for generic params, with
/// codec_config taking priority for format-specific overrides.
fn build_encoding(
    quality: Option<f32>,
    effort: Option<u32>,
    lossless: bool,
    codec_config: Option<&CodecConfig>,
) -> zenwebp::WebpEncoderConfig {
    use zencodec_types::EncoderConfig;

    if lossless {
        if let Some(cfg) = codec_config.and_then(|c| c.webp_lossless.as_ref()) {
            let mut e = zenwebp::WebpEncoderConfig::lossless();
            *e.inner_mut() =
                zenwebp::encoder::config::EncoderConfig::Lossless(cfg.as_ref().clone());
            e
        } else {
            let mut e = zenwebp::WebpEncoderConfig::lossless();
            if let Some(effort) = effort {
                e = e.with_generic_effort(effort as i32);
            }
            e
        }
    } else if let Some(cfg) = codec_config.and_then(|c| c.webp_lossy.as_ref()) {
        let mut e = zenwebp::WebpEncoderConfig::lossy();
        *e.inner_mut() = zenwebp::encoder::config::EncoderConfig::Lossy(cfg.as_ref().clone());
        e
    } else {
        let mut e = zenwebp::WebpEncoderConfig::lossy();
        if let Some(q) = quality {
            e = e.with_generic_quality(q);
        }
        if let Some(effort) = effort {
            e = e.with_generic_effort(effort as i32);
        }
        e
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Trait-based encoder dispatch
// ═══════════════════════════════════════════════════════════════════════

use crate::dispatch::{BuiltEncoder, EncodeParams};
use zenpixels::PixelDescriptor;

static WEBP_SUPPORTED: &[PixelDescriptor] = &[
    PixelDescriptor::RGB8_SRGB,
    PixelDescriptor::RGBA8_SRGB,
    PixelDescriptor::GRAY8_SRGB,
    PixelDescriptor::BGRA8_SRGB,
    PixelDescriptor::RGBF32_LINEAR,
    PixelDescriptor::RGBAF32_LINEAR,
    PixelDescriptor::GRAYF32_LINEAR,
];

pub(crate) fn build_trait_encoder<'a>(params: EncodeParams<'a>) -> BuiltEncoder<'a> {
    BuiltEncoder {
        encoder: Box::new(move |pixels| {
            let enc = build_encoding(
                params.quality,
                params.effort,
                params.lossless,
                params.codec_config,
            );
            let mut job = enc.job();
            if let Some(lim) = params.limits {
                job = job.with_limits(to_resource_limits(lim));
            }
            if let Some(meta) = params.metadata {
                job = job.with_metadata(meta);
            }
            if let Some(s) = params.stop {
                job = job.with_stop(s);
            }
            use zencodec_types::Encoder as _;
            job.encoder()
                .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))?
                .encode(pixels)
                .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))
        }),
        supported: WEBP_SUPPORTED,
    }
}
