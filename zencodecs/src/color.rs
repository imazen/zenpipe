//! ICC profile classification and sRGB detection.
//!
//! Fast-path sRGB detection via hash lookup against known ICC profile binaries,
//! plus CICP-based detection. This catches ~95% of real-world sRGB images in
//! ~100ns. For the long tail of unknown-but-functionally-sRGB profiles, use
//! structural analysis (primaries/TRC matrix comparison) via a CMS library.

use zencodec::decode::SourceColor;

/// FNV-1a 64-bit hash. Deterministic across all platforms.
const fn fnv1a_64(data: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET;
    let mut i = 0;
    while i < data.len() {
        hash ^= data[i] as u64;
        hash = hash.wrapping_mul(PRIME);
        i += 1;
    }
    hash
}

/// Known sRGB ICC profile FNV-1a 64-bit hashes, sorted for binary search.
///
/// Covers canonical profiles from ICC, HP/Lino, Apple, Google, Facebook,
/// lcms, Ghostscript/Artifex, libjxl Compact-ICC, and ICC.org v4/v5.
/// See `tests/icc_srgb.rs` for verification against actual profile files.
const KNOWN_SRGB_HASHES: [u64; 22] = {
    let h = [
        0x01b2_7967_14a9_5fd5, // sRGB_lcms (656 B)
        0x038b_a989_75d3_6160, // sRGB_LUT — Google Android (2,624 B)
        0x131b_e18b_256c_1005, // sRGB_black_scaled (3,048 B)
        0x190f_0cbe_0744_3404, // sRGB2014 — ICC official (3,024 B)
        0x1b89_293e_8c83_89ad, // colord sRGB — freedesktop/colord (20,420 B)
        0x203c_34c1_fba5_38d2, // sRGB_ICC_v4_Appearance (63,868 B)
        0x43f7_b099_aa77_a523, // Artifex sRGB — Ghostscript (2,576 B)
        0x4b41_6441_92da_c35c, // sRGB_v4_ICC_preference (60,960 B)
        0x569a_1a2b_b183_597a, // Kodak sRGB / KCMS (150,368 B)
        0x56d2_cbfc_a6b5_4318, // sRGB IEC61966-2.1 — HP/Lino (3,144 B)
        0x70d6_01da_f84f_28ff, // Compact-ICC sRGB-v4 (480 B)
        0x7271_2df1_0196_b1db, // Compact-ICC sRGB-v2-micro (456 B)
        0x78cb_2b5d_cdf4_e965, // Compact-ICC sRGB-v2-magic (736 B)
        0x7f3b_a380_1001_a58b, // sRGB_D65_MAT — ICC v5 (24,708 B)
        0x869a_3fee_fd88_a489, // sRGB_ICC_v4_beta (63,928 B)
        0x9b9c_0685_797a_bfdb, // sRGB_ISO22028 — ICC v5 (692 B)
        0xb5fe_02fb_0e03_d19b, // sRGB Facebook (524 B)
        0xbd30_9056_9601_1a32, // Artifex esRGB (12,840 B)
        0xc54d_44a1_49a7_d61a, // Compact-ICC sRGB-v2-nano (410 B)
        0xca3e_5c85_c24b_4889, // sRGB_D65_colorimetric — ICC v5 (24,728 B)
        0xcd42_2ac4_b90b_32b3, // sRGB IEC61966-2.1 — HP/Lino large (7,261 B)
        0xe8a3_3e37_d747_9a46, // sRGB_parametric — Google Android (596 B)
    ];
    // Compile-time assertion: array is sorted
    let mut i = 1;
    while i < h.len() {
        assert!(h[i - 1] < h[i], "KNOWN_SRGB_HASHES must be sorted");
        i += 1;
    }
    h
};

/// Check if an ICC profile is a known sRGB profile by hash lookup.
///
/// Computes a FNV-1a 64-bit hash of the full profile bytes and checks
/// against a table of 22 known sRGB ICC profiles from ICC, HP, Apple,
/// Google, Facebook, lcms, Ghostscript, and libjxl Compact-ICC.
///
/// This is a fast-path check (~50-100ns) that catches the vast majority
/// of real-world sRGB images. Returns `false` for unrecognized profiles —
/// use structural analysis (primaries/TRC comparison) for the long tail.
pub fn icc_profile_is_srgb(icc_bytes: &[u8]) -> bool {
    let hash = fnv1a_64(icc_bytes);
    KNOWN_SRGB_HASHES.binary_search(&hash).is_ok()
}

/// Extension trait for sRGB detection on [`SourceColor`].
pub trait SourceColorExt {
    /// Whether this source is sRGB (no color transform needed).
    ///
    /// Returns `true` when applying a CMS transform from this profile to sRGB
    /// would be an identity operation — skip it to avoid rounding errors.
    ///
    /// Detection tiers:
    /// 1. **CICP** (exact) — primaries=1 (BT.709) + transfer=13 (sRGB)
    /// 2. **ICC hash** (fast) — matches against 22 known sRGB profile binaries
    /// 3. **No metadata** — assumes sRGB (the web/browser default)
    ///
    /// Returns `false` for ICC profiles not in the known set. Use structural
    /// analysis (primaries matrix comparison) for the long tail.
    fn is_srgb(&self) -> bool;
}

impl SourceColorExt for SourceColor {
    fn is_srgb(&self) -> bool {
        if let Some(cicp) = self.cicp {
            return cicp.color_primaries == 1 && cicp.transfer_characteristics == 13;
        }

        if let Some(ref icc) = self.icc_profile {
            return icc_profile_is_srgb(icc);
        }

        // No color info — assume sRGB (the web default).
        true
    }
}
