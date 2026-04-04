//! Native image decoding via zencodecs.
//!
//! The authoritative decode path — produces RGBA8 sRGB pixels with full
//! metadata preservation (ICC, EXIF, XMP, CICP, gain maps).
//!
//! The frontend may optionally provide a fast browser-decoded preview
//! via [`EditorState::init_from_rgba()`], but this module handles the
//! high-quality decode that the pipeline actually uses.

#[cfg(feature = "decode")]
use zenpixels::PixelDescriptor;

/// Decoded image: RGBA8 pixels + dimensions (no metadata).
pub struct DecodedImage {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Native decode result: RGBA8 pixels + metadata + format info.
pub struct NativeDecodeOutput {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub metadata: zencodec::Metadata,
    pub format: zencodec::ImageFormat,
    pub has_gain_map: bool,
}

/// Decode image bytes natively via zencodecs with full metadata preservation.
///
/// Returns RGBA8 sRGB pixels + ICC/EXIF/XMP/CICP metadata + detected format.
/// This is the authoritative decode — the frontend's browser decode is only
/// a fast preview; this produces the pixels the pipeline actually uses.
#[cfg(feature = "decode")]
pub fn decode_native(bytes: &[u8]) -> Result<NativeDecodeOutput, String> {
    let output = zencodecs::DecodeRequest::new(bytes)
        .decode_full_frame()
        .map_err(|e| format!("decode: {e}"))?;

    let width = output.width();
    let height = output.height();
    let metadata = output.metadata();
    let format = output.format();
    let has_gain_map = matches!(
        output.info().gain_map,
        zencodec::gainmap::GainMapPresence::Available(_)
    );
    let src_desc = output.descriptor();

    // Convert decoded pixels to tightly-packed RGBA8 sRGB.
    let dst_desc = PixelDescriptor::RGBA8_SRGB;
    let pixels = output.pixels();
    let src_stride = pixels.stride();
    let src_bpp = src_desc.bytes_per_pixel();
    let src_row_bytes = width as usize * src_bpp;
    let dst_row_bytes = width as usize * 4;

    let data = if src_desc == dst_desc {
        // Fast path: already RGBA8 sRGB
        if src_stride == dst_row_bytes {
            pixels.as_strided_bytes()[..dst_row_bytes * height as usize].to_vec()
        } else {
            let mut packed = Vec::with_capacity(dst_row_bytes * height as usize);
            for y in 0..height as usize {
                let start = y * src_stride;
                packed.extend_from_slice(
                    &pixels.as_strided_bytes()[start..start + dst_row_bytes],
                );
            }
            packed
        }
    } else {
        // Use RowConverter for any other format
        let mut converter = zenpipe::RowConverter::new(src_desc, dst_desc)
            .map_err(|e| format!("pixel conversion {src_desc} → {dst_desc}: {e}"))?;
        let mut packed = vec![0u8; dst_row_bytes * height as usize];
        let src_data = pixels.as_strided_bytes();
        for y in 0..height as usize {
            let src_start = y * src_stride;
            let dst_start = y * dst_row_bytes;
            converter.convert_row(
                &src_data[src_start..src_start + src_row_bytes],
                &mut packed[dst_start..dst_start + dst_row_bytes],
                width,
            );
        }
        packed
    };

    Ok(NativeDecodeOutput {
        data,
        width,
        height,
        metadata,
        format,
        has_gain_map,
    })
}

/// Try to decode image bytes. Returns None if format is unrecognized.
///
/// For use when the frontend needs a quick WASM decode fallback
/// (e.g. JXL/AVIF that the browser can't handle natively).
#[cfg(feature = "decode")]
pub fn try_decode(bytes: &[u8]) -> Option<DecodedImage> {
    let output = decode_native(bytes).ok()?;
    Some(DecodedImage {
        data: output.data,
        width: output.width,
        height: output.height,
    })
}

/// Detect image format from magic bytes.
#[cfg(feature = "decode")]
pub fn detect_format(bytes: &[u8]) -> Option<zencodec::ImageFormat> {
    zencodec::ImageFormatRegistry::common().detect(bytes)
}

/// Formats the browser can decode natively — skip WASM fallback for these.
pub fn browser_handles(format: zencodec::ImageFormat) -> bool {
    matches!(
        format,
        zencodec::ImageFormat::Jpeg
            | zencodec::ImageFormat::Png
            | zencodec::ImageFormat::WebP
            | zencodec::ImageFormat::Gif
    )
}

/// List of non-browser formats this decoder supports (for UI display).
pub fn wasm_decode_formats() -> &'static [&'static str] {
    &["jxl", "avif", "heic", "bmp", "qoi", "tga", "hdr"]
}

#[cfg(all(test, feature = "decode"))]
mod tests {
    use super::*;

    #[test]
    fn detect_jpeg() {
        assert_eq!(
            detect_format(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0, 0, 0, 0, 0]),
            Some(zencodec::ImageFormat::Jpeg)
        );
    }

    #[test]
    fn detect_jxl_codestream() {
        assert_eq!(
            detect_format(&[0xFF, 0x0A, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            Some(zencodec::ImageFormat::Jxl)
        );
    }

    #[test]
    fn detect_avif() {
        let mut bytes = vec![0u8; 12];
        bytes[4..8].copy_from_slice(b"ftyp");
        bytes[8..12].copy_from_slice(b"avif");
        assert_eq!(detect_format(&bytes), Some(zencodec::ImageFormat::Avif));
    }

    #[test]
    fn detect_heic() {
        let mut bytes = vec![0u8; 12];
        bytes[4..8].copy_from_slice(b"ftyp");
        bytes[8..12].copy_from_slice(b"heic");
        assert_eq!(detect_format(&bytes), Some(zencodec::ImageFormat::Heic));
    }

    #[test]
    fn detect_bmp() {
        let mut bytes = vec![0u8; 18];
        bytes[0] = b'B';
        bytes[1] = b'M';
        assert_eq!(detect_format(&bytes), Some(zencodec::ImageFormat::Bmp));
    }

    #[test]
    fn browser_formats_skipped() {
        assert!(browser_handles(zencodec::ImageFormat::Jpeg));
        assert!(browser_handles(zencodec::ImageFormat::Png));
        assert!(!browser_handles(zencodec::ImageFormat::Jxl));
        assert!(!browser_handles(zencodec::ImageFormat::Heic));
    }
}
