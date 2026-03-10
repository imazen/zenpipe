//! PNG codec adapter -- delegates to zenpng via trait interface.

use alloc::borrow::Cow;

use crate::config::CodecConfig;
use crate::error::Result;
use crate::limits::to_resource_limits;
use crate::{CodecError, DecodeOutput, ImageFormat, ImageInfo, Limits, Stop};
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
    stop: Option<&dyn Stop>,
) -> Result<DecodeOutput> {
    let dec = zenpng::PngDecoderConfig::new();
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
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
fn build_encoding(
    quality: Option<f32>,
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
    if let Some(q) = quality {
        enc = enc.with_generic_quality(q);
    }
    enc
}

// ===================================================================
// Trait-based encoder dispatch
// ===================================================================

use crate::dispatch::{BuiltEncoder, EncodeParams, build_from_config};

pub(crate) fn build_trait_encoder<'a>(params: EncodeParams<'a>) -> BuiltEncoder<'a> {
    build_from_config(
        |p| build_encoding(p.quality, p.effort, p.codec_config),
        params,
    )
}
