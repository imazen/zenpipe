//! Partial-data image probing.
//!
//! Extracts format, dimensions, alpha, animation, and bit depth from a leading
//! slice of an image file without requiring the full file. Each format has
//! different header structures and minimum byte requirements.
//!
//! All parsers are pure byte parsing — no codec crate dependencies. This means
//! probing works even if a codec feature isn't compiled in.

use crate::ImageFormat;
use crate::info::ImageInfo;

#[cfg(test)]
/// Minimum bytes needed for a useful probe (format-specific).
///
/// Returns the minimum number of leading bytes that typically contain
/// enough header data to extract dimensions and basic metadata.
/// JPEG needs more because SOF can follow large EXIF/APP segments.
fn min_probe_bytes(format: ImageFormat) -> usize {
    match format {
        ImageFormat::Png => 33,    // 8 sig + 25 IHDR
        ImageFormat::Gif => 13,    // 6 header + 7 LSD
        ImageFormat::WebP => 30,   // RIFF(12) + chunk header + VP8X dims
        ImageFormat::Jpeg => 2048, // SOF can follow large EXIF/APP segments
        ImageFormat::Avif => 512,  // ISOBMFF box traversal (ftyp + meta)
        ImageFormat::Jxl => 256,   // codestream header or container + jxlc
        ImageFormat::Heic => 512,  // ISOBMFF like AVIF
        _ => 256,                  // conservative default
    }
}

/// Result of probing partial image data.
///
/// All fields except `format` are `Option`, since partial data may not contain
/// enough bytes for dimensions or other metadata.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ProbeResult {
    /// Detected image format (always present if probe succeeds).
    pub format: ImageFormat,
    /// Image width in pixels.
    pub width: Option<u32>,
    /// Image height in pixels.
    pub height: Option<u32>,
    /// Whether the image has an alpha channel.
    pub has_alpha: Option<bool>,
    /// Whether the image contains animation (multiple frames).
    pub has_animation: Option<bool>,
    /// Number of frames (None if unknown without full parse).
    pub frame_count: Option<u32>,
    /// Bits per channel (e.g., 8 for typical images, 16 for HDR).
    pub bit_depth: Option<u8>,
    /// Whether the image contains an HDR gain map (ISO 21496-1).
    pub has_gain_map: Option<bool>,
    /// Number of bytes examined from the input.
    pub bytes_examined: usize,
}

impl ProbeResult {
    /// Convert to `ImageInfo` when width and height are both present.
    ///
    /// Returns `None` if either dimension is missing (insufficient data).
    pub fn into_image_info(self) -> Option<ImageInfo> {
        let width = self.width?;
        let height = self.height?;

        let mut ii = ImageInfo::new(width, height, self.format)
            .with_alpha(self.has_alpha.unwrap_or(false))
            .with_animation(self.has_animation.unwrap_or(false));
        if let Some(count) = self.frame_count {
            ii = ii.with_frame_count(count);
        }
        if let Some(depth) = self.bit_depth {
            ii = ii.with_bit_depth(depth);
        }
        if self.has_gain_map == Some(true) {
            ii = ii.with_gain_map(true);
        }
        Some(ii)
    }

    /// Probe data for a specific format.
    ///
    /// Dispatches to the format-specific parser. Does not verify magic bytes —
    /// the caller is responsible for format detection.
    pub(crate) fn for_format(data: &[u8], format: ImageFormat) -> Self {
        match format {
            ImageFormat::Png => probe_png(data),
            ImageFormat::Gif => probe_gif(data),
            ImageFormat::WebP => probe_webp(data),
            ImageFormat::Jpeg => probe_jpeg(data),
            ImageFormat::Avif => probe_avif(data),
            ImageFormat::Jxl => probe_jxl(data),
            _ => ProbeResult {
                format,
                width: None,
                height: None,
                has_alpha: None,
                has_animation: None,
                frame_count: None,
                bit_depth: None,
                has_gain_map: None,
                bytes_examined: 0,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// PNG: 8-byte signature + 25-byte IHDR (4 len + 4 type + 13 data + 4 CRC)
// Total: 33 bytes for full dimension + color type info
// ---------------------------------------------------------------------------

fn probe_png(data: &[u8]) -> ProbeResult {
    let mut result = ProbeResult {
        format: ImageFormat::Png,
        width: None,
        height: None,
        has_alpha: None,
        has_animation: None,
        frame_count: None,
        bit_depth: None,
        has_gain_map: None,
        bytes_examined: data.len().min(33),
    };

    // Need at least 33 bytes: 8 sig + 4 chunk_len + 4 chunk_type + 13 IHDR data + 4 CRC
    if data.len() < 33 {
        return result;
    }

    // Verify IHDR chunk type at offset 12
    if &data[12..16] != b"IHDR" {
        return result;
    }

    // Width and height are big-endian u32 at offsets 16 and 20
    let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    let bit_depth = data[24];
    let color_type = data[25];

    result.width = Some(width);
    result.height = Some(height);
    result.bit_depth = Some(bit_depth);

    // Color type 4 = grayscale+alpha, 6 = RGBA
    result.has_alpha = Some(color_type == 4 || color_type == 6);

    // Animation detection requires scanning for acTL chunk — too expensive for
    // partial probe. Leave as None.
    result.has_animation = None;

    result
}

// ---------------------------------------------------------------------------
// GIF: 6-byte header + 7-byte Logical Screen Descriptor = 13 bytes
// ---------------------------------------------------------------------------

fn probe_gif(data: &[u8]) -> ProbeResult {
    let mut result = ProbeResult {
        format: ImageFormat::Gif,
        width: None,
        height: None,
        has_alpha: None,
        has_animation: None,
        frame_count: None,
        bit_depth: None,
        has_gain_map: None,
        bytes_examined: data.len().min(13),
    };

    // Need at least 13 bytes: 6 header + 7 LSD
    if data.len() < 13 {
        return result;
    }

    // Width and height are little-endian u16 at offsets 6 and 8
    let width = u16::from_le_bytes([data[6], data[7]]);
    let height = u16::from_le_bytes([data[8], data[9]]);

    result.width = Some(width as u32);
    result.height = Some(height as u32);
    // GIF supports transparency via GCE
    result.has_alpha = Some(true);
    result.bit_depth = Some(8);
    // Animation/frame_count requires full parse
    result.has_animation = None;

    result
}

// ---------------------------------------------------------------------------
// WebP: RIFF header (12) + chunk header (8+) = varies by sub-format
//
// Three sub-formats at offset 12:
// - VP8X (extended): flags at byte 20, canvas dimensions at 24..30 (24-bit LE, +1)
// - VP8  (lossy): keyframe tag at 20..23, dimensions at 26..30 (LE u16, masked to 14 bits)
// - VP8L (lossless): signature 0x2F at byte 20, dimensions bit-packed in bytes 21..25
// ---------------------------------------------------------------------------

fn probe_webp(data: &[u8]) -> ProbeResult {
    let mut result = ProbeResult {
        format: ImageFormat::WebP,
        width: None,
        height: None,
        has_alpha: None,
        has_animation: None,
        frame_count: None,
        bit_depth: None,
        has_gain_map: None,
        bytes_examined: data.len().min(30),
    };

    if data.len() < 16 {
        return result;
    }

    // First chunk type at offset 12
    let chunk = &data[12..16];

    if chunk == b"VP8X" {
        // Extended format: need at least 30 bytes
        if data.len() < 30 {
            return result;
        }

        let flags = data[20];
        let has_alpha = (flags & 0x10) != 0;
        let has_animation = (flags & 0x02) != 0;

        // Canvas width: 24-bit LE at bytes 24..27, stored as (width - 1)
        let canvas_w = (data[24] as u32) | ((data[25] as u32) << 8) | ((data[26] as u32) << 16);
        let canvas_h = (data[27] as u32) | ((data[28] as u32) << 8) | ((data[29] as u32) << 16);

        result.width = Some(canvas_w + 1);
        result.height = Some(canvas_h + 1);
        result.has_alpha = Some(has_alpha);
        result.has_animation = Some(has_animation);
        result.bit_depth = Some(8);
    } else if chunk == b"VP8 " {
        // Lossy: need at least 30 bytes
        // Bytes 20..23: chunk data starts; first 3 bytes are frame tag
        // Bytes 23..26: should be keyframe signature 0x9D 0x01 0x2A
        // Bytes 26..28: width (LE u16, low 14 bits)
        // Bytes 28..30: height (LE u16, low 14 bits)
        if data.len() < 30 {
            return result;
        }

        // Skip 4 bytes chunk header (type already read) + 4 bytes chunk size
        // Actual VP8 bitstream starts at offset 20 (12 RIFF + 8 chunk header)
        // Frame tag: 3 bytes, then keyframe signature: 3 bytes, then dimensions
        // Actually: chunk header is at 12..20 (4 type + 4 size), data at 20+
        // VP8 frame: bytes 20..23 are frame tag (3 bytes for uncompressed size + keyframe bit)
        // Keyframe signature at 23..26: 0x9D 0x01 0x2A
        // Width at 26..28, height at 28..30

        if data.len() >= 30 && data[23] == 0x9D && data[24] == 0x01 && data[25] == 0x2A {
            let width = u16::from_le_bytes([data[26], data[27]]) & 0x3FFF;
            let height = u16::from_le_bytes([data[28], data[29]]) & 0x3FFF;

            result.width = Some(width as u32);
            result.height = Some(height as u32);
            result.has_alpha = Some(false);
            result.has_animation = Some(false);
            result.bit_depth = Some(8);
        }
    } else if chunk == b"VP8L" {
        // Lossless: chunk header at 12..20 (4 type + 4 size), data starts at 20
        // Signature byte 0x2F at offset 20
        // Dimensions bit-packed in bytes 21..25
        if data.len() < 25 {
            return result;
        }

        if data[20] == 0x2F {
            let bits = u32::from_le_bytes([data[21], data[22], data[23], data[24]]);
            // Width: bits[0..14] + 1
            let width = (bits & 0x3FFF) + 1;
            // Height: bits[14..28] + 1
            let height = ((bits >> 14) & 0x3FFF) + 1;

            result.width = Some(width);
            result.height = Some(height);
            result.has_alpha = Some(true); // VP8L supports alpha natively
            result.has_animation = Some(false);
            result.bit_depth = Some(8);
        }
    }

    result
}

// ---------------------------------------------------------------------------
// JPEG: Scan marker segments for SOF (Start of Frame)
//
// Structure: SOI (FF D8), then marker segments (FF xx, 2-byte length, payload).
// Stop at SOF0-SOF2 (C0-C2) to read precision, height, width.
// Stop at SOS (DA) — no more markers to scan without entropy-coded data parsing.
// ---------------------------------------------------------------------------

fn probe_jpeg(data: &[u8]) -> ProbeResult {
    let mut result = ProbeResult {
        format: ImageFormat::Jpeg,
        width: None,
        height: None,
        has_alpha: Some(false),
        has_animation: Some(false),
        frame_count: Some(1),
        bit_depth: None,
        has_gain_map: None,
        bytes_examined: 0,
    };

    if data.len() < 4 {
        result.bytes_examined = data.len();
        return result;
    }

    // Skip SOI marker (FF D8)
    let mut pos = 2;

    while pos + 1 < data.len() {
        // Find next marker
        if data[pos] != 0xFF {
            // Lost sync — not a valid marker position
            break;
        }

        // Skip padding FF bytes
        while pos + 1 < data.len() && data[pos + 1] == 0xFF {
            pos += 1;
        }

        if pos + 1 >= data.len() {
            break;
        }

        let marker = data[pos + 1];
        pos += 2; // past marker bytes

        // Standalone markers (no length field)
        if marker == 0x00 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }

        // SOS (start of scan) — stop scanning
        if marker == 0xDA {
            break;
        }

        // EOI (end of image) — stop scanning
        if marker == 0xD9 {
            break;
        }

        // All other markers have a 2-byte length field
        if pos + 2 > data.len() {
            break;
        }

        let seg_len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;

        // SOF markers: C0 (baseline), C1 (extended), C2 (progressive)
        // Also C3 (lossless), C5-C7, C9-CB, CD-CF — all have same layout
        let is_sof = matches!(
            marker,
            0xC0 | 0xC1
                | 0xC2
                | 0xC3
                | 0xC5
                | 0xC6
                | 0xC7
                | 0xC9
                | 0xCA
                | 0xCB
                | 0xCD
                | 0xCE
                | 0xCF
        );

        if is_sof {
            // SOF: length (2) + precision (1) + height (2) + width (2) + components (1) = 8
            if pos + 7 < data.len() {
                let precision = data[pos + 2];
                let height = u16::from_be_bytes([data[pos + 3], data[pos + 4]]);
                let width = u16::from_be_bytes([data[pos + 5], data[pos + 6]]);

                result.width = Some(width as u32);
                result.height = Some(height as u32);
                result.bit_depth = Some(precision);
                result.bytes_examined = pos + 8;
                return result;
            }
            // Not enough data to read SOF contents
            break;
        }

        // Skip this segment
        if seg_len < 2 {
            break; // Invalid segment length
        }
        pos += seg_len;
    }

    result.bytes_examined = pos.min(data.len());
    result
}

// ---------------------------------------------------------------------------
// AVIF (ISOBMFF): Walk top-level boxes → meta → iprp → ipco → ispe
//
// Box structure: 4-byte size (BE u32) + 4-byte type. If size==1, 8-byte
// extended size follows. If size==0, box extends to EOF.
//
// `meta` is a FullBox: 4 extra version/flags bytes after the header.
// `ispe`: 4 version/flags + 4 width (BE u32) + 4 height (BE u32)
// ---------------------------------------------------------------------------

fn probe_avif(data: &[u8]) -> ProbeResult {
    let mut result = ProbeResult {
        format: ImageFormat::Avif,
        width: None,
        height: None,
        has_alpha: None,
        has_animation: None,
        frame_count: None,
        bit_depth: None,
        has_gain_map: None,
        bytes_examined: 0,
    };

    // Find and enter the `meta` box at the top level
    if let Some((meta_data, meta_end)) = find_box(data, b"meta") {
        // `meta` is a FullBox — skip 4 bytes of version/flags
        if meta_data.len() >= 4 {
            let inner = &meta_data[4..];

            // Check for gain map (tmap box indicates tone-mapped derived item)
            if find_box(inner, b"tmap").is_some() {
                result.has_gain_map = Some(true);
            }

            // Find `iprp` inside `meta`
            if let Some((iprp_data, _)) = find_box(inner, b"iprp") {
                // Find `ipco` inside `iprp`
                if let Some((ipco_data, _)) = find_box(iprp_data, b"ipco") {
                    // Find `ispe` inside `ipco`
                    if let Some((ispe_data, _)) = find_box(ipco_data, b"ispe") {
                        // ispe: 4 version/flags + 4 width + 4 height = 12 bytes
                        if ispe_data.len() >= 12 {
                            let width = u32::from_be_bytes([
                                ispe_data[4],
                                ispe_data[5],
                                ispe_data[6],
                                ispe_data[7],
                            ]);
                            let height = u32::from_be_bytes([
                                ispe_data[8],
                                ispe_data[9],
                                ispe_data[10],
                                ispe_data[11],
                            ]);
                            result.width = Some(width);
                            result.height = Some(height);
                        }
                    }
                }
            }
        }
        result.bytes_examined = meta_end;
    } else {
        result.bytes_examined = data.len();
    }

    result
}

/// Find a box by type in ISOBMFF data. Returns (box_content, end_offset).
fn find_box<'a>(data: &'a [u8], box_type: &[u8; 4]) -> Option<(&'a [u8], usize)> {
    let mut pos = 0;

    while pos + 8 <= data.len() {
        let size = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        let btype = &data[pos + 4..pos + 8];

        let (header_size, box_size) = if size == 1 {
            // Extended size: 64-bit
            if pos + 16 > data.len() {
                return None;
            }
            let ext_size = u64::from_be_bytes([
                data[pos + 8],
                data[pos + 9],
                data[pos + 10],
                data[pos + 11],
                data[pos + 12],
                data[pos + 13],
                data[pos + 14],
                data[pos + 15],
            ]);
            (16usize, ext_size as usize)
        } else if size == 0 {
            // Box extends to end of data
            (8usize, data.len() - pos)
        } else {
            (8usize, size as usize)
        };

        if box_size < header_size {
            return None; // Invalid box
        }

        let content_start = pos + header_size;
        let box_end = pos + box_size;

        if btype == box_type {
            let content_end = box_end.min(data.len());
            if content_start <= content_end {
                return Some((&data[content_start..content_end], box_end));
            }
            return None;
        }

        if box_end <= pos {
            return None; // Prevent infinite loop
        }
        pos = box_end;
    }

    None
}

// ---------------------------------------------------------------------------
// JXL: Two container formats
//
// 1. Bare codestream (FF 0A): parse SizeHeader from the codestream
// 2. ISOBMFF container (00 00 00 0C 4A 58 4C 20 ...): find jxlc/jxlp box,
//    then parse SizeHeader from contained codestream
//
// SizeHeader layout (from libjxl spec):
//   - Bit 0: small (1 = dimensions ≤256, uses 5 bits each; 0 = uses varint)
//   - If small=1:
//     - Bits 1..9: height-1 (9 bits, so up to 256, but spec says 5+div8, actually
//       the encoding is more complex)
//
// For simplicity and correctness, we delegate to jxl-rs when available.
// When not compiled in, we return format-only (no dimensions).
// ---------------------------------------------------------------------------

fn probe_jxl(data: &[u8]) -> ProbeResult {
    // Delegate to jxl-rs if compiled in
    #[cfg(feature = "jxl-decode")]
    {
        probe_jxl_with_crate(data)
    }

    // Without jxl-rs, return format-only
    #[cfg(not(feature = "jxl-decode"))]
    {
        ProbeResult {
            format: ImageFormat::Jxl,
            width: None,
            height: None,
            has_alpha: None,
            has_animation: None,
            frame_count: None,
            bit_depth: None,
            has_gain_map: None,
            bytes_examined: data.len().min(12),
        }
    }
}

#[cfg(feature = "jxl-decode")]
fn probe_jxl_with_crate(data: &[u8]) -> ProbeResult {
    match zenjxl::probe(data) {
        Ok(info) => ProbeResult {
            format: ImageFormat::Jxl,
            width: Some(info.width),
            height: Some(info.height),
            has_alpha: Some(info.has_alpha),
            has_animation: Some(info.has_animation),
            frame_count: None,
            bit_depth: info.bit_depth,
            has_gain_map: None,
            bytes_examined: data.len(),
        },
        Err(_) => {
            // Not enough data or parse error — return format-only
            ProbeResult {
                format: ImageFormat::Jxl,
                width: None,
                height: None,
                has_alpha: None,
                has_animation: None,
                frame_count: None,
                bit_depth: None,
                has_gain_map: None,
                bytes_examined: data.len(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    // ---- PNG tests ----

    #[test]
    fn probe_png_full_header() {
        // PNG signature + IHDR: 100x50, 8-bit RGBA (color type 6)
        let mut data = vec![0u8; 33];
        // PNG signature
        data[..8].copy_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
        // IHDR chunk length (13)
        data[8..12].copy_from_slice(&[0x00, 0x00, 0x00, 0x0D]);
        // IHDR type
        data[12..16].copy_from_slice(b"IHDR");
        // Width: 100 (BE u32)
        data[16..20].copy_from_slice(&100u32.to_be_bytes());
        // Height: 50 (BE u32)
        data[20..24].copy_from_slice(&50u32.to_be_bytes());
        // Bit depth: 8
        data[24] = 8;
        // Color type: 6 (RGBA)
        data[25] = 6;

        let result = probe_png(&data);
        assert_eq!(result.width, Some(100));
        assert_eq!(result.height, Some(50));
        assert_eq!(result.has_alpha, Some(true));
        assert_eq!(result.bit_depth, Some(8));
    }

    #[test]
    fn probe_png_rgb_no_alpha() {
        let mut data = vec![0u8; 33];
        data[..8].copy_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
        data[8..12].copy_from_slice(&[0x00, 0x00, 0x00, 0x0D]);
        data[12..16].copy_from_slice(b"IHDR");
        data[16..20].copy_from_slice(&200u32.to_be_bytes());
        data[20..24].copy_from_slice(&100u32.to_be_bytes());
        data[24] = 8;
        data[25] = 2; // Color type 2 = RGB

        let result = probe_png(&data);
        assert_eq!(result.width, Some(200));
        assert_eq!(result.height, Some(100));
        assert_eq!(result.has_alpha, Some(false));
    }

    #[test]
    fn probe_png_16bit() {
        let mut data = vec![0u8; 33];
        data[..8].copy_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
        data[8..12].copy_from_slice(&[0x00, 0x00, 0x00, 0x0D]);
        data[12..16].copy_from_slice(b"IHDR");
        data[16..20].copy_from_slice(&1920u32.to_be_bytes());
        data[20..24].copy_from_slice(&1080u32.to_be_bytes());
        data[24] = 16;
        data[25] = 6; // RGBA

        let result = probe_png(&data);
        assert_eq!(result.bit_depth, Some(16));
        assert_eq!(result.width, Some(1920));
        assert_eq!(result.height, Some(1080));
    }

    #[test]
    fn probe_png_too_short() {
        let data = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let result = probe_png(&data);
        assert_eq!(result.width, None);
        assert_eq!(result.height, None);
        assert_eq!(result.format, ImageFormat::Png);
    }

    // ---- GIF tests ----

    #[test]
    fn probe_gif_full_header() {
        let mut data = vec![0u8; 13];
        data[..6].copy_from_slice(b"GIF89a");
        // Width: 320 (LE u16)
        data[6..8].copy_from_slice(&320u16.to_le_bytes());
        // Height: 240 (LE u16)
        data[8..10].copy_from_slice(&240u16.to_le_bytes());

        let result = probe_gif(&data);
        assert_eq!(result.width, Some(320));
        assert_eq!(result.height, Some(240));
        assert_eq!(result.has_alpha, Some(true));
        assert_eq!(result.bit_depth, Some(8));
    }

    #[test]
    fn probe_gif_too_short() {
        let data = b"GIF89a";
        let result = probe_gif(data);
        assert_eq!(result.width, None);
    }

    // ---- WebP VP8X tests ----

    #[test]
    fn probe_webp_vp8x() {
        let mut data = vec![0u8; 30];
        data[..4].copy_from_slice(b"RIFF");
        data[8..12].copy_from_slice(b"WEBP");
        data[12..16].copy_from_slice(b"VP8X");
        // Chunk size (4 bytes LE)
        data[16..20].copy_from_slice(&10u32.to_le_bytes());
        // Flags: alpha=0x10, animation=0x02
        data[20] = 0x10 | 0x02;
        // Canvas width: 639 (stored as 639, value = 640)
        let w = 639u32;
        data[24] = w as u8;
        data[25] = (w >> 8) as u8;
        data[26] = (w >> 16) as u8;
        // Canvas height: 479 (stored as 479, value = 480)
        let h = 479u32;
        data[27] = h as u8;
        data[28] = (h >> 8) as u8;
        data[29] = (h >> 16) as u8;

        let result = probe_webp(&data);
        assert_eq!(result.width, Some(640));
        assert_eq!(result.height, Some(480));
        assert_eq!(result.has_alpha, Some(true));
        assert_eq!(result.has_animation, Some(true));
    }

    #[test]
    fn probe_webp_vp8_lossy() {
        let mut data = vec![0u8; 30];
        data[..4].copy_from_slice(b"RIFF");
        data[8..12].copy_from_slice(b"WEBP");
        data[12..16].copy_from_slice(b"VP8 ");
        // VP8 frame tag (3 bytes) + keyframe signature
        data[23] = 0x9D;
        data[24] = 0x01;
        data[25] = 0x2A;
        // Width: 800, Height: 600 (LE u16, low 14 bits)
        data[26..28].copy_from_slice(&800u16.to_le_bytes());
        data[28..30].copy_from_slice(&600u16.to_le_bytes());

        let result = probe_webp(&data);
        assert_eq!(result.width, Some(800));
        assert_eq!(result.height, Some(600));
        assert_eq!(result.has_alpha, Some(false));
        assert_eq!(result.has_animation, Some(false));
    }

    #[test]
    fn probe_webp_vp8l() {
        let mut data = vec![0u8; 25];
        data[..4].copy_from_slice(b"RIFF");
        data[8..12].copy_from_slice(b"WEBP");
        data[12..16].copy_from_slice(b"VP8L");
        // Chunk size
        data[16..20].copy_from_slice(&5u32.to_le_bytes());
        // Signature byte at offset 20
        data[20] = 0x2F;
        // Dimensions bit-packed at offset 21..25:
        // bits[0..14] = width-1 = 254, bits[14..28] = height-1 = 126
        let bits: u32 = 254 | (126 << 14);
        data[21..25].copy_from_slice(&bits.to_le_bytes());

        let result = probe_webp(&data);
        assert_eq!(result.width, Some(255));
        assert_eq!(result.height, Some(127));
        assert_eq!(result.has_alpha, Some(true));
    }

    #[test]
    fn probe_webp_too_short() {
        let data = b"RIFF\x00\x00\x00\x00WEBP";
        let result = probe_webp(data);
        assert_eq!(result.width, None);
        assert_eq!(result.format, ImageFormat::WebP);
    }

    // ---- JPEG tests ----

    #[test]
    fn probe_jpeg_sof() {
        // Minimal JPEG: SOI + APP0 (short) + SOF0
        let mut data = vec![0u8; 30];
        // SOI
        data[0] = 0xFF;
        data[1] = 0xD8;
        // APP0 marker
        data[2] = 0xFF;
        data[3] = 0xE0;
        // APP0 length = 16 (including 2-byte length field)
        data[4] = 0x00;
        data[5] = 0x10;
        // (14 bytes of APP0 data, we skip them)
        // SOF0 at offset 20
        data[20] = 0xFF;
        data[21] = 0xC0;
        // SOF0 length
        data[22] = 0x00;
        data[23] = 0x0B;
        // Precision
        data[24] = 8;
        // Height: 480 (BE u16)
        data[25] = 0x01;
        data[26] = 0xE0;
        // Width: 640 (BE u16)
        data[27] = 0x02;
        data[28] = 0x80;

        let result = probe_jpeg(&data);
        assert_eq!(result.width, Some(640));
        assert_eq!(result.height, Some(480));
        assert_eq!(result.bit_depth, Some(8));
        assert_eq!(result.has_alpha, Some(false));
        assert_eq!(result.has_animation, Some(false));
    }

    #[test]
    fn probe_jpeg_truncated_before_sof() {
        // SOI + APP0 with large length, truncated
        let mut data = vec![0u8; 20];
        data[0] = 0xFF;
        data[1] = 0xD8;
        data[2] = 0xFF;
        data[3] = 0xE0;
        // APP0 length = 1000 (much more data than we have)
        data[4] = 0x03;
        data[5] = 0xE8;

        let result = probe_jpeg(&data);
        assert_eq!(result.width, None);
        assert_eq!(result.height, None);
        assert_eq!(result.format, ImageFormat::Jpeg);
    }

    #[test]
    fn probe_jpeg_too_short() {
        let data = [0xFF, 0xD8];
        let result = probe_jpeg(&data);
        assert_eq!(result.width, None);
    }

    // ---- AVIF tests ----

    #[test]
    fn probe_avif_ispe() {
        // Build a minimal AVIF with ftyp + meta containing iprp/ipco/ispe
        let mut data = Vec::new();

        // ftyp box: size=20, type="ftyp", brand="avif", version=0, compat="avif"
        let ftyp_size = 20u32;
        data.extend_from_slice(&ftyp_size.to_be_bytes());
        data.extend_from_slice(b"ftyp");
        data.extend_from_slice(b"avif");
        data.extend_from_slice(&0u32.to_be_bytes()); // minor version
        data.extend_from_slice(b"avif");

        // ispe box: size=20, type="ispe", version/flags=0, width=1920, height=1080
        let mut ispe = Vec::new();
        let ispe_size = 20u32;
        ispe.extend_from_slice(&ispe_size.to_be_bytes());
        ispe.extend_from_slice(b"ispe");
        ispe.extend_from_slice(&0u32.to_be_bytes()); // version/flags
        ispe.extend_from_slice(&1920u32.to_be_bytes());
        ispe.extend_from_slice(&1080u32.to_be_bytes());

        // ipco box wrapping ispe
        let ipco_size = (8 + ispe.len()) as u32;
        let mut ipco = Vec::new();
        ipco.extend_from_slice(&ipco_size.to_be_bytes());
        ipco.extend_from_slice(b"ipco");
        ipco.extend_from_slice(&ispe);

        // iprp box wrapping ipco
        let iprp_size = (8 + ipco.len()) as u32;
        let mut iprp = Vec::new();
        iprp.extend_from_slice(&iprp_size.to_be_bytes());
        iprp.extend_from_slice(b"iprp");
        iprp.extend_from_slice(&ipco);

        // meta box (FullBox — 4 extra bytes for version/flags) wrapping iprp
        let meta_size = (8 + 4 + iprp.len()) as u32;
        data.extend_from_slice(&meta_size.to_be_bytes());
        data.extend_from_slice(b"meta");
        data.extend_from_slice(&0u32.to_be_bytes()); // version/flags
        data.extend_from_slice(&iprp);

        let result = probe_avif(&data);
        assert_eq!(result.width, Some(1920));
        assert_eq!(result.height, Some(1080));
    }

    #[test]
    fn probe_avif_no_meta() {
        // Just ftyp, no meta box
        let mut data = Vec::new();
        let ftyp_size = 20u32;
        data.extend_from_slice(&ftyp_size.to_be_bytes());
        data.extend_from_slice(b"ftyp");
        data.extend_from_slice(b"avif");
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(b"avif");

        let result = probe_avif(&data);
        assert_eq!(result.width, None);
        assert_eq!(result.height, None);
        assert_eq!(result.format, ImageFormat::Avif);
    }

    // ---- ProbeResult conversion ----

    #[test]
    fn probe_result_into_image_info() {
        let result = ProbeResult {
            format: ImageFormat::Png,
            width: Some(100),
            height: Some(50),
            has_alpha: Some(true),
            has_animation: Some(false),
            frame_count: None,
            bit_depth: Some(8),
            has_gain_map: None,
            bytes_examined: 33,
        };

        let info = result.into_image_info().unwrap();
        assert_eq!(info.width, 100);
        assert_eq!(info.height, 50);
        assert!(info.has_alpha);
        assert!(!info.has_animation);
    }

    #[test]
    fn probe_result_into_image_info_missing_dims() {
        let result = ProbeResult {
            format: ImageFormat::Jpeg,
            width: None,
            height: None,
            has_alpha: Some(false),
            has_animation: None,
            frame_count: None,
            bit_depth: None,
            has_gain_map: None,
            bytes_examined: 2,
        };

        assert!(result.into_image_info().is_none());
    }

    // ---- Dispatch tests ----

    #[test]
    fn probe_for_format_dispatches() {
        let png_data = vec![0u8; 33];
        let result = ProbeResult::for_format(&png_data, ImageFormat::Png);
        assert_eq!(result.format, ImageFormat::Png);

        let gif_data = vec![0u8; 13];
        let result = ProbeResult::for_format(&gif_data, ImageFormat::Gif);
        assert_eq!(result.format, ImageFormat::Gif);
    }

    // ---- Integration tests with real encoded images ----

    /// Helper: create a 64x48 RGB test image.
    fn test_rgb_pixels() -> (Vec<u8>, u32, u32) {
        let w = 64u32;
        let h = 48u32;
        let mut pixels = vec![0u8; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 3) as usize;
                pixels[i] = (x * 4) as u8;
                pixels[i + 1] = (y * 5) as u8;
                pixels[i + 2] = ((x + y) * 2) as u8;
            }
        }
        (pixels, w, h)
    }

    /// Verify probe at various truncation lengths never panics and gives
    /// correct results when enough data is present.
    fn verify_probe_truncation(
        encoded: &[u8],
        format: ImageFormat,
        expected_w: u32,
        expected_h: u32,
    ) {
        // With 12 bytes: format detection works but dimensions may not
        if encoded.len() >= 12 {
            let result = ProbeResult::for_format(&encoded[..12], format);
            assert_eq!(result.format, format);
        }

        // With min_probe_bytes: dimensions should be present
        let min = min_probe_bytes(format);
        if encoded.len() >= min {
            let result = ProbeResult::for_format(&encoded[..min], format);
            assert_eq!(result.format, format);
            // For most formats, min_probe_bytes should give dimensions
            // (JPEG is an exception — SOF position is variable)
            if format != ImageFormat::Jpeg {
                assert_eq!(
                    result.width,
                    Some(expected_w),
                    "width mismatch at min_probe_bytes for {format:?}"
                );
                assert_eq!(
                    result.height,
                    Some(expected_h),
                    "height mismatch at min_probe_bytes for {format:?}"
                );
            }
        }

        // With full data: should always have dimensions
        let result = ProbeResult::for_format(encoded, format);
        assert_eq!(
            result.width,
            Some(expected_w),
            "width mismatch for {format:?}"
        );
        assert_eq!(
            result.height,
            Some(expected_h),
            "height mismatch for {format:?}"
        );

        // Random truncation should never panic
        for len in (1..encoded.len()).step_by(7) {
            let _ = ProbeResult::for_format(&encoded[..len], format);
        }
    }

    #[cfg(feature = "jpeg")]
    #[test]
    fn probe_real_jpeg() {
        let (pixels, w, h) = test_rgb_pixels();
        let config = zenjpeg::encoder::EncoderConfig::ycbcr(
            85,
            zenjpeg::encoder::ChromaSubsampling::Quarter,
        );
        let encoded = config
            .request()
            .encode_bytes(&pixels, w, h, zenjpeg::encoder::PixelLayout::Rgb8Srgb)
            .unwrap();

        // Full probe
        let result = probe_jpeg(&encoded);
        assert_eq!(result.width, Some(w));
        assert_eq!(result.height, Some(h));
        assert_eq!(result.has_alpha, Some(false));
        assert_eq!(result.bit_depth, Some(8));

        // Matches codec probe
        let info = crate::from_bytes(&encoded).unwrap();
        assert_eq!(result.width, Some(info.width));
        assert_eq!(result.height, Some(info.height));

        // Truncation safety
        for len in (1..encoded.len()).step_by(17) {
            let _ = probe_jpeg(&encoded[..len]);
        }
    }

    #[cfg(feature = "webp")]
    #[test]
    fn probe_real_webp_lossy() {
        let (pixels, w, h) = test_rgb_pixels();
        let config = zenwebp::LossyConfig::new().with_quality(75.0);
        let encoded =
            zenwebp::EncodeRequest::lossy(&config, &pixels, zenwebp::PixelLayout::Rgb8, w, h)
                .encode()
                .unwrap();

        verify_probe_truncation(&encoded, ImageFormat::WebP, w, h);

        let result = probe_webp(&encoded);
        assert_eq!(result.has_alpha, Some(false));
        assert_eq!(result.has_animation, Some(false));
    }

    #[cfg(feature = "webp")]
    #[test]
    fn probe_real_webp_lossless() {
        let (pixels, w, h) = test_rgb_pixels();
        let config = zenwebp::LosslessConfig::default();
        let encoded =
            zenwebp::EncodeRequest::lossless(&config, &pixels, zenwebp::PixelLayout::Rgb8, w, h)
                .encode()
                .unwrap();

        let result = probe_webp(&encoded);
        assert_eq!(result.width, Some(w));
        assert_eq!(result.height, Some(h));
    }

    #[cfg(feature = "png")]
    #[test]
    fn probe_real_png() {
        // Encode a PNG via the png crate
        let (pixels, w, h) = test_rgb_pixels();
        let mut png_buf = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_buf, w, h);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&pixels).unwrap();
        }

        verify_probe_truncation(&png_buf, ImageFormat::Png, w, h);

        let result = probe_png(&png_buf);
        assert_eq!(result.has_alpha, Some(false));
        assert_eq!(result.bit_depth, Some(8));
    }

    #[cfg(feature = "png")]
    #[test]
    fn probe_real_png_rgba() {
        let w = 32u32;
        let h = 24u32;
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        for i in (0..pixels.len()).step_by(4) {
            pixels[i] = 128;
            pixels[i + 1] = 64;
            pixels[i + 2] = 32;
            pixels[i + 3] = 200;
        }

        let mut png_buf = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_buf, w, h);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&pixels).unwrap();
        }

        let result = probe_png(&png_buf);
        assert_eq!(result.width, Some(w));
        assert_eq!(result.height, Some(h));
        assert_eq!(result.has_alpha, Some(true));
    }

    #[cfg(feature = "gif")]
    #[test]
    fn probe_real_gif() {
        // Encode a GIF via zencodecs EncodeRequest
        let w = 16u32;
        let h = 12u32;
        let pixels: Vec<crate::pixel::Rgba<u8>> = vec![
            crate::pixel::Rgba {
                r: 128,
                g: 64,
                b: 32,
                a: 255
            };
            (w * h) as usize
        ];
        let img = crate::pixel::ImgVec::new(pixels, w as usize, h as usize);
        let encoded = crate::EncodeRequest::new(ImageFormat::Gif)
            .encode_rgba8(img.as_ref())
            .unwrap();

        verify_probe_truncation(encoded.data(), ImageFormat::Gif, w, h);
    }

    // ---- ImageInfo::probe integration ----

    #[cfg(feature = "png")]
    #[test]
    fn image_info_probe_png() {
        let (pixels, w, h) = test_rgb_pixels();
        let mut png_buf = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_buf, w, h);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&pixels).unwrap();
        }

        let probe = crate::info::probe(&png_buf).unwrap();
        assert_eq!(probe.format, ImageFormat::Png);
        assert_eq!(probe.width, Some(w));
        assert_eq!(probe.height, Some(h));

        // Convert to ImageInfo
        let info = probe.into_image_info().unwrap();
        assert_eq!(info.width, w);
        assert_eq!(info.height, h);
    }

    #[cfg(feature = "jpeg")]
    #[test]
    fn image_info_probe_matches_from_bytes() {
        let (pixels, w, h) = test_rgb_pixels();
        let config = zenjpeg::encoder::EncoderConfig::ycbcr(
            85,
            zenjpeg::encoder::ChromaSubsampling::Quarter,
        );
        let encoded = config
            .request()
            .encode_bytes(&pixels, w, h, zenjpeg::encoder::PixelLayout::Rgb8Srgb)
            .unwrap();

        let probe = crate::info::probe(&encoded).unwrap();
        let full = crate::from_bytes(&encoded).unwrap();

        assert_eq!(probe.width, Some(full.width));
        assert_eq!(probe.height, Some(full.height));
    }

    #[test]
    fn image_info_probe_unrecognized() {
        let data = b"not an image format at all";
        let result = crate::info::probe(data);
        assert!(matches!(result, Err(crate::CodecError::UnrecognizedFormat)));
    }

    #[test]
    fn image_info_probe_with_registry_disabled() {
        let jpeg_data = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        let registry = crate::CodecRegistry::none();
        let result = crate::info::probe_with_registry(&jpeg_data, &registry);
        assert!(matches!(result, Err(crate::CodecError::DisabledFormat(_))));
    }

    // ---- min_probe_bytes sanity ----

    #[test]
    fn min_probe_bytes_values() {
        assert_eq!(min_probe_bytes(ImageFormat::Png), 33);
        assert_eq!(min_probe_bytes(ImageFormat::Gif), 13);
        assert_eq!(min_probe_bytes(ImageFormat::WebP), 30);
        assert_eq!(min_probe_bytes(ImageFormat::Jpeg), 2048);
        assert_eq!(min_probe_bytes(ImageFormat::Avif), 512);
        assert_eq!(min_probe_bytes(ImageFormat::Jxl), 256);
        assert_eq!(ImageFormat::RECOMMENDED_PROBE_BYTES, 4096);
    }
}
