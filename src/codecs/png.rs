//! PNG codec adapter -- delegates to zenpng via trait interface.

use alloc::borrow::Cow;

use crate::config::CodecConfig;
use crate::error::Result;
use crate::limits::to_resource_limits;
use crate::{CodecError, DecodeOutput, ImageFormat, ImageInfo, Limits, StopToken};
use whereat::at;
use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
use zencodec::encode::EncoderConfig as _;

/// Probe PNG metadata without decoding pixels.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo> {
    zenpng::PngDecoderConfig::new()
        .probe_header(data)
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Png, e)))
}

/// Decode PNG to pixels.
pub(crate) fn decode(
    data: &[u8],
    limits: Option<&Limits>,
    stop: Option<StopToken>,
    decode_policy: Option<zencodec::decode::DecodePolicy>,
) -> Result<DecodeOutput> {
    let dec = zenpng::PngDecoderConfig::new();
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    if let Some(dp) = decode_policy {
        job = job.with_policy(dp);
    }
    job.decoder(Cow::Borrowed(data), &[])
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Png, e)))?
        .decode()
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Png, e)))
}

/// Build a PngEncoderConfig from quality/effort/codec_config.
///
/// Uses `EncoderConfig` trait methods for generic params, with
/// codec_config taking priority for format-specific overrides.
pub(crate) fn build_encoding(
    quality: Option<f32>,
    effort: Option<u32>,
    lossless: bool,
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
    if let Some(q) = quality {
        enc = enc.with_generic_quality(q);
    }
    // Restore lossless if explicitly requested — with_generic_quality may
    // have overridden it when quality < 100.
    if lossless {
        enc = enc.with_lossless(true);
    }
    enc
}

// ===================================================================
// Trait-based encoder dispatch
// ===================================================================

use crate::dispatch::{BuiltEncoder, EncodeParams, StreamingEncoder, build_from_config};

pub(crate) fn build_trait_encoder<'a>(params: EncodeParams<'a>) -> BuiltEncoder<'a> {
    build_from_config(
        |p| build_encoding(p.quality, p.effort, p.lossless, p.codec_config),
        params,
    )
}

pub(crate) fn build_streaming(params: EncodeParams<'_>) -> crate::error::Result<StreamingEncoder> {
    crate::dispatch::build_streaming_from_config(
        |p| build_encoding(p.quality, p.effort, p.lossless, p.codec_config),
        params,
    )
}
