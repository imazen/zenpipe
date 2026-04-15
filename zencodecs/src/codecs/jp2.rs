//! JPEG 2000 decode adapter -- delegates to zenjp2 via trait interface.

use alloc::borrow::Cow;

use crate::error::Result;
use crate::limits::to_resource_limits;
use crate::{CodecError, DecodeOutput, ImageFormat, ImageInfo, Limits, StopToken};
use whereat::{ResultAtExt, at_crate};
use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};

/// Probe JPEG 2000 metadata without decoding pixels.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo> {
    at_crate!(zenjp2::Jp2DecoderConfig::new().job().probe(data))
        .map_err_at(|e| CodecError::from_codec(ImageFormat::Jp2, e))
}

/// Decode JPEG 2000 to pixels.
pub(crate) fn decode(
    data: &[u8],
    limits: Option<&Limits>,
    stop: Option<StopToken>,
    decode_policy: Option<zencodec::decode::DecodePolicy>,
) -> Result<DecodeOutput> {
    let dec = zenjp2::Jp2DecoderConfig::new();
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
        .map_err_at(|e| CodecError::from_codec(ImageFormat::Jp2, e))?;
    at_crate!(decoder.decode()).map_err_at(|e| CodecError::from_codec(ImageFormat::Jp2, e))
}
