//! GIF codec adapter using zengif.
//!
//! GIF decode uses the native API (for frame counting).
//! GIF encode uses the trait interface where possible.

extern crate std;

use crate::config::CodecConfig;
use crate::pixel::{Gray, ImgRef, ImgVec, Rgb, Rgba};
use crate::{
    CodecError, DecodeOutput, EncodeJob, EncodeOutput, EncoderConfig, ImageFormat,
    ImageInfo, Limits, Stop,
};
use zencodec_types::{PixelBuffer, PixelDescriptor};
use zencodec_types::{EncodeGray8, EncodeGrayF32, EncodeRgbF32, EncodeRgbaF32, PixelSlice};

/// Create a default GIF encoder config with the best available quantizer.
fn default_encoder_config() -> zengif::EncoderConfig {
    let config = zengif::EncoderConfig::new();
    #[cfg(any(feature = "gif-quantizr", feature = "gif-imagequant"))]
    let config = config.quantizer(zengif::Quantizer::auto());
    config
}

/// Probe GIF metadata without decoding pixels.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo, CodecError> {
    zengif::GifDecoderConfig::new()
        .probe_header(data)
        .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))
}

/// Convert zencodecs Limits to zengif Limits.
fn to_gif_limits(limits: Option<&Limits>) -> zengif::Limits {
    let mut gif_limits = zengif::Limits::default();
    if let Some(lim) = limits {
        if let Some(max_w) = lim.max_width {
            gif_limits.max_width = Some(max_w.min(u16::MAX as u64) as u16);
        }
        if let Some(max_h) = lim.max_height {
            gif_limits.max_height = Some(max_h.min(u16::MAX as u64) as u16);
        }
        if let Some(max_px) = lim.max_pixels {
            gif_limits.max_total_pixels = Some(max_px);
        }
        if let Some(max_mem) = lim.max_memory_bytes {
            gif_limits.max_memory = Some(max_mem);
        }
    }
    gif_limits
}

/// Decode GIF to pixels (first frame only).
///
/// Uses native API for frame counting (iterates all frames to determine count).
pub(crate) fn decode(
    data: &[u8],
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<DecodeOutput, CodecError> {
    let stop = crate::limits::stop_or_default(stop);
    let gif_limits = to_gif_limits(limits);

    let cursor = std::io::Cursor::new(data);
    let mut decoder = zengif::Decoder::new(cursor, gif_limits, stop)
        .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))?;

    let metadata = decoder.metadata().clone();

    let frame = decoder
        .next_frame()
        .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))?
        .ok_or_else(|| CodecError::InvalidInput("GIF has no frames".into()))?;

    let width = metadata.width as usize;
    let height = metadata.height as usize;

    let rgba_pixels: alloc::vec::Vec<Rgba<u8>> = frame.pixels.into_iter().map(Rgba::from).collect();

    let img = ImgVec::new(rgba_pixels, width, height);

    // Count remaining frames to determine animation status
    let mut frame_count: u32 = 1;
    while decoder
        .next_frame()
        .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))?
        .is_some()
    {
        frame_count += 1;
    }

    let buf = PixelBuffer::from_imgvec(img).with_descriptor(PixelDescriptor::RGBA8_SRGB);
    Ok(DecodeOutput::new(
        buf.into(),
        ImageInfo::new(width as u32, height as u32, ImageFormat::Gif)
            .with_alpha(true)
            .with_animation(frame_count > 1)
            .with_frame_count(frame_count),
    ))
}

/// Pre-flight memory check for GIF encoding (4 bytes/pixel for RGBA).
fn check_gif_memory_limit(w: u32, h: u32, limits: Option<&Limits>) -> Result<(), CodecError> {
    if let Some(lim) = limits {
        if let Some(max_mem) = lim.max_memory_bytes {
            let estimated = w as u64 * h as u64 * 4;
            if estimated > max_mem {
                return Err(CodecError::LimitExceeded(alloc::format!(
                    "memory {estimated} bytes exceeds limit {max_mem}"
                )));
            }
        }
    }
    Ok(())
}

/// Encode RGB8 pixels to GIF (single frame).
pub(crate) fn encode_rgb8(
    img: ImgRef<Rgb<u8>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let width: u16 = (img.width() as u32)
        .try_into()
        .map_err(|_| CodecError::InvalidInput("width exceeds GIF maximum (65535)".into()))?;
    let height: u16 = (img.height() as u32)
        .try_into()
        .map_err(|_| CodecError::InvalidInput("height exceeds GIF maximum (65535)".into()))?;

    check_gif_memory_limit(width as u32, height as u32, limits)?;

    let (buf, _, _) = img.to_contiguous_buf();
    let rgba_bytes: alloc::vec::Vec<u8> = buf.iter().flat_map(|p| [p.r, p.g, p.b, 255u8]).collect();

    let frame = zengif::FrameInput::from_bytes(width, height, 10, &rgba_bytes);

    let config = codec_config
        .and_then(|c| c.gif_encoder.as_ref())
        .map(|c| c.as_ref().clone())
        .unwrap_or_else(default_encoder_config);
    let gif_limits = to_gif_limits(limits);
    let stop = crate::limits::stop_or_default(stop);

    let gif_data = zengif::encode_gif(alloc::vec![frame], width, height, config, gif_limits, stop)
        .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))?;

    Ok(EncodeOutput::new(gif_data, ImageFormat::Gif))
}

/// Encode RGBA8 pixels to GIF (single frame).
pub(crate) fn encode_rgba8(
    img: ImgRef<Rgba<u8>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let width: u16 = (img.width() as u32)
        .try_into()
        .map_err(|_| CodecError::InvalidInput("width exceeds GIF maximum (65535)".into()))?;
    let height: u16 = (img.height() as u32)
        .try_into()
        .map_err(|_| CodecError::InvalidInput("height exceeds GIF maximum (65535)".into()))?;

    check_gif_memory_limit(width as u32, height as u32, limits)?;

    let (buf, _, _) = img.to_contiguous_buf();
    let rgba_bytes: &[u8] = bytemuck::cast_slice(buf.as_ref());

    let frame = zengif::FrameInput::from_bytes(width, height, 10, rgba_bytes);

    let config = codec_config
        .and_then(|c| c.gif_encoder.as_ref())
        .map(|c| c.as_ref().clone())
        .unwrap_or_else(default_encoder_config);
    let gif_limits = to_gif_limits(limits);
    let stop = crate::limits::stop_or_default(stop);

    let gif_data = zengif::encode_gif(alloc::vec![frame], width, height, config, gif_limits, stop)
        .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))?;

    Ok(EncodeOutput::new(gif_data, ImageFormat::Gif))
}

/// Build a GifEncoderConfig from codec config.
fn build_gif_encoding(codec_config: Option<&CodecConfig>) -> zengif::GifEncoderConfig {
    let mut enc = zengif::GifEncoderConfig::new();
    if let Some(cfg) = codec_config.and_then(|c| c.gif_encoder.as_ref()) {
        *enc.inner_mut() = cfg.as_ref().clone();
    }
    enc
}

/// Encode Gray8 pixels to GIF (single frame).
pub(crate) fn encode_gray8(
    img: ImgRef<Gray<u8>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_gif_encoding(codec_config);
    let mut job = enc.job();
    if let Some(lim) = limits {
        job = job.with_limits(crate::limits::to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.encoder()
        .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))?
        .encode_gray8(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))
}

/// Encode linear RGB f32 pixels to GIF (single frame).
pub(crate) fn encode_rgb_f32(
    img: ImgRef<Rgb<f32>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_gif_encoding(codec_config);
    let mut job = enc.job();
    if let Some(lim) = limits {
        job = job.with_limits(crate::limits::to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.encoder()
        .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))?
        .encode_rgb_f32(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))
}

/// Encode linear RGBA f32 pixels to GIF (single frame).
pub(crate) fn encode_rgba_f32(
    img: ImgRef<Rgba<f32>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_gif_encoding(codec_config);
    let mut job = enc.job();
    if let Some(lim) = limits {
        job = job.with_limits(crate::limits::to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.encoder()
        .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))?
        .encode_rgba_f32(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))
}

/// Encode linear grayscale f32 pixels to GIF (single frame).
pub(crate) fn encode_gray_f32(
    img: ImgRef<Gray<f32>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_gif_encoding(codec_config);
    let mut job = enc.job();
    if let Some(lim) = limits {
        job = job.with_limits(crate::limits::to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.encoder()
        .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))?
        .encode_gray_f32(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Gif, e))
}
