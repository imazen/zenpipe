//! Still-image container decode adapter (delegates to the backend via trait interface).

use alloc::borrow::Cow;

use crate::error::Result;
use crate::limits::to_resource_limits;
use crate::{
    CodecError, DecodeJob, DecodeOutput, DecoderConfig, GainMapRender, ImageFormat, ImageInfo,
    Limits, OrientationHint, SharedPulse,
};
use whereat::{ResultAtExt, at_crate};
use zencodec::decode::Decode;

/// Probe container metadata without decoding pixels.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo> {
    let dec = heic::HeicDecoderConfig::new();
    let job = dec.job();
    at_crate!(job.probe(data)).map_err_at(|e| CodecError::from_codec(ImageFormat::Heic, e))
}

/// Decode container stills to pixels.
pub(crate) fn decode(
    data: &[u8],
    limits: Option<&Limits>,
    pulse: SharedPulse,
    decode_policy: Option<zencodec::decode::DecodePolicy>,
    gain_map_render: GainMapRender,
    extract_gain_map: bool,
    orientation: OrientationHint,
) -> Result<DecodeOutput> {
    let mut dec = heic::HeicDecoderConfig::new();
    if extract_gain_map {
        dec = dec.with_extract_gain_map(true);
    }
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    job = job.with_pulse(pulse);
    if let Some(dp) = decode_policy {
        // Refuse a policy the codec cannot enforce instead of ignoring it.
        job.try_set_policy(dp)
            .map_err(|e| whereat::at!(CodecError::from_codec(ImageFormat::Heic, e)))?;
    }
    // BaseOnly (default) is a no-op; ReconstructHdr makes the backend apply the gain map
    // and emit linear-float HDR pixels (via ultrahdr-core's reconstruction).
    job = job.with_gain_map_render(gain_map_render);
    // Preserve (default) leaves the EXIF tag authoritative; Correct bakes it.
    job = job.with_orientation(orientation);
    let decoder = at_crate!(job.decoder(Cow::Borrowed(data), &[]))
        .map_err_at(|e| CodecError::from_codec(ImageFormat::Heic, e))?;
    at_crate!(decoder.decode()).map_err_at(|e| CodecError::from_codec(ImageFormat::Heic, e))
}
