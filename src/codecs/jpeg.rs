//! JPEG codec adapter using zenjpeg via trait interface.
//!
//! Probe, encode, and decode use the trait interface.
//! UltraHDR decode uses the native API (needs mid-decode extras access).

use alloc::borrow::Cow;

use crate::config::CodecConfig;
use crate::limits::to_resource_limits;
use crate::{CodecError, DecodeOutput, ImageFormat, ImageInfo, Limits, Stop};
#[cfg(feature = "jpeg-ultrahdr")]
use crate::{EncodeOutput, MetadataView, pixel::ImgRef};
use zc::decode::{Decode, DecodeJob as _, DecoderConfig as _};

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
            ii = ii.with_orientation(zc::Orientation::from_exif(orient as u16));
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
#[cfg(feature = "jpeg-ultrahdr")]
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
pub(crate) fn decode(
    data: &[u8],
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<DecodeOutput, CodecError> {
    let mut dec = zenjpeg::JpegDecoderConfig::new();
    if let Some(cfg) = codec_config.and_then(|c| c.jpeg_decoder.as_ref()) {
        *dec.inner_mut() = cfg.as_ref().clone();
    }
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.decoder(Cow::Borrowed(data), &[])
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))?
        .decode()
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))
}

/// Compute actual output dimensions for JPEG (applies DctScale, auto_orient).
pub(crate) fn decode_info(
    data: &[u8],
    _codec_config: Option<&CodecConfig>,
) -> Result<ImageInfo, CodecError> {
    // Use probe_full which returns complete metadata
    zenjpeg::JpegDecoderConfig::new()
        .job()
        .probe_full(data)
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))
}

/// Build a JpegEncoderConfig from codec config or generic quality.
///
/// Uses `EncoderConfig` trait methods for generic params, with
/// codec_config taking priority for format-specific overrides.
fn build_encoding(
    quality: Option<f32>,
    codec_config: Option<&CodecConfig>,
) -> zenjpeg::JpegEncoderConfig {
    use zc::encode::EncoderConfig;

    if let Some(cfg) = codec_config.and_then(|c| c.jpeg_encoder.as_ref()) {
        let mut e = zenjpeg::JpegEncoderConfig::new();
        *e.inner_mut() = cfg.as_ref().clone();
        e
    } else {
        let mut enc = zenjpeg::JpegEncoderConfig::new();
        if let Some(q) = quality {
            enc = enc.with_generic_quality(q);
        }
        enc
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Trait-based encoder dispatch
// ═══════════════════════════════════════════════════════════════════════

use crate::dispatch::{BuiltEncoder, EncodeParams, build_from_config};

pub(crate) fn build_trait_encoder<'a>(params: EncodeParams<'a>) -> BuiltEncoder<'a> {
    build_from_config(|p| build_encoding(p.quality, p.codec_config), params)
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
