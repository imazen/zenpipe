//! GIF codec adapter using zengif.
//!
//! GIF decode uses the native API (for frame counting).
//! GIF encode uses the trait interface where possible.

extern crate std;

use crate::config::CodecConfig;
use crate::pixel::{ImgVec, Rgba};
use crate::{
    CodecError, DecodeOutput, EncodeJob, EncoderConfig, ImageFormat, ImageInfo, Limits, Stop,
};
use alloc::boxed::Box;
use zenpixels::{PixelBuffer, PixelDescriptor};

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

static GIF_SUPPORTED: &[PixelDescriptor] = &[
    PixelDescriptor::RGBA8_SRGB,
    PixelDescriptor::RGB8_SRGB,
    PixelDescriptor::GRAY8_SRGB,
    PixelDescriptor::RGBF32_LINEAR,
    PixelDescriptor::RGBAF32_LINEAR,
    PixelDescriptor::GRAYF32_LINEAR,
];

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
        supported: GIF_SUPPORTED,
    }
}
