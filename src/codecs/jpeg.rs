//! JPEG codec adapter using zenjpeg.
//!
//! Probe and encode use the trait interface. Decode uses native API to
//! preserve JPEG extras (DecodedExtras with DCT coefficients etc.).

use crate::config::CodecConfig;
use crate::limits::to_resource_limits;
use crate::pixel::{Bgra, ImgRef, ImgVec, Rgb, Rgba};
use crate::{
    CodecError, DecodeJob, DecodeOutput, DecoderConfig, EncodeJob, EncodeOutput, EncoderConfig,
    ImageFormat, ImageInfo, Limits, MetadataView, PixelData, Stop,
};
use zencodec_types::{Decoder, Encoder, PixelSlice, PixelSliceMut};

/// Probe JPEG metadata without decoding pixels.
///
/// Uses `Permissive` strictness so we can extract dimensions and metadata from
/// structurally damaged files that would fail a full decode.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo, CodecError> {
    let info = zenjpeg::decoder::DecodeConfig::new()
        .permissive()
        .read_info(data)
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?;
    Ok(jpeg_info_to_image_info(&info))
}

/// Convert zenjpeg's `JpegInfo` to zencodec-types `ImageInfo`.
fn jpeg_info_to_image_info(info: &zenjpeg::decoder::JpegInfo) -> ImageInfo {
    let mut ii = ImageInfo::new(
        info.dimensions.width,
        info.dimensions.height,
        ImageFormat::Jpeg,
    )
    .with_bit_depth(info.precision)
    .with_channel_count(info.num_components);
    if let Some(ref icc) = info.icc_profile {
        ii = ii.with_icc_profile(icc.clone());
    }
    if let Some(ref exif) = info.exif {
        if let Some(orient) = zenjpeg::lossless::parse_exif_orientation(exif) {
            ii = ii.with_orientation(zencodec_types::Orientation::from_exif(orient as u16));
        }
        ii = ii.with_exif(exif.clone());
    }
    if let Some(ref xmp) = info.xmp {
        // Detect UltraHDR gain map from XMP hdrgm namespace
        if xmp.contains("hdrgm:Version") || xmp.contains("hdrgm:GainMapMax") {
            ii = ii.with_gain_map(true);
        }
        ii = ii.with_xmp(xmp.as_bytes().to_vec());
    }
    ii
}

/// Build a zenjpeg Decoder from codec config and limits.
fn build_decoder(
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
) -> zenjpeg::decoder::DecodeConfig {
    let mut decoder = codec_config
        .and_then(|c| c.jpeg_decoder.as_ref())
        .map(|d| d.as_ref().clone())
        .unwrap_or_default();

    if let Some(lim) = limits {
        if let Some(max_px) = lim.max_pixels {
            decoder = decoder.max_pixels(max_px);
        }
        if let Some(max_mem) = lim.max_memory_bytes {
            decoder = decoder.max_memory(max_mem);
        }
    }

    decoder
}

/// Decode JPEG to pixels.
///
/// Uses native API to preserve JPEG extras (DecodedExtras).
pub(crate) fn decode(
    data: &[u8],
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<DecodeOutput, CodecError> {
    let stop = crate::limits::stop_or_default(stop);
    let decoder = build_decoder(codec_config, limits);

    let mut result = decoder
        .decode(data, stop)
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?;

    let width = result.width();
    let height = result.height();

    // Validate dimensions against zencodecs limits (zenjpeg has its own, but
    // this catches width/height limits that zenjpeg doesn't check)
    if let Some(lim) = limits {
        lim.validate(width, height, 3)?;
    }

    let extras = result.extras();
    let icc_profile = extras
        .and_then(|e| e.icc_profile())
        .map(|p: &[u8]| p.to_vec());
    let exif = extras.and_then(|e| e.exif()).map(|p: &[u8]| p.to_vec());
    let xmp = extras
        .and_then(|e| e.xmp())
        .map(|s: &str| s.as_bytes().to_vec());

    let raw_pixels = result
        .pixels_u8()
        .ok_or_else(|| CodecError::InvalidInput("no pixel data in decoded image".into()))?;

    let rgb_pixels: &[Rgb<u8>] = bytemuck::cast_slice(raw_pixels);
    let img = ImgVec::new(rgb_pixels.to_vec(), width as usize, height as usize);

    let jpeg_extras = result.take_extras();

    let mut ii = ImageInfo::new(width, height, ImageFormat::Jpeg).with_frame_count(1);
    if let Some(icc) = icc_profile {
        ii = ii.with_icc_profile(icc);
    }
    if let Some(exif) = exif {
        ii = ii.with_exif(exif);
    }
    if let Some(xmp) = xmp {
        // Detect UltraHDR gain map from XMP hdrgm namespace
        if let Ok(xmp_str) = core::str::from_utf8(&xmp) {
            if xmp_str.contains("hdrgm:Version") || xmp_str.contains("hdrgm:GainMapMax") {
                ii = ii.with_gain_map(true);
            }
        }
        ii = ii.with_xmp(xmp);
    }

    let mut output = DecodeOutput::new(PixelData::Rgb8(img), ii);
    if let Some(extras) = jpeg_extras {
        output = output.with_extras(extras);
    }
    Ok(output)
}

/// Compute actual output dimensions for JPEG (applies DctScale, auto_orient).
pub(crate) fn decode_info(
    data: &[u8],
    _codec_config: Option<&CodecConfig>,
) -> Result<ImageInfo, CodecError> {
    // Use probe_full which returns complete metadata
    zenjpeg::JpegDecoderConfig::new()
        .probe_full(data)
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))
}

/// Decode JPEG directly into a caller-provided RGB8 buffer.
pub(crate) fn decode_into_rgb8(
    data: &[u8],
    dst: imgref::ImgRefMut<'_, Rgb<u8>>,
    _codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<ImageInfo, CodecError> {
    let dec = zenjpeg::JpegDecoderConfig::new();
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.decoder()
        .decode_into(data, PixelSliceMut::from(dst))
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))
}

/// Decode JPEG directly into a caller-provided RGBA8 buffer.
pub(crate) fn decode_into_rgba8(
    data: &[u8],
    dst: imgref::ImgRefMut<'_, Rgba<u8>>,
    _codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<ImageInfo, CodecError> {
    let dec = zenjpeg::JpegDecoderConfig::new();
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.decoder()
        .decode_into(data, PixelSliceMut::from(dst))
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))
}

/// Decode JPEG directly into a caller-provided Gray8 buffer.
pub(crate) fn decode_into_gray8(
    data: &[u8],
    dst: imgref::ImgRefMut<'_, crate::pixel::Gray<u8>>,
    _codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<ImageInfo, CodecError> {
    let dec = zenjpeg::JpegDecoderConfig::new();
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.decoder()
        .decode_into(data, PixelSliceMut::from(dst))
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))
}

/// Build a JpegEncoderConfig from codec config or generic quality.
fn build_encoding(
    quality: Option<f32>,
    codec_config: Option<&CodecConfig>,
) -> zenjpeg::JpegEncoderConfig {
    if let Some(cfg) = codec_config.and_then(|c| c.jpeg_encoder.as_ref()) {
        let mut e = zenjpeg::JpegEncoderConfig::new();
        *e.inner_mut() = cfg.as_ref().clone();
        e
    } else {
        let q = quality.unwrap_or(85.0).clamp(0.0, 100.0);
        zenjpeg::JpegEncoderConfig::new().with_calibrated_quality(q)
    }
}

/// Encode RGB8 pixels to JPEG.
pub(crate) fn encode_rgb8(
    img: ImgRef<Rgb<u8>>,
    quality: Option<f32>,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality, codec_config);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))
}

/// Encode RGBA8 pixels to JPEG (alpha is discarded).
pub(crate) fn encode_rgba8(
    img: ImgRef<Rgba<u8>>,
    quality: Option<f32>,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality, codec_config);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))
}

/// Encode BGRA8 pixels to JPEG (native BGRA path, alpha discarded).
pub(crate) fn encode_bgra8(
    img: ImgRef<Bgra<u8>>,
    quality: Option<f32>,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    _limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let config = if let Some(cfg) = codec_config.and_then(|c| c.jpeg_encoder.as_ref()) {
        cfg.as_ref().clone()
    } else {
        let quality = quality.unwrap_or(85.0).clamp(0.0, 100.0) as u8;
        zenjpeg::encoder::EncoderConfig::ycbcr(
            quality,
            zenjpeg::encoder::ChromaSubsampling::Quarter,
        )
    };

    let width = img.width() as u32;
    let height = img.height() as u32;
    let (buf, _, _) = img.to_contiguous_buf();
    let bytes: &[u8] = bytemuck::cast_slice(buf.as_ref());

    let mut request = config.request();
    request = apply_metadata(request, metadata);
    if let Some(s) = stop {
        request = request.stop(s);
    }

    let jpeg_data = request
        .encode_bytes(
            bytes,
            width,
            height,
            zenjpeg::encoder::PixelLayout::Bgra8Srgb,
        )
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?;

    Ok(EncodeOutput::new(jpeg_data, ImageFormat::Jpeg))
}

/// Encode BGRX8 pixels to JPEG (native BGRX path, padding byte ignored).
pub(crate) fn encode_bgrx8(
    img: ImgRef<Bgra<u8>>,
    quality: Option<f32>,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    _limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let config = if let Some(cfg) = codec_config.and_then(|c| c.jpeg_encoder.as_ref()) {
        cfg.as_ref().clone()
    } else {
        let quality = quality.unwrap_or(85.0).clamp(0.0, 100.0) as u8;
        zenjpeg::encoder::EncoderConfig::ycbcr(
            quality,
            zenjpeg::encoder::ChromaSubsampling::Quarter,
        )
    };

    let width = img.width() as u32;
    let height = img.height() as u32;
    let (buf, _, _) = img.to_contiguous_buf();
    let bytes: &[u8] = bytemuck::cast_slice(buf.as_ref());

    let mut request = config.request();
    request = apply_metadata(request, metadata);
    if let Some(s) = stop {
        request = request.stop(s);
    }

    let jpeg_data = request
        .encode_bytes(
            bytes,
            width,
            height,
            zenjpeg::encoder::PixelLayout::Bgrx8Srgb,
        )
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?;

    Ok(EncodeOutput::new(jpeg_data, ImageFormat::Jpeg))
}

/// Encode Gray8 pixels to JPEG.
pub(crate) fn encode_gray8(
    img: ImgRef<crate::pixel::Gray<u8>>,
    quality: Option<f32>,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality, codec_config);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))
}

/// Encode linear RGB f32 pixels to JPEG.
pub(crate) fn encode_rgb_f32(
    img: ImgRef<Rgb<f32>>,
    quality: Option<f32>,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality, codec_config);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))
}

/// Encode linear RGBA f32 pixels to JPEG (alpha discarded).
pub(crate) fn encode_rgba_f32(
    img: ImgRef<Rgba<f32>>,
    quality: Option<f32>,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality, codec_config);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))
}

/// Encode linear grayscale f32 pixels to JPEG.
pub(crate) fn encode_gray_f32(
    img: ImgRef<crate::pixel::Gray<f32>>,
    quality: Option<f32>,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality, codec_config);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))
}

/// Apply zencodecs MetadataView to a zenjpeg EncodeRequest.
fn apply_metadata<'a>(
    mut request: zenjpeg::encoder::EncodeRequest<'a>,
    metadata: Option<&'a MetadataView<'a>>,
) -> zenjpeg::encoder::EncodeRequest<'a> {
    if let Some(meta) = metadata {
        if let Some(icc) = meta.icc_profile {
            request = request.icc_profile(icc);
        }
        if let Some(exif) = meta.exif {
            request = request.exif(zenjpeg::encoder::Exif::raw(exif.to_vec()));
        }
        if let Some(xmp) = meta.xmp {
            request = request.xmp(xmp);
        }
    }
    request
}
