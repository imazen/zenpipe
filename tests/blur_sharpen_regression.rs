//! Regression tests for blur and sharpen filters.
//!
//! These tests exercise behavior not covered by the inline unit tests
//! in `filters/blur.rs` and `filters/sharpen.rs`, or the cross-library
//! validation in `reference_validation.rs`.
//!
//! Coverage added:
//!
//! ## Blur
//! - Checkerboard pattern: blur reduces contrast between adjacent pixels
//! - Monotonicity: larger sigma produces more smoothing
//! - Small sigma (~0.5) produces only mild smoothing (not identity)
//! - Mean luminance preservation (energy conservation)
//!
//! ## Sharpen
//! - Monotonicity: increasing amount produces increasing divergence from original
//! - Mean luminance approximately preserved
//! - Sharpening does not modify chroma channels (a, b untouched)
//! - Edge contrast enhancement verified
//! - Parameter range produces finite, bounded output
//! - Documentation of ImageResizer4 `f.sharpen` correspondence

use zenfilters::filters::{Blur, Sharpen};
use zenfilters::{Filter, FilterContext, OklabPlanes};

// ─── Helpers ───────────────────────────────────────────────────────────

fn new_blur(sigma: f32) -> Blur {
    let mut b = Blur::default();
    b.sigma = sigma;
    b
}

fn new_sharpen(sigma: f32, amount: f32) -> Sharpen {
    let mut s = Sharpen::default();
    s.sigma = sigma;
    s.amount = amount;
    s
}

/// Create a checkerboard pattern on the L plane.
/// Even cells get `lo`, odd cells get `hi`.
fn make_checkerboard(width: u32, height: u32, lo: f32, hi: f32) -> OklabPlanes {
    let mut planes = OklabPlanes::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let i = planes.index(x, y);
            planes.l[i] = if (x + y) % 2 == 0 { lo } else { hi };
        }
    }
    planes
}

/// Create a step-edge pattern: left half = `lo`, right half = `hi`.
fn make_step_edge(width: u32, height: u32, lo: f32, hi: f32) -> OklabPlanes {
    let mut planes = OklabPlanes::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let i = planes.index(x, y);
            planes.l[i] = if x < width / 2 { lo } else { hi };
        }
    }
    planes
}

/// Root-mean-square difference between two slices.
fn rms_diff(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    let sum_sq: f32 = a
        .iter()
        .zip(b.iter())
        .map(|(&x, &y)| (x - y) * (x - y))
        .sum();
    (sum_sq / a.len() as f32).sqrt()
}

/// Standard deviation of a slice.
fn std_dev(data: &[f32]) -> f32 {
    let n = data.len() as f32;
    let mean = data.iter().sum::<f32>() / n;
    let var: f32 = data.iter().map(|&v| (v - mean) * (v - mean)).sum::<f32>() / n;
    var.sqrt()
}

/// Maximum absolute difference between two slices.
fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(&x, &y)| (x - y).abs())
        .fold(0.0f32, f32::max)
}

// ═══════════════════════════════════════════════════════════════════════
// BLUR REGRESSION TESTS
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn blur_checkerboard_reduces_adjacent_contrast() {
    // A 1-pixel checkerboard is the hardest pattern for a blur to smooth:
    // every pixel differs from all its neighbors.
    let mut planes = make_checkerboard(128, 128, 0.2, 0.8);
    let original = planes.l.clone();

    let std_before = std_dev(&original);
    assert!(
        std_before > 0.2,
        "checkerboard should have substantial stddev: {std_before}"
    );

    new_blur(2.0).apply(&mut planes, &mut FilterContext::new());

    let std_after = std_dev(&planes.l);
    assert!(
        std_after < std_before * 0.5,
        "blur should substantially reduce checkerboard contrast: {std_before:.4} -> {std_after:.4}"
    );

    // Also verify individual adjacent pixels are closer together after blur.
    // Sample an interior pair at (63, 64) and (64, 64).
    let a = planes.l[planes.index(63, 64)];
    let b = planes.l[planes.index(64, 64)];
    let orig_a = original[planes.index(63, 64)];
    let orig_b = original[planes.index(64, 64)];
    assert!(
        (a - b).abs() < (orig_a - orig_b).abs(),
        "adjacent pixels should be closer after blur: |{a}-{b}|={:.4} vs original |{orig_a}-{orig_b}|={:.4}",
        (a - b).abs(),
        (orig_a - orig_b).abs()
    );
}

#[test]
fn blur_larger_sigma_produces_more_smoothing() {
    // Monotonicity: sigma_small < sigma_large should mean less smoothing < more smoothing.
    let sigmas = [0.5, 1.0, 2.0, 4.0, 8.0];
    let mut rms_diffs = Vec::new();

    for &sigma in &sigmas {
        // Use 256x256 so even sigma=8 (kernel ~49px) has ample interior.
        let mut planes = make_step_edge(256, 256, 0.2, 0.8);
        let original = planes.l.clone();
        new_blur(sigma).apply(&mut planes, &mut FilterContext::new());
        let diff = rms_diff(&planes.l, &original);
        rms_diffs.push(diff);
    }

    // Each larger sigma should produce at least as much change as the previous.
    for i in 1..rms_diffs.len() {
        assert!(
            rms_diffs[i] >= rms_diffs[i - 1] * 0.99, // 1% tolerance for float
            "blur sigma={} (rms_diff={:.6}) should produce >= smoothing than sigma={} (rms_diff={:.6})",
            sigmas[i],
            rms_diffs[i],
            sigmas[i - 1],
            rms_diffs[i - 1]
        );
    }

    // The largest sigma should produce meaningfully more smoothing than the smallest.
    assert!(
        rms_diffs[rms_diffs.len() - 1] > rms_diffs[0] * 3.0,
        "sigma=8 should produce >3.0x the smoothing of sigma=0.5: {:.6} vs {:.6}",
        rms_diffs[rms_diffs.len() - 1],
        rms_diffs[0]
    );
}

#[test]
fn blur_small_sigma_is_not_identity() {
    // sigma=0.5 is small but should still produce a visible effect (not a no-op).
    let mut planes = make_checkerboard(128, 128, 0.2, 0.8);
    let original = planes.l.clone();
    new_blur(0.5).apply(&mut planes, &mut FilterContext::new());

    let diff = max_abs_diff(&planes.l, &original);
    assert!(
        diff > 0.01,
        "blur sigma=0.5 should produce a visible effect on checkerboard, max_diff={diff}"
    );
}

#[test]
fn blur_preserves_mean_luminance() {
    // Gaussian blur should preserve the mean value of the plane (energy conservation).
    let mut planes = make_step_edge(128, 128, 0.2, 0.8);
    let mean_before: f32 = planes.l.iter().sum::<f32>() / planes.l.len() as f32;

    new_blur(3.0).apply(&mut planes, &mut FilterContext::new());

    let mean_after: f32 = planes.l.iter().sum::<f32>() / planes.l.len() as f32;
    assert!(
        (mean_before - mean_after).abs() < 0.005,
        "blur should preserve mean luminance: {mean_before:.4} -> {mean_after:.4}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// SHARPEN REGRESSION TESTS
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn sharpen_output_differs_from_input() {
    // Basic functionality: sharpening with nonzero amount on a patterned image
    // should produce output different from input.
    let mut planes = make_step_edge(128, 128, 0.3, 0.7);
    let original = planes.l.clone();

    new_sharpen(1.0, 0.5).apply(&mut planes, &mut FilterContext::new());

    let diff = max_abs_diff(&planes.l, &original);
    assert!(
        diff > 0.01,
        "sharpen amount=0.5 should change the image, max_diff={diff}"
    );
}

#[test]
fn sharpen_zero_amount_is_identity() {
    // Redundant with the inline test, but verifies through the public API
    // that amount=0 truly produces no change regardless of sigma.
    for sigma in [0.5, 1.0, 2.0, 3.0] {
        let mut planes = make_checkerboard(16, 16, 0.3, 0.7);
        let original = planes.l.clone();

        new_sharpen(sigma, 0.0).apply(&mut planes, &mut FilterContext::new());

        assert_eq!(
            planes.l, original,
            "sharpen amount=0, sigma={sigma} should be exact identity"
        );
    }
}

#[test]
fn sharpen_increasing_amount_increases_divergence() {
    // Monotonicity: higher sharpening amount should produce larger difference from original.
    let amounts = [0.1, 0.3, 0.5, 1.0, 2.0];
    let mut rms_diffs = Vec::new();

    for &amount in &amounts {
        let mut planes = make_step_edge(128, 128, 0.3, 0.7);
        let original = planes.l.clone();

        new_sharpen(1.0, amount).apply(&mut planes, &mut FilterContext::new());

        let diff = rms_diff(&planes.l, &original);
        rms_diffs.push(diff);
    }

    // Each larger amount should produce more change.
    for i in 1..rms_diffs.len() {
        assert!(
            rms_diffs[i] >= rms_diffs[i - 1] * 0.95, // small tolerance
            "sharpen amount={} (rms={:.6}) should produce >= change than amount={} (rms={:.6})",
            amounts[i],
            rms_diffs[i],
            amounts[i - 1],
            rms_diffs[i - 1]
        );
    }
}

#[test]
fn sharpen_preserves_mean_luminance() {
    // Unsharp mask sharpening should approximately preserve mean luminance.
    // The formula is L' = L + amount * (L - blur(L)) which has zero-mean detail,
    // so the mean should be preserved (with edge effects at boundaries).
    let mut planes = make_step_edge(128, 128, 0.3, 0.7);
    let mean_before: f32 = planes.l.iter().sum::<f32>() / planes.l.len() as f32;

    new_sharpen(1.0, 1.0).apply(&mut planes, &mut FilterContext::new());

    let mean_after: f32 = planes.l.iter().sum::<f32>() / planes.l.len() as f32;
    // Note: the `.max(0.0)` clamp in unsharp_fuse can shift the mean slightly upward
    // for very dark regions near edges. Allow some tolerance.
    assert!(
        (mean_before - mean_after).abs() < 0.02,
        "sharpen should approximately preserve mean: {mean_before:.4} -> {mean_after:.4}"
    );
}

#[test]
fn sharpen_does_not_modify_chroma() {
    // Sharpen operates on L_ONLY -- the a and b channels must be untouched.
    // Use a gradient/checkerboard pattern in a and b to catch cross-channel edge bleed.
    let width = 128u32;
    let height = 128u32;
    let mut planes = make_step_edge(width, height, 0.3, 0.7);
    for y in 0..height {
        for x in 0..width {
            let i = planes.index(x, y);
            // a channel: horizontal gradient from -0.1 to +0.1
            planes.a[i] = -0.1 + 0.2 * (x as f32) / (width as f32 - 1.0);
            // b channel: checkerboard between -0.05 and +0.05
            planes.b[i] = if (x + y) % 2 == 0 { -0.05 } else { 0.05 };
        }
    }
    let a_orig = planes.a.clone();
    let b_orig = planes.b.clone();

    new_sharpen(1.0, 1.0).apply(&mut planes, &mut FilterContext::new());

    assert_eq!(planes.a, a_orig, "sharpen must not modify a channel");
    assert_eq!(planes.b, b_orig, "sharpen must not modify b channel");
}

#[test]
fn sharpen_increases_edge_contrast() {
    // On a step edge, sharpening should push dark-side pixels darker and
    // bright-side pixels brighter (overshoot at the edge).
    let mut planes = make_step_edge(128, 128, 0.3, 0.7);

    new_sharpen(1.0, 1.0).apply(&mut planes, &mut FilterContext::new());

    // Interior pixels far from the edge should be nearly unchanged.
    let far_left = planes.l[planes.index(16, 64)];
    let far_right = planes.l[planes.index(112, 64)];

    // Pixels just at the edge should be pushed apart.
    let edge_left = planes.l[planes.index(63, 64)];
    let edge_right = planes.l[planes.index(64, 64)];

    // The edge contrast should exceed the original 0.4 gap.
    assert!(
        edge_right - edge_left > 0.4,
        "edge contrast should be enhanced: edge_left={edge_left:.4}, edge_right={edge_right:.4}, gap={:.4}",
        edge_right - edge_left
    );

    // Interior should remain close to original values.
    assert!(
        (far_left - 0.3).abs() < 0.05,
        "far interior left should be near 0.3, got {far_left:.4}"
    );
    assert!(
        (far_right - 0.7).abs() < 0.05,
        "far interior right should be near 0.7, got {far_right:.4}"
    );
}

/// Document the parameter ranges and behavior of zenfilters::Sharpen
/// relative to ImageResizer4's `f.sharpen` parameter.
///
/// ImageResizer4's `f.sharpen` was a single 0-100 value that controlled an
/// unsharp mask applied after the resize. The mapping was approximately:
///   - f.sharpen maps to unsharp mask amount
///   - Sigma was fixed or proportional to output dimensions
///
/// zenfilters::Sharpen exposes two independent controls:
///   - `sigma`: blur radius for detail extraction (0.5 - 3.0 px, default 1.0)
///   - `amount`: sharpening strength (0.0 - 2.0, default 0.0 = identity)
///
/// The formula is: L' = max(0, L + amount * (L - gaussian_blur(L, sigma)))
///
/// To approximate IR4's f.sharpen=N:
///   - sigma = 1.0 (IR4 used a similar small radius)
///   - amount = N / 100.0 * 2.0  (IR4's 0-100 maps to zenfilters 0-2.0)
///
/// This test verifies the parameter range produces sensible results.
#[test]
fn sharpen_parameter_range_produces_sensible_output() {
    // Verify sigma range (per SHARPEN_SCHEMA: 0.5 to 3.0).
    for sigma in [0.5, 1.0, 1.5, 2.0, 3.0] {
        let mut planes = make_step_edge(128, 128, 0.3, 0.7);
        new_sharpen(sigma, 0.5).apply(&mut planes, &mut FilterContext::new());

        // All output values should be finite and in a reasonable range.
        // Theoretical max with input [0.3, 0.7] and amount 2.0 is 1.5,
        // so 1.6 provides a tight bound with headroom.
        for &v in &planes.l {
            assert!(
                v.is_finite() && v >= 0.0 && v <= 1.6,
                "sharpen sigma={sigma} produced out-of-range value {v}"
            );
        }
    }

    // Verify amount range (per SHARPEN_SCHEMA: 0.0 to 2.0).
    for amount in [0.0, 0.5, 1.0, 1.5, 2.0] {
        let mut planes = make_step_edge(128, 128, 0.3, 0.7);
        new_sharpen(1.0, amount).apply(&mut planes, &mut FilterContext::new());

        for &v in &planes.l {
            assert!(
                v.is_finite() && v >= 0.0 && v <= 1.6,
                "sharpen amount={amount} produced out-of-range value {v}"
            );
        }
    }
}
