//! JXL decode adapter -- delegates to zenjxl via trait interface.

use alloc::borrow::Cow;

use crate::error::Result;
use crate::limits::to_resource_limits;
use crate::{
    CodecError, DecodeJob, DecodeOutput, DecoderConfig, ImageFormat, ImageInfo, Limits, StopToken,
};
use whereat::{ResultAtExt, at_crate};
use zencodec::decode::Decode;

/// Probe JXL metadata without decoding pixels.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo> {
    let dec = zenjxl::JxlDecoderConfig::new();
    let job = dec.job();
    at_crate!(job.probe(data)).map_err_at(|e| CodecError::from_codec(ImageFormat::Jxl, e))
}

/// Decode JXL to pixels.
pub(crate) fn decode(
    data: &[u8],
    limits: Option<&Limits>,
    stop: Option<StopToken>,
    decode_policy: Option<zencodec::decode::DecodePolicy>,
    extract_gain_map: bool,
) -> Result<DecodeOutput> {
    let dec = zenjxl::JxlDecoderConfig::new();
    let mut job = dec.job();
    if extract_gain_map {
        job = job.with_extract_gain_map(true);
    }
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
        .map_err_at(|e| CodecError::from_codec(ImageFormat::Jxl, e))?;
    at_crate!(decoder.decode()).map_err_at(|e| CodecError::from_codec(ImageFormat::Jxl, e))
}
