//! PNM/PAM/PFM codec adapter using zenbitmaps via trait interface.

use alloc::borrow::Cow;

use crate::error::Result;
use crate::limits::to_resource_limits;
use crate::{CodecError, DecodeOutput, ImageFormat, ImageInfo, Limits, Stop};
use whereat::at;
use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};

/// Probe PNM metadata without decoding pixels.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo> {
    zenbitmaps::PnmDecoderConfig::new()
        .job()
        .probe(data)
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Pnm, e)))
}

/// Decode PNM to pixels.
pub(crate) fn decode(
    data: &[u8],
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<DecodeOutput> {
    let dec = zenbitmaps::PnmDecoderConfig::new();
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.decoder(Cow::Borrowed(data), &[])
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Pnm, e)))?
        .decode()
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Pnm, e)))
}

// ═══════════════════════════════════════════════════════════════════════
// Trait-based encoder dispatch
// ═══════════════════════════════════════════════════════════════════════

use crate::dispatch::{BuiltEncoder, EncodeParams, build_from_config};

pub(crate) fn build_trait_encoder<'a>(params: EncodeParams<'a>) -> BuiltEncoder<'a> {
    build_from_config(|_p| zenbitmaps::PnmEncoderConfig::new(), params)
}
