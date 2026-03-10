//! Metadata parsing for `zcimg info --metadata`.
//!
//! Parses EXIF, ICC profile headers, CICP, and XMP into human-readable
//! structures for display and JSON output.

use std::collections::BTreeMap;

use serde::Serialize;
use zencodec::Cicp;

// --- EXIF ---

/// Curated EXIF tags to display (covers ~95% of useful inspection cases).
///
/// Orientation is excluded — it's already shown in the main info section.
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
    /// Brief explanation of what this profile means in practice.
    pub note: String,
    pub version: String,
    pub profile_class: String,
    pub color_space: String,
    pub pcs: String,
    /// TRC gamma value (if a simple power-law curve).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trc_gamma: Option<f32>,
    /// TRC description (e.g. "sRGB curve", "gamma 2.2", "LUT (1024 entries)").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trc_description: Option<String>,
    /// Transfer function formula, if derivable from the TRC.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trc_formula: Option<String>,
    pub size: usize,
}

/// Identify well-known ICC profiles and return a note explaining their significance.
fn icc_profile_note(description: Option<&str>, color_space: &str, trc_desc: Option<&str>) -> String {
    let desc_lower = description.unwrap_or("").to_lowercase();

    // Match by description — covers most real-world profiles
    if desc_lower.contains("srgb") && !desc_lower.contains("p3") {
        return "Standard web/display color space. Safe for all screens.".to_string();
    }
    if desc_lower.contains("display p3") || desc_lower.contains("dci-p3") {
        return "Wide-gamut display profile (Apple devices, modern displays). ~25% more colors than sRGB.".to_string();
    }
    if desc_lower.contains("adobe rgb") || desc_lower.contains("adobergb") {
        return "Wide-gamut profile for print/photography. Needs color management to display correctly.".to_string();
    }
    if desc_lower.contains("prophoto") || desc_lower.contains("romm rgb") {
        return "Ultra-wide gamut for archival photography. Includes colors outside human vision.".to_string();
    }
    if desc_lower.contains("rec.2020") || desc_lower.contains("bt.2020") || desc_lower.contains("rec2020") {
        return "HDR/broadcast gamut (BT.2020). Used for HDR10, Dolby Vision content.".to_string();
    }
    if desc_lower.contains("aces") {
        return "Academy Color Encoding System. Film/VFX interchange format.".to_string();
    }
    if desc_lower.contains("linear") {
        return "Linear-light profile (gamma 1.0). Used in compositing/rendering pipelines.".to_string();
    }

    // Fallback based on color space
    match color_space {
        "CMYK" => "CMYK profile for print output. Not directly displayable on screens.".to_string(),
        "Gray" => "Grayscale profile.".to_string(),
        "Lab" => "CIE Lab profile. Device-independent color space.".to_string(),
        _ => {
            // Check for known TRC patterns
            if let Some(trc) = trc_desc {
                if trc.contains("sRGB") {
                    return "sRGB transfer function, but non-standard primaries. Uncommon — verify this is intentional.".to_string();
                }
            }
            "Uncommon profile — may need color management to display correctly.".to_string()
        }
    }
}

/// Parse an ICC profile header to extract key fields.
pub fn parse_icc(raw: &[u8]) -> Option<ParsedIcc> {
    if raw.len() < 128 {
        return None;
    }

    let major = raw[8];
    let minor = raw[9] >> 4;
    let bugfix = raw[9] & 0x0F;
    let version = format!("{major}.{minor}.{bugfix}");

    let profile_class = decode_signature(&raw[12..16]);
    let color_space = decode_signature(&raw[16..20]);
    let pcs = decode_signature(&raw[20..24]);

    let description = extract_icc_tag_data(raw, b"desc").and_then(|d| parse_desc_tag(d));
    let trc = extract_icc_tag_data(raw, b"rTRC").and_then(|d| parse_trc_tag(d));
    let (trc_gamma, trc_description, trc_formula) = match trc {
        Some(trc) => (trc.gamma, Some(trc.description), trc.formula),
        None => (None, None, None),
    };

    let cs_name = icc_color_space_name(&color_space);
    let note = icc_profile_note(
        description.as_deref(),
        &cs_name,
        trc_description.as_deref(),
    );

    Some(ParsedIcc {
        description,
        note,
        version,
        profile_class: icc_class_name(&profile_class),
        color_space: cs_name,
        pcs: icc_color_space_name(&pcs),
        trc_gamma,
        trc_description,
        trc_formula,
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

/// Extract raw tag data for a given 4-byte signature from the ICC tag table.
fn extract_icc_tag_data<'a>(raw: &'a [u8], target_sig: &[u8; 4]) -> Option<&'a [u8]> {
    if raw.len() < 132 {
        return None;
    }

    let tag_count = u32::from_be_bytes([raw[128], raw[129], raw[130], raw[131]]) as usize;

    for i in 0..tag_count {
        let base = 132 + i * 12;
        if base + 12 > raw.len() {
            break;
        }
        if &raw[base..base + 4] != target_sig {
            continue;
        }

        let offset =
            u32::from_be_bytes([raw[base + 4], raw[base + 5], raw[base + 6], raw[base + 7]])
                as usize;
        let size =
            u32::from_be_bytes([raw[base + 8], raw[base + 9], raw[base + 10], raw[base + 11]])
                as usize;

        if offset + size <= raw.len() && size >= 8 {
            return Some(&raw[offset..offset + size]);
        }
        return None;
    }

    None
}

/// Parse an ICC `desc` (textDescriptionType) or `mluc` (multiLocalizedUnicodeType) tag.
fn parse_desc_tag(tag_data: &[u8]) -> Option<String> {
    let type_sig = &tag_data[0..4];

    match type_sig {
        b"desc" => {
            if tag_data.len() < 12 {
                return None;
            }
            let str_len =
                u32::from_be_bytes([tag_data[8], tag_data[9], tag_data[10], tag_data[11]]) as usize;
            if str_len == 0 || 12 + str_len > tag_data.len() {
                return None;
            }
            let s = &tag_data[12..12 + str_len];
            let s = std::str::from_utf8(s).ok()?;
            Some(s.trim_end_matches('\0').to_string())
        }
        b"mluc" => {
            if tag_data.len() < 16 {
                return None;
            }
            let num_records =
                u32::from_be_bytes([tag_data[8], tag_data[9], tag_data[10], tag_data[11]]) as usize;
            if num_records == 0 || tag_data.len() < 16 + num_records * 12 {
                return None;
            }
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
    }
}

/// Parsed TRC (Tone Response Curve) from an ICC profile.
struct ParsedTrc {
    gamma: Option<f32>,
    description: String,
    formula: Option<String>,
}

/// Parse an ICC `curv` or `para` TRC tag.
fn parse_trc_tag(tag_data: &[u8]) -> Option<ParsedTrc> {
    let type_sig = &tag_data[0..4];

    match type_sig {
        b"curv" => {
            if tag_data.len() < 12 {
                return None;
            }
            let count =
                u32::from_be_bytes([tag_data[8], tag_data[9], tag_data[10], tag_data[11]]) as usize;
            match count {
                0 => Some(ParsedTrc {
                    gamma: Some(1.0),
                    description: "Linear (gamma 1.0)".to_string(),
                    formula: Some("Y = X".to_string()),
                }),
                1 => {
                    if tag_data.len() < 14 {
                        return None;
                    }
                    let raw = u16::from_be_bytes([tag_data[12], tag_data[13]]);
                    let gamma = raw as f32 / 256.0;
                    let formula = if (gamma - 2.2).abs() < 0.01 {
                        "Y = X^2.2".to_string()
                    } else if (gamma - 1.8).abs() < 0.01 {
                        "Y = X^1.8".to_string()
                    } else {
                        format!("Y = X^{gamma:.2}")
                    };
                    Some(ParsedTrc {
                        gamma: Some(gamma),
                        description: format!("Gamma {gamma:.2}"),
                        formula: Some(formula),
                    })
                }
                n => Some(ParsedTrc {
                    gamma: None,
                    description: format!("LUT ({n} entries)"),
                    formula: None,
                }),
            }
        }
        b"para" => {
            if tag_data.len() < 16 {
                return None;
            }
            let func_type = u16::from_be_bytes([tag_data[8], tag_data[9]]);
            let g_raw =
                i32::from_be_bytes([tag_data[12], tag_data[13], tag_data[14], tag_data[15]]);
            let g = g_raw as f64 / 65536.0;

            let read_s15f16 = |offset: usize| -> Option<f64> {
                if offset + 4 <= tag_data.len() {
                    let raw = i32::from_be_bytes([
                        tag_data[offset],
                        tag_data[offset + 1],
                        tag_data[offset + 2],
                        tag_data[offset + 3],
                    ]);
                    Some(raw as f64 / 65536.0)
                } else {
                    None
                }
            };

            match func_type {
                0 => Some(ParsedTrc {
                    gamma: Some(g as f32),
                    description: format!("Parametric gamma {g:.2}"),
                    formula: Some(format!("Y = X^{g:.4}")),
                }),
                3 => {
                    let a = read_s15f16(16).unwrap_or(0.0);
                    let b = read_s15f16(20).unwrap_or(0.0);
                    let c = read_s15f16(24).unwrap_or(0.0);
                    let d = read_s15f16(28).unwrap_or(0.0);

                    let (desc, formula) = if is_srgb_para(g, a, b, c, d) {
                        (
                            "sRGB curve".to_string(),
                            format!(
                                "X >= {d:.4}: Y = ({a:.6}*X + {b:.6})^{g:.4}\n\
                                 X <  {d:.4}: Y = {c:.6}*X"
                            ),
                        )
                    } else {
                        (
                            format!("Parametric (type 3, gamma {g:.2})"),
                            format!(
                                "X >= {d:.4}: Y = ({a:.6}*X + {b:.6})^{g:.4}\n\
                                 X <  {d:.4}: Y = {c:.6}*X"
                            ),
                        )
                    };
                    Some(ParsedTrc {
                        gamma: Some(g as f32),
                        description: desc,
                        formula: Some(formula),
                    })
                }
                4 => {
                    let a = read_s15f16(16).unwrap_or(0.0);
                    let b = read_s15f16(20).unwrap_or(0.0);
                    let c = read_s15f16(24).unwrap_or(0.0);
                    let d = read_s15f16(28).unwrap_or(0.0);
                    let e = read_s15f16(32).unwrap_or(0.0);
                    let f = read_s15f16(36).unwrap_or(0.0);
                    Some(ParsedTrc {
                        gamma: Some(g as f32),
                        description: format!("Parametric (type 4, gamma {g:.2})"),
                        formula: Some(format!(
                            "X >= {d:.4}: Y = ({a:.6}*X + {b:.6})^{g:.4} + {e:.6}\n\
                             X <  {d:.4}: Y = {c:.6}*X + {f:.6}"
                        )),
                    })
                }
                1 => {
                    let a = read_s15f16(16).unwrap_or(0.0);
                    let b = read_s15f16(20).unwrap_or(0.0);
                    Some(ParsedTrc {
                        gamma: Some(g as f32),
                        description: format!("Parametric (type 1, gamma {g:.2})"),
                        formula: Some(format!("Y = ({a:.6}*X + {b:.6})^{g:.4}")),
                    })
                }
                2 => {
                    let a = read_s15f16(16).unwrap_or(0.0);
                    let b = read_s15f16(20).unwrap_or(0.0);
                    let c = read_s15f16(24).unwrap_or(0.0);
                    Some(ParsedTrc {
                        gamma: Some(g as f32),
                        description: format!("Parametric (type 2, gamma {g:.2})"),
                        formula: Some(format!("Y = ({a:.6}*X + {b:.6})^{g:.4} + {c:.6}")),
                    })
                }
                _ => Some(ParsedTrc {
                    gamma: Some(g as f32),
                    description: format!("Parametric (type {func_type}, gamma {g:.2})"),
                    formula: None,
                }),
            }
        }
        _ => None,
    }
}

/// Check if parametric TRC type 3 parameters match sRGB.
fn is_srgb_para(g: f64, a: f64, b: f64, c: f64, d: f64) -> bool {
    (g - 2.4).abs() < 0.01
        && (a - 1.0 / 1.055).abs() < 0.005
        && (b - 0.055 / 1.055).abs() < 0.005
        && (c - 1.0 / 12.92).abs() < 0.005
        && (d - 0.04045).abs() < 0.005
}

// --- CICP ---

#[derive(Debug, Serialize)]
pub struct ParsedCicp {
    pub color_primaries: u8,
    pub color_primaries_name: String,
    pub transfer_characteristics: u8,
    pub transfer_characteristics_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transfer_formula: Option<String>,
    pub matrix_coefficients: u8,
    pub matrix_coefficients_name: String,
    pub full_range: bool,
    pub summary: String,
    /// Brief explanation of what this CICP combination means in practice.
    pub note: String,
}

/// Identify well-known CICP combinations and return a note.
fn cicp_note(cp: u8, tc: u8, mc: u8, full_range: bool) -> String {
    // Match common combinations
    match (cp, tc, mc) {
        (1, 13, _) if full_range => "sRGB — standard web/display color space.".to_string(),
        (1, 13, _) if !full_range => {
            "sRGB primaries + sRGB transfer but limited range. Unusual for stills — limited range is a video convention.".to_string()
        }
        (1, 1, _) if full_range => {
            "BT.709 primaries + BT.709 transfer. Same primaries as sRGB but different transfer curve (BT.709 vs sRGB).".to_string()
        }
        (12, 13, _) if full_range => {
            "Display P3 — wide-gamut display profile (Apple devices, modern displays). ~25% more colors than sRGB."
                .to_string()
        }
        (9, 16, 9) if full_range => {
            "BT.2100 PQ (HDR10) — wide-gamut HDR with Perceptual Quantizer. Up to 10000 nits."
                .to_string()
        }
        (9, 18, 9) if full_range => {
            "BT.2100 HLG — wide-gamut HDR with Hybrid Log-Gamma. Backwards-compatible with SDR."
                .to_string()
        }
        (9, 16, _) => "BT.2020 gamut with PQ transfer (HDR).".to_string(),
        (9, 18, _) => "BT.2020 gamut with HLG transfer (HDR).".to_string(),
        (9, _, _) => "BT.2020 wide gamut. Used in HDR and broadcast.".to_string(),
        (1, 4, _) => {
            "BT.709 primaries with gamma 2.2 transfer. Common in older AVIF encoders — functionally close to sRGB."
                .to_string()
        }
        (1, 8, _) => {
            "BT.709 primaries, linear transfer. Used in compositing/rendering pipelines.".to_string()
        }
        _ if !full_range => {
            format!(
                "Uncommon combination ({}/{}/{}, limited range). Limited range is unusual for stills — may indicate video-origin content.",
                cp, tc, mc
            )
        }
        _ => format!(
            "Uncommon combination ({}/{}/{}). May need color management for accurate display.",
            cp, tc, mc
        ),
    }
}

/// Transfer function formula for a CICP transfer characteristics code.
fn cicp_transfer_formula(tc: u8) -> Option<&'static str> {
    match tc {
        1 | 6 => Some(
            "V >= 0.018: V = 1.099 * L^0.45 - 0.099\n\
             V <  0.018: V = 4.500 * L",
        ),
        4 => Some("V = L^(1/2.2)  [gamma 2.2]"),
        5 => Some("V = L^(1/2.8)  [gamma 2.8]"),
        8 => Some("V = L  [linear]"),
        13 => Some(
            "X >= 0.0031308: V = 1.055 * L^(1/2.4) - 0.055\n\
             X <  0.0031308: V = 12.92 * L",
        ),
        16 => Some(
            "PQ (SMPTE ST 2084):\n\
             V = ((c1 + c2 * Y^m1) / (1 + c3 * Y^m1))^m2\n\
             c1=0.8359, c2=18.8516, c3=18.6875, m1=0.1593, m2=78.8438\n\
             Y = L / 10000 (normalized to 10000 nits)",
        ),
        18 => Some(
            "HLG (ARIB STD-B67):\n\
             L <= 1/12: V = sqrt(3 * L)\n\
             L >  1/12: V = 0.17883 * ln(12*L - 0.28467) + 0.55991",
        ),
        _ => None,
    }
}

/// Parse CICP into human-readable form using zencodec name helpers.
pub fn parse_cicp(cicp: &Cicp) -> ParsedCicp {
    ParsedCicp {
        color_primaries: cicp.color_primaries,
        color_primaries_name: Cicp::color_primaries_name(cicp.color_primaries).to_string(),
        transfer_characteristics: cicp.transfer_characteristics,
        transfer_characteristics_name: Cicp::transfer_characteristics_name(
            cicp.transfer_characteristics,
        )
        .to_string(),
        transfer_formula: cicp_transfer_formula(cicp.transfer_characteristics).map(String::from),
        matrix_coefficients: cicp.matrix_coefficients,
        matrix_coefficients_name: Cicp::matrix_coefficients_name(cicp.matrix_coefficients)
            .to_string(),
        full_range: cicp.full_range,
        summary: cicp.to_string(),
        note: cicp_note(
            cicp.color_primaries,
            cicp.transfer_characteristics,
            cicp.matrix_coefficients,
            cicp.full_range,
        ),
    }
}

// --- XMP ---

/// An XMP property: either a simple value or a nested structure.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum XmpValue {
    Text(String),
    List(Vec<String>),
    Nested(BTreeMap<String, XmpValue>),
}

/// Parsed XMP metadata as a structured key/value tree.
#[derive(Debug, Serialize)]
pub struct ParsedXmp {
    pub properties: BTreeMap<String, XmpValue>,
    pub namespaces: BTreeMap<String, String>,
}

/// Well-known XMP namespace URIs → short display prefixes.
fn xmp_namespace_label(uri: &str) -> Option<&'static str> {
    // Order: most common first
    match uri {
        "http://purl.org/dc/elements/1.1/" => Some("dc"),
        "http://ns.adobe.com/xap/1.0/" => Some("xmp"),
        "http://ns.adobe.com/exif/1.0/" => Some("exif"),
        "http://ns.adobe.com/tiff/1.0/" => Some("tiff"),
        "http://ns.adobe.com/photoshop/1.0/" => Some("photoshop"),
        "http://ns.adobe.com/xap/1.0/mm/" => Some("xmpMM"),
        "http://ns.adobe.com/xap/1.0/rights/" => Some("xmpRights"),
        "http://ns.adobe.com/camera-raw-settings/1.0/" => Some("crs"),
        "http://ns.adobe.com/hdr-gain-map/1.0/" => Some("hdrgm"),
        "http://ns.google.com/photos/1.0/container/" => Some("Container"),
        "http://ns.google.com/photos/1.0/container/item/" => Some("Item"),
        "http://ns.google.com/photos/1.0/image/" => Some("GImage"),
        "http://cipa.jp/exif/1.0/" => Some("exifEX"),
        "http://ns.adobe.com/xmp/1.0/DynamicMedia/" => Some("xmpDM"),
        "http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/" => Some("Iptc4xmpCore"),
        _ => None,
    }
}

/// Build a prefixed key from a node's namespace + local name.
fn prefixed_key(node: &roxmltree::Node) -> String {
    if let Some(ns) = node.tag_name().namespace() {
        let prefix = xmp_namespace_label(ns).unwrap_or_else(|| {
            // Fall back to document prefix if available
            node.lookup_prefix(ns).unwrap_or("?")
        });
        format!("{}:{}", prefix, node.tag_name().name())
    } else {
        node.tag_name().name().to_string()
    }
}

/// Parse XMP XML into a structured property tree.
pub fn parse_xmp(raw: &[u8]) -> Option<ParsedXmp> {
    let s = std::str::from_utf8(raw).ok()?;
    let doc = roxmltree::Document::parse(s).ok()?;

    let mut properties = BTreeMap::new();
    let mut namespaces = BTreeMap::new();

    // Collect namespace declarations
    for ns in doc.root_element().namespaces() {
        if let Some(prefix) = ns.name() {
            // Skip rdf/x/xml standard prefixes
            if !matches!(prefix, "rdf" | "x" | "xml" | "xmlns") {
                namespaces.insert(prefix.to_string(), ns.uri().to_string());
            }
        }
    }

    // Find rdf:Description elements — that's where XMP properties live
    for desc_node in doc.descendants() {
        if desc_node.tag_name().name() != "Description" {
            continue;
        }

        // Collect namespace declarations from this Description element too
        for ns in desc_node.namespaces() {
            if let Some(prefix) = ns.name() {
                if !matches!(prefix, "rdf" | "x" | "xml" | "xmlns") {
                    namespaces.insert(prefix.to_string(), ns.uri().to_string());
                }
            }
        }

        // Attributes on rdf:Description are simple properties
        for attr in desc_node.attributes() {
            if attr.namespace() == Some("http://www.w3.org/1999/02/22-rdf-syntax-ns#") {
                continue; // Skip rdf:about etc
            }
            let key = if let Some(ns) = attr.namespace() {
                let prefix = xmp_namespace_label(ns)
                    .or_else(|| desc_node.lookup_prefix(ns))
                    .unwrap_or("?");
                format!("{prefix}:{}", attr.name())
            } else {
                attr.name().to_string()
            };
            properties.insert(key, XmpValue::Text(attr.value().to_string()));
        }

        // Child elements are structured properties
        for child in desc_node.children().filter(|n| n.is_element()) {
            let key = prefixed_key(&child);
            let value = extract_xmp_value(&child);
            properties.insert(key, value);
        }
    }

    if properties.is_empty() {
        return None;
    }

    Some(ParsedXmp {
        properties,
        namespaces,
    })
}

/// Extract the value of an XMP property element.
fn extract_xmp_value(node: &roxmltree::Node) -> XmpValue {
    // Check for rdf:Bag, rdf:Seq, rdf:Alt children (list types)
    for child in node.children().filter(|n| n.is_element()) {
        let name = child.tag_name().name();
        if matches!(name, "Bag" | "Seq" | "Alt") {
            let items: Vec<String> = child
                .children()
                .filter(|n| n.is_element() && n.tag_name().name() == "li")
                .map(|li| {
                    // An rdf:li can have attributes (rdf:parseType="Resource") with child elements
                    if li.has_children()
                        && li.children().any(|c| c.is_element())
                        && li.text().map_or(true, |t| t.trim().is_empty())
                    {
                        // Structured list item — collect child elements as key=value
                        let parts: Vec<String> = li
                            .children()
                            .filter(|n| n.is_element())
                            .map(|c| {
                                let k = prefixed_key(&c);
                                let v = c
                                    .text()
                                    .map(|t| t.trim().to_string())
                                    .unwrap_or_default();
                                if v.is_empty() {
                                    // Check attributes
                                    let attrs: Vec<String> = c
                                        .attributes()
                                        .map(|a| format!("{}={}", a.name(), a.value()))
                                        .collect();
                                    if attrs.is_empty() {
                                        k
                                    } else {
                                        format!("{k} ({})", attrs.join(", "))
                                    }
                                } else {
                                    format!("{k}={v}")
                                }
                            })
                            .collect();
                        parts.join(", ")
                    } else {
                        li.text().unwrap_or("").trim().to_string()
                    }
                })
                .filter(|s| !s.is_empty())
                .collect();
            return XmpValue::List(items);
        }
    }

    // Check for nested elements (structured value without rdf container)
    let child_elements: Vec<_> = node.children().filter(|n| n.is_element()).collect();
    if !child_elements.is_empty() {
        let mut nested = BTreeMap::new();
        for child in child_elements {
            let key = prefixed_key(&child);
            let value = extract_xmp_value(&child);
            nested.insert(key, value);
        }
        return XmpValue::Nested(nested);
    }

    // Simple text value
    let text = node.text().unwrap_or("").trim();

    // Also check attributes (some properties store values as attributes)
    if text.is_empty() {
        let attrs: Vec<String> = node
            .attributes()
            .filter(|a| {
                a.namespace() != Some("http://www.w3.org/1999/02/22-rdf-syntax-ns#")
                    && a.name() != "parseType"
            })
            .map(|a| format!("{}={}", a.name(), a.value()))
            .collect();
        if !attrs.is_empty() {
            return XmpValue::Text(attrs.join(", "));
        }
    }

    XmpValue::Text(text.to_string())
}

/// Format parsed XMP as indented key/value text for human display.
pub fn format_xmp_tree(xmp: &ParsedXmp) -> String {
    let mut lines = Vec::new();
    format_xmp_props(&xmp.properties, &mut lines, 0);
    lines.join("\n")
}

fn format_xmp_props(props: &BTreeMap<String, XmpValue>, lines: &mut Vec<String>, depth: usize) {
    let indent = "  ".repeat(depth);
    for (key, value) in props {
        match value {
            XmpValue::Text(s) => {
                if s.is_empty() {
                    lines.push(format!("{indent}{key}"));
                } else {
                    lines.push(format!("{indent}{key}: {s}"));
                }
            }
            XmpValue::List(items) => {
                if items.len() == 1 {
                    lines.push(format!("{indent}{key}: {}", items[0]));
                } else {
                    lines.push(format!("{indent}{key}:"));
                    for item in items {
                        lines.push(format!("{indent}  - {item}"));
                    }
                }
            }
            XmpValue::Nested(nested) => {
                lines.push(format!("{indent}{key}:"));
                format_xmp_props(nested, lines, depth + 1);
            }
        }
    }
}
