//! Image metadata probing without full decode.

pub use zc::ImageInfo;

use crate::{CodecError, CodecRegistry, ImageFormat};

/// Detect image format from magic bytes using the common format registry.
pub(crate) fn detect_format(data: &[u8]) -> Option<ImageFormat> {
    zc::ImageFormatRegistry::common().detect(data)
}

/// Probe partial image data for metadata without decoding pixels.
///
/// Unlike [`from_bytes`], this works with truncated data (e.g., first N bytes
/// from an HTTP range request). Missing dimensions result in `None` fields
/// rather than an error. All compiled-in codecs are attempted.
///
/// Uses pure byte parsing — no codec crate dependencies. Works even if a
/// codec feature isn't compiled in.
pub fn probe(data: &[u8]) -> Result<crate::ProbeResult, CodecError> {
    let format = detect_format(data).ok_or(CodecError::UnrecognizedFormat)?;
    Ok(crate::ProbeResult::for_format(data, format))
}

/// Probe partial image data with a specific registry.
///
/// Only formats enabled in the registry will be accepted.
pub fn probe_with_registry(
    data: &[u8],
    registry: &CodecRegistry,
) -> Result<crate::ProbeResult, CodecError> {
    let format = detect_format(data).ok_or(CodecError::UnrecognizedFormat)?;
    if !registry.can_decode(format) {
        return Err(CodecError::DisabledFormat(format));
    }
    Ok(crate::ProbeResult::for_format(data, format))
}

/// Probe partial image data for a known format (skips auto-detection).
///
/// Never fails — insufficient data results in `None` fields.
pub fn probe_format(data: &[u8], format: ImageFormat) -> crate::ProbeResult {
    crate::ProbeResult::for_format(data, format)
}

/// Probe image metadata without decoding pixels.
///
/// Uses format auto-detection and dispatches to the appropriate codec's probe.
/// All compiled-in codecs are attempted.
pub fn from_bytes(data: &[u8]) -> Result<ImageInfo, CodecError> {
    from_bytes_with_registry(data, &CodecRegistry::all())
}

/// Probe image metadata with a specific registry.
///
/// Only formats enabled in the registry will be attempted.
pub fn from_bytes_with_registry(
    data: &[u8],
    registry: &CodecRegistry,
) -> Result<ImageInfo, CodecError> {
    let format = detect_format(data).ok_or(CodecError::UnrecognizedFormat)?;

    if !registry.can_decode(format) {
        return Err(CodecError::DisabledFormat(format));
    }

    probe_format_full(data, format)
}

/// Probe with a known format (skips auto-detection).
pub fn from_bytes_format(data: &[u8], format: ImageFormat) -> Result<ImageInfo, CodecError> {
    probe_format_full(data, format)
}

/// Compute actual decode output dimensions for an image.
///
/// Unlike [`from_bytes`] which returns stored file dimensions, this applies
/// codec-specific config transforms (JPEG DctScale, auto_orient) to predict
/// what [`DecodeRequest::decode`](crate::DecodeRequest::decode) will actually produce.
///
/// For codecs without dimension transforms, this is equivalent to `from_bytes`.
pub fn decode_info(data: &[u8]) -> Result<ImageInfo, CodecError> {
    decode_info_with_config(data, None)
}

/// Compute actual decode output dimensions with codec-specific config.
pub fn decode_info_with_config(
    data: &[u8],
    codec_config: Option<&crate::config::CodecConfig>,
) -> Result<ImageInfo, CodecError> {
    let format = detect_format(data).ok_or(CodecError::UnrecognizedFormat)?;
    decode_info_format(data, format, codec_config)
}

/// Compute actual decode output dimensions for a known format.
fn decode_info_format(
    data: &[u8],
    format: ImageFormat,
    codec_config: Option<&crate::config::CodecConfig>,
) -> Result<ImageInfo, CodecError> {
    match format {
        // JPEG needs codec support for DctScale dimension transforms
        #[cfg(feature = "jpeg")]
        ImageFormat::Jpeg => {
            if data.len() > PROBE_CAP {
                if let Ok(info) = crate::codecs::jpeg::decode_info(&data[..PROBE_CAP], codec_config)
                {
                    return Ok(info);
                }
                // Capped probe failed — retry with full data and warn
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
/// - PNG: IHDR at byte 8, iCCP/tEXt before IDAT — usually <10KB
/// - GIF: header is 13 bytes, global color table <1KB
/// - WebP: VP8X at byte 12, metadata chunks — usually <10KB
/// - JPEG: all metadata markers before SOS — usually <100KB, rarely >256KB
/// - AVIF: ftyp+meta boxes — usually <20KB
/// - JXL: frame header — usually <4KB
///
/// 256KB handles >99.9% of real-world files on the first attempt.
const PROBE_CAP: usize = 256 * 1024;

/// Dispatch to format-specific codec probe.
///
/// Tries with capped input first to avoid scanning pixel data in large files.
/// Falls back to full data if the capped probe fails.
fn probe_format_full(data: &[u8], format: ImageFormat) -> Result<ImageInfo, CodecError> {
    if data.len() > PROBE_CAP {
        if let Ok(info) = probe_codec(&data[..PROBE_CAP], format) {
            return Ok(info);
        }
        // Capped probe failed — retry with full data and warn
        let mut info = probe_codec(data, format)?;
        info.warnings.push(alloc::format!(
            "metadata located beyond {}KB fast-probe cap; required full scan",
            PROBE_CAP / 1024
        ));
        return Ok(info);
    }
    probe_codec(data, format)
}

/// Dispatch to the format-specific codec probe (requires codec feature).
fn probe_codec(data: &[u8], format: ImageFormat) -> Result<ImageInfo, CodecError> {
    match format {
        #[cfg(feature = "jpeg")]
        ImageFormat::Jpeg => crate::codecs::jpeg::probe(data),
        #[cfg(not(feature = "jpeg"))]
        ImageFormat::Jpeg => Err(CodecError::UnsupportedFormat(format)),

        #[cfg(feature = "webp")]
        ImageFormat::WebP => crate::codecs::webp::probe(data),
        #[cfg(not(feature = "webp"))]
        ImageFormat::WebP => Err(CodecError::UnsupportedFormat(format)),

        #[cfg(feature = "gif")]
        ImageFormat::Gif => crate::codecs::gif::probe(data),
        #[cfg(not(feature = "gif"))]
        ImageFormat::Gif => Err(CodecError::UnsupportedFormat(format)),

        #[cfg(feature = "png")]
        ImageFormat::Png => crate::codecs::png::probe(data),
        #[cfg(not(feature = "png"))]
        ImageFormat::Png => Err(CodecError::UnsupportedFormat(format)),

        #[cfg(feature = "avif-decode")]
        ImageFormat::Avif => crate::codecs::avif_dec::probe(data),
        #[cfg(not(feature = "avif-decode"))]
        ImageFormat::Avif => Err(CodecError::UnsupportedFormat(format)),

        #[cfg(feature = "jxl-decode")]
        ImageFormat::Jxl => crate::codecs::jxl_dec::probe(data),
        #[cfg(not(feature = "jxl-decode"))]
        ImageFormat::Jxl => Err(CodecError::UnsupportedFormat(format)),

        #[cfg(feature = "heic-decode")]
        ImageFormat::Heic => crate::codecs::heic::probe(data),
        #[cfg(not(feature = "heic-decode"))]
        ImageFormat::Heic => Err(CodecError::UnsupportedFormat(format)),

        _ => Err(CodecError::UnsupportedFormat(format)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrecognized_format() {
        let data = b"not an image";
        let result = from_bytes(data);
        assert!(matches!(result, Err(CodecError::UnrecognizedFormat)));
    }

    #[test]
    fn disabled_format() {
        let jpeg_data = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        let registry = CodecRegistry::none();

        let result = from_bytes_with_registry(&jpeg_data, &registry);
        assert!(matches!(result, Err(CodecError::DisabledFormat(_))));
    }
}
