//! GIF codec adapter using zengif via trait interface.

use crate::config::CodecConfig;
use crate::limits::to_resource_limits;
use crate::{
    CodecError, DecodeOutput, EncodeJob, EncoderConfig, ImageFormat, ImageInfo, Limits, Stop,
};
use alloc::boxed::Box;
use zencodec_types::{Decode, DecodeJob as _, DecoderConfig as _};

/// Probe GIF metadata without decoding pixels.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo, CodecError> {
    zengif::GifDecoderConfig::new()
        .probe_header(data)
        .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))
}

/// Decode GIF to pixels (first frame only).
pub(crate) fn decode(
    data: &[u8],
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<DecodeOutput, CodecError> {
    let dec = zengif::GifDecoderConfig::new();
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.decoder(data, &[])
        .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))?
        .decode()
        .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))
}

/// Build a GifEncoderConfig from codec config.
fn build_gif_encoding(codec_config: Option<&CodecConfig>) -> zengif::GifEncoderConfig {
    let mut enc = zengif::GifEncoderConfig::new();
    if let Some(cfg) = codec_config.and_then(|c| c.gif_encoder.as_ref()) {
        *enc.inner_mut() = cfg.as_ref().clone();
    }
    enc
}

// ═══════════════════════════════════════════════════════════════════════
// Trait-based encoder dispatch
// ═══════════════════════════════════════════════════════════════════════

use crate::dispatch::{BuiltEncoder, EncodeParams};

pub(crate) fn build_trait_encoder<'a>(params: EncodeParams<'a>) -> BuiltEncoder<'a> {
    BuiltEncoder {
        encoder: Box::new(move |pixels| {
            let enc = build_gif_encoding(params.codec_config);
            let mut job = enc.job();
            if let Some(lim) = params.limits {
                job = job.with_limits(crate::limits::to_resource_limits(lim));
            }
            if let Some(s) = params.stop {
                job = job.with_stop(s);
            }
            use zencodec_types::Encoder as _;
            job.encoder()
                .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))?
                .encode(pixels)
                .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))
        }),
        supported: zengif::GifEncoderConfig::supported_descriptors(),
    }
}
