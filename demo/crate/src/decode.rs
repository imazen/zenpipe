//! Image decoding — detects format and decodes to RGBA8 pixels.
//!
//! Two decode paths:
//! - `try_decode()` — streaming fallback for JXL/AVIF when browser can't decode
//! - `decode_native()` — full zencodecs decode with metadata preservation
//!   (used for two-phase upgrade: browser preview → native decode in background)

use std::borrow::Cow;
use zencodec::decode::{DecodeJob, DecoderConfig, StreamingDecode};
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

/// Try to decode image bytes using WASM codecs.
///
/// Probes the format from magic bytes, then decodes to RGBA8 sRGB.
/// Returns `None` if the format is not recognized or not supported.
pub fn try_decode(bytes: &[u8]) -> Option<DecodedImage> {
    // Detect format from magic bytes
    let format = detect_format(bytes)?;

    match format {
        "jxl" => decode_jxl(bytes),
        "avif" => decode_avif(bytes),
        // JPEG/PNG/WebP/GIF are handled by the browser — we only need
        // fallback for formats browsers don't support.
        _ => None,
    }
}

/// Decode image bytes natively via zencodecs with full metadata preservation.
///
/// Returns RGBA8 sRGB pixels + ICC/EXIF/XMP/CICP metadata + detected format.
/// Used for the two-phase upgrade: browser shows instant preview, then this
/// runs in background to provide the authoritative decode with metadata.
pub fn decode_native(bytes: &[u8]) -> Result<NativeDecodeOutput, String> {
    let output = zencodecs::DecodeRequest::new(bytes)
        .decode_full_frame()
        .map_err(|e| format!("decode: {e}"))?;

    let width = output.width();
    let height = output.height();
    let metadata = output.metadata();
    let format = output.format();
    let has_gain_map = matches!(output.info().gain_map, zencodec::gainmap::GainMapPresence::Available(_));
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
                packed.extend_from_slice(&pixels.as_strided_bytes()[start..start + dst_row_bytes]);
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

/// List of formats this decoder supports (for UI display).
pub fn supported_formats() -> &'static [&'static str] {
    &["jxl", "avif"]
}

fn detect_format(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() < 12 {
        return None;
    }
    // JXL naked codestream
    if bytes[0] == 0xFF && bytes[1] == 0x0A {
        return Some("jxl");
    }
    // JXL container
    if bytes[..12]
        == [
            0, 0, 0, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
        ]
    {
        return Some("jxl");
    }
    // AVIF/HEIF (ftyp box)
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        if brand == b"avif" || brand == b"avis" || brand == b"mif1" {
            return Some("avif");
        }
        if brand == b"heic" || brand == b"heix" || brand == b"hevc" || brand == b"hevx" {
            return Some("heic");
        }
    }
    // JPEG
    if bytes[0] == 0xFF && bytes[1] == 0xD8 {
        return Some("jpeg");
    }
    // PNG
    if bytes[..4] == [0x89, 0x50, 0x4E, 0x47] {
        return Some("png");
    }
    // WebP
    if bytes[..4] == *b"RIFF" && bytes.len() >= 12 && bytes[8..12] == *b"WEBP" {
        return Some("webp");
    }
    // GIF
    if bytes[..3] == *b"GIF" {
        return Some("gif");
    }
    None
}

fn decode_jxl(bytes: &[u8]) -> Option<DecodedImage> {
    let config = zenjxl::JxlDecoderConfig::new();
    let job = config.job();
    let mut decoder = job
        .streaming_decoder(Cow::Borrowed(bytes), &[PixelDescriptor::RGBA8_SRGB])
        .ok()?;

    let info = decoder.info();
    let width = info.width;
    let height = info.height;
    let bpp = 4usize;
    let row_bytes = width as usize * bpp;
    let mut data = vec![0u8; row_bytes * height as usize];
    let mut y = 0u32;

    while let Ok(Some((_batch_y, pixels))) = decoder.next_batch() {
        let rows = pixels.rows();
        for r in 0..rows {
            let row = pixels.row(r);
            let dst = (y + r) as usize * row_bytes;
            data[dst..dst + row_bytes].copy_from_slice(&row[..row_bytes]);
        }
        y += rows;
    }

    if y == 0 {
        return None;
    }

    Some(DecodedImage {
        data,
        width,
        height,
    })
}

fn decode_avif(bytes: &[u8]) -> Option<DecodedImage> {
    let config = zenavif::AvifDecoderConfig::new();
    let job = config.job();
    let mut decoder = job
        .streaming_decoder(Cow::Borrowed(bytes), &[PixelDescriptor::RGBA8_SRGB])
        .ok()?;

    let info = decoder.info();
    let width = info.width;
    let height = info.height;
    let bpp = 4usize;
    let row_bytes = width as usize * bpp;
    let mut data = vec![0u8; row_bytes * height as usize];
    let mut y = 0u32;

    while let Ok(Some((_batch_y, pixels))) = decoder.next_batch() {
        let rows = pixels.rows();
        for r in 0..rows {
            let row = pixels.row(r);
            let dst = (y + r) as usize * row_bytes;
            data[dst..dst + row_bytes].copy_from_slice(&row[..row_bytes]);
        }
        y += rows;
    }

    if y == 0 {
        return None;
    }

    Some(DecodedImage {
        data,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_jpeg() {
        assert_eq!(
            detect_format(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0, 0, 0, 0, 0]),
            Some("jpeg")
        );
    }

    #[test]
    fn detect_jxl_codestream() {
        assert_eq!(
            detect_format(&[0xFF, 0x0A, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            Some("jxl")
        );
    }

    #[test]
    fn detect_avif() {
        let mut bytes = vec![0u8; 12];
        bytes[4..8].copy_from_slice(b"ftyp");
        bytes[8..12].copy_from_slice(b"avif");
        assert_eq!(detect_format(&bytes), Some("avif"));
    }
}
