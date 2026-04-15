//! RAW/DNG decode adapter -- delegates to zenraw via trait interface.

use alloc::borrow::Cow;

use crate::config::CodecConfig;
use crate::error::Result;
use crate::limits::to_resource_limits;
use crate::{CodecError, DecodeOutput, ImageInfo, Limits, StopToken};
use whereat::at;
use zencodec::decode::{Decode, DecodeJob as _, DecoderConfig as _};

/// The ImageFormat for DNG files, re-exported from zenraw.
pub(crate) fn dng_format() -> zencodec::ImageFormat {
    zencodec::ImageFormat::Custom(&zenraw::DNG_FORMAT)
}

/// The ImageFormat for generic RAW files, re-exported from zenraw.
pub(crate) fn raw_format() -> zencodec::ImageFormat {
    zencodec::ImageFormat::Custom(&zenraw::RAW_FORMAT)
}

/// Detect the specific RAW format (DNG vs generic RAW).
pub(crate) fn detect_raw_format(data: &[u8]) -> Option<zencodec::ImageFormat> {
    if !zenraw::is_raw_file(data) {
        return None;
    }
    // zenraw classifies into specific sub-formats
    let classified = zenraw::classify(data);
    match classified {
        zenraw::FileFormat::Dng | zenraw::FileFormat::AppleDng => Some(dng_format()),
        _ if classified.is_raw() => Some(raw_format()),
        _ => None,
    }
}

/// Build a RawDecoderConfig, optionally applying codec config overrides.
fn build_raw_decoder(codec_config: Option<&CodecConfig>) -> zenraw::RawDecoderConfig {
    let mut config = zenraw::RawDecoderConfig::new();
    if let Some(cfg) = codec_config.and_then(|c| c.raw_decoder.as_ref()) {
        config = zenraw::RawDecoderConfig::from_config(cfg.as_ref().clone());
    }
    config
}

/// Map a zenraw error to a CodecError.
fn map_err(
    format: zencodec::ImageFormat,
    e: impl core::error::Error + Send + Sync + 'static,
) -> whereat::At<CodecError> {
    at!(CodecError::Codec {
        format,
        source: alloc::boxed::Box::new(e),
    })
}

/// Probe RAW/DNG metadata without decoding pixels.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo> {
    let format = detect_raw_format(data).unwrap_or_else(raw_format);
    let dec = build_raw_decoder(None);
    let job = dec.job();
    job.probe(data).map_err(|e| map_err(format, e))
}

/// Decode RAW/DNG to pixels.
pub(crate) fn decode(
    data: &[u8],
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<StopToken>,
) -> Result<DecodeOutput> {
    let format = detect_raw_format(data).unwrap_or_else(raw_format);
    let dec = build_raw_decoder(codec_config);
    let mut job = dec.job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job.decoder(Cow::Borrowed(data), &[])
        .map_err(|e| map_err(format, e))?
        .decode()
        .map_err(|e| map_err(format, e))
}

/// Extract the embedded JPEG preview from a DNG/TIFF RAW file.
///
/// DNG files commonly contain a reduced-resolution JPEG preview in IFD0.
/// Apple ProRAW files embed a full-resolution sRGB JPEG rendered by the
/// camera pipeline.
///
/// Returns the raw JPEG bytes, or `None` if no preview is found.
#[cfg(feature = "raw-decode-exif")]
pub(crate) fn extract_preview(data: &[u8]) -> Option<alloc::vec::Vec<u8>> {
    zenraw::exif::extract_dng_preview(data)
}

/// Read structured EXIF+DNG metadata from a RAW file using zenraw's
/// kamadak-exif-based parser.
///
/// This returns zenraw's own `ExifMetadata` which includes DNG-specific
/// fields (color matrices, white balance, calibration illuminants).
#[cfg(feature = "raw-decode-exif")]
pub(crate) fn read_raw_metadata(data: &[u8]) -> Option<zenraw::exif::ExifMetadata> {
    zenraw::exif::read_metadata(data)
}

/// Extract an ISO 21496-1 gain map from a DNG/RAW file.
///
/// Apple APPLEDNG files (iPhone ProRAW) embed a preview JPEG that may contain
/// an HDR gain map via MPF (Multi-Picture Format), the same structure used by
/// UltraHDR JPEGs. This function extracts the gain map JPEG, decodes it to
/// pixels, and converts the XMP metadata to [`crate::gainmap::GainMapMetadata`].
///
/// Returns `None` if:
/// - The file is not an Apple DNG with a gain map
/// - The gain map JPEG cannot be decoded
/// - The `raw-decode-gainmap` feature is not enabled
#[cfg(feature = "raw-decode-gainmap")]
pub(crate) fn extract_gainmap(data: &[u8]) -> Option<crate::gainmap::DecodedGainMap> {
    use crate::gainmap::{DecodedGainMap, GainMap, GainMapMetadata};

    let gm_info = zenraw::apple::extract_gain_map(data)?;

    // Decode the gain map JPEG to pixels using zencodecs' own JPEG decoder.
    let gm_output = crate::codecs::jpeg::decode(&gm_info.jpeg_data, None, None, None, None).ok()?;
    use zenpixels_convert::PixelBufferConvertTypedExt as _;
    let gm_rgb8 = gm_output.into_buffer().to_rgb8();
    let gm_ref = gm_rgb8.as_imgref();
    let gm_w = gm_ref.width() as u32;
    let gm_h = gm_ref.height() as u32;
    let gm_bytes: alloc::vec::Vec<u8> = bytemuck::cast_slice(gm_ref.buf()).to_vec();

    // Determine if the content is effectively grayscale (R==G==B).
    let is_gray = gm_bytes
        .chunks_exact(3)
        .take(100)
        .all(|px| px[0] == px[1] && px[1] == px[2]);
    let (gm_data, channels) = if is_gray {
        let gray: alloc::vec::Vec<u8> = gm_bytes.chunks_exact(3).map(|px| px[0]).collect();
        (gray, 1u8)
    } else {
        (gm_bytes, 3u8)
    };

    // Convert zenraw's GainMapInfo XMP fields to GainMapMetadata.
    //
    // Apple gain maps use two XMP namespaces:
    // 1. HDRGainMap (older Apple style): headroom in stops
    // 2. HDRToneMap (ISO 21496-1 style): full gain map parameters
    //
    // If HDRToneMap fields are present, use them directly.
    // Otherwise, synthesize from Apple's headroom value.
    let metadata = if gm_info.gain_map_max.is_some() || gm_info.alternate_headroom.is_some() {
        // ISO 21496-1 style metadata present — convert to GainMapMetadata.
        // XMP gain/headroom values are already in log2 domain.
        let gain_max = gm_info
            .gain_map_max
            .or(gm_info.alternate_headroom)
            .unwrap_or(1.0);
        let gain_min = gm_info.gain_map_min.unwrap_or(0.0);
        let gamma = gm_info.gamma.unwrap_or(1.0);
        let offset_sdr = gm_info.offset_sdr.unwrap_or(1.0 / 64.0);
        let offset_hdr = gm_info.offset_hdr.unwrap_or(1.0 / 64.0);
        let base_headroom = gm_info.base_headroom.unwrap_or(0.0);
        let alt_headroom = gm_info
            .alternate_headroom
            .or(gm_info.gain_map_max)
            .unwrap_or(1.0);

        {
            let mut m = GainMapMetadata::new();
            m.gain_map_max = [gain_max; 3];
            m.gain_map_min = [gain_min; 3];
            m.gamma = [gamma; 3];
            m.base_offset = [offset_sdr; 3];
            m.alternate_offset = [offset_hdr; 3];
            m.base_hdr_headroom = base_headroom;
            m.alternate_hdr_headroom = alt_headroom;
            m.use_base_color_space = true;
            m
        }
    } else if let Some(headroom) = gm_info.headroom {
        // Apple HDRGainMap style — synthesize ISO 21496-1 metadata from headroom.
        // headroom is max brightness in stops (log2 domain), e.g., 2.89 = ~7.4x boost.
        {
            let mut m = GainMapMetadata::new();
            m.gain_map_max = [headroom; 3];
            m.alternate_hdr_headroom = headroom;
            m
        }
    } else {
        // No metadata at all — use defaults.
        GainMapMetadata::default()
    };

    // Determine source format: AMPF files start with JPEG SOI but are
    // handled here for gain map extraction. Use the RAW format if detected,
    // otherwise fall back to JPEG (for AMPF) or DNG.
    let format = detect_raw_format(data).unwrap_or_else(|| {
        if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8 {
            zencodec::ImageFormat::Jpeg
        } else {
            dng_format()
        }
    });

    Some(DecodedGainMap {
        gain_map: GainMap {
            data: gm_data,
            width: gm_w,
            height: gm_h,
            channels,
        },
        metadata,
        base_is_hdr: gm_info.base_rendition_is_hdr.unwrap_or(false),
        source_format: format,
    })
}
