//! WASM decode helpers — thin wrappers over zeneditor's native decode.
//!
//! Two paths:
//! - Browser-native formats (JPEG/PNG/WebP/GIF): browser decodes, skip WASM
//! - Non-browser formats (JXL/AVIF/HEIC/BMP/QOI/TGA/HDR): zeneditor decodes
//!
//! The frontend's browser decode is an optional fast preview for initial
//! startup. zeneditor::decode_native() is the authoritative decode path.

/// Re-export for the WASM API layer.
pub use zeneditor::decode::{DecodedImage, NativeDecodeOutput, browser_handles, decode_native};

/// Try to decode image bytes using WASM codecs.
///
/// Returns `None` for browser-native formats (JPEG/PNG/WebP/GIF) — the
/// browser should handle those directly. Returns decoded pixels for
/// formats the browser can't handle.
pub fn try_decode(bytes: &[u8]) -> Option<DecodedImage> {
    let format = zeneditor::decode::detect_format(bytes)?;
    if browser_handles(format) {
        return None;
    }
    zeneditor::decode::try_decode(bytes)
}

/// Check if WASM should decode this format (non-browser formats only).
pub fn try_decode_check(bytes: &[u8]) -> bool {
    zeneditor::decode::detect_format(bytes).is_some_and(|f| !browser_handles(f))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_jpeg() {
        let format = zeneditor::decode::detect_format(
            &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        assert_eq!(format, Some(zencodec::ImageFormat::Jpeg));
    }

    #[test]
    fn detect_jxl_codestream() {
        let format = zeneditor::decode::detect_format(
            &[0xFF, 0x0A, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        assert_eq!(format, Some(zencodec::ImageFormat::Jxl));
    }

    #[test]
    fn detect_avif() {
        let mut bytes = vec![0u8; 12];
        bytes[4..8].copy_from_slice(b"ftyp");
        bytes[8..12].copy_from_slice(b"avif");
        assert_eq!(
            zeneditor::decode::detect_format(&bytes),
            Some(zencodec::ImageFormat::Avif)
        );
    }

    #[test]
    fn detect_heic() {
        let mut bytes = vec![0u8; 12];
        bytes[4..8].copy_from_slice(b"ftyp");
        bytes[8..12].copy_from_slice(b"heic");
        assert_eq!(
            zeneditor::decode::detect_format(&bytes),
            Some(zencodec::ImageFormat::Heic)
        );
    }

    #[test]
    fn detect_bmp() {
        let mut bytes = vec![0u8; 18];
        bytes[0] = b'B';
        bytes[1] = b'M';
        assert_eq!(
            zeneditor::decode::detect_format(&bytes),
            Some(zencodec::ImageFormat::Bmp)
        );
    }

    #[test]
    fn browser_formats_skipped() {
        assert!(browser_handles(zencodec::ImageFormat::Jpeg));
        assert!(browser_handles(zencodec::ImageFormat::Png));
        assert!(!browser_handles(zencodec::ImageFormat::Jxl));
        assert!(!browser_handles(zencodec::ImageFormat::Heic));
    }
}
