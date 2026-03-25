//! Image metadata probing without full decode.

pub use zencodec::ImageInfo;

use crate::error::Result;
use crate::{AllowedFormats, CodecError, ImageFormat};
use whereat::at;

/// Detect image format from magic bytes using the common format registry.
///
/// RAW/DNG detection is attempted before common formats for TIFF-based files,
/// since DNG files share TIFF magic bytes but should be dispatched to the
/// RAW decoder, not the TIFF handler.
pub(crate) fn detect_format(data: &[u8]) -> Option<ImageFormat> {
    // Try RAW/DNG before common formats — DNG shares TIFF magic bytes,
    // and we want the more specific RAW match to take priority.
    #[cfg(feature = "raw-decode")]
    if let Some(fmt) = crate::codecs::raw::detect_raw_format(data) {
        return Some(fmt);
    }
    // Try common formats (JPEG, PNG, GIF, WebP, TIFF, etc.)
    if let Some(fmt) = zencodec::ImageFormatRegistry::common().detect(data) {
        return Some(fmt);
    }
    None
}

/// Probe image metadata without decoding pixels.
///
/// Uses format auto-detection and dispatches to the appropriate codec's probe.
/// All compiled-in codecs are attempted.
pub fn from_bytes(data: &[u8]) -> Result<ImageInfo> {
    from_bytes_with_registry(data, &AllowedFormats::all())
}

/// Probe image metadata with a specific registry.
///
/// Only formats enabled in the registry will be attempted.
pub fn from_bytes_with_registry(data: &[u8], registry: &AllowedFormats) -> Result<ImageInfo> {
    let format = detect_format(data).ok_or_else(|| at!(CodecError::UnrecognizedFormat))?;

    if !registry.can_decode(format) {
        return Err(at!(CodecError::DisabledFormat(format)));
    }

    probe_format_full(data, format)
}

/// Probe with a known format (skips auto-detection).
pub fn from_bytes_format(data: &[u8], format: ImageFormat) -> Result<ImageInfo> {
    probe_format_full(data, format)
}

/// Probe format — used by DecodeRequest::probe().
pub(crate) fn probe_format(data: &[u8], format: ImageFormat) -> Result<ImageInfo> {
    probe_format_full(data, format)
}

/// Compute actual decode output dimensions for an image.
///
/// Unlike [`from_bytes`] which returns stored file dimensions, this applies
/// codec-specific config transforms (JPEG DctScale, auto_orient) to predict
/// what [`DecodeRequest::decode`](crate::DecodeRequest::decode) will actually produce.
///
/// For codecs without dimension transforms, this is equivalent to `from_bytes`.
pub fn decode_info(data: &[u8]) -> Result<ImageInfo> {
    decode_info_with_config(data, None)
}

/// Compute actual decode output dimensions with codec-specific config.
pub fn decode_info_with_config(
    data: &[u8],
    codec_config: Option<&crate::config::CodecConfig>,
) -> Result<ImageInfo> {
    let format = detect_format(data).ok_or_else(|| at!(CodecError::UnrecognizedFormat))?;
    decode_info_format(data, format, codec_config)
}

/// Compute actual decode output dimensions for a known format.
fn decode_info_format(
    data: &[u8],
    format: ImageFormat,
    codec_config: Option<&crate::config::CodecConfig>,
) -> Result<ImageInfo> {
    match format {
        // JPEG needs codec support for DctScale dimension transforms
        #[cfg(feature = "jpeg")]
        ImageFormat::Jpeg => {
            if data.len() > PROBE_CAP {
                if let Ok(info) = crate::codecs::jpeg::decode_info(&data[..PROBE_CAP], codec_config)
                {
                    return Ok(info);
                }
                // Capped probe failed -- retry with full data and warn
                let mut info = crate::codecs::jpeg::decode_info(data, codec_config)?;
                info.warnings.push(alloc::format!(
                    "metadata located beyond {}KB fast-probe cap; required full scan",
                    PROBE_CAP / 1024
                ));
                return Ok(info);
            }
            crate::codecs::jpeg::decode_info(data, codec_config)
        }

        // Other codecs: decode_info == probe (no dimension transforms)
        _ => probe_format_full(data, format),
    }
}

/// Maximum bytes to pass to codec probes on the initial attempt.
///
/// Codec probes only need file headers and metadata markers, not pixel data.
/// Capping the input avoids scanning megabytes of entropy-coded data in large
/// files. If the probe fails with capped data (e.g., a JPEG with a very large
/// ICC profile pushing SOF past the cap), we retry with the full data.
///
/// Per-format header sizes (typical):
/// - PNG: IHDR at byte 8, iCCP/tEXt before IDAT -- usually <10KB
/// - GIF: header is 13 bytes, global color table <1KB
/// - WebP: VP8X at byte 12, metadata chunks -- usually <10KB
/// - JPEG: all metadata markers before SOS -- usually <100KB, rarely >256KB
/// - AVIF: ftyp+meta boxes -- usually <20KB
/// - JXL: frame header -- usually <4KB
///
/// 256KB handles >99.9% of real-world files on the first attempt.
const PROBE_CAP: usize = 256 * 1024;

/// Dispatch to format-specific codec probe.
///
/// Tries with capped input first to avoid scanning pixel data in large files.
/// Falls back to full data if the capped probe fails.
fn probe_format_full(data: &[u8], format: ImageFormat) -> Result<ImageInfo> {
    if data.len() > PROBE_CAP {
        if let Ok(info) = probe_codec(&data[..PROBE_CAP], format) {
            return Ok(info);
        }
        // Capped probe failed -- retry with full data and warn
        let mut info = probe_codec(data, format)?;
        info.warnings.push(alloc::format!(
            "metadata located beyond {}KB fast-probe cap; required full scan",
            PROBE_CAP / 1024
        ));
        return Ok(info);
    }
    probe_codec(data, format)
}

/// Set gain map presence based on format capabilities.
///
/// Called after codec-specific probe to fill in `ImageInfo.gain_map` when
/// the underlying codec doesn't populate it. Also keeps
/// `supplements.gain_map` in sync.
fn finalize_gain_map_presence(info: &mut ImageInfo) {
    // If the codec already set a non-Unknown value, respect it
    if !info.gain_map.is_unknown() {
        // Sync supplements flag with the resolved gain_map state
        info.supplements.gain_map = info.gain_map.is_present();
        return;
    }

    match info.format {
        // Formats that never contain gain maps
        ImageFormat::Png
        | ImageFormat::WebP
        | ImageFormat::Gif
        | ImageFormat::Bmp
        | ImageFormat::Pnm
        | ImageFormat::Farbfeld
        | ImageFormat::Tiff => {
            info.gain_map = zencodec::gainmap::GainMapPresence::Absent;
            info.supplements.gain_map = false;
        }
        // Formats that CAN contain gain maps — leave Unknown until decode
        // (JPEG, AVIF, JXL, HEIC, RAW need full decode or deeper parsing)
        _ => {}
    }
}

/// Dispatch to the format-specific codec probe (requires codec feature).
fn probe_codec(data: &[u8], format: ImageFormat) -> Result<ImageInfo> {
    let mut info = match format {
        #[cfg(feature = "jpeg")]
        ImageFormat::Jpeg => crate::codecs::jpeg::probe(data)?,
        #[cfg(not(feature = "jpeg"))]
        ImageFormat::Jpeg => return Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "webp")]
        ImageFormat::WebP => crate::codecs::webp::probe(data)?,
        #[cfg(not(feature = "webp"))]
        ImageFormat::WebP => return Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "gif")]
        ImageFormat::Gif => crate::codecs::gif::probe(data)?,
        #[cfg(not(feature = "gif"))]
        ImageFormat::Gif => return Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "png")]
        ImageFormat::Png => crate::codecs::png::probe(data)?,
        #[cfg(not(feature = "png"))]
        ImageFormat::Png => return Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "avif-decode")]
        ImageFormat::Avif => crate::codecs::avif_dec::probe(data)?,
        #[cfg(not(feature = "avif-decode"))]
        ImageFormat::Avif => return Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "jxl-decode")]
        ImageFormat::Jxl => crate::codecs::jxl_dec::probe(data)?,
        #[cfg(not(feature = "jxl-decode"))]
        ImageFormat::Jxl => return Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "heic-decode")]
        ImageFormat::Heic => crate::codecs::heic::probe(data)?,
        #[cfg(not(feature = "heic-decode"))]
        ImageFormat::Heic => return Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "bitmaps")]
        ImageFormat::Pnm => crate::codecs::pnm::probe(data)?,
        #[cfg(not(feature = "bitmaps"))]
        ImageFormat::Pnm => return Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "bitmaps-bmp")]
        ImageFormat::Bmp => crate::codecs::bmp::probe(data)?,
        #[cfg(not(feature = "bitmaps-bmp"))]
        ImageFormat::Bmp => return Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "bitmaps")]
        ImageFormat::Farbfeld => crate::codecs::farbfeld::probe(data)?,
        #[cfg(not(feature = "bitmaps"))]
        ImageFormat::Farbfeld => return Err(at!(CodecError::UnsupportedFormat(format))),

        #[cfg(feature = "tiff")]
        ImageFormat::Tiff => crate::codecs::tiff::probe(data)?,
        #[cfg(not(feature = "tiff"))]
        ImageFormat::Tiff => return Err(at!(CodecError::UnsupportedFormat(format))),

        // RAW/DNG: Custom format from zenraw
        #[cfg(feature = "raw-decode")]
        ImageFormat::Custom(def) if def.name == "dng" || def.name == "raw" => {
            crate::codecs::raw::probe(data)?
        }

        _ => return Err(at!(CodecError::UnsupportedFormat(format))),
    };
    finalize_gain_map_presence(&mut info);
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrecognized_format() {
        let data = b"not an image";
        let result = from_bytes(data);
        assert!(matches!(
            result.as_ref().map_err(|e| e.error()),
            Err(CodecError::UnrecognizedFormat)
        ));
    }

    #[test]
    fn disabled_format() {
        let jpeg_data = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        let registry = AllowedFormats::none();

        let result = from_bytes_with_registry(&jpeg_data, &registry);
        assert!(matches!(
            result.as_ref().map_err(|e| e.error()),
            Err(CodecError::DisabledFormat(_))
        ));
    }
}
