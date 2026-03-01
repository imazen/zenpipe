//! JXL encode adapter — delegates to zenjxl via trait interface.

use crate::config::CodecConfig;
use crate::limits::to_resource_limits;
use crate::pixel::{Bgra, ImgRef, Rgb, Rgba};
use crate::{
    CodecError, EncodeJob, EncodeOutput, EncoderConfig, ImageFormat, Limits, MetadataView, Stop,
};
use alloc::boxed::Box;
use zencodec_types::{
    EncodeGray8, EncodeGrayF32, EncodeRgb8, EncodeRgbF32, EncodeRgba8, EncodeRgbaF32, PixelSlice,
};

/// Map 0-100 quality percentage to butteraugli distance.
fn percent_to_distance(quality: f32) -> f32 {
    let q = quality.clamp(0.0, 99.9) as u32;
    if q >= 90 {
        (100 - q) as f32 / 10.0
    } else if q >= 70 {
        1.0 + (90 - q) as f32 / 20.0
    } else {
        2.0 + (70 - q) as f32 / 10.0
    }
}

/// Build a JxlEncoderConfig from quality.
fn build_encoding(quality: Option<f32>) -> zenjxl::JxlEncoderConfig {
    let distance = quality.map_or(1.0, percent_to_distance);
    zenjxl::JxlEncoderConfig::lossy(distance)
}

/// Encode RGB8 pixels to JXL.
pub(crate) fn encode_rgb8(
    img: ImgRef<Rgb<u8>>,
    quality: Option<f32>,
    metadata: Option<&MetadataView<'_>>,
    _codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jxl, e))?
        .encode_rgb8(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Jxl, e))
}

/// Encode RGBA8 pixels to JXL.
pub(crate) fn encode_rgba8(
    img: ImgRef<Rgba<u8>>,
    quality: Option<f32>,
    metadata: Option<&MetadataView<'_>>,
    _codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jxl, e))?
        .encode_rgba8(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Jxl, e))
}

/// Encode Gray8 pixels to JXL.
pub(crate) fn encode_gray8(
    img: ImgRef<crate::pixel::Gray<u8>>,
    quality: Option<f32>,
    metadata: Option<&MetadataView<'_>>,
    _codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jxl, e))?
        .encode_gray8(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Jxl, e))
}

/// Encode linear RGB f32 pixels to JXL.
pub(crate) fn encode_rgb_f32(
    img: ImgRef<Rgb<f32>>,
    quality: Option<f32>,
    metadata: Option<&MetadataView<'_>>,
    _codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jxl, e))?
        .encode_rgb_f32(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Jxl, e))
}

/// Encode linear RGBA f32 pixels to JXL.
pub(crate) fn encode_rgba_f32(
    img: ImgRef<Rgba<f32>>,
    quality: Option<f32>,
    metadata: Option<&MetadataView<'_>>,
    _codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jxl, e))?
        .encode_rgba_f32(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Jxl, e))
}

/// Encode linear grayscale f32 pixels to JXL.
pub(crate) fn encode_gray_f32(
    img: ImgRef<crate::pixel::Gray<f32>>,
    quality: Option<f32>,
    metadata: Option<&MetadataView<'_>>,
    _codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality);
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Jxl, e))?
        .encode_gray_f32(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Jxl, e))
}

/// Encode BGRA8 pixels to JXL (native BGRA path).
pub(crate) fn encode_bgra8(
    img: ImgRef<Bgra<u8>>,
    quality: Option<f32>,
    _metadata: Option<&MetadataView<'_>>,
    _codec_config: Option<&CodecConfig>,
    _limits: Option<&Limits>,
    _stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let distance = quality.map_or(1.0, percent_to_distance);
    let config = zenjxl::LossyConfig::new(distance);
    let data = zenjxl::encode_bgra8(img, &config)
        .map_err(|e| CodecError::from_codec(ImageFormat::Jxl, e))?;
    Ok(EncodeOutput::new(data, ImageFormat::Jxl))
}

// ═══════════════════════════════════════════════════════════════════════
// DynEncoder implementation
// ═══════════════════════════════════════════════════════════════════════

use crate::dispatch::{DynEncoder, EncodeParams};
use zencodec_types::PixelDescriptor;

static JXL_SUPPORTED: &[PixelDescriptor] = &[
    PixelDescriptor::RGB8_SRGB,
    PixelDescriptor::RGBA8_SRGB,
    PixelDescriptor::GRAY8_SRGB,
    PixelDescriptor::BGRA8_SRGB,
    PixelDescriptor::RGBF32_LINEAR,
    PixelDescriptor::RGBAF32_LINEAR,
    PixelDescriptor::GRAYF32_LINEAR,
];

pub(crate) struct JxlDynEncoder<'a> {
    quality: Option<f32>,
    metadata: Option<&'a MetadataView<'a>>,
    codec_config: Option<&'a CodecConfig>,
    limits: Option<&'a Limits>,
    stop: Option<&'a dyn Stop>,
}

pub(crate) fn build_dyn_encoder(params: EncodeParams<'_>) -> JxlDynEncoder<'_> {
    JxlDynEncoder {
        quality: params.quality,
        metadata: params.metadata,
        codec_config: params.codec_config,
        limits: params.limits,
        stop: params.stop,
    }
}

impl DynEncoder for JxlDynEncoder<'_> {
    fn format(&self) -> ImageFormat {
        ImageFormat::Jxl
    }

    fn supported_descriptors(&self) -> &'static [PixelDescriptor] {
        JXL_SUPPORTED
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
                encode_bgra8(
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
                "JXL encoder does not support pixel format: {}",
                descriptor
            ))),
        }
    }
}
