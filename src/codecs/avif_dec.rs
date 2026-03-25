//! AVIF decode adapter using zenavif via trait interface.

use alloc::borrow::Cow;

use crate::config::CodecConfig;
use crate::error::Result;
use crate::limits::to_resource_limits;
use crate::{
    CodecError, DecodeJob, DecodeOutput, DecoderConfig, ImageFormat, ImageInfo, Limits, StopToken,
};
use whereat::at;
use zencodec::decode::Decode;

/// Probe AVIF metadata without decoding pixels.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo> {
    zenavif::AvifDecoderConfig::new()
        .probe_header(data)
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Avif, e)))
}

/// Decode AVIF to pixels.
pub(crate) fn decode(
    data: &[u8],
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<StopToken>,
    decode_policy: Option<zencodec::decode::DecodePolicy>,
) -> Result<DecodeOutput> {
    let mut dec = zenavif::AvifDecoderConfig::new();
    // Apply codec config if provided
    if let Some(cfg) = codec_config.and_then(|c| c.avif_decoder.as_ref()) {
        *dec.inner_mut() = cfg.as_ref().clone();
    }
    // AvifDecoderConfig has inherent with_limits
    if let Some(lim) = limits {
        dec = dec.with_limits(to_resource_limits(lim));
    }
    let mut job = dec.job();
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    if let Some(dp) = decode_policy {
        job = job.with_policy(dp);
    }
    job.decoder(Cow::Borrowed(data), &[])
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Avif, e)))?
        .decode()
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Avif, e)))
}
