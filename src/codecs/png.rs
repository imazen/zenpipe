//! PNG codec adapter — delegates to zenpng via trait interface.

use crate::config::CodecConfig;
use crate::limits::to_resource_limits;
use crate::pixel::{ImgRef, Rgb, Rgba};
use crate::{
    CodecError, DecodeJob, DecodeOutput, DecoderConfig, EncodeJob, EncodeOutput, EncoderConfig,
    ImageFormat, ImageInfo, Limits, MetadataView, Stop,
};
use alloc::boxed::Box;
use zencodec_types::{
    Decode, EncodeGray8, EncodeGrayF32, EncodeRgb8, EncodeRgbF32, EncodeRgba8, EncodeRgbaF32,
    PixelSlice,
};

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
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))?
        .decode(data, &[])
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))?
        .encode_rgb8(PixelSlice::from(img))
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))?
        .encode_rgba8(PixelSlice::from(img))
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))?
        .encode_gray8(PixelSlice::from(img))
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))?
        .encode_rgb_f32(PixelSlice::from(img))
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))?
        .encode_rgba_f32(PixelSlice::from(img))
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
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))?
        .encode_gray_f32(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Png, e))
}

// ═══════════════════════════════════════════════════════════════════════
// DynEncoder implementation
// ═══════════════════════════════════════════════════════════════════════

use crate::dispatch::{DynEncoder, EncodeParams};
use zencodec_types::PixelDescriptor;

static PNG_SUPPORTED: &[PixelDescriptor] = &[
    PixelDescriptor::RGB8_SRGB,
    PixelDescriptor::RGBA8_SRGB,
    PixelDescriptor::GRAY8_SRGB,
    PixelDescriptor::RGBF32_LINEAR,
    PixelDescriptor::RGBAF32_LINEAR,
    PixelDescriptor::GRAYF32_LINEAR,
];

pub(crate) struct PngDynEncoder<'a> {
    metadata: Option<&'a MetadataView<'a>>,
    codec_config: Option<&'a CodecConfig>,
    limits: Option<&'a Limits>,
    stop: Option<&'a dyn Stop>,
}

pub(crate) fn build_dyn_encoder(params: EncodeParams<'_>) -> PngDynEncoder<'_> {
    PngDynEncoder {
        metadata: params.metadata,
        codec_config: params.codec_config,
        limits: params.limits,
        stop: params.stop,
    }
}

impl DynEncoder for PngDynEncoder<'_> {
    fn format(&self) -> ImageFormat {
        ImageFormat::Png
    }

    fn supported_descriptors(&self) -> &'static [PixelDescriptor] {
        PNG_SUPPORTED
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
                    self.metadata,
                    self.codec_config,
                    self.limits,
                    self.stop,
                )
            }
            _ => Err(CodecError::InvalidInput(alloc::format!(
                "PNG encoder does not support pixel format: {}",
                descriptor
            ))),
        }
    }
}
