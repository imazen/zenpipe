//! WebP codec adapter using zenwebp.
//!
//! Probe, decode, and encode all use the trait interface.

use alloc::borrow::Cow;

use crate::config::CodecConfig;
use crate::error::Result;
use crate::limits::to_resource_limits;
use crate::{CodecError, DecodeOutput, ImageFormat, ImageInfo, Limits, StopToken};
use whereat::at;
use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};

/// Probe WebP metadata without decoding pixels.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo> {
    zenwebp::WebpDecoderConfig::new()
        .probe_header(data)
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::WebP, e)))
}

/// Decode WebP to pixels.
pub(crate) fn decode(
    data: &[u8],
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<StopToken>,
) -> Result<DecodeOutput> {
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
    job.decoder(Cow::Borrowed(data), &[])
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::WebP, e)))?
        .decode()
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::WebP, e)))
}

/// Build a WebpEncoderConfig from quality/lossless/codec_config.
///
/// Uses `EncoderConfig` trait methods for generic params, with
/// codec_config taking priority for format-specific overrides.
pub(crate) fn build_encoding(
    quality: Option<f32>,
    effort: Option<u32>,
    lossless: bool,
    codec_config: Option<&CodecConfig>,
) -> zenwebp::WebpEncoderConfig {
    use zencodec::encode::EncoderConfig;

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
