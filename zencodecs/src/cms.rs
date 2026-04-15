//! Color management: ICC profile analysis, sRGB detection, and profile synthesis.
//!
//! This module provides the tools needed to decide *whether* a color transform
//! is required and to *build* the ICC profile bytes for that transform. It does
//! NOT apply transforms to pixels -- that's the pipeline's job (e.g. zenpipe's
//! `IccTransformSource`).
//!
//! ## Capabilities
//!
//! - **Structural sRGB detection**: Parse ICC profiles via moxcms and compare
//!   primaries + TRC curves against sRGB. Catches vendor sRGB variants (Canon,
//!   Sony, etc.) that have different bytes but identical color behavior.
//! - **PNG chunk parsing**: Extract gAMA, cHRM, sRGB, and cICP chunks from raw
//!   PNG bytes for color management decisions.
//! - **ICC profile synthesis**: Generate ICC profiles from gAMA+cHRM metadata
//!   or cICP color description values.
//! - **CMS mode**: Configurable strictness for sRGB detection (compat vs strict).
//!
//! ## Feature gate
//!
//! Requires the `cms` feature (which pulls in `moxcms`).

use alloc::vec;
use alloc::vec::Vec;

// ─── CMS mode ───

/// Color management strictness mode.
///
/// Controls how aggressively sRGB detection skips transforms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CmsMode {
    /// Loose sRGB matching (hash lookup + description match).
    /// Vendor-calibrated profiles with "sRGB" in their description are treated
    /// as sRGB -- matches legacy imageflow v2 behavior.
    #[default]
    Compat,

    /// Strict sRGB matching (structural primaries + TRC comparison).
    /// Only skips the transform if the profile's colorimetry actually matches
    /// sRGB within tolerance. More correct, but may apply unnecessary transforms
    /// to vendor profiles that are functionally identical to sRGB.
    SceneReferred,
}

// ─── Cached sRGB ICC profile ───

/// Get the sRGB ICC profile bytes (without CICP to avoid moxcms TRC override).
///
/// Cached after first call. The CICP field is cleared because moxcms issue #154
/// causes CICP transfer characteristics to override the curv/para TRC, which
/// gives wrong results for profiles with non-sRGB transfer functions.
#[cfg(feature = "std")]
pub fn srgb_icc_profile() -> Vec<u8> {
    use std::sync::OnceLock;
    static SRGB: OnceLock<Vec<u8>> = OnceLock::new();
    SRGB.get_or_init(|| {
        let mut profile = moxcms::ColorProfile::new_srgb();
        profile.cicp = None;
        profile.encode().unwrap_or_default()
    })
    .clone()
}

/// Get the sRGB ICC profile bytes (non-cached, no_std version).
#[cfg(not(feature = "std"))]
pub fn srgb_icc_profile() -> Vec<u8> {
    let mut profile = moxcms::ColorProfile::new_srgb();
    profile.cicp = None;
    profile.encode().unwrap_or_default()
}

// ─── Structural sRGB detection ───

/// Check if an ICC profile is sRGB-equivalent by comparing primaries AND TRC curves.
///
/// Uses moxcms to parse the profile and compares colorants (with 0.0001 tolerance
/// via `Xyzd::PartialEq`) and TRC parametric parameters (with tolerance for vendor
/// rounding). Catches vendor sRGB variants (Canon, Sony, etc.) that have different
/// bytes but identical color behavior.
pub fn is_srgb_icc_structural(icc_bytes: &[u8]) -> bool {
    let Ok(src) = moxcms::ColorProfile::new_from_slice(icc_bytes) else {
        return false;
    };
    let srgb = moxcms::ColorProfile::new_srgb();

    // 1. Primaries must match (Xyzd::PartialEq has 0.0001 tolerance).
    if src.red_colorant != srgb.red_colorant
        || src.green_colorant != srgb.green_colorant
        || src.blue_colorant != srgb.blue_colorant
    {
        return false;
    }

    // 2. TRC: must be sRGB-equivalent (parametric or LUT).
    trc_matches_srgb(&src.red_trc)
        && trc_matches_srgb(&src.green_trc)
        && trc_matches_srgb(&src.blue_trc)
}

/// Check if a TRC curve matches the sRGB parametric curve within tolerance.
///
/// sRGB TRC is parametric type 4: `[2.4, 1/1.055, 0.055/1.055, 1/12.92, 0.04045]`.
/// Vendor profiles may round these differently (e.g., 0.947867 vs 0.9479).
fn trc_matches_srgb(trc: &Option<moxcms::ToneReprCurve>) -> bool {
    let Some(trc) = trc else { return false };

    match trc {
        moxcms::ToneReprCurve::Parametric(params) => {
            const SRGB_PARAMS: [f32; 5] = [
                2.4,
                1.0 / 1.055,   // 0.947867...
                0.055 / 1.055, // 0.052132...
                1.0 / 12.92,   // 0.077399...
                0.04045,
            ];
            const TOL: f32 = 0.001;

            if params.len() < 5 {
                return false;
            }
            params[..5]
                .iter()
                .zip(SRGB_PARAMS.iter())
                .all(|(a, b)| (a - b).abs() < TOL)
        }
        moxcms::ToneReprCurve::Lut(lut) => {
            if lut.is_empty() {
                return false;
            }
            let n = lut.len();
            let check_points = [n / 4, n / 2, 3 * n / 4];
            for &idx in &check_points {
                let input = idx as f64 / (n - 1) as f64;
                let expected = if input <= 0.04045 {
                    input / 12.92
                } else {
                    ((input + 0.055) / 1.055).powf(2.4)
                };
                let actual = lut[idx] as f64 / 65535.0;
                if (actual - expected).abs() > 0.002 {
                    return false;
                }
            }
            true
        }
    }
}

/// Determine whether an ICC profile should be treated as sRGB based on CMS mode.
///
/// - `Compat`: hash lookup (fast, covers 22 known profiles) + description match.
/// - `SceneReferred`: structural comparison of primaries + TRC curves.
pub fn is_srgb_for_mode(icc_bytes: &[u8], mode: CmsMode) -> bool {
    match mode {
        CmsMode::Compat => crate::icc_profile_is_srgb(icc_bytes),
        CmsMode::SceneReferred => is_srgb_icc_structural(icc_bytes),
    }
}

// ─── PNG color chunk parsing ───

/// Parsed cICP (Coding-Independent Code Points) values from a PNG chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CicpValues {
    /// Colour primaries: 1=BT.709/sRGB, 9=BT.2020, 12=Display P3.
    pub colour_primaries: u8,
    /// Transfer characteristics: 1=BT.709, 13=sRGB, 16=PQ, 18=HLG.
    pub transfer_characteristics: u8,
    /// Matrix coefficients: 0=identity for RGB.
    pub matrix_coefficients: u8,
    /// Full range flag: 1=full range, 0=video range.
    pub full_range: u8,
}

/// Color metadata extracted from PNG chunks.
#[derive(Clone, Debug, Default)]
pub struct PngColorInfo {
    /// gAMA value (scaled by 100000). E.g., 45455 = gamma 0.45455.
    pub gamma: Option<u32>,
    /// cHRM chromaticities [white_x, white_y, red_x, red_y, green_x, green_y, blue_x, blue_y],
    /// each scaled by 100000.
    pub chromaticities: Option<[u32; 8]>,
    /// Whether an sRGB chunk is present.
    pub has_srgb_chunk: bool,
    /// cICP chunk values, if present.
    pub cicp: Option<CicpValues>,
    /// Whether an iCCP chunk is present (ICC profile handled separately).
    pub has_iccp_chunk: bool,
}

/// Parse PNG color-related chunks from raw PNG bytes.
///
/// Scans for gAMA, cHRM, sRGB, cICP, and iCCP chunks before the first IDAT.
/// Does not parse the full PNG -- stops at IDAT/IEND.
pub fn parse_png_color_chunks(data: &[u8]) -> PngColorInfo {
    let mut info = PngColorInfo::default();

    if data.len() < 8 || &data[0..8] != b"\x89PNG\r\n\x1a\n" {
        return info;
    }
    let mut pos = 8;
    while pos + 8 <= data.len() {
        let len =
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let chunk_type = &data[pos + 4..pos + 8];
        let chunk_data_start = pos + 8;
        let chunk_end = chunk_data_start + len + 4; // +4 for CRC
        if chunk_end > data.len() {
            break;
        }

        match chunk_type {
            b"gAMA" if len == 4 => {
                info.gamma = Some(u32::from_be_bytes([
                    data[chunk_data_start],
                    data[chunk_data_start + 1],
                    data[chunk_data_start + 2],
                    data[chunk_data_start + 3],
                ]));
            }
            b"cHRM" if len == 32 => {
                let d = &data[chunk_data_start..];
                let r =
                    |off: usize| u32::from_be_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]]);
                info.chromaticities = Some([r(0), r(4), r(8), r(12), r(16), r(20), r(24), r(28)]);
            }
            b"sRGB" => {
                info.has_srgb_chunk = true;
            }
            b"iCCP" => {
                info.has_iccp_chunk = true;
            }
            b"cICP" if len == 4 => {
                info.cicp = Some(CicpValues {
                    colour_primaries: data[chunk_data_start],
                    transfer_characteristics: data[chunk_data_start + 1],
                    matrix_coefficients: data[chunk_data_start + 2],
                    full_range: data[chunk_data_start + 3],
                });
            }
            b"IDAT" | b"IEND" => break,
            _ => {}
        }
        pos = chunk_end;
    }
    info
}

// ─── ICC profile synthesis ───

/// Synthesize an ICC profile from PNG gAMA and optional cHRM metadata.
///
/// Returns `None` if:
/// - The gamma value is in the neutral sRGB range (0.4318..0.4773) with sRGB primaries
///   (no transform needed -- the image is effectively sRGB).
/// - The profile cannot be encoded.
///
/// The neutral range is Chrome/Skia's threshold: `gamma * 2.2` within +/-0.05 of 1.0.
pub fn synthesize_icc_from_gama(
    gamma_scaled: u32,
    chromaticities: &Option<[u32; 8]>,
) -> Option<Vec<u8>> {
    let gamma_f = gamma_scaled as f64 / 100000.0;
    let neutral_low = 0.4318;
    let neutral_high = 0.4773;

    let chrm_is_srgb = chromaticities.is_none_or(|c| {
        let srgb = [31270u32, 32900, 64000, 33000, 30000, 60000, 15000, 6000];
        c.iter()
            .zip(srgb.iter())
            .all(|(a, b)| (*a as i64 - *b as i64).unsigned_abs() < 1000)
    });

    if gamma_f >= neutral_low && gamma_f <= neutral_high && chrm_is_srgb {
        return None;
    }

    let display_gamma = 1.0 / gamma_f;
    let mut profile = moxcms::ColorProfile::new_srgb();

    if let Some(c) = chromaticities
        && !chrm_is_srgb
    {
        let white = moxcms::XyY::new(c[0] as f64 / 100000.0, c[1] as f64 / 100000.0, 1.0);
        let primaries = moxcms::ColorPrimaries {
            red: moxcms::Chromaticity {
                x: c[2] as f32 / 100000.0,
                y: c[3] as f32 / 100000.0,
            },
            green: moxcms::Chromaticity {
                x: c[4] as f32 / 100000.0,
                y: c[5] as f32 / 100000.0,
            },
            blue: moxcms::Chromaticity {
                x: c[6] as f32 / 100000.0,
                y: c[7] as f32 / 100000.0,
            },
        };
        profile.update_rgb_colorimetry(white, primaries);
    }

    let trc = moxcms::ToneReprCurve::Parametric(vec![display_gamma as f32]);
    profile.red_trc = Some(trc.clone());
    profile.green_trc = Some(trc.clone());
    profile.blue_trc = Some(trc);

    // Clear CICP to prevent it from overriding our TRC (moxcms issue #154).
    profile.cicp = None;

    profile.encode().ok()
}

/// Synthesize an ICC profile from cICP (Coding-Independent Code Points) values.
///
/// Supports:
/// - Primaries: BT.709 (1), BT.2020 (9), Display P3 (12)
/// - Transfer: BT.709 (1/6), sRGB (13)
///
/// Returns `None` for unrecognized primaries/transfer combinations (e.g., PQ, HLG)
/// which require scene-referred handling, not simple ICC profile synthesis.
pub fn synthesize_icc_from_cicp(cicp: &CicpValues) -> Option<Vec<u8>> {
    let mut profile = moxcms::ColorProfile::new_srgb();

    match cicp.colour_primaries {
        1 => {
            // BT.709 / sRGB primaries -- already default from new_srgb()
        }
        9 => {
            // BT.2020
            let white = moxcms::XyY::new(0.3127, 0.3290, 1.0);
            let primaries = moxcms::ColorPrimaries {
                red: moxcms::Chromaticity { x: 0.708, y: 0.292 },
                green: moxcms::Chromaticity { x: 0.170, y: 0.797 },
                blue: moxcms::Chromaticity { x: 0.131, y: 0.046 },
            };
            profile.update_rgb_colorimetry(white, primaries);
        }
        12 => {
            // Display P3
            let white = moxcms::XyY::new(0.3127, 0.3290, 1.0);
            let primaries = moxcms::ColorPrimaries {
                red: moxcms::Chromaticity { x: 0.680, y: 0.320 },
                green: moxcms::Chromaticity { x: 0.265, y: 0.690 },
                blue: moxcms::Chromaticity { x: 0.150, y: 0.060 },
            };
            profile.update_rgb_colorimetry(white, primaries);
        }
        _ => return None,
    }

    match cicp.transfer_characteristics {
        1 | 6 => {
            // BT.709 / BT.601 transfer
            let trc = moxcms::ToneReprCurve::Parametric(vec![
                0.45_f32, // gamma
                1.099,    // a
                -0.099,   // b (offset)
                4.5,      // c (linear slope)
                0.018,    // d (linear cutoff)
            ]);
            profile.red_trc = Some(trc.clone());
            profile.green_trc = Some(trc.clone());
            profile.blue_trc = Some(trc);
        }
        13 => {
            // sRGB transfer -- leave TRC from new_srgb() as-is.
        }
        _ => return None,
    }

    profile.cicp = None;
    profile.encode().ok()
}

// ─── Transform decision helpers ───

/// Determine the ICC transform needed to convert a source to sRGB.
///
/// Returns `Some((src_icc, dst_icc))` when a transform is needed, or `None`
/// when the source is already sRGB (no transform needed).
///
/// For PNG sources, also checks gAMA/cHRM/cICP chunks if no ICC profile is
/// embedded.
pub fn srgb_transform_icc(
    source_color: &zencodec::decode::SourceColor,
    raw_data: Option<&[u8]>,
    mode: CmsMode,
) -> Option<(Vec<u8>, Vec<u8>)> {
    let dst_icc = srgb_icc_profile();

    // 1. Try embedded ICC profile.
    if let Some(icc) = &source_color.icc_profile
        && !icc.is_empty()
    {
        if is_srgb_for_mode(icc, mode) {
            return None; // Already sRGB
        }
        return Some((icc.to_vec(), dst_icc));
    }

    // 2. Check CICP on SourceColor (non-PNG path).
    if let Some(cicp_val) = source_color.cicp {
        if cicp_val.color_primaries == 1 && cicp_val.transfer_characteristics == 13 {
            return None; // sRGB via CICP
        }
        let cicp = CicpValues {
            colour_primaries: cicp_val.color_primaries,
            transfer_characteristics: cicp_val.transfer_characteristics,
            matrix_coefficients: cicp_val.matrix_coefficients,
            full_range: if cicp_val.full_range { 1 } else { 0 },
        };
        if let Some(src_icc) = synthesize_icc_from_cicp(&cicp) {
            return Some((src_icc, dst_icc));
        }
    }

    // 3. For PNG: parse raw bytes for gAMA/cHRM/cICP chunks.
    if let Some(data) = raw_data {
        return png_srgb_transform_icc(data, mode);
    }

    // 4. No color info -- assume sRGB.
    None
}

/// Determine the ICC transform needed for a PNG source based on its chunks.
///
/// PNG 3rd Ed precedence: cICP > iCCP > sRGB > gAMA+cHRM > assume sRGB.
/// iCCP is handled by the ICC profile path. This handles cICP and gAMA+cHRM.
///
/// `honor_gama_only` controls whether gAMA without cHRM triggers a transform.
/// When false (default), gAMA-only is ignored (matching Chrome/Firefox behavior
/// for most PNGs). When true, gAMA-only uses assumed sRGB primaries.
pub fn png_srgb_transform_icc(data: &[u8], _mode: CmsMode) -> Option<(Vec<u8>, Vec<u8>)> {
    png_srgb_transform_icc_ex(data, false)
}

/// Extended PNG transform with control over gAMA-only behavior.
pub fn png_srgb_transform_icc_ex(data: &[u8], honor_gama_only: bool) -> Option<(Vec<u8>, Vec<u8>)> {
    let info = parse_png_color_chunks(data);
    let dst_icc = srgb_icc_profile();

    // cICP takes highest precedence.
    if let Some(cicp) = info.cicp {
        if cicp.transfer_characteristics == 13 && cicp.colour_primaries == 1 {
            return None; // sRGB
        }
        if let Some(src_icc) = synthesize_icc_from_cicp(&cicp) {
            return Some((src_icc, dst_icc));
        }
        return None; // Unrecognized CICP -- skip
    }

    // iCCP chunk present means ICC profile handled by the caller.
    if info.has_iccp_chunk {
        return None;
    }

    // sRGB chunk -> already sRGB.
    if info.has_srgb_chunk {
        return None;
    }

    let gamma = match info.gamma {
        Some(g) if g > 0 => g,
        _ => return None,
    };

    // Validate cHRM: reject degenerate chromaticities (y=0 causes division by zero).
    if let Some(ref c) = info.chromaticities
        && c.iter().enumerate().any(|(i, v)| i % 2 == 1 && *v == 0)
    {
        return None;
    }

    // gAMA-only without cHRM is ignored unless honor_gama_only is set.
    if info.chromaticities.is_none() && !honor_gama_only {
        return None;
    }

    let src_icc = synthesize_icc_from_gama(gamma, &info.chromaticities)?;
    Some((src_icc, dst_icc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_icc_profile_is_valid() {
        let profile = srgb_icc_profile();
        assert!(!profile.is_empty());
        // Should parse successfully with moxcms.
        let parsed = moxcms::ColorProfile::new_from_slice(&profile);
        assert!(parsed.is_ok());
    }

    #[test]
    fn srgb_icc_detected_structurally() {
        let profile = srgb_icc_profile();
        assert!(is_srgb_icc_structural(&profile));
    }

    #[test]
    fn non_srgb_not_detected() {
        // Empty profile
        assert!(!is_srgb_icc_structural(&[]));
        // Random bytes
        assert!(!is_srgb_icc_structural(&[1, 2, 3, 4]));
    }

    #[test]
    fn neutral_gamma_returns_none() {
        // gamma = 0.45455 (sRGB neutral), no cHRM
        let result = synthesize_icc_from_gama(45455, &None);
        assert!(result.is_none());
    }

    #[test]
    fn non_neutral_gamma_returns_profile() {
        // gamma = 0.22727 (2.2 display gamma)
        let result = synthesize_icc_from_gama(22727, &None);
        assert!(result.is_some());
        let profile = result.unwrap();
        assert!(!profile.is_empty());
    }

    #[test]
    fn cicp_srgb_returns_profile() {
        let cicp = CicpValues {
            colour_primaries: 1,
            transfer_characteristics: 13,
            matrix_coefficients: 0,
            full_range: 1,
        };
        // sRGB CICP -- synthesize should still work (caller decides to skip)
        let result = synthesize_icc_from_cicp(&cicp);
        assert!(result.is_some());
    }

    #[test]
    fn cicp_bt2020_returns_profile() {
        let cicp = CicpValues {
            colour_primaries: 9,
            transfer_characteristics: 1,
            matrix_coefficients: 0,
            full_range: 1,
        };
        let result = synthesize_icc_from_cicp(&cicp);
        assert!(result.is_some());
    }

    #[test]
    fn cicp_pq_returns_none() {
        let cicp = CicpValues {
            colour_primaries: 9,
            transfer_characteristics: 16, // PQ
            matrix_coefficients: 0,
            full_range: 1,
        };
        let result = synthesize_icc_from_cicp(&cicp);
        assert!(result.is_none());
    }

    #[test]
    fn png_parsing_minimal() {
        // Valid PNG header + IHDR + IEND, no color chunks
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        // IHDR chunk (13 bytes data)
        png.extend_from_slice(&[0, 0, 0, 13]); // length
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&[0, 0, 0, 1]); // width=1
        png.extend_from_slice(&[0, 0, 0, 1]); // height=1
        png.push(8); // bit depth
        png.push(2); // color type (RGB)
        png.extend_from_slice(&[0, 0, 0]); // compression, filter, interlace
        png.extend_from_slice(&[0, 0, 0, 0]); // CRC (fake)
        // IEND
        png.extend_from_slice(&[0, 0, 0, 0]);
        png.extend_from_slice(b"IEND");
        png.extend_from_slice(&[0, 0, 0, 0]); // CRC

        let info = parse_png_color_chunks(&png);
        assert!(info.gamma.is_none());
        assert!(info.chromaticities.is_none());
        assert!(!info.has_srgb_chunk);
        assert!(info.cicp.is_none());
    }

    #[test]
    fn cms_mode_default_is_compat() {
        assert_eq!(CmsMode::default(), CmsMode::Compat);
    }
}
