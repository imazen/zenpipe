//! Otsu's method for optimal binarization threshold.
//!
//! Finds the threshold that maximizes inter-class variance between
//! foreground and background in the L plane histogram. This is the
//! standard binarization method for document processing.

/// Number of histogram bins. 256 matches 8-bit quantization of [0,1] L values.
const BINS: usize = 256;

/// Compute the optimal binarization threshold for the L plane using Otsu's method.
///
/// Returns a threshold in [0.0, 1.0]. Pixels with L >= threshold are foreground.
///
/// The algorithm:
/// 1. Build a 256-bin histogram of L values
/// 2. For each candidate threshold t, compute the inter-class variance:
///    σ²_B(t) = w₀(t) · w₁(t) · (μ₀(t) - μ₁(t))²
///    where w₀, w₁ are class weights and μ₀, μ₁ are class means
/// 3. Return the t that maximizes σ²_B
pub fn otsu_threshold(l_plane: &[f32]) -> f32 {
    if l_plane.is_empty() {
        return 0.5;
    }

    // Build histogram
    let mut hist = [0u32; BINS];
    for &v in l_plane {
        let bin = (v.clamp(0.0, 1.0) * (BINS - 1) as f32) as usize;
        hist[bin] += 1;
    }

    let total = l_plane.len() as f64;

    // Precompute total weighted sum
    let mut sum_total = 0.0f64;
    for (i, &count) in hist.iter().enumerate() {
        sum_total += i as f64 * count as f64;
    }

    // Sweep threshold, maximize inter-class variance
    let mut best_threshold = 0usize;
    let mut best_variance = 0.0f64;

    let mut w0 = 0.0f64; // weight of class 0 (background)
    let mut sum0 = 0.0f64; // weighted sum of class 0

    for (t, &count) in hist.iter().enumerate() {
        w0 += count as f64;
        if w0 == 0.0 {
            continue;
        }

        let w1 = total - w0;
        if w1 == 0.0 {
            break;
        }

        sum0 += t as f64 * count as f64;
        let mean0 = sum0 / w0;
        let mean1 = (sum_total - sum0) / w1;

        let variance = w0 * w1 * (mean0 - mean1) * (mean0 - mean1);

        if variance > best_variance {
            best_variance = variance;
            best_threshold = t;
        }
    }

    best_threshold as f32 / (BINS - 1) as f32
}

/// Binarize an L plane in-place using the given threshold.
///
/// Values > threshold become 1.0, values at or below become 0.0.
pub fn binarize(plane: &mut [f32], threshold: f32) {
    for v in plane.iter_mut() {
        *v = if *v > threshold { 1.0 } else { 0.0 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    #[test]
    fn bimodal_distribution() {
        // 50% dark (0.2) + 50% bright (0.8) → threshold should separate them
        let mut plane = vec![0.2f32; 500];
        plane.extend(vec![0.8f32; 500]);
        let t = otsu_threshold(&plane);
        // Otsu picks the first threshold that maximizes inter-class variance,
        // which is at the left edge of the gap. With binarize using >, this
        // correctly separates 0.2 (background) from 0.8 (foreground).
        assert!(
            t >= 0.15 && t < 0.75,
            "bimodal 0.2/0.8 should give threshold that separates them, got {t}"
        );
        // Verify binarize actually works
        let mut test = vec![0.2, 0.8];
        binarize(&mut test, t);
        assert_eq!(test, vec![0.0, 1.0], "binarize should separate 0.2 and 0.8");
    }

    #[test]
    fn document_like_distribution() {
        // Spread of dark values + spread of bright values (realistic document)
        let mut plane = Vec::with_capacity(1000);
        for i in 0..800 {
            plane.push(0.7 + 0.25 * (i as f32 / 800.0)); // bright: 0.70–0.95
        }
        for i in 0..200 {
            plane.push(0.05 + 0.15 * (i as f32 / 200.0)); // dark: 0.05–0.20
        }
        let t = otsu_threshold(&plane);
        // With spreads 0.05-0.20 and 0.70-0.95, threshold should fall in the gap
        assert!(
            t > 0.15 && t < 0.75,
            "should split text (0.05-0.20) from background (0.70-0.95), got {t}"
        );
        // Verify binarization: well-below threshold → 0, well-above → 1
        let mut test = vec![0.01, 0.99];
        binarize(&mut test, t);
        assert_eq!(test[0], 0.0, "0.01 should be background at threshold {t}");
        assert_eq!(test[1], 1.0, "0.99 should be foreground at threshold {t}");
    }

    #[test]
    fn constant_plane() {
        let plane = vec![0.5f32; 100];
        let t = otsu_threshold(&plane);
        // Any threshold is equally good; just shouldn't panic
        assert!(t >= 0.0 && t <= 1.0, "constant plane threshold: {t}");
    }

    #[test]
    fn binarize_works() {
        let mut plane = vec![0.1, 0.3, 0.5, 0.7, 0.9];
        binarize(&mut plane, 0.5);
        assert_eq!(plane, vec![0.0, 0.0, 0.0, 1.0, 1.0]); // 0.5 is NOT > 0.5
    }

    #[test]
    fn empty_plane() {
        assert_eq!(otsu_threshold(&[]), 0.5);
    }
}
