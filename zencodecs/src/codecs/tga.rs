//! TGA codec adapter using zenbitmaps via trait interface.

use alloc::borrow::Cow;

use crate::error::Result;
use crate::limits::to_resource_limits;
use crate::{CodecError, DecodeOutput, ImageFormat, ImageInfo, Limits, SharedPulse};
use whereat::at;
use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};

pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo> {
    zenbitmaps::TgaDecoderConfig::new()
        .job()
        .probe(data)
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Tga, e)))
}

pub(crate) fn decode(
    data: &[u8],
    limits: Option<&Limits>,
    pulse: SharedPulse,
    decode_policy: Option<zencodec::decode::DecodePolicy>,
) -> Result<DecodeOutput> {
    let dec = zenbitmaps::TgaDecoderConfig::new();
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    job = job.with_pulse(pulse);
    if let Some(dp) = decode_policy {
        // Refuse a policy the codec cannot enforce instead of ignoring it.
        job.try_set_policy(dp)
            .map_err(|e| at!(CodecError::from_codec(ImageFormat::Tga, e)))?;
    }
    job.decoder(Cow::Borrowed(data), &[])
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Tga, e)))?
        .decode()
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Tga, e)))
}

use crate::dispatch::{BuiltEncoder, EncodeParams, StreamingEncoder, build_from_config};

pub(crate) fn build_trait_encoder<'a>(params: EncodeParams<'a>) -> BuiltEncoder<'a> {
    build_from_config(|_p| zenbitmaps::TgaEncoderConfig::new(), params)
}

pub(crate) fn build_streaming(params: EncodeParams<'_>) -> crate::error::Result<StreamingEncoder> {
    crate::dispatch::build_streaming_from_config(|_p| zenbitmaps::TgaEncoderConfig::new(), params)
}
