//! JPEG codec adapter using zenjpeg via trait interface.
//!
//! Probe, encode, and decode use the trait interface.
//! UltraHDR decode uses the native API (needs mid-decode extras access).

use alloc::borrow::Cow;

use crate::config::CodecConfig;
use crate::error::Result;
use crate::limits::to_resource_limits;
use crate::{CodecError, DecodeOutput, ImageFormat, ImageInfo, Limits, StopToken};
#[cfg(feature = "jpeg-ultrahdr")]
use crate::{EncodeOutput, Metadata, pixel::ImgRef};
#[cfg(feature = "jpeg-ultrahdr")]
use rgb::{Rgb, Rgba};
use whereat::at;
use zencodec::decode::{Decode, DecodeJob as _, DecoderConfig as _};

/// Probe JPEG metadata without decoding pixels.
///
/// Uses `Permissive` strictness so we can extract dimensions and metadata from
/// structurally damaged files that would fail a full decode.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo> {
    let info = zenjpeg::decoder::DecodeConfig::new()
        .permissive()
        .read_info(data)
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Jpeg, e)))?;
    Ok(jpeg_info_to_image_info(&info))
}

/// Convert zenjpeg's `JpegInfo` to zencodec `ImageInfo`.
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
            ii = ii.with_orientation(zencodec::Orientation::from_exif(orient as u16));
        }
        ii = ii.with_exif(exif.clone());
    }
    if let Some(ref xmp) = info.xmp {
        ii = ii.with_xmp(xmp.as_bytes().to_vec());
    }

    // Detect UltraHDR gain map from XMP metadata.
    // Gain map image dimensions are 0 because they can't be determined at
    // probe time (the gain map is in an MPF secondary image).
    #[cfg(feature = "jpeg-ultrahdr")]
    if let Some(ref xmp) = info.xmp
        && let Ok((metadata, _item_len)) = zenjpeg::ultrahdr::parse_xmp(xmp)
    {
        let params = crate::gainmap::metadata_to_params(&metadata);
        ii.supplements.gain_map = true;
        ii.gain_map = zencodec::gainmap::GainMapPresence::Available(alloc::boxed::Box::new(
            zencodec::gainmap::GainMapInfo::new(params, 0, 0, 0),
        ));
    }

    ii
}

/// Decode JPEG to pixels.
pub(crate) fn decode(
    data: &[u8],
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<StopToken>,
) -> Result<DecodeOutput> {
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
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Jpeg, e)))?
        .decode()
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Jpeg, e)))
}

/// Compute actual output dimensions for JPEG (applies DctScale, auto_orient).
pub(crate) fn decode_info(data: &[u8], _codec_config: Option<&CodecConfig>) -> Result<ImageInfo> {
    // Use probe_full which returns complete metadata
    zenjpeg::JpegDecoderConfig::new()
        .job()
        .probe_full(data)
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Jpeg, e)))
}

/// Build a JpegEncoderConfig from codec config or generic quality.
///
/// Uses `EncoderConfig` trait methods for generic params, with
/// codec_config taking priority for format-specific overrides.
pub(crate) fn build_encoding(
    quality: Option<f32>,
    codec_config: Option<&CodecConfig>,
) -> zenjpeg::JpegEncoderConfig {
    use zencodec::encode::EncoderConfig;

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

use crate::dispatch::{BuiltEncoder, EncodeParams, StreamingEncoder, build_from_config};

pub(crate) fn build_trait_encoder<'a>(params: EncodeParams<'a>) -> BuiltEncoder<'a> {
    build_from_config(|p| build_encoding(p.quality, p.codec_config), params)
}

pub(crate) fn build_streaming(
    params: EncodeParams<'_>,
) -> crate::error::Result<StreamingEncoder> {
    crate::dispatch::build_streaming_from_config(
        |p| build_encoding(p.quality, p.codec_config),
        params,
    )
}

/// Decode UltraHDR JPEG to linear f32 RGBA HDR pixels.
///
/// Encode linear f32 RGB pixels to UltraHDR JPEG.
///
/// Takes HDR content in linear f32 RGB and produces a backward-compatible
/// UltraHDR JPEG with embedded gain map.
#[cfg(feature = "jpeg-ultrahdr")]
pub(crate) fn encode_ultrahdr_rgb_f32(
    img: ImgRef<Rgb<f32>>,
    quality: Option<f32>,
    gainmap_quality: Option<f32>,
    _metadata: Option<&Metadata>,
    codec_config: Option<&CodecConfig>,
    _limits: Option<&Limits>,
    stop: Option<StopToken>,
) -> Result<EncodeOutput> {
    use zenjpeg::ultrahdr::{
        GainMapConfig, ToneMapConfig, UhdrColorGamut, UhdrColorTransfer, UhdrPixelFormat,
        UhdrRawImage, encode_ultrahdr,
    };

    let stop_token = crate::limits::stop_or_default(&stop);
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
    .map_err(|e| at!(CodecError::from_codec(ImageFormat::Jpeg, e)))?;

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
    .map_err(|e| at!(CodecError::from_codec(ImageFormat::Jpeg, e)))?;

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
    _metadata: Option<&Metadata>,
    codec_config: Option<&CodecConfig>,
    _limits: Option<&Limits>,
    stop: Option<StopToken>,
) -> Result<EncodeOutput> {
    use zenjpeg::ultrahdr::{
        GainMapConfig, ToneMapConfig, UhdrColorGamut, UhdrColorTransfer, UhdrPixelFormat,
        UhdrRawImage, encode_ultrahdr,
    };

    let stop_token = crate::limits::stop_or_default(&stop);
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
    .map_err(|e| at!(CodecError::from_codec(ImageFormat::Jpeg, e)))?;

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
    .map_err(|e| at!(CodecError::from_codec(ImageFormat::Jpeg, e)))?;

    Ok(EncodeOutput::new(jpeg_data, ImageFormat::Jpeg))
}

/// Encode SDR pixels + precomputed gain map to UltraHDR JPEG.
#[cfg(feature = "jpeg-ultrahdr")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn encode_with_precomputed_gainmap(
    sdr_pixels: &[u8],
    width: u32,
    height: u32,
    channels: u8,
    quality: Option<f32>,
    codec_config: Option<&CodecConfig>,
    gain_map: &crate::gainmap::GainMap,
    metadata: &crate::gainmap::GainMapMetadata,
    stop: Option<StopToken>,
) -> Result<EncodeOutput> {
    use zenjpeg::ultrahdr::{
        UhdrColorGamut, UhdrColorTransfer, UhdrPixelFormat, UhdrRawImage, encode_with_gainmap,
    };

    let stop_token = crate::limits::stop_or_default(&stop);
    let pixel_format = match channels {
        3 => UhdrPixelFormat::Rgb8,
        4 => UhdrPixelFormat::Rgba8,
        _ => {
            return Err(at!(CodecError::InvalidInput(alloc::format!(
                "unsupported {channels} channels for gain map encode"
            ))));
        }
    };

    let sdr = UhdrRawImage::from_data(
        width,
        height,
        pixel_format,
        UhdrColorGamut::Bt709,
        UhdrColorTransfer::Srgb,
        sdr_pixels.to_vec(),
    )
    .map_err(|e| at!(CodecError::from_codec(ImageFormat::Jpeg, e)))?;

    let enc = build_encoding(quality, codec_config);

    let out = encode_with_gainmap(
        &sdr,
        gain_map,
        metadata,
        enc.inner(),
        quality.unwrap_or(75.0).min(85.0),
        stop_token,
    )
    .map_err(|e| at!(CodecError::from_codec(ImageFormat::Jpeg, e)))?;

    Ok(EncodeOutput::new(out, ImageFormat::Jpeg))
}
