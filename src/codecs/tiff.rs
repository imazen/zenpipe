//! TIFF codec adapter using zentiff via trait interface.

use alloc::borrow::Cow;

use crate::error::Result;
use crate::limits::to_resource_limits;
use crate::{CodecError, DecodeOutput, ImageFormat, ImageInfo, Limits, StopToken};
use whereat::{ResultAtExt, at_crate};
use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};

/// Probe TIFF metadata without decoding pixels.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo> {
    at_crate!(zentiff::codec::TiffDecoderCodecConfig::new()
        .job()
        .probe(data))
        .map_err_at(|e| CodecError::from_codec(ImageFormat::Tiff, e))
}

/// Decode TIFF to pixels.
pub(crate) fn decode(
    data: &[u8],
    limits: Option<&Limits>,
    stop: Option<StopToken>,
    decode_policy: Option<zencodec::decode::DecodePolicy>,
) -> Result<DecodeOutput> {
    let dec = zentiff::codec::TiffDecoderCodecConfig::new();
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
        .map_err_at(|e| CodecError::from_codec(ImageFormat::Tiff, e))?;
    at_crate!(decoder.decode())
        .map_err_at(|e| CodecError::from_codec(ImageFormat::Tiff, e))
}

// ═══════════════════════════════════════════════════════════════════════
// Trait-based encoder dispatch
// ═══════════════════════════════════════════════════════════════════════

use crate::dispatch::{BuiltEncoder, EncodeParams, StreamingEncoder, build_from_config};

pub(crate) fn build_trait_encoder<'a>(params: EncodeParams<'a>) -> BuiltEncoder<'a> {
    build_from_config(|_p| zentiff::codec::TiffEncoderCodecConfig::new(), params)
}

pub(crate) fn build_streaming(params: EncodeParams<'_>) -> crate::error::Result<StreamingEncoder> {
    crate::dispatch::build_streaming_from_config(
        |_p| zentiff::codec::TiffEncoderCodecConfig::new(),
        params,
    )
}
