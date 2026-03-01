//! JPEG codec adapter using zenjpeg.
//!
//! Probe and encode use the trait interface. Decode uses native API to
//! preserve JPEG extras (DecodedExtras with DCT coefficients etc.).

use crate::config::CodecConfig;
use crate::limits::to_resource_limits;
use crate::pixel::{Bgra, ImgRef, ImgVec, Rgb, Rgba};
use crate::{
    CodecError, DecodeJob, DecodeOutput, DecoderConfig, EncodeJob, EncodeOutput, EncoderConfig,
    ImageFormat, ImageInfo, Limits, MetadataView, Stop,
};
use alloc::boxed::Box;
use zencodec_types::{
    EncodeGray8, EncodeGrayF32, EncodeRgb8, EncodeRgbF32, EncodeRgba8, EncodeRgbaF32, PixelSlice,
    PixelSliceMut,
};
use zencodec_types::{PixelBuffer, PixelDescriptor};

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

    let buf = PixelBuffer::from_imgvec(img).with_descriptor(PixelDescriptor::RGB8_SRGB);
    let mut output = DecodeOutput::new(buf.into(), ii);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?
        .encode_rgb8(PixelSlice::from(img))
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?
        .encode_rgba8(PixelSlice::from(img))
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?
        .encode_gray8(PixelSlice::from(img))
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?
        .encode_rgb_f32(PixelSlice::from(img))
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?
        .encode_rgba_f32(PixelSlice::from(img))
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?
        .encode_gray_f32(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))
}

// ═══════════════════════════════════════════════════════════════════════
// DynEncoder implementation
// ═══════════════════════════════════════════════════════════════════════

use crate::dispatch::{DynEncoder, EncodeParams};

/// Supported pixel descriptors for JPEG encoding (in preference order).
static JPEG_SUPPORTED: &[PixelDescriptor] = &[
    PixelDescriptor::RGB8_SRGB,
    PixelDescriptor::GRAY8_SRGB,
    PixelDescriptor::RGBA8_SRGB,
    PixelDescriptor::BGRA8_SRGB,
    PixelDescriptor::BGRX8_SRGB,
    PixelDescriptor::RGBF32_LINEAR,
    PixelDescriptor::RGBAF32_LINEAR,
    PixelDescriptor::GRAYF32_LINEAR,
];

pub(crate) struct JpegDynEncoder<'a> {
    quality: Option<f32>,
    metadata: Option<&'a MetadataView<'a>>,
    codec_config: Option<&'a CodecConfig>,
    limits: Option<&'a Limits>,
    stop: Option<&'a dyn Stop>,
}

pub(crate) fn build_dyn_encoder(params: EncodeParams<'_>) -> JpegDynEncoder<'_> {
    JpegDynEncoder {
        quality: params.quality,
        metadata: params.metadata,
        codec_config: params.codec_config,
        limits: params.limits,
        stop: params.stop,
    }
}

impl DynEncoder for JpegDynEncoder<'_> {
    fn format(&self) -> ImageFormat {
        ImageFormat::Jpeg
    }

    fn supported_descriptors(&self) -> &'static [PixelDescriptor] {
        JPEG_SUPPORTED
    }

    fn encode_pixels(
        self: Box<Self>,
        data: &[u8],
        descriptor: PixelDescriptor,
        width: u32,
        height: u32,
        stride: usize,
    ) -> Result<EncodeOutput, CodecError> {
        let w = width as usize;
        let h = height as usize;

        match descriptor.pixel_format() {
            Some(zencodec_types::PixelFormat::Rgb8) => {
                let pixels: &[Rgb<u8>] = bytemuck::cast_slice(data);
                let img = imgref::ImgRef::new_stride(pixels, w, h, stride / 3);
                encode_rgb8(
                    img,
                    self.quality,
                    self.metadata,
                    self.codec_config,
                    self.limits,
                    self.stop,
                )
            }
            Some(zencodec_types::PixelFormat::Rgba8) => {
                let pixels: &[Rgba<u8>] = bytemuck::cast_slice(data);
                let img = imgref::ImgRef::new_stride(pixels, w, h, stride / 4);
                encode_rgba8(
                    img,
                    self.quality,
                    self.metadata,
                    self.codec_config,
                    self.limits,
                    self.stop,
                )
            }
            Some(zencodec_types::PixelFormat::Bgra8) => {
                let pixels: &[Bgra<u8>] = bytemuck::cast_slice(data);
                let img = imgref::ImgRef::new_stride(pixels, w, h, stride / 4);
                // Check descriptor alpha to distinguish BGRA vs BGRX
                if descriptor.alpha == Some(zencodec_types::AlphaMode::Undefined) {
                    encode_bgrx8(
                        img,
                        self.quality,
                        self.metadata,
                        self.codec_config,
                        self.limits,
                        self.stop,
                    )
                } else {
                    encode_bgra8(
                        img,
                        self.quality,
                        self.metadata,
                        self.codec_config,
                        self.limits,
                        self.stop,
                    )
                }
            }
            Some(zencodec_types::PixelFormat::Bgrx8) => {
                let pixels: &[Bgra<u8>] = bytemuck::cast_slice(data);
                let img = imgref::ImgRef::new_stride(pixels, w, h, stride / 4);
                encode_bgrx8(
                    img,
                    self.quality,
                    self.metadata,
                    self.codec_config,
                    self.limits,
                    self.stop,
                )
            }
            Some(zencodec_types::PixelFormat::Gray8) => {
                let pixels: &[crate::pixel::Gray<u8>] = bytemuck::cast_slice(data);
                let img = imgref::ImgRef::new_stride(pixels, w, h, stride);
                encode_gray8(
                    img,
                    self.quality,
                    self.metadata,
                    self.codec_config,
                    self.limits,
                    self.stop,
                )
            }
            Some(zencodec_types::PixelFormat::RgbF32) => {
                let pixels: &[Rgb<f32>] = bytemuck::cast_slice(data);
                let img = imgref::ImgRef::new_stride(pixels, w, h, stride / 12);
                encode_rgb_f32(
                    img,
                    self.quality,
                    self.metadata,
                    self.codec_config,
                    self.limits,
                    self.stop,
                )
            }
            Some(zencodec_types::PixelFormat::RgbaF32) => {
                let pixels: &[Rgba<f32>] = bytemuck::cast_slice(data);
                let img = imgref::ImgRef::new_stride(pixels, w, h, stride / 16);
                encode_rgba_f32(
                    img,
                    self.quality,
                    self.metadata,
                    self.codec_config,
                    self.limits,
                    self.stop,
                )
            }
            Some(zencodec_types::PixelFormat::GrayF32) => {
                let pixels: &[crate::pixel::Gray<f32>] = bytemuck::cast_slice(data);
                let img = imgref::ImgRef::new_stride(pixels, w, h, stride / 4);
                encode_gray_f32(
                    img,
                    self.quality,
                    self.metadata,
                    self.codec_config,
                    self.limits,
                    self.stop,
                )
            }
            _ => Err(CodecError::InvalidInput(alloc::format!(
                "JPEG encoder does not support pixel format: {}",
                descriptor
            ))),
        }
    }
}

/// Decode UltraHDR JPEG to linear f32 RGBA HDR pixels.
///
/// Decodes the SDR base image, extracts the gain map, and reconstructs
/// HDR content at the specified display boost level.
///
/// Returns linear f32 RGBA pixels suitable for HDR display or further processing.
#[cfg(feature = "jpeg-ultrahdr")]
pub(crate) fn decode_hdr(
    data: &[u8],
    display_boost: f32,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<DecodeOutput, CodecError> {
    use linear_srgb::default::srgb_u8_to_linear;
    use zenjpeg::ultrahdr::{UltraHdrExtras, create_hdr_reconstructor};

    let stop_token = crate::limits::stop_or_default(stop);
    let decoder = build_decoder(codec_config, limits);

    let mut result = decoder
        .decode(data, stop_token)
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?;

    let width = result.width();
    let height = result.height();

    if let Some(lim) = limits {
        lim.validate(width, height, 4)?;
    }

    let extras = result
        .extras()
        .ok_or_else(|| CodecError::InvalidInput("no extras in decoded JPEG".into()))?;

    if !extras.is_ultrahdr() {
        return Err(CodecError::InvalidInput(
            "JPEG does not contain UltraHDR gain map".into(),
        ));
    }

    // Create HDR reconstructor from gain map metadata
    let mut reconstructor = create_hdr_reconstructor(width, height, extras, display_boost)
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?;

    // Get SDR pixels and convert to linear f32
    let raw_pixels = result
        .pixels_u8()
        .ok_or_else(|| CodecError::InvalidInput("no pixel data in decoded image".into()))?;

    let row_stride = width as usize * 3; // RGB8

    // Process in batches of 16 rows
    let batch_size = 16u32;
    let mut hdr_pixels: alloc::vec::Vec<Rgba<f32>> =
        alloc::vec::Vec::with_capacity(width as usize * height as usize);

    let mut y = 0u32;
    while y < height {
        let batch_height = batch_size.min(height - y);
        let batch_pixel_count = width as usize * batch_height as usize;

        // Convert this batch from sRGB u8 to linear f32 RGB
        let batch_start = y as usize * row_stride;
        let batch_end = batch_start + batch_height as usize * row_stride;
        let sdr_bytes = &raw_pixels[batch_start..batch_end];

        let mut sdr_linear: alloc::vec::Vec<f32> =
            alloc::vec::Vec::with_capacity(batch_pixel_count * 3);
        for pixel in sdr_bytes.chunks_exact(3) {
            sdr_linear.push(srgb_u8_to_linear(pixel[0]));
            sdr_linear.push(srgb_u8_to_linear(pixel[1]));
            sdr_linear.push(srgb_u8_to_linear(pixel[2]));
        }

        // Reconstruct HDR (returns linear f32 RGBA)
        let hdr_batch = reconstructor
            .process_rows(&sdr_linear, batch_height)
            .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?;

        // Convert f32 slice to Rgba<f32> pixels
        for rgba in hdr_batch.chunks_exact(4) {
            hdr_pixels.push(Rgba {
                r: rgba[0],
                g: rgba[1],
                b: rgba[2],
                a: rgba[3],
            });
        }

        y += batch_height;
    }

    let img = imgref::ImgVec::new(hdr_pixels, width as usize, height as usize);
    let buf = PixelBuffer::from_imgvec(img).with_descriptor(PixelDescriptor::RGBAF32_LINEAR);

    let mut ii = ImageInfo::new(width, height, ImageFormat::Jpeg)
        .with_frame_count(1)
        .with_gain_map(true);

    // Preserve metadata from extras
    let extras = result.extras();
    if let Some(extras) = extras {
        if let Some(icc) = extras.icc_profile() {
            ii = ii.with_icc_profile(icc.to_vec());
        }
        if let Some(exif) = extras.exif() {
            ii = ii.with_exif(exif.to_vec());
        }
        if let Some(xmp) = extras.xmp() {
            ii = ii.with_xmp(xmp.as_bytes().to_vec());
        }
    }

    let jpeg_extras = result.take_extras();
    let mut output = DecodeOutput::new(buf.into(), ii);
    if let Some(extras) = jpeg_extras {
        output = output.with_extras(extras);
    }
    Ok(output)
}

/// Encode linear f32 RGB pixels to UltraHDR JPEG.
///
/// Takes HDR content in linear f32 RGB and produces a backward-compatible
/// UltraHDR JPEG with embedded gain map.
#[cfg(feature = "jpeg-ultrahdr")]
pub(crate) fn encode_ultrahdr_rgb_f32(
    img: ImgRef<Rgb<f32>>,
    quality: Option<f32>,
    gainmap_quality: Option<f32>,
    _metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    _limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    use zenjpeg::ultrahdr::{
        GainMapConfig, ToneMapConfig, UhdrColorGamut, UhdrColorTransfer, UhdrPixelFormat,
        UhdrRawImage, encode_ultrahdr,
    };

    let stop_token = crate::limits::stop_or_default(stop);
    let width = img.width() as u32;
    let height = img.height() as u32;

    // Convert ImgRef<Rgb<f32>> to RawImage (RGBA f32)
    let (buf, _, _) = img.to_contiguous_buf();
    let mut rgba_data: alloc::vec::Vec<u8> =
        alloc::vec::Vec::with_capacity(width as usize * height as usize * 16);
    for px in buf.iter() {
        rgba_data.extend_from_slice(&px.r.to_le_bytes());
        rgba_data.extend_from_slice(&px.g.to_le_bytes());
        rgba_data.extend_from_slice(&px.b.to_le_bytes());
        rgba_data.extend_from_slice(&1.0f32.to_le_bytes()); // alpha = 1.0
    }

    let hdr = UhdrRawImage::from_data(
        width,
        height,
        UhdrPixelFormat::Rgba32F,
        UhdrColorGamut::Bt709,
        UhdrColorTransfer::Linear,
        rgba_data,
    )
    .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?;

    let enc = build_encoding(quality, codec_config);
    let gm_quality = gainmap_quality.unwrap_or(75.0);

    let jpeg_data = encode_ultrahdr(
        &hdr,
        &GainMapConfig::default(),
        &ToneMapConfig::default(),
        enc.inner(),
        gm_quality,
        stop_token,
    )
    .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?;

    Ok(EncodeOutput::new(jpeg_data, ImageFormat::Jpeg))
}

/// Encode linear f32 RGBA pixels to UltraHDR JPEG.
///
/// Takes HDR content in linear f32 RGBA and produces a backward-compatible
/// UltraHDR JPEG with embedded gain map. Alpha is discarded.
#[cfg(feature = "jpeg-ultrahdr")]
pub(crate) fn encode_ultrahdr_rgba_f32(
    img: ImgRef<Rgba<f32>>,
    quality: Option<f32>,
    gainmap_quality: Option<f32>,
    _metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    _limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    use zenjpeg::ultrahdr::{
        GainMapConfig, ToneMapConfig, UhdrColorGamut, UhdrColorTransfer, UhdrPixelFormat,
        UhdrRawImage, encode_ultrahdr,
    };

    let stop_token = crate::limits::stop_or_default(stop);
    let width = img.width() as u32;
    let height = img.height() as u32;

    // Convert ImgRef<Rgba<f32>> to RawImage (RGBA f32)
    let (buf, _, _) = img.to_contiguous_buf();
    let rgba_bytes: &[u8] = bytemuck::cast_slice(buf.as_ref());

    let hdr = UhdrRawImage::from_data(
        width,
        height,
        UhdrPixelFormat::Rgba32F,
        UhdrColorGamut::Bt709,
        UhdrColorTransfer::Linear,
        rgba_bytes.to_vec(),
    )
    .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?;

    let enc = build_encoding(quality, codec_config);
    let gm_quality = gainmap_quality.unwrap_or(75.0);

    let jpeg_data = encode_ultrahdr(
        &hdr,
        &GainMapConfig::default(),
        &ToneMapConfig::default(),
        enc.inner(),
        gm_quality,
        stop_token,
    )
    .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?;

    Ok(EncodeOutput::new(jpeg_data, ImageFormat::Jpeg))
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
