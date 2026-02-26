//! HEIC decode adapter — delegates to heic-decoder via trait interface.

use crate::limits::to_resource_limits;
use crate::{
    CodecError, DecodeJob, DecodeOutput, DecoderConfig, ImageFormat, ImageInfo, Limits, Stop,
};
use zencodec_types::Decode;

/// Probe HEIC metadata without decoding pixels.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo, CodecError> {
    heic_decoder::HeicDecoderConfig::new()
        .probe_header(data)
        .map_err(|e| CodecError::from_codec(ImageFormat::Heic, e))
}

/// Decode HEIC to pixels.
pub(crate) fn decode(
    data: &[u8],
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<DecodeOutput, CodecError> {
    let dec = heic_decoder::HeicDecoderConfig::new();
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.decoder()
        .map_err(|e| CodecError::from_codec(ImageFormat::Heic, e))?
        .decode(data, &[])
        .map_err(|e| CodecError::from_codec(ImageFormat::Heic, e))
}
