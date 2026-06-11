//! ICC profile classification and sRGB detection.
//!
//! Fast-path sRGB detection via normalized-hash lookup against known ICC
//! profile binaries, plus CICP-based detection. This catches ~95% of
//! real-world sRGB images in well under a microsecond. For the long tail of
//! unknown-but-functionally-sRGB profiles, use structural analysis
//! (primaries/TRC matrix comparison) via a CMS library.

use zencodec::decode::SourceColor;

/// Check if an ICC profile is a known sRGB profile.
///
/// Delegates to [`zenpixels::icc::is_common_srgb`]: a normalized-hash lookup
/// over the web-corpus-verified table of common profiles. Normalization makes
/// it robust to timestamp/padding-only variants — a superset of the exact-byte
/// FNV table this crate used to carry.
///
/// Returns `false` for unrecognized profiles — use structural analysis
/// (primaries/TRC comparison, see `cms::is_srgb_icc_structural`) for the
/// long tail.
pub fn icc_profile_is_srgb(icc_bytes: &[u8]) -> bool {
    zenpixels::icc::is_common_srgb(icc_bytes)
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
    /// 2. **ICC hash** (fast) — normalized-hash match against known sRGB profiles
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
