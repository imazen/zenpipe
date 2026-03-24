//! Precomputed gamut boundary lookup table and soft chroma compression.
//!
//! For a given color primaries set, computes the maximum Oklch chroma
//! that stays within gamut at each (L, hue) pair. The soft compress
//! function smoothly reduces out-of-gamut chroma using a rational
//! knee function that preserves hue and lightness.

use zenpixels_convert::gamut::GamutMatrix;
use zenpixels_convert::oklab;

/// Precomputed sRGB (or P3/BT.2020) gamut boundary in Oklch space.
///
/// Stores the maximum in-gamut chroma for a grid of (L, hue) values.
/// Constructed once per primaries set and reused across frames.
pub(crate) struct GamutBoundaryLut {
    /// Flattened `[L_STEPS][H_STEPS]` array of max chroma values.
    data: alloc::vec::Vec<f32>,
}

/// Number of lightness steps in the LUT (0..=1).
const L_STEPS: usize = 64;
/// Number of hue angle steps in the LUT (0..2π).
const H_STEPS: usize = 256;
/// Maximum chroma to search during LUT construction.
/// Oklch chroma rarely exceeds 0.4 for sRGB, but P3/BT.2020 can go higher.
const MAX_SEARCH_CHROMA: f32 = 0.5;
/// Binary search iterations for gamut boundary (2^-20 ≈ 1e-6 precision).
const BISECT_ITERS: u32 = 20;

impl GamutBoundaryLut {
    /// Build the gamut boundary LUT for a given primaries set.
    ///
    /// `m1_inv` is the combined LMS→linear RGB matrix for the target primaries,
    /// from `oklab::lms_to_rgb_matrix(primaries)`.
    pub(crate) fn new(m1_inv: &GamutMatrix) -> Self {
        let mut data = alloc::vec![0.0f32; L_STEPS * H_STEPS];

        for li in 0..L_STEPS {
            let l = li as f32 / (L_STEPS - 1) as f32;
            for hi in 0..H_STEPS {
                let h = hi as f32 / H_STEPS as f32 * core::f32::consts::TAU;
                data[li * H_STEPS + hi] = find_max_chroma(l, h, m1_inv);
            }
        }

        Self { data }
    }

    /// Look up the maximum in-gamut chroma for a given (L, hue) with
    /// bilinear interpolation.
    #[inline]
    fn max_chroma(&self, l: f32, h: f32) -> f32 {
        let l_clamped = l.clamp(0.0, 1.0);
        // Normalize hue to [0, 2π)
        let h_norm = h.rem_euclid(core::f32::consts::TAU);

        let l_f = l_clamped * (L_STEPS - 1) as f32;
        let h_f = h_norm / core::f32::consts::TAU * H_STEPS as f32;

        let l0 = (l_f as usize).min(L_STEPS - 2);
        let l1 = l0 + 1;
        let h0 = h_f as usize % H_STEPS;
        let h1 = (h0 + 1) % H_STEPS;

        let lt = l_f - l0 as f32;
        let ht = h_f - h0 as f32;

        let v00 = self.data[l0 * H_STEPS + h0];
        let v01 = self.data[l0 * H_STEPS + h1];
        let v10 = self.data[l1 * H_STEPS + h0];
        let v11 = self.data[l1 * H_STEPS + h1];

        let top = v00 + (v01 - v00) * ht;
        let bot = v10 + (v11 - v10) * ht;
        top + (bot - top) * lt
    }

    /// Apply soft chroma compression to Oklab planes in-place.
    ///
    /// For each pixel, if chroma exceeds `knee * max_chroma`, smoothly
    /// compresses it toward the gamut boundary using a rational function
    /// that preserves hue and lightness.
    ///
    /// `knee` is the fraction of max chroma where compression starts (0.0-1.0).
    /// Typical value: 0.9 (start compressing at 90% of gamut boundary).
    pub(crate) fn compress_planes(&self, l: &[f32], a: &mut [f32], b: &mut [f32], knee: f32) {
        let n = l.len();
        debug_assert!(a.len() == n && b.len() == n);

        for i in 0..n {
            let av = a[i];
            let bv = b[i];

            // Compute chroma
            let c = (av * av + bv * bv).sqrt();
            if c < 1e-10 {
                continue; // achromatic, nothing to compress
            }

            // Compute hue angle
            let h = bv.atan2(av);

            // Look up gamut boundary
            let max_c = self.max_chroma(l[i], h);
            if max_c < 1e-10 {
                // At L=0 or L=1, max chroma is 0 — force achromatic
                a[i] = 0.0;
                b[i] = 0.0;
                continue;
            }

            let knee_c = knee * max_c;
            if c <= knee_c {
                continue; // within knee threshold, pass through
            }

            // Rational compression: maps [knee_c, ∞) → [knee_c, max_c)
            //
            // f(C) = knee_c + range * excess / (excess + range)
            //
            // Properties:
            //   f(knee_c) = knee_c          (C0 continuous)
            //   f'(knee_c) = 1              (C1 continuous — slope matches passthrough)
            //   f(∞) → max_c               (asymptotic limit)
            let range = max_c - knee_c;
            let excess = c - knee_c;
            let compressed_c = knee_c + range * excess / (excess + range);

            // Scale a, b to match compressed chroma while preserving hue
            let scale = compressed_c / c;
            a[i] = av * scale;
            b[i] = bv * scale;
        }
    }
}

/// Binary search for the maximum in-gamut chroma at a given (L, hue).
fn find_max_chroma(l: f32, h: f32, m1_inv: &GamutMatrix) -> f32 {
    let cos_h = h.cos();
    let sin_h = h.sin();

    let mut lo = 0.0f32;
    let mut hi = MAX_SEARCH_CHROMA;

    for _ in 0..BISECT_ITERS {
        let mid = (lo + hi) * 0.5;
        let a = mid * cos_h;
        let b = mid * sin_h;

        if is_in_gamut(l, a, b, m1_inv) {
            lo = mid;
        } else {
            hi = mid;
        }
    }

    lo
}

/// Check if an Oklab color is within the RGB gamut for the given primaries.
#[inline]
fn is_in_gamut(l: f32, a: f32, b: f32, m1_inv: &GamutMatrix) -> bool {
    let rgb = oklab::oklab_to_rgb(l, a, b, m1_inv);
    rgb[0] >= 0.0
        && rgb[0] <= 1.0
        && rgb[1] >= 0.0
        && rgb[1] <= 1.0
        && rgb[2] >= 0.0
        && rgb[2] <= 1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;
    use zenpixels::ColorPrimaries;

    fn bt709_lut() -> GamutBoundaryLut {
        let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();
        GamutBoundaryLut::new(&m1_inv)
    }

    #[test]
    fn lut_boundary_at_extremes() {
        let lut = bt709_lut();

        // At L=0 (black) and L=1 (white), max chroma should be ~0
        for hi in 0..H_STEPS {
            let h = hi as f32 / H_STEPS as f32 * core::f32::consts::TAU;
            assert!(
                lut.max_chroma(0.0, h) < 0.01,
                "L=0 max chroma should be ~0, got {}",
                lut.max_chroma(0.0, h)
            );
            assert!(
                lut.max_chroma(1.0, h) < 0.01,
                "L=1 max chroma should be ~0, got {}",
                lut.max_chroma(1.0, h)
            );
        }
    }

    #[test]
    fn lut_boundary_has_positive_chroma_at_mid_l() {
        let lut = bt709_lut();

        // At mid lightness, there should be substantial gamut
        let mut max_found = 0.0f32;
        for hi in 0..H_STEPS {
            let h = hi as f32 / H_STEPS as f32 * core::f32::consts::TAU;
            let mc = lut.max_chroma(0.5, h);
            max_found = max_found.max(mc);
        }
        assert!(
            max_found > 0.1,
            "mid-L should have substantial gamut, max chroma = {max_found}"
        );
    }

    #[test]
    fn lut_boundary_is_monotonic_toward_extremes() {
        let lut = bt709_lut();

        // For a fixed hue, chroma boundary should increase from L=0 to
        // some peak, then decrease to L=1 (spindle shape).
        let h = 0.5; // arbitrary hue
        let mut found_peak = false;
        let mut prev = 0.0f32;
        for li in 0..L_STEPS {
            let l = li as f32 / (L_STEPS - 1) as f32;
            let mc = lut.max_chroma(l, h);
            if mc < prev {
                found_peak = true;
            }
            if found_peak {
                assert!(
                    mc <= prev + 0.01,
                    "chroma should decrease after peak at L={l}"
                );
            }
            prev = mc;
        }
    }

    #[test]
    fn compress_preserves_in_gamut_colors() {
        let lut = bt709_lut();
        let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();

        // Create a set of known in-gamut colors
        let l = alloc::vec![0.5, 0.3, 0.7, 0.9];
        let mut a = alloc::vec![0.01, -0.02, 0.005, 0.0];
        let mut b = alloc::vec![0.01, 0.01, -0.01, 0.0];
        let a_orig = a.clone();
        let b_orig = b.clone();

        // Verify they're in gamut
        for i in 0..l.len() {
            assert!(
                is_in_gamut(l[i], a[i], b[i], &m1_inv),
                "test color {i} should be in gamut"
            );
        }

        lut.compress_planes(&l, &mut a, &mut b, 0.9);

        // Should be unchanged (within float precision)
        for i in 0..l.len() {
            assert!(
                (a[i] - a_orig[i]).abs() < 1e-6,
                "in-gamut color {i} a should be unchanged"
            );
            assert!(
                (b[i] - b_orig[i]).abs() < 1e-6,
                "in-gamut color {i} b should be unchanged"
            );
        }
    }

    #[test]
    fn compress_reduces_out_of_gamut_chroma() {
        let lut = bt709_lut();

        // Create out-of-gamut colors (very high chroma)
        let l = alloc::vec![0.5, 0.5, 0.5];
        let mut a = alloc::vec![0.3, -0.3, 0.0];
        let mut b = alloc::vec![0.0, 0.0, 0.3];

        let orig_chroma: alloc::vec::Vec<f32> = a
            .iter()
            .zip(b.iter())
            .map(|(&av, &bv): (&f32, &f32)| (av * av + bv * bv).sqrt())
            .collect();

        lut.compress_planes(&l, &mut a, &mut b, 0.9);

        // Chroma should be reduced
        for i in 0..l.len() {
            let new_chroma = (a[i] * a[i] + b[i] * b[i]).sqrt();
            assert!(
                new_chroma < orig_chroma[i],
                "color {i} chroma should decrease: {:.4} -> {:.4}",
                orig_chroma[i],
                new_chroma
            );
        }
    }

    #[test]
    fn compress_preserves_hue() {
        let lut = bt709_lut();

        let l = alloc::vec![0.5, 0.5];
        let mut a = alloc::vec![0.3, -0.2];
        let mut b = alloc::vec![0.1, 0.25];

        let orig_hue: alloc::vec::Vec<f32> = a
            .iter()
            .zip(b.iter())
            .map(|(&av, &bv): (&f32, &f32)| bv.atan2(av))
            .collect();

        lut.compress_planes(&l, &mut a, &mut b, 0.9);

        for i in 0..l.len() {
            let new_hue = b[i].atan2(a[i]);
            let hue_diff = (new_hue - orig_hue[i]).abs();
            assert!(
                hue_diff < 1e-4,
                "hue should be preserved: {:.6} -> {:.6}",
                orig_hue[i],
                new_hue
            );
        }
    }

    #[test]
    fn compress_output_is_in_gamut() {
        let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();
        let lut = bt709_lut();

        // Create heavily out-of-gamut colors
        let l = alloc::vec![0.5, 0.3, 0.8, 0.5, 0.1, 0.95];
        let mut a = alloc::vec![0.4, -0.3, 0.2, -0.4, 0.15, 0.05];
        let mut b = alloc::vec![0.3, 0.4, -0.3, -0.2, 0.2, -0.03];

        lut.compress_planes(&l, &mut a, &mut b, 0.9);

        for i in 0..l.len() {
            let rgb = oklab::oklab_to_rgb(l[i], a[i], b[i], &m1_inv);
            // Allow tiny overshoot from bilinear interpolation
            assert!(
                rgb[0] >= -0.01 && rgb[0] <= 1.01,
                "R out of gamut after compress: color {i} R={:.4}",
                rgb[0]
            );
            assert!(
                rgb[1] >= -0.01 && rgb[1] <= 1.01,
                "G out of gamut after compress: color {i} G={:.4}",
                rgb[1]
            );
            assert!(
                rgb[2] >= -0.01 && rgb[2] <= 1.01,
                "B out of gamut after compress: color {i} B={:.4}",
                rgb[2]
            );
        }
    }
}
