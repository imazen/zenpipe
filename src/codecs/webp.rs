//! WebP codec adapter using zenwebp.
//!
//! Probe and encode (RGB8/RGBA8) use the trait interface.
//! Decode uses native API for metadata extraction.
//! BGRA encode uses native API for zero-copy path.

use crate::config::CodecConfig;
use crate::limits::to_resource_limits;
use crate::pixel::{Bgra, ImgRef, Rgb, Rgba};
use crate::{
    CodecError, DecodeJob, DecodeOutput, DecoderConfig, EncodeJob, EncodeOutput, EncoderConfig,
    ImageFormat, ImageInfo, MetadataView, Limits, Stop,
};
use zencodec_types::{Decoder, Encoder, PixelSlice, PixelSliceMut};

/// Probe WebP metadata without decoding pixels.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo, CodecError> {
    zenwebp::WebpDecoderConfig::new()
        .probe_header(data)
        .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))
}

/// Decode WebP to pixels.
pub(crate) fn decode(
    data: &[u8],
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<DecodeOutput, CodecError> {
    let mut dec = zenwebp::WebpDecoderConfig::new();
    if let Some(cfg) = codec_config.and_then(|c| c.webp_decoder.as_ref()) {
        *dec.inner_mut() = cfg.as_ref().clone();
    }
    // Set limits on both config (native limits) and job (pre-flight checks)
    if let Some(lim) = limits {
        let rl = to_resource_limits(lim);
        dec = dec.with_limits(rl);
    }
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.decoder()
        .decode(data)
        .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))
}

/// Decode WebP directly into a caller-provided RGBA8 buffer.
pub(crate) fn decode_into_rgba8(
    data: &[u8],
    dst: imgref::ImgRefMut<'_, Rgba<u8>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<ImageInfo, CodecError> {
    let mut dec = zenwebp::WebpDecoderConfig::new();
    if let Some(cfg) = codec_config.and_then(|c| c.webp_decoder.as_ref()) {
        *dec.inner_mut() = cfg.as_ref().clone();
    }
    // Set limits on both config (native limits) and job (pre-flight checks)
    if let Some(lim) = limits {
        let rl = to_resource_limits(lim);
        dec = dec.with_limits(rl);
    }
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.decoder()
        .decode_into(data, PixelSliceMut::from(dst))
        .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))
}

/// Build a WebpEncoderConfig from quality/lossless/codec_config.
fn build_encoding(
    quality: Option<f32>,
    lossless: bool,
    codec_config: Option<&CodecConfig>,
) -> zenwebp::WebpEncoderConfig {
    if lossless {
        let mut e = zenwebp::WebpEncoderConfig::lossless();
        if let Some(cfg) = codec_config.and_then(|c| c.webp_lossless.as_ref()) {
            *e.inner_mut() =
                zenwebp::encoder::config::EncoderConfig::Lossless(cfg.as_ref().clone());
        }
        e
    } else {
        let mut e = zenwebp::WebpEncoderConfig::lossy();
        if let Some(cfg) = codec_config.and_then(|c| c.webp_lossy.as_ref()) {
            *e.inner_mut() = zenwebp::encoder::config::EncoderConfig::Lossy(cfg.as_ref().clone());
        } else {
            let q = quality.unwrap_or(85.0).clamp(0.0, 100.0);
            e = e.with_quality(q);
        }
        e
    }
}

/// Encode RGB8 pixels to WebP.
pub(crate) fn encode_rgb8(
    img: ImgRef<Rgb<u8>>,
    quality: Option<f32>,
    lossless: bool,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality, lossless, codec_config);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))
}

/// Encode RGBA8 pixels to WebP.
pub(crate) fn encode_rgba8(
    img: ImgRef<Rgba<u8>>,
    quality: Option<f32>,
    lossless: bool,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality, lossless, codec_config);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))
}

/// Encode Gray8 pixels to WebP.
pub(crate) fn encode_gray8(
    img: ImgRef<crate::pixel::Gray<u8>>,
    quality: Option<f32>,
    lossless: bool,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality, lossless, codec_config);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))
}

/// Encode linear RGB f32 pixels to WebP.
pub(crate) fn encode_rgb_f32(
    img: ImgRef<Rgb<f32>>,
    quality: Option<f32>,
    lossless: bool,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality, lossless, codec_config);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))
}

/// Encode linear RGBA f32 pixels to WebP.
pub(crate) fn encode_rgba_f32(
    img: ImgRef<Rgba<f32>>,
    quality: Option<f32>,
    lossless: bool,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality, lossless, codec_config);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))
}

/// Encode linear grayscale f32 pixels to WebP.
pub(crate) fn encode_gray_f32(
    img: ImgRef<crate::pixel::Gray<f32>>,
    quality: Option<f32>,
    lossless: bool,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality, lossless, codec_config);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))
}

/// Encode BGRA8 pixels to WebP (native BGRA path).
///
/// Uses native zenwebp API for zero-copy BGRA encoding.
pub(crate) fn encode_bgra8(
    img: ImgRef<Bgra<u8>>,
    quality: Option<f32>,
    lossless: bool,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    _limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let width = img.width() as u32;
    let height = img.height() as u32;
    let (buf, _, _) = img.to_contiguous_buf();
    let bytes: &[u8] = bytemuck::cast_slice(buf.as_ref());
    let webp_meta = to_webp_metadata(metadata);

    let webp_data = if lossless {
        let config = codec_config
            .and_then(|c| c.webp_lossless.as_ref())
            .map(|c| c.as_ref().clone())
            .unwrap_or_default();
        let mut request = zenwebp::EncodeRequest::lossless(
            &config,
            bytes,
            zenwebp::PixelLayout::Bgra8,
            width,
            height,
        )
        .with_metadata(webp_meta);
        if let Some(s) = stop {
            request = request.with_stop(s);
        }
        request
            .encode()
            .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))?
    } else {
        let quality = quality.unwrap_or(85.0).clamp(0.0, 100.0);
        let config = codec_config
            .and_then(|c| c.webp_lossy.as_ref())
            .map(|c| c.as_ref().clone())
            .unwrap_or_else(|| zenwebp::LossyConfig::new().with_quality(quality));
        let mut request = zenwebp::EncodeRequest::lossy(
            &config,
            bytes,
            zenwebp::PixelLayout::Bgra8,
            width,
            height,
        )
        .with_metadata(webp_meta);
        if let Some(s) = stop {
            request = request.with_stop(s);
        }
        request
            .encode()
            .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))?
    };

    Ok(EncodeOutput::new(webp_data, ImageFormat::WebP))
}

/// Convert zencodecs MetadataView to zenwebp ImageMetadata.
fn to_webp_metadata<'a>(metadata: Option<&'a MetadataView<'a>>) -> zenwebp::ImageMetadata<'a> {
    let mut webp_meta = zenwebp::ImageMetadata::new();
    if let Some(meta) = metadata {
        if let Some(icc) = meta.icc_profile {
            webp_meta = webp_meta.with_icc_profile(icc);
        }
        if let Some(exif) = meta.exif {
            webp_meta = webp_meta.with_exif(exif);
        }
        if let Some(xmp) = meta.xmp {
            webp_meta = webp_meta.with_xmp(xmp);
        }
    }
    webp_meta
}
