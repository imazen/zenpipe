//! Measured JPEG quality-dial ↔ SSIMULACRA2 ↔ bpp calibration.
//!
//! The JPEG/libjpeg quality dial (0–100) is the number Windows, Photoshop, and most
//! tooling expose — it's the human-mindshare quality knob. These tables are the
//! **measured** relationship between that dial and SSIMULACRA2 (and bits-per-pixel),
//! so callers can translate a user-facing quality into a perceptual target (or vice
//! versa) instead of guessing.
//!
//! # Provenance
//!
//! Sweep `jpeg-q-ssim2-cal`, 2026-06-26, on an aarch64 box. Corpus (codec-corpus):
//! CID22 (250) + clic2025 (62) + gb82 (25) + gb82-sc (11) + imazen-26 (154) = **502
//! source images** × {64, 256, 1024, native ≤ 4 MP} sizes × 24 quality values × 2
//! encoders = **81,552 encode+score cells**. Metric: `fast-ssim2` (SSIMULACRA2).
//! Encoders measured:
//! - [`LIBJPEG_TURBO`]: stock libjpeg-turbo 2.1.5 via `cjpeg -quality Q -sample 2x2`
//!   (fixed 4:2:0, Annex-K tables, no trellis).
//! - [`MOZJPEG_EVALCHROMA`]: the imageflow-2 path — mozjpeg 0.10.13 defaults (trellis,
//!   tuned tables, optimized coding) with `evalchroma` 1.0.3 content-adaptive chroma
//!   subsampling.
//!
//! Raw per-cell Parquet (with per-image `bytes`/`bpp`/`chroma`):
//! `/mnt/v/output/jpeg-q-ssim2-cal/2026-06-26/sweep.parquet`. Per-content-class rosetta
//! CSVs alongside it and in `imageflow/benchmarks/jpeg-q-ssim2-2026-06-26/`.
//!
//! # Accuracy (read before trusting a single value)
//!
//! Each anchor is the **median** over the whole corpus at that quality. The
//! conversions are central estimates, and their accuracy differs by axis:
//! - **quality ↔ SSIMULACRA2** is usable: ~±7 SSIMULACRA2 inter-quartile spread
//!   (wider at the tails — high-resolution detail craters at low quality). The two
//!   encoders agree to within ~1–2 points through q95.
//! - **anything involving bpp is content-bound**: at a fixed quality, bpp spans
//!   **5–8×** across images (a flat sky vs dense texture). The `q_to_bpp` result is a
//!   planning median, **not** a per-image prediction. For per-image accuracy you must
//!   measure the image (or predict bpp from image features).
//!
//! # Key finding
//!
//! libjpeg-turbo and mozjpeg+evalchroma track quality→SSIMULACRA2 nearly identically
//! through q95; at q100 mozjpeg's evalchroma adopts 4:4:4 on ~21% of images and reaches
//! 91.9 vs libjpeg-turbo's 4:2:0-capped 88.4. mozjpeg is 15–30% more byte-efficient at
//! equal quality, with the largest edge at low bitrate (at 0.5 bpp, mozjpeg ≈ 40
//! SSIMULACRA2 vs libjpeg-turbo's ≈ 18) — i.e. exactly the aggressive-web regime.

// This module is `pub(crate)` and not yet wired into the encode path — the tables
// and helpers are staged for an internal quality-conversion seam. Allow dead_code
// until that seam lands; drop this once the helpers have crate-internal callers.
#![allow(dead_code)]

/// One measured anchor: `(quality, median SSIMULACRA2, median bits-per-pixel)`.
///
/// `quality` is the libjpeg/mozjpeg 0–100 dial. `bpp` is content-bound — a planning
/// median, not a per-image value (see the module accuracy note).
pub(crate) type QCalAnchor = (f32, f32, f32);

/// Stock **libjpeg-turbo** (4:2:0, Annex-K, no trellis) — the human-mindshare dial.
/// Fixed 4:2:0 caps achievable SSIMULACRA2 (~88 even at q100).
pub(crate) const LIBJPEG_TURBO: &[QCalAnchor] = &[
    (5.0, -26.4, 0.33),
    (10.0, 10.5, 0.46),
    (15.0, 28.6, 0.57),
    (20.0, 39.2, 0.66),
    (25.0, 46.1, 0.74),
    (30.0, 51.1, 0.83),
    (35.0, 55.4, 0.90),
    (40.0, 58.1, 0.97),
    (45.0, 60.7, 1.04),
    (50.0, 62.9, 1.10),
    (55.0, 64.7, 1.17),
    (60.0, 66.4, 1.25),
    (65.0, 68.6, 1.35),
    (70.0, 70.9, 1.48),
    (75.0, 73.4, 1.61),
    (80.0, 76.1, 1.82),
    (85.0, 79.1, 2.11),
    (88.0, 81.0, 2.39),
    (90.0, 82.5, 2.61),
    (92.0, 83.9, 2.87),
    (94.0, 85.5, 3.34),
    (96.0, 86.9, 4.03),
    (98.0, 87.8, 5.01),
    (100.0, 88.4, 6.85),
];

/// **imageflow-2 mozjpeg + evalchroma** (trellis + tuned tables + content-adaptive
/// chroma). More byte-efficient than libjpeg-turbo at equal quality, and its chroma
/// adaptation raises the q100 ceiling to ~91.9.
pub(crate) const MOZJPEG_EVALCHROMA: &[QCalAnchor] = &[
    (5.0, -37.3, 0.15),
    (10.0, 2.0, 0.25),
    (15.0, 22.0, 0.35),
    (20.0, 32.9, 0.43),
    (25.0, 41.3, 0.51),
    (30.0, 46.8, 0.58),
    (35.0, 51.4, 0.65),
    (40.0, 54.9, 0.72),
    (45.0, 57.4, 0.78),
    (50.0, 60.2, 0.84),
    (55.0, 62.4, 0.90),
    (60.0, 64.3, 0.97),
    (65.0, 67.0, 1.07),
    (70.0, 69.2, 1.17),
    (75.0, 72.3, 1.30),
    (80.0, 75.1, 1.50),
    (85.0, 78.0, 1.77),
    (88.0, 80.1, 2.01),
    (90.0, 81.8, 2.24),
    (92.0, 83.3, 2.50),
    (94.0, 85.2, 2.97),
    (96.0, 87.4, 3.75),
    (98.0, 89.7, 4.89),
    (100.0, 91.9, 6.80),
];

/// Quality dial → median SSIMULACRA2 (piecewise-linear over the anchors, clamped to
/// the endpoints). See the module accuracy note: this is a central estimate.
#[must_use]
pub(crate) fn q_to_ssim2(table: &[QCalAnchor], q: f32) -> f32 {
    pwl(q, table, |a| a.0, |a| a.1)
}

/// Median SSIMULACRA2 → quality dial — inverts the monotone curve. Returns the dial
/// whose median SSIMULACRA2 matches `ssim2`; clamps outside the measured range.
#[must_use]
pub(crate) fn ssim2_to_q(table: &[QCalAnchor], ssim2: f32) -> f32 {
    pwl(ssim2, table, |a| a.1, |a| a.0)
}

/// Quality dial → median bits-per-pixel. ⚠ **content-bound** (5–8× spread at fixed
/// quality) — a planning estimate, never a per-image prediction.
#[must_use]
pub(crate) fn q_to_bpp(table: &[QCalAnchor], q: f32) -> f32 {
    pwl(q, table, |a| a.0, |a| a.2)
}

/// Piecewise-linear interpolation of `key(anchor) -> val(anchor)`, clamped to the
/// endpoints. The `key` axis must be non-decreasing across the table (true for both
/// quality and SSIMULACRA2, which rise together).
fn pwl(
    x: f32,
    table: &[QCalAnchor],
    key: impl Fn(&QCalAnchor) -> f32,
    val: impl Fn(&QCalAnchor) -> f32,
) -> f32 {
    debug_assert!(table.len() >= 2);
    if x <= key(&table[0]) {
        return val(&table[0]);
    }
    for w in table.windows(2) {
        let (x0, x1) = (key(&w[0]), key(&w[1]));
        if x <= x1 {
            let span = x1 - x0;
            let t = if span > 0.0 { (x - x0) / span } else { 0.0 };
            return val(&w[0]) + t * (val(&w[1]) - val(&w[0]));
        }
    }
    val(&table[table.len() - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssim2_is_monotone_in_quality() {
        for table in [LIBJPEG_TURBO, MOZJPEG_EVALCHROMA] {
            let mut prev = f32::NEG_INFINITY;
            for q in (5..=100).step_by(5) {
                let s = q_to_ssim2(table, q as f32);
                assert!(
                    s >= prev,
                    "ssim2 must be non-decreasing in q (q={q}, s={s})"
                );
                prev = s;
            }
        }
    }

    #[test]
    fn quality_ssim2_roundtrips() {
        for q in [30.0_f32, 60.0, 85.0, 95.0] {
            let s = q_to_ssim2(LIBJPEG_TURBO, q);
            let q2 = ssim2_to_q(LIBJPEG_TURBO, s);
            assert!(
                (q - q2).abs() < 1.0,
                "roundtrip q={q} -> {q2} (via ssim2 {s})"
            );
        }
    }

    #[test]
    fn anchors_and_clamping() {
        // exact measured anchors
        assert!((q_to_ssim2(LIBJPEG_TURBO, 90.0) - 82.5).abs() < 0.05);
        assert!((q_to_ssim2(MOZJPEG_EVALCHROMA, 100.0) - 91.9).abs() < 0.05);
        // clamps below/above range
        assert_eq!(q_to_ssim2(LIBJPEG_TURBO, 0.0), -26.4);
        assert_eq!(q_to_ssim2(LIBJPEG_TURBO, 200.0), 88.4);
        // mozjpeg is more byte-efficient at equal quality
        assert!(q_to_bpp(MOZJPEG_EVALCHROMA, 90.0) < q_to_bpp(LIBJPEG_TURBO, 90.0));
    }
}
