//! PNG codec adapter — delegates to zenpng via trait interface.

use crate::config::CodecConfig;
use crate::limits::to_resource_limits;
use crate::{CodecError, DecodeOutput, ImageFormat, ImageInfo, Limits, Stop};
use alloc::boxed::Box;
use zencodec_types::{
    Decode as _, DecodeJob as _, DecoderConfig as _, EncodeJob as _, Encoder as _,
    EncoderConfig as _,
};

/// Probe PNG metadata without decoding pixels.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo, CodecError> {
    zenpng::PngDecoderConfig::new()
        .probe_header(data)
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))
}

/// Decode PNG to pixels.
pub(crate) fn decode(
    data: &[u8],
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<DecodeOutput, CodecError> {
    let dec = zenpng::PngDecoderConfig::new();
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.decoder(data, &[])
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))?
        .decode()
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))
}

/// Build a PngEncoderConfig from effort/codec_config.
///
/// Uses `EncoderConfig` trait methods for generic params, with
/// codec_config taking priority for format-specific overrides.
fn build_encoding(
    effort: Option<u32>,
    codec_config: Option<&CodecConfig>,
) -> zenpng::PngEncoderConfig {
    let mut enc = zenpng::PngEncoderConfig::new();
    if let Some(cfg) = codec_config {
        if let Some(compression) = cfg.png_compression {
            enc = enc.with_compression(compression);
        }
        if let Some(filter) = cfg.png_filter {
            enc = enc.with_filter(filter);
        }
    } else if let Some(effort) = effort {
        enc = enc.with_generic_effort(effort as i32);
    }
    enc
}

// ═══════════════════════════════════════════════════════════════════════
// Trait-based encoder dispatch
// ═══════════════════════════════════════════════════════════════════════

use crate::dispatch::{BuiltEncoder, EncodeParams};
use zenpixels::PixelDescriptor;

static PNG_SUPPORTED: &[PixelDescriptor] = &[
    PixelDescriptor::RGB8_SRGB,
    PixelDescriptor::RGBA8_SRGB,
    PixelDescriptor::GRAY8_SRGB,
    PixelDescriptor::RGBF32_LINEAR,
    PixelDescriptor::RGBAF32_LINEAR,
    PixelDescriptor::GRAYF32_LINEAR,
];

pub(crate) fn build_trait_encoder<'a>(params: EncodeParams<'a>) -> BuiltEncoder<'a> {
    BuiltEncoder {
        encoder: Box::new(move |pixels| {
            let enc = build_encoding(params.effort, params.codec_config);
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
            job.encoder()
                .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))?
                .encode(pixels)
                .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))
        }),
        supported: PNG_SUPPORTED,
    }
}
