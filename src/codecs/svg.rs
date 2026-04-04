//! SVG codec adapter -- delegates to zensvg via trait interface.

use alloc::borrow::Cow;

use crate::error::Result;
use crate::limits::to_resource_limits;
use crate::{CodecError, DecodeOutput, Limits, StopToken};
use whereat::at;
use zencodec::ImageInfo;
use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};

/// The SVG image format (custom, not a built-in variant).
fn svg_format() -> zencodec::ImageFormat {
    zensvg::svg_format()
}

/// Probe SVG metadata without rendering.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo> {
    let config = zensvg::SvgDecoderConfig::new();
    let job = config.job();
    job.probe(data)
        .map_err(|e| at!(CodecError::from_codec(svg_format(), e)))
}

/// Decode (render) SVG to pixels.
pub(crate) fn decode(
    data: &[u8],
    limits: Option<&Limits>,
    stop: Option<StopToken>,
    decode_policy: Option<zencodec::decode::DecodePolicy>,
) -> Result<DecodeOutput> {
    let config = zensvg::SvgDecoderConfig::new();
    let mut job = config.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    if let Some(dp) = decode_policy {
        job = job.with_policy(dp);
    }
    let format = svg_format();
    let decoder = job
        .decoder(Cow::Borrowed(data), &[])
        .map_err(|e| at!(CodecError::from_codec(format, e)))?;
    decoder
        .decode()
        .map_err(|e| at!(CodecError::from_codec(format, e)))
}

/// Detect whether data looks like SVG or SVGZ.
pub(crate) fn detect_svg(data: &[u8]) -> bool {
    zensvg::detect_svg(data)
}
