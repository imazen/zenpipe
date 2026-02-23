//! PNG codec adapter — delegates to zenpng via trait interface.

use crate::config::CodecConfig;
use crate::limits::to_resource_limits;
use crate::pixel::{ImgRef, Rgb, Rgba};
use crate::{
    CodecError, DecodeJob, DecodeOutput, DecoderConfig, EncodeJob, EncodeOutput, EncoderConfig,
    ImageFormat, ImageInfo, MetadataView, Limits, Stop,
};
use zencodec_types::{Decoder, Encoder, PixelSlice};

/// Probe PNG metadata without decoding pixels.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo, CodecError> {
    zenpng::PngDecoderConfig::new()
        .probe_header(data)
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))
}

/// Decode PNG to pixels.
pub(crate) fn decode(
    data: &[u8],
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<DecodeOutput, CodecError> {
    let dec = zenpng::PngDecoderConfig::new();
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.decoder()
        .decode(data)
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))
}

/// Build a PngEncoderConfig from codec config.
fn build_encoding(codec_config: Option<&CodecConfig>) -> zenpng::PngEncoderConfig {
    let mut enc = zenpng::PngEncoderConfig::new();
    if let Some(cfg) = codec_config {
        if let Some(compression) = cfg.png_compression {
            enc = enc.with_compression(compression);
        }
        if let Some(filter) = cfg.png_filter {
            enc = enc.with_filter(filter);
        }
    }
    enc
}

/// Encode RGB8 pixels to PNG.
pub(crate) fn encode_rgb8(
    img: ImgRef<Rgb<u8>>,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(codec_config);
    let mut job = enc.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(meta) = metadata {
        job = job.with_metadata(meta);
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.encoder()
        .encode(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))
}

/// Encode RGBA8 pixels to PNG.
pub(crate) fn encode_rgba8(
    img: ImgRef<Rgba<u8>>,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(codec_config);
    let mut job = enc.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(meta) = metadata {
        job = job.with_metadata(meta);
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.encoder()
        .encode(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))
}

/// Encode Gray8 pixels to PNG.
pub(crate) fn encode_gray8(
    img: ImgRef<crate::pixel::Gray<u8>>,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(codec_config);
    let mut job = enc.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(meta) = metadata {
        job = job.with_metadata(meta);
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.encoder()
        .encode(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))
}

/// Encode linear RGB f32 pixels to PNG.
pub(crate) fn encode_rgb_f32(
    img: ImgRef<Rgb<f32>>,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(codec_config);
    let mut job = enc.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(meta) = metadata {
        job = job.with_metadata(meta);
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.encoder()
        .encode(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))
}

/// Encode linear RGBA f32 pixels to PNG.
pub(crate) fn encode_rgba_f32(
    img: ImgRef<Rgba<f32>>,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(codec_config);
    let mut job = enc.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(meta) = metadata {
        job = job.with_metadata(meta);
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.encoder()
        .encode(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))
}

/// Encode linear grayscale f32 pixels to PNG.
pub(crate) fn encode_gray_f32(
    img: ImgRef<crate::pixel::Gray<f32>>,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(codec_config);
    let mut job = enc.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(meta) = metadata {
        job = job.with_metadata(meta);
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.encoder()
        .encode(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))
}
