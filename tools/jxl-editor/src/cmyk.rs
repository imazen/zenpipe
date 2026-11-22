use jxl_encoder::{ImageMetadata, Limits, LosslessConfig, LossyConfig, PixelLayout};
use serde::Deserialize;
use zenjxl_decoder::api::{JxlCms, JxlColorEncoding, JxlColorProfile, MoxCms};

use crate::{MAX_CODEC_MEMORY, limits};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// Build a clean, self-contained ICC v2.1.0 CMYK Output profile (`prtr`, `CMYK` -> `XYZ `)
/// using a 2x2x2x2 16-bit CLUT (`mft2`) for standard subtractive ink-to-D50-XYZ conversion.
///
/// Original Imazen code with no third-party profile copyright restrictions.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn generate_minimal_cmyk_icc() -> Vec<u8> {
    fn push_be_u16(out: &mut Vec<u8>, v: u16) {
        out.extend_from_slice(&v.to_be_bytes());
    }
    fn push_be_u32(out: &mut Vec<u8>, v: u32) {
        out.extend_from_slice(&v.to_be_bytes());
    }
    fn push_s15fixed16(out: &mut Vec<u8>, v: f64) {
        let fixed = (v * 65536.0).round() as i32;
        out.extend_from_slice(&fixed.to_be_bytes());
    }
    fn make_text_desc(ascii: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"desc");
        push_be_u32(&mut buf, 0);
        push_be_u32(&mut buf, (ascii.len() + 1) as u32);
        buf.extend_from_slice(ascii.as_bytes());
        buf.push(0);
        // Unicode language code (4) + count (4) + ScriptCode code (2) + count (1) + 67 bytes
        buf.extend_from_slice(&[0u8; 4 + 4 + 2 + 1 + 67]);
        buf
    }
    fn make_text(ascii: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"text");
        push_be_u32(&mut buf, 0);
        buf.extend_from_slice(ascii.as_bytes());
        buf.push(0);
        buf
    }
    fn make_xyz(x: f64, y: f64, z: f64) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"XYZ ");
        push_be_u32(&mut buf, 0);
        push_s15fixed16(&mut buf, x);
        push_s15fixed16(&mut buf, y);
        push_s15fixed16(&mut buf, z);
        buf
    }
    // sRGB linear -> D50 XYZ (Bradford adapted, standard ICC matrix)
    fn rgb_to_d50_xyz_u16(r: f64, g: f64, b: f64) -> [u16; 3] {
        let x = 0.436_074_7 * r + 0.385_064_9 * g + 0.143_080_4 * b;
        let y = 0.222_504_5 * r + 0.716_878_6 * g + 0.060_616_9 * b;
        let z = 0.013_932_2 * r + 0.097_104_5 * g + 0.714_173_3 * b;
        // ICC PCS XYZ 16-bit encoding: u1Fixed15Number (1.0 = 0x8000 = 32768).
        let enc = |v: f64| (v * 32768.0).round().clamp(0.0, 65535.0) as u16;
        [enc(x), enc(y), enc(z)]
    }
    fn make_a2b_mft2() -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"mft2");
        push_be_u32(&mut buf, 0);
        buf.push(4); // input channels (C, M, Y, K)
        buf.push(3); // output channels (X, Y, Z)
        buf.push(2); // CLUT grid points per dimension (2^4 = 16)
        buf.push(0); // padding
        // 3x3 identity matrix (s15Fixed16)
        for &(r, c) in &[
            (0, 0),
            (0, 1),
            (0, 2),
            (1, 0),
            (1, 1),
            (1, 2),
            (2, 0),
            (2, 1),
            (2, 2),
        ] {
            push_s15fixed16(&mut buf, if r == c { 1.0 } else { 0.0 });
        }
        push_be_u16(&mut buf, 2); // input table entries per channel
        push_be_u16(&mut buf, 2); // output table entries per channel
        // Input tables (4 channels x 2 entries: identity [0, 65535])
        for _ in 0..4 {
            push_be_u16(&mut buf, 0);
            push_be_u16(&mut buf, 65535);
        }
        // 2x2x2x2 CLUT (C outermost, K innermost; ICC convention: 0 = no ink, 1 = full ink)
        for ci in 0..2 {
            let c = ci as f64;
            for mi in 0..2 {
                let m = mi as f64;
                for yi in 0..2 {
                    let y = yi as f64;
                    for ki in 0..2 {
                        let k = ki as f64;
                        let r = (1.0 - c) * (1.0 - k);
                        let g = (1.0 - m) * (1.0 - k);
                        let b = (1.0 - y) * (1.0 - k);
                        for v in rgb_to_d50_xyz_u16(r, g, b) {
                            push_be_u16(&mut buf, v);
                        }
                    }
                }
            }
        }
        // Output tables (3 channels x 2 entries: identity [0, 65535])
        for _ in 0..3 {
            push_be_u16(&mut buf, 0);
            push_be_u16(&mut buf, 65535);
        }
        buf
    }

    let desc = make_text_desc("Simple Subtractive CMYK");
    let cprt = make_text("Imazen minimal subtractive CMYK profile");
    let wtpt = make_xyz(0.964_202_88, 1.0, 0.824_905_40);
    let a2b = make_a2b_mft2();

    // Tag table: desc, cprt, wtpt, A2B0, A2B1 (sharing payload with A2B0)
    let tags: [([u8; 4], &[u8]); 5] = [
        (*b"desc", &desc),
        (*b"cprt", &cprt),
        (*b"wtpt", &wtpt),
        (*b"A2B0", &a2b),
        (*b"A2B1", &a2b),
    ];
    let header_len = 128usize;
    let table_len = 4 + tags.len() * 12;
    let mut offset = header_len + table_len;
    let mut records = Vec::with_capacity(tags.len());
    let mut payload = Vec::new();
    let mut a2b_offset = None;

    for (sig, data) in tags {
        if sig == *b"A2B1"
            && let Some(shared) = a2b_offset
        {
            records.push((sig, shared, data.len() as u32));
            continue;
        }
        let start = offset as u32;
        if sig == *b"A2B0" {
            a2b_offset = Some(start);
        }
        records.push((sig, start, data.len() as u32));
        payload.extend_from_slice(data);
        let pad = (4 - (data.len() % 4)) % 4;
        payload.extend(core::iter::repeat_n(0u8, pad));
        offset += data.len() + pad;
    }

    let total_size = (header_len + table_len + payload.len()) as u32;
    let mut icc = vec![0u8; header_len];
    icc[0..4].copy_from_slice(&total_size.to_be_bytes());
    icc[8..12].copy_from_slice(&[0x02, 0x10, 0x00, 0x00]); // v2.1.0
    icc[12..16].copy_from_slice(b"prtr");
    icc[16..20].copy_from_slice(b"CMYK");
    icc[20..24].copy_from_slice(b"XYZ ");
    // Creation date 2026-01-01 00:00:00
    icc[24..26].copy_from_slice(&2026u16.to_be_bytes());
    icc[26..28].copy_from_slice(&1u16.to_be_bytes());
    icc[28..30].copy_from_slice(&1u16.to_be_bytes());
    icc[36..40].copy_from_slice(b"acsp");
    // PCS illuminant D50 (0.9642, 1.0, 0.8249)
    icc[68..72].copy_from_slice(&0x0000_F6D6u32.to_be_bytes());
    icc[72..76].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    icc[76..80].copy_from_slice(&0x0000_D32Du32.to_be_bytes());

    push_be_u32(&mut icc, records.len() as u32);
    for (sig, off, len) in records {
        icc.extend_from_slice(&sig);
        push_be_u32(&mut icc, off);
        push_be_u32(&mut icc, len);
    }
    icc.extend_from_slice(&payload);
    icc
}

fn srgb_u8_to_linear(v: u8) -> f32 {
    let x = f32::from(v) / 255.0;
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

fn rgb_to_jxl_ink_f32(r: u8, g: u8, b: u8) -> [f32; 4] {
    // Subtractive separation in linear light so the built-in subtractive CMYK ICC
    // maps colors back close to the original sRGB appearance.
    let rl = srgb_u8_to_linear(r);
    let gl = srgb_u8_to_linear(g);
    let bl = srgb_u8_to_linear(b);
    let max_rgb = rl.max(gl).max(bl);
    let k_ink = 1.0 - max_rgb;
    let (c_ink, m_ink, y_ink) = if max_rgb > 1e-6 {
        (
            (max_rgb - rl) / max_rgb,
            (max_rgb - gl) / max_rgb,
            (max_rgb - bl) / max_rgb,
        )
    } else {
        (0.0, 0.0, 0.0)
    };
    // JPEG XL ink convention: 0 = full ink, 1 = no ink (inverted).
    [
        (1.0 - c_ink).clamp(0.0, 1.0),
        (1.0 - m_ink).clamp(0.0, 1.0),
        (1.0 - y_ink).clamp(0.0, 1.0),
        (1.0 - k_ink).clamp(0.0, 1.0),
    ]
}

/// Separate sRGB RGBA8 pixels into JXL-convention inverted CMYK8 (`0 = full ink`, `255 = no ink`).
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn rgba8_to_cmyk8_jxl(rgba: &[u8]) -> Result<Vec<u8>, String> {
    if !rgba.len().is_multiple_of(4) {
        return Err("RGBA8 buffer length must be a multiple of 4.".into());
    }
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.as_chunks::<4>().0 {
        let [c, m, y, k] = rgb_to_jxl_ink_f32(px[0], px[1], px[2]);
        out.extend_from_slice(&[
            (c * 255.0).round() as u8,
            (m * 255.0).round() as u8,
            (y * 255.0).round() as u8,
            (k * 255.0).round() as u8,
        ]);
    }
    Ok(out)
}

/// Separate sRGB RGBA8 pixels into JXL-convention inverted CMYK16 little-endian bytes
/// (`0 = full ink`, `65535 = no ink`).
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn rgba8_to_cmyk16_jxl(rgba: &[u8]) -> Result<Vec<u8>, String> {
    if !rgba.len().is_multiple_of(4) {
        return Err("RGBA8 buffer length must be a multiple of 4.".into());
    }
    let mut out = Vec::with_capacity(rgba.len() * 2);
    for px in rgba.as_chunks::<4>().0 {
        let [c, m, y, k] = rgb_to_jxl_ink_f32(px[0], px[1], px[2]);
        for v in [c, m, y, k] {
            out.extend_from_slice(&((v * 65535.0).round() as u16).to_le_bytes());
        }
    }
    Ok(out)
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CmykEncodeOptions {
    pub mode: Option<String>,
    pub distance: Option<f32>,
    pub effort: Option<u8>,
}

fn validate_cmyk_icc(icc: &[u8]) -> Result<(), String> {
    if icc.len() < 128 || icc.get(16..20) != Some(b"CMYK") {
        return Err(
            "CMYK encoding requires a valid ICC profile with the 'CMYK' color space tag.".into(),
        );
    }
    Ok(())
}

/// Encode JXL-convention inverted CMYK8 or CMYK16 (`0 = full ink`, `max = no ink`) with
/// a matching CMYK ICC profile.
///
/// - `mode: "modular"` (default): Lossless Level 10 JXL preserving exact C, M, Y, K codes
///   (K stored in Black extra channel index 0) and exact ICC bytes.
/// - `mode: "vardct"`: Lossy Level 10 JXL that converts CMY/K via the embedded ICC profile
///   to linear sRGB XYB while preserving K as Black extra channel index 0.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn encode_cmyk_jxl(
    cmyk_bytes: &[u8],
    width: u32,
    height: u32,
    bit_depth: u8,
    icc: &[u8],
    options_json: &str,
) -> Result<Vec<u8>, String> {
    limits()
        .check_dimensions(width, height)
        .map_err(|e| e.to_string())?;
    validate_cmyk_icc(icc)?;
    let (layout, bpp) = match bit_depth {
        8 => (PixelLayout::Cmyk8, 4usize),
        16 => (PixelLayout::Cmyk16, 8usize),
        _ => return Err("CMYK bit depth must be 8 or 16.".into()),
    };
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(bpp))
        .ok_or_else(|| "CMYK image byte size overflow.".to_string())?;
    if cmyk_bytes.len() != expected {
        return Err(format!(
            "CMYK buffer length mismatch: expected {expected} bytes, got {}.",
            cmyk_bytes.len()
        ));
    }
    let opts: CmykEncodeOptions = if options_json.trim().is_empty() {
        CmykEncodeOptions::default()
    } else {
        serde_json::from_str(options_json)
            .map_err(|e| format!("Invalid CMYK encoding options: {e}"))?
    };

    // Normalize 16-bit input from little-endian wire bytes to native endian if needed.
    #[cfg(target_endian = "big")]
    let native_buf: Vec<u8>;
    let payload: &[u8] = {
        #[cfg(target_endian = "big")]
        {
            if bit_depth == 16 {
                native_buf = cmyk_bytes
                    .chunks_exact(2)
                    .flat_map(|c| u16::from_le_bytes([c[0], c[1]]).to_ne_bytes())
                    .collect();
                &native_buf
            } else {
                cmyk_bytes
            }
        }
        #[cfg(target_endian = "little")]
        {
            cmyk_bytes
        }
    };

    let metadata = ImageMetadata::new().with_icc_profile(icc);
    let codec_limits = Limits::default().with_max_memory_bytes(MAX_CODEC_MEMORY);
    let mode = opts.mode.as_deref().unwrap_or("modular");
    match mode {
        "modular" => {
            let effort = opts.effort.unwrap_or(3).clamp(1, 10);
            LosslessConfig::new()
                .with_effort(effort)
                .encode_request(width, height, layout)
                .with_metadata(&metadata)
                .with_limits(&codec_limits)
                .encode(payload)
                .map_err(|e| e.to_string())
        }
        "vardct" => {
            let distance = opts.distance.unwrap_or(1.0);
            if !distance.is_finite() || !(0.01..=15.0).contains(&distance) {
                return Err("Lossy CMYK distance must be between 0.01 and 15.0.".into());
            }
            let effort = opts.effort.unwrap_or(4).clamp(1, 10);
            LossyConfig::new(distance)
                .with_effort(effort)
                .encode_request(width, height, layout)
                .with_metadata(&metadata)
                .with_limits(&codec_limits)
                .encode(payload)
                .map_err(|e| e.to_string())
        }
        _ => Err(format!("Unsupported CMYK coding mode '{mode}'.")),
    }
}

/// Render a color-managed sRGB RGBA8 display preview of JXL-convention CMYK8 or CMYK16
/// samples (`0 = full ink`, `max = no ink`) using the provided CMYK ICC profile.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn preview_cmyk_to_rgba8(
    cmyk_bytes: &[u8],
    width: u32,
    height: u32,
    bit_depth: u8,
    icc: &[u8],
) -> Result<Vec<u8>, String> {
    limits()
        .check_dimensions(width, height)
        .map_err(|e| e.to_string())?;
    validate_cmyk_icc(icc)?;
    let bpp = match bit_depth {
        8 => 4usize,
        16 => 8usize,
        _ => return Err("CMYK bit depth must be 8 or 16.".into()),
    };
    let w = width as usize;
    let h = height as usize;
    let count = w
        .checked_mul(h)
        .ok_or_else(|| "CMYK preview size overflow.".to_string())?;
    if cmyk_bytes.len() != count * bpp {
        return Err("CMYK preview buffer size mismatch.".into());
    }
    let src_profile = JxlColorProfile::Icc(icc.to_vec());
    let dst_profile = JxlColorProfile::Simple(JxlColorEncoding::srgb(false));
    let (_, mut converters) = MoxCms
        .initialize_transforms(1, w, src_profile, dst_profile, 255.0)
        .map_err(|e| e.to_string())?;
    let mut cms = converters
        .pop()
        .ok_or_else(|| "CMS transform initialization returned no converter.".to_string())?;

    let mut row_in = vec![0.0f32; w * 4];
    let mut row_out = vec![0.0f32; w * 3];
    let mut rgba = Vec::with_capacity(count * 4);
    let to_u8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;

    for y in 0..h {
        let row_bytes = &cmyk_bytes[y * w * bpp..(y + 1) * w * bpp];
        if bit_depth == 8 {
            for (dst, &src) in row_in.iter_mut().zip(row_bytes.iter()) {
                *dst = f32::from(src) / 255.0;
            }
        } else {
            for (dst, chunk) in row_in.iter_mut().zip(row_bytes.as_chunks::<2>().0.iter()) {
                *dst = f32::from(u16::from_le_bytes(*chunk)) / 65535.0;
            }
        }
        cms.do_transform(&row_in, &mut row_out)
            .map_err(|e| e.to_string())?;
        for rgb in row_out.as_chunks::<3>().0 {
            rgba.extend_from_slice(&[to_u8(rgb[0]), to_u8(rgb[1]), to_u8(rgb[2]), 255]);
        }
    }
    Ok(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenjxl_decoder::api::{
        ExtraChannel, JxlColorType, JxlDataFormat, JxlDecoder, JxlDecoderOptions, JxlOutputBuffer,
        JxlPixelFormat, ProcessingResult,
    };

    fn decode_raw_cmyk_u8(jxl: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let mut input = jxl;
        let options = JxlDecoderOptions::default()
            .with_limits(crate::decoder_limits())
            .with_coalescing(true);
        let ProcessingResult::Complete { result: mut dec } =
            JxlDecoder::new(options).process(&mut input).unwrap()
        else {
            panic!("truncated");
        };
        let icc = match dec.embedded_color_profile() {
            JxlColorProfile::Icc(b) => b.clone(),
            _ => panic!("expected ICC"),
        };
        let info = dec.basic_info().clone();
        assert_eq!(info.extra_channels.len(), 1);
        assert_eq!(info.extra_channels[0].ec_type, ExtraChannel::Black);
        let (w, h) = info.size;
        dec.set_pixel_format(JxlPixelFormat {
            color_type: JxlColorType::Cmyk,
            color_data_format: Some(JxlDataFormat::U8 { bit_depth: 8 }),
            extra_channel_format: vec![None],
        });
        let ProcessingResult::Complete { result: frame } = dec.process(&mut input).unwrap() else {
            panic!("truncated frame");
        };
        let mut out = vec![0u8; w * h * 4];
        let mut bufs = [JxlOutputBuffer::new(&mut out, h, w * 4)];
        let ProcessingResult::Complete { .. } = frame.process(&mut input, &mut bufs).unwrap()
        else {
            panic!("truncated pixels");
        };
        (out, icc)
    }

    #[test]
    fn minimal_cmyk_icc_roundtrips_lossless_cmyk8_cmyk16_and_lossy_cmyk() {
        let icc = generate_minimal_cmyk_icc();
        assert_eq!(&icc[16..20], b"CMYK");
        // Swatches in JXL inverted ink convention (0 = full ink, 255 = no ink):
        // White, Cyan, Magenta, Yellow, Black, Mixed
        let cmyk8: Vec<u8> = [
            [255, 255, 255, 255],
            [0, 255, 255, 255],
            [255, 0, 255, 255],
            [255, 255, 0, 255],
            [255, 255, 255, 0],
            [64, 128, 192, 224],
        ]
        .into_iter()
        .flatten()
        .collect();

        // Preview sanity check: White -> white, Black -> black, Cyan -> cyan
        let preview = preview_cmyk_to_rgba8(&cmyk8, 3, 2, 8, &icc).unwrap();
        assert_eq!(&preview[0..4], &[255, 255, 255, 255], "white swatch");
        assert!(
            preview[4] <= 2 && preview[5] >= 253 && preview[6] >= 253,
            "cyan swatch: {:?}",
            &preview[4..8]
        );
        assert_eq!(&preview[16..20], &[0, 0, 0, 255], "black swatch");

        // 1. Lossless CMYK8 exact code + exact ICC preservation
        let lossless8 =
            encode_cmyk_jxl(&cmyk8, 3, 2, 8, &icc, r#"{"mode":"modular","effort":3}"#).unwrap();
        let (decoded8, decoded_icc) = decode_raw_cmyk_u8(&lossless8);
        assert_eq!(decoded_icc, icc);
        assert_eq!(decoded8, cmyk8);

        // 2. Lossless CMYK16 exact 16-bit code preservation
        let cmyk16_u16: Vec<u16> = cmyk8
            .iter()
            .enumerate()
            .map(|(i, &b)| u16::from(b) * 257 ^ ((i as u16) & 7))
            .collect();
        let cmyk16_bytes: Vec<u8> = cmyk16_u16.iter().flat_map(|v| v.to_le_bytes()).collect();
        let lossless16 =
            encode_cmyk_jxl(&cmyk16_bytes, 3, 2, 16, &icc, r#"{"mode":"modular"}"#).unwrap();
        let mut input = lossless16.as_slice();
        let ProcessingResult::Complete { result: mut dec } =
            JxlDecoder::new(JxlDecoderOptions::default().with_limits(crate::decoder_limits()))
                .process(&mut input)
                .unwrap()
        else {
            panic!();
        };
        assert_eq!(dec.basic_info().bit_depth.bits_per_sample(), 16);
        dec.set_pixel_format(JxlPixelFormat {
            color_type: JxlColorType::Cmyk,
            color_data_format: Some(JxlDataFormat::U16 {
                endianness: zenjxl_decoder::api::Endianness::LittleEndian,
                bit_depth: 16,
            }),
            extra_channel_format: vec![None],
        });
        let ProcessingResult::Complete { result: frame } = dec.process(&mut input).unwrap() else {
            panic!();
        };
        let mut out16 = vec![0u8; 3 * 2 * 8];
        let mut bufs = [JxlOutputBuffer::new(&mut out16, 2, 3 * 8)];
        let ProcessingResult::Complete { .. } = frame.process(&mut input, &mut bufs).unwrap()
        else {
            panic!();
        };
        assert_eq!(out16, cmyk16_bytes);

        // 3. Lossy CMYK (std/moxcms path) on 24x16 swatch blocks preserves ICC + Black extra
        // channel and decodes white/black swatches cleanly.
        let mut swatches = vec![0u8; 24 * 16 * 4];
        let palette = [
            [255u8, 255, 255, 255], // white
            [0, 255, 255, 255],     // cyan
            [255, 0, 255, 255],     // magenta
            [255, 255, 0, 255],     // yellow
            [255, 255, 255, 0],     // black
            [64, 128, 192, 224],    // mixed
        ];
        for y in 0..16usize {
            for x in 0..24usize {
                let idx = (y / 8) * 3 + (x / 8);
                let off = (y * 24 + x) * 4;
                swatches[off..off + 4].copy_from_slice(&palette[idx]);
            }
        }
        let lossy8 = encode_cmyk_jxl(
            &swatches,
            24,
            16,
            8,
            &icc,
            r#"{"mode":"vardct","distance":1.0,"effort":4}"#,
        )
        .unwrap();
        let header = zenjxl_decoder::read_header_with(&lossy8, crate::decoder_limits()).unwrap();
        assert_eq!(header.info.extra_channels.len(), 1);
        assert_eq!(header.info.extra_channels[0].ec_type, ExtraChannel::Black);
        let decoded_rgb = crate::decode_jxl(&lossy8).unwrap().into_pixels();
        assert_eq!(decoded_rgb.len(), 24 * 16 * 4);
        // Center of white 8x8 block (4, 4) stays bright; center of black 8x8 block (12, 12) stays dark.
        let w_off = (4 * 24 + 4) * 4;
        let k_off = (12 * 24 + 12) * 4;
        assert!(
            decoded_rgb[w_off] > 235
                && decoded_rgb[w_off + 1] > 235
                && decoded_rgb[w_off + 2] > 235,
            "white center={:?}",
            &decoded_rgb[w_off..w_off + 4]
        );
        assert!(
            decoded_rgb[k_off] < 20 && decoded_rgb[k_off + 1] < 20 && decoded_rgb[k_off + 2] < 20,
            "black center={:?}",
            &decoded_rgb[k_off..k_off + 4]
        );
    }
}
