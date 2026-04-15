//! Radiance HDR codec adapter using zenbitmaps via trait interface.

use alloc::borrow::Cow;

use crate::error::Result;
use crate::limits::to_resource_limits;
use crate::{CodecError, DecodeOutput, ImageFormat, ImageInfo, Limits, StopToken};
use whereat::at;
use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};

pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo> {
    zenbitmaps::HdrDecoderConfig::new()
        .job()
        .probe(data)
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Hdr, e)))
}

pub(crate) fn decode(
    data: &[u8],
    limits: Option<&Limits>,
    stop: Option<StopToken>,
    decode_policy: Option<zencodec::decode::DecodePolicy>,
) -> Result<DecodeOutput> {
    let dec = zenbitmaps::HdrDecoderConfig::new();
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
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Hdr, e)))?
        .decode()
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Hdr, e)))
}

use crate::dispatch::{BuiltEncoder, EncodeParams, StreamingEncoder, build_from_config};

pub(crate) fn build_trait_encoder<'a>(params: EncodeParams<'a>) -> BuiltEncoder<'a> {
    build_from_config(|_p| zenbitmaps::HdrEncoderConfig::new(), params)
}

pub(crate) fn build_streaming(params: EncodeParams<'_>) -> crate::error::Result<StreamingEncoder> {
    crate::dispatch::build_streaming_from_config(|_p| zenbitmaps::HdrEncoderConfig::new(), params)
}
