//! Verification of `icc_profile_is_srgb` against real ICC profile files.
//!
//! These tests read actual ICC profiles from the skcms and Compact-ICC-Profiles
//! directories shipped in the jpegli-cpp third-party tree. They verify that:
//! - All known sRGB profiles are detected
//! - Non-sRGB profiles with confusingly similar names are rejected
//! - The hash function produces correct, deterministic results

use zencodecs::{SourceColorExt, icc_profile_is_srgb};

/// Path to the skcms profiles directory (from jpegli-cpp third-party).
const SKCMS: &str = concat!(
    env!("HOME"),
    "/work/zen/zenjpeg/internal/jpegli-cpp/third_party/skcms/profiles"
);

/// Path to Compact-ICC-Profiles (from jpegli-cpp testdata).
const COMPACT: &str = concat!(
    env!("HOME"),
    "/work/zen/zenjpeg/internal/jpegli-cpp/testdata/external/Compact-ICC-Profiles/profiles"
);

fn read_profile(base: &str, name: &str) -> Vec<u8> {
    let path = format!("{base}/{name}");
    std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

fn check(base: &str, name: &str, expected: bool) {
    let data = read_profile(base, name);
    let result = icc_profile_is_srgb(&data);
    assert_eq!(
        result,
        expected,
        "{name}: expected {expected}, got {result} (size={})",
        data.len()
    );
}

// ── Tier 1: Most common in the wild ────────────────────────────────────

#[test]
fn srgb_hp_3144() {
    check(SKCMS, "misc/sRGB_HP.icc", true);
}

#[test]
fn srgb_hp_7261() {
    check(SKCMS, "misc/sRGB_HP_2.icc", true);
}

#[test]
fn srgb_2014_icc_official() {
    check(SKCMS, "color.org/sRGB2014.icc", true);
}

#[test]
fn srgb_facebook() {
    check(SKCMS, "sRGB_Facebook.icc", true);
}

#[test]
fn srgb_parametric_google() {
    check(SKCMS, "mobile/sRGB_parametric.icc", true);
}

// ── Tier 2: Common in software/export pipelines ────────────────────────

#[test]
fn srgb_lcms() {
    check(SKCMS, "misc/sRGB_lcms.icc", true);
}

#[test]
fn srgb_lut_google() {
    check(SKCMS, "mobile/sRGB_LUT.icc", true);
}

#[test]
fn srgb_black_scaled() {
    check(SKCMS, "misc/sRGB_black_scaled.icc", true);
}

/// `Kodak_sRGB.icc` is a 150 KB LUT profile with a real perceptual
/// rendering: moxcms measures max u16 identity error 15265 (~59 u8 steps)
/// under perceptual intent (rel-colorimetric/saturation: unsupported
/// profile connection). Substituting "it's sRGB, skip the CMS" would
/// silently drop that transform — the pre-0.2.13 `true` was a bug.
#[test]
fn srgb_kodak() {
    check(SKCMS, "misc/Kodak_sRGB.icc", false);
}

// ── Compact-ICC-Profiles (libjxl) ──────────────────────────────────────

#[test]
fn compact_srgb_v2_magic() {
    check(COMPACT, "sRGB-v2-magic.icc", true);
}

#[test]
fn compact_srgb_v2_micro() {
    check(COMPACT, "sRGB-v2-micro.icc", true);
}

#[test]
fn compact_srgb_v2_nano() {
    check(COMPACT, "sRGB-v2-nano.icc", true);
}

#[test]
fn compact_srgb_v4() {
    check(COMPACT, "sRGB-v4.icc", true);
}

// ── ICC.org v4/v5 variants ─────────────────────────────────────────────
//
// None of these are skip-CMS-safe, so `is_common_srgb` must say `false`
// (zenpipe#42, measured 2026-06-11 via `icc-gen`'s probe42 — moxcms
// RGB-lattice identity error against sRGB, per rendering intent):
//
// - v5/iccMAX profiles (`sRGB_D65_MAT`, `sRGB_D65_colorimetric`,
//   `sRGB_ISO22028` — header version 05000000; ISO22028 is even class
//   `cenc`): the production CMS (moxcms) cannot parse iccMAX at all, so
//   matrix+TRC equivalence is unverifiable and substitution untestable.
// - v4 LUT profiles (`Appearance`, `beta`, `preference`): genuinely
//   non-identity renderings — perceptual max u16 error 20014–28671
//   (~78–112 u8 steps), rel-colorimetric 2403–7610 (~9–30 u8 steps).
//   The v4 *preference* profile is deliberately non-colorimetric.
//
// The pre-0.2.13 recognizer returned `true` for all of these, silently
// skipping real color transforms.

#[test]
fn srgb_d65_mat_v5() {
    check(SKCMS, "color.org/sRGB_D65_MAT.icc", false);
}

#[test]
fn srgb_d65_colorimetric_v5() {
    check(SKCMS, "color.org/sRGB_D65_colorimetric.icc", false);
}

#[test]
fn srgb_v4_appearance() {
    check(SKCMS, "color.org/sRGB_ICC_v4_Appearance.icc", false);
}

#[test]
fn srgb_iso22028() {
    check(SKCMS, "color.org/sRGB_ISO22028.icc", false);
}

#[test]
fn srgb_v4_preference() {
    check(SKCMS, "color.org/sRGB_v4_ICC_preference.icc", false);
}

#[test]
fn srgb_v4_beta() {
    check(SKCMS, "misc/sRGB_ICC_v4_beta.icc", false);
}

// ── System profiles ────────────────────────────────────────────────────

#[test]
fn ghostscript_srgb() {
    let path = "/usr/share/color/icc/ghostscript/srgb.icc";
    if let Ok(data) = std::fs::read(path) {
        assert!(
            icc_profile_is_srgb(&data),
            "ghostscript srgb.icc should match"
        );
    }
}

/// e-sRGB (PIMA 7667) is the *extended-range* sRGB variant — its encoding
/// maps a wider range, so the transform to plain sRGB is nowhere near
/// identity: moxcms measures max u16 error 24575 (~96 u8 steps) across
/// all rendering intents. Recognizing it as sRGB was a pre-0.2.13 bug.
#[test]
fn ghostscript_esrgb() {
    let path = "/usr/share/color/icc/ghostscript/esrgb.icc";
    if let Ok(data) = std::fs::read(path) {
        assert!(
            !icc_profile_is_srgb(&data),
            "ghostscript esrgb.icc (e-sRGB, extended-range) must NOT match plain sRGB"
        );
    }
}

#[test]
fn colord_srgb() {
    let path = "/usr/share/color/icc/colord/sRGB.icc";
    if let Ok(data) = std::fs::read(path) {
        assert!(icc_profile_is_srgb(&data), "colord sRGB.icc should match");
    }
}

// ── Negative tests: NOT sRGB ───────────────────────────────────────────

#[test]
fn not_srgb_calibrated_heterogeneous() {
    check(SKCMS, "misc/sRGB_Calibrated_Heterogeneous.icc", false);
}

#[test]
fn not_srgb_calibrated_homogeneous() {
    check(SKCMS, "misc/sRGB_Calibrated_Homogeneous.icc", false);
}

#[test]
fn not_compact_scrgb() {
    // scRGB has same primaries but linear transfer — NOT sRGB.
    check(COMPACT, "scRGB-v2.icc", false);
}

#[test]
fn not_display_p3() {
    check(COMPACT, "DisplayP3-v4.icc", false);
}

#[test]
fn not_rec2020() {
    check(COMPACT, "Rec2020-v4.icc", false);
}

#[test]
fn not_adobe_rgb() {
    check(SKCMS, "misc/AdobeRGB.icc", false);
}

#[test]
fn not_apple_wide_color() {
    check(SKCMS, "misc/Apple_Wide_Color.icc", false);
}

// ── Edge cases ─────────────────────────────────────────────────────────

#[test]
fn empty_bytes() {
    assert!(!icc_profile_is_srgb(&[]));
}

#[test]
fn too_short() {
    assert!(!icc_profile_is_srgb(&[0u8; 100]));
}

#[test]
fn random_bytes() {
    assert!(!icc_profile_is_srgb(&[0xDE, 0xAD, 0xBE, 0xEF]));
}

// ── SourceColorExt::is_srgb() integration ──────────────────────────────

#[test]
fn source_color_cicp_srgb() {
    use zencodecs::SourceColor;
    use zenpixels::Cicp;
    let sc = SourceColor::default().with_cicp(Cicp::SRGB);
    assert!(sc.is_srgb());
}

#[test]
fn source_color_cicp_pq() {
    use zencodecs::SourceColor;
    use zenpixels::Cicp;
    let sc = SourceColor::default().with_cicp(Cicp::BT2100_PQ);
    assert!(!sc.is_srgb());
}

#[test]
fn source_color_no_metadata_assumes_srgb() {
    use zencodecs::SourceColor;
    assert!(SourceColor::default().is_srgb());
}

#[test]
fn source_color_known_icc_profile() {
    use std::sync::Arc;
    use zencodecs::SourceColor;

    let data = read_profile(SKCMS, "misc/sRGB_HP.icc");
    let sc = SourceColor::default().with_icc_profile(Arc::<[u8]>::from(data.as_slice()));
    assert!(sc.is_srgb());
}

#[test]
fn source_color_unknown_icc_profile() {
    use std::sync::Arc;
    use zencodecs::SourceColor;

    let data = read_profile(SKCMS, "misc/AdobeRGB.icc");
    let sc = SourceColor::default().with_icc_profile(Arc::<[u8]>::from(data.as_slice()));
    assert!(!sc.is_srgb());
}
