//! AVIF encode adapter using zenavif via trait interface.

use crate::config::CodecConfig;
use crate::limits::to_resource_limits;
use crate::pixel::{ImgRef, Rgb, Rgba};
use crate::{
    CodecError, EncodeJob, EncodeOutput, EncoderConfig, ImageFormat, Limits, MetadataView, Stop,
};
use alloc::boxed::Box;
use zencodec_types::{
    EncodeGray8, EncodeGrayF32, EncodeRgb8, EncodeRgbF32, EncodeRgba8, EncodeRgbaF32, PixelSlice,
};

/// Encode RGB8 pixels to AVIF.
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Avif, e))?
        .encode_rgb8(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Avif, e))
}

/// Encode RGBA8 pixels to AVIF.
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Avif, e))?
        .encode_rgba8(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Avif, e))
}

/// Encode Gray8 pixels to AVIF.
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Avif, e))?
        .encode_gray8(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Avif, e))
}

/// Encode linear RGB f32 pixels to AVIF.
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Avif, e))?
        .encode_rgb_f32(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Avif, e))
}

/// Encode linear RGBA f32 pixels to AVIF.
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Avif, e))?
        .encode_rgba_f32(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Avif, e))
}

/// Encode linear grayscale f32 pixels to AVIF.
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Avif, e))?
        .encode_gray_f32(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Avif, e))
}

// ═══════════════════════════════════════════════════════════════════════
// DynEncoder implementation
// ═══════════════════════════════════════════════════════════════════════

use crate::dispatch::{DynEncoder, EncodeParams};
use zencodec_types::PixelDescriptor;

static AVIF_SUPPORTED: &[PixelDescriptor] = &[
    PixelDescriptor::RGB8_SRGB,
    PixelDescriptor::RGBA8_SRGB,
    PixelDescriptor::GRAY8_SRGB,
    PixelDescriptor::RGBF32_LINEAR,
    PixelDescriptor::RGBAF32_LINEAR,
    PixelDescriptor::GRAYF32_LINEAR,
];

pub(crate) struct AvifDynEncoder<'a> {
    quality: Option<f32>,
    metadata: Option<&'a MetadataView<'a>>,
    codec_config: Option<&'a CodecConfig>,
    limits: Option<&'a Limits>,
    stop: Option<&'a dyn Stop>,
}

pub(crate) fn build_dyn_encoder(params: EncodeParams<'_>) -> AvifDynEncoder<'_> {
    AvifDynEncoder {
        quality: params.quality,
        metadata: params.metadata,
        codec_config: params.codec_config,
        limits: params.limits,
        stop: params.stop,
    }
}

impl DynEncoder for AvifDynEncoder<'_> {
    fn format(&self) -> ImageFormat {
        ImageFormat::Avif
    }

    fn supported_descriptors(&self) -> &'static [PixelDescriptor] {
        AVIF_SUPPORTED
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
                "AVIF encoder does not support pixel format: {}",
                descriptor
            ))),
        }
    }
}

/// Build an AvifEncoderConfig from quality/codec_config.
fn build_encoding(
    quality: Option<f32>,
    codec_config: Option<&CodecConfig>,
) -> zenavif::AvifEncoderConfig {
    let q = codec_config
        .and_then(|c| c.avif_quality)
        .or(quality)
        .unwrap_or(75.0)
        .clamp(0.0, 100.0);

    let speed = codec_config.and_then(|c| c.avif_speed).unwrap_or(4);

    let mut enc = zenavif::AvifEncoderConfig::new()
        .with_quality(q)
        .with_effort_u32(speed as u32);

    if let Some(alpha_q) = codec_config.and_then(|c| c.avif_alpha_quality) {
        enc = enc.with_alpha_quality(alpha_q);
    }

    enc
}
