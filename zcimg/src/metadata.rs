//! Metadata parsing for `zcimg info --metadata`.
//!
//! Parses EXIF, ICC profile headers, CICP, and XMP into human-readable
//! structures for display and JSON output.

use std::collections::BTreeMap;

use serde::Serialize;
use zencodec_types::Cicp;

// --- EXIF ---

/// Curated EXIF tags to display (covers ~95% of useful inspection cases).
const DISPLAY_TAGS: &[(exif::Tag, &str)] = &[
    (exif::Tag::Make, "Make"),
    (exif::Tag::Model, "Model"),
    (exif::Tag::Software, "Software"),
    (exif::Tag::Artist, "Artist"),
    (exif::Tag::Copyright, "Copyright"),
    (exif::Tag::ImageDescription, "Description"),
    (exif::Tag::DateTimeOriginal, "Date/Time Original"),
    (exif::Tag::DateTime, "Date/Time"),
    (exif::Tag::DateTimeDigitized, "Date/Time Digitized"),
    (exif::Tag::ExposureTime, "Exposure Time"),
    (exif::Tag::FNumber, "F-Number"),
    (exif::Tag::PhotographicSensitivity, "ISO"),
    (exif::Tag::ExposureProgram, "Exposure Program"),
    (exif::Tag::FocalLength, "Focal Length"),
    (exif::Tag::FocalLengthIn35mmFilm, "Focal Length (35mm)"),
    (exif::Tag::LensModel, "Lens Model"),
    (exif::Tag::WhiteBalance, "White Balance"),
    (exif::Tag::Flash, "Flash"),
    (exif::Tag::MeteringMode, "Metering Mode"),
    (exif::Tag::Orientation, "Orientation"),
    (exif::Tag::GPSLatitude, "GPS Latitude"),
    (exif::Tag::GPSLatitudeRef, "GPS Latitude Ref"),
    (exif::Tag::GPSLongitude, "GPS Longitude"),
    (exif::Tag::GPSLongitudeRef, "GPS Longitude Ref"),
    (exif::Tag::GPSAltitude, "GPS Altitude"),
];

#[derive(Debug, Serialize)]
pub struct ParsedExif {
    pub fields: BTreeMap<String, String>,
    pub total_tags: usize,
}

/// Parse raw EXIF bytes into a curated set of human-readable fields.
///
/// Handles both raw TIFF data (starting with `II`/`MM`) and APP1 payload
/// (starting with `Exif\0\0` followed by TIFF data).
pub fn parse_exif(raw: &[u8]) -> Option<ParsedExif> {
    // Strip "Exif\0\0" APP1 prefix if present — kamadak-exif expects raw TIFF
    let tiff_data = if raw.starts_with(b"Exif\0\0") {
        raw[6..].to_vec()
    } else {
        raw.to_vec()
    };

    let reader = exif::Reader::new();
    let exif = reader.read_raw(tiff_data).ok()?;

    let total_tags = exif.fields().count();
    let mut fields = BTreeMap::new();

    for &(tag, label) in DISPLAY_TAGS {
        if let Some(field) = exif.get_field(tag, exif::In::PRIMARY) {
            let value = field.display_value().with_unit(&exif).to_string();
            if !value.is_empty() {
                fields.insert(label.to_string(), value);
            }
        }
    }

    Some(ParsedExif { fields, total_tags })
}

// --- ICC ---

#[derive(Debug, Serialize)]
pub struct ParsedIcc {
    pub description: Option<String>,
    pub version: String,
    pub profile_class: String,
    pub color_space: String,
    pub pcs: String,
    pub size: usize,
}

/// Parse an ICC profile header to extract key fields.
///
/// The ICC header is a well-defined 128-byte structure. We also scan the
/// tag table for a `desc` tag to extract the profile description.
pub fn parse_icc(raw: &[u8]) -> Option<ParsedIcc> {
    if raw.len() < 128 {
        return None;
    }

    // Version: bytes 8-11, major.minor.bugfix
    let major = raw[8];
    let minor = raw[9] >> 4;
    let bugfix = raw[9] & 0x0F;
    let version = format!("{major}.{minor}.{bugfix}");

    // Profile class: bytes 12-15 (4-char signature)
    let profile_class = decode_signature(&raw[12..16]);

    // Color space: bytes 16-19
    let color_space = decode_signature(&raw[16..20]);

    // PCS (Profile Connection Space): bytes 20-23
    let pcs = decode_signature(&raw[20..24]);

    // Try to extract description from tag table
    let description = extract_icc_description(raw);

    Some(ParsedIcc {
        description,
        version,
        profile_class: icc_class_name(&profile_class),
        color_space: icc_color_space_name(&color_space),
        pcs: icc_color_space_name(&pcs),
        size: raw.len(),
    })
}

/// Decode a 4-byte ICC signature into a trimmed ASCII string.
fn decode_signature(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..4]).trim().to_string()
}

/// Human-readable ICC profile class name.
fn icc_class_name(sig: &str) -> String {
    match sig {
        "scnr" => "Input (Scanner)".to_string(),
        "mntr" => "Display".to_string(),
        "prtr" => "Output (Printer)".to_string(),
        "link" => "Device Link".to_string(),
        "spac" => "Color Space".to_string(),
        "abst" => "Abstract".to_string(),
        "nmcl" => "Named Color".to_string(),
        other => other.to_string(),
    }
}

/// Human-readable ICC color space name.
fn icc_color_space_name(sig: &str) -> String {
    match sig {
        "XYZ" => "XYZ".to_string(),
        "Lab" => "Lab".to_string(),
        "Luv" => "Luv".to_string(),
        "YCbr" | "YCbC" => "YCbCr".to_string(),
        "Yxy" => "Yxy".to_string(),
        "RGB" => "RGB".to_string(),
        "GRAY" => "Gray".to_string(),
        "HSV" => "HSV".to_string(),
        "HLS" => "HLS".to_string(),
        "CMYK" => "CMYK".to_string(),
        "CMY" => "CMY".to_string(),
        other => other.to_string(),
    }
}

/// Extract profile description from the ICC tag table.
///
/// Handles both v2 `textDescriptionType` and v4 `multiLocalizedUnicodeType`.
fn extract_icc_description(raw: &[u8]) -> Option<String> {
    if raw.len() < 132 {
        return None;
    }

    // Tag count at bytes 128-131
    let tag_count = u32::from_be_bytes([raw[128], raw[129], raw[130], raw[131]]) as usize;

    // Each tag entry is 12 bytes: signature(4) + offset(4) + size(4)
    for i in 0..tag_count {
        let base = 132 + i * 12;
        if base + 12 > raw.len() {
            break;
        }
        let sig = &raw[base..base + 4];
        if sig != b"desc" {
            continue;
        }

        let offset =
            u32::from_be_bytes([raw[base + 4], raw[base + 5], raw[base + 6], raw[base + 7]])
                as usize;
        let size =
            u32::from_be_bytes([raw[base + 8], raw[base + 9], raw[base + 10], raw[base + 11]])
                as usize;

        if offset + size > raw.len() || size < 8 {
            return None;
        }

        let tag_data = &raw[offset..offset + size];
        let type_sig = &tag_data[0..4];

        return match type_sig {
            // v2: textDescriptionType ('desc')
            b"desc" => {
                if tag_data.len() < 12 {
                    return None;
                }
                let str_len =
                    u32::from_be_bytes([tag_data[8], tag_data[9], tag_data[10], tag_data[11]])
                        as usize;
                if str_len == 0 || 12 + str_len > tag_data.len() {
                    return None;
                }
                // ASCII string, may have trailing NUL
                let s = &tag_data[12..12 + str_len];
                let s = std::str::from_utf8(s).ok()?;
                Some(s.trim_end_matches('\0').to_string())
            }
            // v4: multiLocalizedUnicodeType ('mluc')
            b"mluc" => {
                if tag_data.len() < 16 {
                    return None;
                }
                let num_records =
                    u32::from_be_bytes([tag_data[8], tag_data[9], tag_data[10], tag_data[11]])
                        as usize;
                if num_records == 0 || tag_data.len() < 16 + num_records * 12 {
                    return None;
                }
                // Use first record
                let rec_base = 16;
                let str_len = u32::from_be_bytes([
                    tag_data[rec_base + 4],
                    tag_data[rec_base + 5],
                    tag_data[rec_base + 6],
                    tag_data[rec_base + 7],
                ]) as usize;
                let str_offset = u32::from_be_bytes([
                    tag_data[rec_base + 8],
                    tag_data[rec_base + 9],
                    tag_data[rec_base + 10],
                    tag_data[rec_base + 11],
                ]) as usize;

                if str_offset + str_len > tag_data.len() || str_len < 2 {
                    return None;
                }

                // UTF-16BE
                let utf16_data = &tag_data[str_offset..str_offset + str_len];
                let chars: Vec<u16> = utf16_data
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16(&chars)
                    .ok()
                    .map(|s| s.trim_end_matches('\0').to_string())
            }
            _ => None,
        };
    }

    None
}

// --- CICP ---

#[derive(Debug, Serialize)]
pub struct ParsedCicp {
    pub color_primaries: u8,
    pub color_primaries_name: String,
    pub transfer_characteristics: u8,
    pub transfer_characteristics_name: String,
    pub matrix_coefficients: u8,
    pub matrix_coefficients_name: String,
    pub full_range: bool,
    pub summary: String,
}

/// Parse CICP into human-readable form using zencodec-types name helpers.
pub fn parse_cicp(cicp: &Cicp) -> ParsedCicp {
    ParsedCicp {
        color_primaries: cicp.color_primaries,
        color_primaries_name: Cicp::color_primaries_name(cicp.color_primaries).to_string(),
        transfer_characteristics: cicp.transfer_characteristics,
        transfer_characteristics_name: Cicp::transfer_characteristics_name(
            cicp.transfer_characteristics,
        )
        .to_string(),
        matrix_coefficients: cicp.matrix_coefficients,
        matrix_coefficients_name: Cicp::matrix_coefficients_name(cicp.matrix_coefficients)
            .to_string(),
        full_range: cicp.full_range,
        summary: cicp.to_string(),
    }
}

// --- XMP ---

/// Return XMP as a UTF-8 string (XMP is human-readable XML).
pub fn parse_xmp(raw: &[u8]) -> Option<String> {
    std::str::from_utf8(raw).ok().map(String::from)
}
