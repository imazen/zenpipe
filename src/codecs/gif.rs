//! GIF codec adapter using zengif via trait interface.

use alloc::borrow::Cow;

use crate::config::CodecConfig;
use crate::error::Result;
use crate::limits::to_resource_limits;
use crate::{CodecError, DecodeOutput, ImageFormat, ImageInfo, Limits, StopToken};
use whereat::{ResultAtExt, at_crate};
use zencodec::decode::{Decode, DecodeJob as _, DecoderConfig as _};

/// Probe GIF metadata without decoding pixels.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo> {
    at_crate!(zengif::GifDecoderConfig::new()
        .probe_header(data))
        .map_err_at(|e| CodecError::from_codec(ImageFormat::Gif, e))
}

/// Decode GIF to pixels (first frame only).
pub(crate) fn decode(
    data: &[u8],
    limits: Option<&Limits>,
    stop: Option<StopToken>,
    decode_policy: Option<zencodec::decode::DecodePolicy>,
) -> Result<DecodeOutput> {
    let dec = zengif::GifDecoderConfig::new();
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
    let decoder = at_crate!(job.decoder(Cow::Borrowed(data), &[]))
        .map_err_at(|e| CodecError::from_codec(ImageFormat::Gif, e))?;
    at_crate!(decoder.decode())
        .map_err_at(|e| CodecError::from_codec(ImageFormat::Gif, e))
}

/// Build a GifEncoderConfig from codec config.
fn build_gif_encoding(codec_config: Option<&CodecConfig>) -> zengif::GifEncoderConfig {
    let mut enc = zengif::GifEncoderConfig::new();
    if let Some(cfg) = codec_config.and_then(|c| c.gif_encoder.as_ref()) {
        *enc.inner_mut() = cfg.as_ref().clone();
    }
    enc
}

// ===================================================================
// Trait-based encoder dispatch
// ===================================================================

use crate::dispatch::{BuiltEncoder, EncodeParams, StreamingEncoder, build_from_config};

pub(crate) fn build_trait_encoder<'a>(params: EncodeParams<'a>) -> BuiltEncoder<'a> {
    build_from_config(|p| build_gif_encoding(p.codec_config), params)
}

pub(crate) fn build_streaming(params: EncodeParams<'_>) -> crate::error::Result<StreamingEncoder> {
    crate::dispatch::build_streaming_from_config(|p| build_gif_encoding(p.codec_config), params)
}
