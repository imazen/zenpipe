//! Fast scalar approximations for per-pixel polar/cartesian conversions.
//!
//! These replace `std` trig functions (atan2, sin, cos) in hot loops where
//! ~0.004 radian max error is acceptable — well within perceptual thresholds
//! for hue-angle operations in Oklab color space.

use core::f32::consts::{FRAC_PI_2, PI, TAU};

/// Fast atan2 approximation using a 7th-order minimax polynomial.
///
/// Returns values in `[-PI, PI]`, matching `f32::atan2` semantics.
/// Max error: ~0.0038 radians (~0.22 degrees).
#[inline]
pub(crate) fn fast_atan2(y: f32, x: f32) -> f32 {
    let ax = x.abs();
    let ay = y.abs();
    let mn = ax.min(ay);
    let mx = ax.max(ay);
    if mx < 1e-20 {
        return 0.0;
    }
    let a = mn / mx;
    // Minimax polynomial for atan(a) on [0, 1]
    let s = a * a;
    let mut r = ((-0.046_496_473 * s + 0.15931422) * s - 0.327_622_77) * s * a + a;
    if ay > ax {
        r = FRAC_PI_2 - r;
    }
    if x < 0.0 {
        r = PI - r;
    }
    if y < 0.0 {
        r = -r;
    }
    r
}

/// Fast simultaneous sin and cos via quadrant reduction + minimax polynomials.
///
/// Accepts any `f32` input (range-reduces internally).
/// Max error: ~0.0002 for both sin and cos.
///
/// Returns `(sin, cos)`.
#[inline]
pub(crate) fn fast_sincos(x: f32) -> (f32, f32) {
    // Range-reduce to [-PI, PI]
    let reduced = x - (x * (1.0 / TAU)).round() * TAU;

    // Further reduce to [-PI/2, PI/2] using symmetry
    // sin(x) = sin(PI - x), cos(x) = -cos(PI - x) for x in [PI/2, PI]
    let (r, cos_sign) = if reduced > FRAC_PI_2 {
        (PI - reduced, -1.0f32)
    } else if reduced < -FRAC_PI_2 {
        (-PI - reduced, -1.0f32)
    } else {
        (reduced, 1.0f32)
    };

    // Now r is in [-PI/2, PI/2].
    // Use degree-7 sin polynomial and degree-6 cos polynomial (Maclaurin).
    // On [-PI/2, PI/2] these converge excellently:
    //   sin(r) = r - r^3/6 + r^5/120 - r^7/5040
    //   cos(r) = 1 - r^2/2 + r^4/24 - r^6/720
    let r2 = r * r;

    let sin_val = r * (1.0 - r2 * (1.0 / 6.0 - r2 * (1.0 / 120.0 - r2 * (1.0 / 5040.0))));
    let cos_val =
        cos_sign * (1.0 - r2 * (0.5 - r2 * (1.0 / 24.0 - r2 * (1.0 / 720.0))));

    (sin_val, cos_val)
}

/// Fast scalar powf approximation: `base.powf(exp)` via `exp2(exp * log2(base))`.
///
/// Uses the same polynomial approach as magetypes `pow_lowp_unchecked` but in
/// scalar form. For use in SIMD tail loops (0-7 remaining pixels) where the
/// full SIMD path isn't available.
///
/// Max relative error: ~1% (same as magetypes lowp tier).
/// Only valid for `base > 0.0`. Returns 0.0 for `base <= 0.0`.
#[inline]
#[allow(clippy::approx_constant)]
pub(crate) fn fast_powf(base: f32, exp: f32) -> f32 {
    if base <= 0.0 {
        return 0.0;
    }
    // Scalar port of magetypes pow_lowp: exp2(n * log2(base))
    //
    // log2_lowp: rational polynomial on mantissa, same coefficients as magetypes
    let log2_base = {
        const P0: f32 = -1.850_383_3e-6;
        const P1: f32 = 1.428_716_1;
        const P2: f32 = 0.742_458_7;
        const Q0: f32 = 0.990_328_14;
        const Q1: f32 = 1.009_671_8;
        const Q2: f32 = 0.174_093_43;

        let x_bits = base.to_bits() as i32;
        let offset = 0x3f2a_aaab_u32 as i32;
        let exp_bits = x_bits.wrapping_sub(offset);
        let exp_shifted = exp_bits >> 23; // arithmetic shift
        let mantissa_bits = x_bits.wrapping_sub(exp_shifted << 23);
        let mantissa = f32::from_bits(mantissa_bits as u32);
        let exp_val = exp_shifted as f32;

        let m = mantissa - 1.0;
        let yp = P2 * m + P1;
        let yp = yp * m + P0;
        let yq = Q2 * m + Q1;
        let yq = yq * m + Q0;

        yp / yq + exp_val
    };

    // exp2_lowp: polynomial on fractional part, same coefficients as magetypes
    let product = (exp * log2_base).clamp(-126.0, 126.0);
    let xi = product.floor();
    let xf = product - xi;

    let poly = 0.055_504_11 * xf + 0.240_226_5;
    let poly = poly * xf + core::f32::consts::LN_2;
    let poly = poly * xf + 1.0;

    let scale_bits = ((xi as i32 + 127) << 23) as u32;
    poly * f32::from_bits(scale_bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_atan2_matches_std_range() {
        // Verify output is in [-PI, PI] for a sweep of angles
        let test_cases: &[(f32, f32)] = &[
            (0.0, 1.0),
            (1.0, 0.0),
            (0.0, -1.0),
            (-1.0, 0.0),
            (1.0, 1.0),
            (-1.0, 1.0),
            (1.0, -1.0),
            (-1.0, -1.0),
            (0.0, 0.0),
            (0.3, 0.7),
            (-0.3, -0.7),
            (0.001, -0.999),
            (100.0, 0.001),
        ];

        for &(y, x) in test_cases {
            let fast = fast_atan2(y, x);
            assert!(
                fast >= -PI && fast <= PI,
                "fast_atan2({y}, {x}) = {fast} out of range"
            );
        }
    }

    #[test]
    fn fast_atan2_accuracy() {
        // Check accuracy against std atan2 across many angles
        let mut max_err: f32 = 0.0;
        let steps = 1000;
        for i in 0..steps {
            let angle = -PI + TAU * (i as f32) / (steps as f32);
            let y = angle.sin();
            let x = angle.cos();
            let std_val = y.atan2(x);
            let fast_val = fast_atan2(y, x);
            let err = (std_val - fast_val).abs();
            if err > max_err {
                max_err = err;
            }
        }
        assert!(
            max_err < 0.004,
            "fast_atan2 max error {max_err} exceeds 0.004 radians"
        );
    }

    #[test]
    fn fast_atan2_quadrant_signs() {
        // Q1: y>0, x>0 => positive
        assert!(fast_atan2(1.0, 1.0) > 0.0);
        // Q2: y>0, x<0 => positive, > PI/2
        assert!(fast_atan2(1.0, -1.0) > FRAC_PI_2);
        // Q3: y<0, x<0 => negative
        assert!(fast_atan2(-1.0, -1.0) < 0.0);
        // Q4: y<0, x>0 => negative
        assert!(fast_atan2(-1.0, 1.0) < 0.0);
    }

    #[test]
    fn fast_atan2_zero_inputs() {
        assert_eq!(fast_atan2(0.0, 0.0), 0.0);
        let r = fast_atan2(0.0, 1.0);
        assert!(r.abs() < 1e-6, "atan2(0, 1) should be ~0, got {r}");
        let r = fast_atan2(1.0, 0.0);
        assert!(
            (r - FRAC_PI_2).abs() < 0.004,
            "atan2(1, 0) should be ~PI/2, got {r}"
        );
    }

    #[test]
    fn fast_sincos_accuracy() {
        let mut max_sin_err: f32 = 0.0;
        let mut max_cos_err: f32 = 0.0;
        let steps = 2000;
        for i in 0..steps {
            let angle = -PI + TAU * (i as f32) / (steps as f32);
            let (fast_sin, fast_cos) = fast_sincos(angle);
            let std_sin = angle.sin();
            let std_cos = angle.cos();
            let sin_err = (std_sin - fast_sin).abs();
            let cos_err = (std_cos - fast_cos).abs();
            if sin_err > max_sin_err {
                max_sin_err = sin_err;
            }
            if cos_err > max_cos_err {
                max_cos_err = cos_err;
            }
        }
        assert!(
            max_sin_err < 0.002,
            "fast_sincos sin max error {max_sin_err} exceeds 0.002"
        );
        assert!(
            max_cos_err < 0.002,
            "fast_sincos cos max error {max_cos_err} exceeds 0.002"
        );
    }

    #[test]
    fn fast_sincos_large_inputs() {
        // Test range reduction with large inputs
        let large_angles = [10.0, -10.0, 100.0, -100.0, 1000.0, -1000.0];
        for &angle in &large_angles {
            let (fast_sin, fast_cos) = fast_sincos(angle);
            let std_sin = angle.sin();
            let std_cos = angle.cos();
            let sin_err = (std_sin - fast_sin).abs();
            let cos_err = (std_cos - fast_cos).abs();
            assert!(
                sin_err < 0.01,
                "fast_sincos sin({angle}) error {sin_err} too large"
            );
            assert!(
                cos_err < 0.01,
                "fast_sincos cos({angle}) error {cos_err} too large"
            );
        }
    }

    #[test]
    fn fast_sincos_cardinal_points() {
        // sin(0) = 0, cos(0) = 1
        let (s, c) = fast_sincos(0.0);
        assert!(s.abs() < 1e-6, "sin(0) = {s}");
        assert!((c - 1.0).abs() < 1e-6, "cos(0) = {c}");

        // sin(PI/2) = 1, cos(PI/2) = 0
        let (s, c) = fast_sincos(FRAC_PI_2);
        assert!((s - 1.0).abs() < 0.002, "sin(PI/2) = {s}");
        assert!(c.abs() < 0.002, "cos(PI/2) = {c}");

        // sin(PI) = 0, cos(PI) = -1
        let (s, c) = fast_sincos(PI);
        assert!(s.abs() < 0.002, "sin(PI) = {s}");
        assert!((c + 1.0).abs() < 0.002, "cos(PI) = {c}");

        // sin(-PI/2) = -1, cos(-PI/2) = 0
        let (s, c) = fast_sincos(-FRAC_PI_2);
        assert!((s + 1.0).abs() < 0.002, "sin(-PI/2) = {s}");
        assert!(c.abs() < 0.002, "cos(-PI/2) = {c}");
    }

    #[test]
    fn roundtrip_atan2_sincos() {
        // Verify that fast_sincos(fast_atan2(y, x)) reconstructs the direction
        let test_points: &[(f32, f32)] = &[
            (0.1, 0.05),
            (-0.1, 0.15),
            (0.05, -0.1),
            (-0.05, -0.15),
            (0.2, 0.0),
            (0.0, 0.2),
        ];

        for &(a, b) in test_points {
            let chroma = (a * a + b * b).sqrt();
            let hue = fast_atan2(b, a);
            let (sin_h, cos_h) = fast_sincos(hue);
            let a_recon = chroma * cos_h;
            let b_recon = chroma * sin_h;
            assert!(
                (a - a_recon).abs() < 0.002,
                "roundtrip a: {a} vs {a_recon} (hue={hue})"
            );
            assert!(
                (b - b_recon).abs() < 0.002,
                "roundtrip b: {b} vs {b_recon} (hue={hue})"
            );
        }
    }

    #[test]
    fn fast_powf_accuracy() {
        let bases: [f32; 12] = [0.01, 0.1, 0.25, 0.5, 0.75, 0.9, 0.99, 1.0, 1.5, 2.0, 5.0, 10.0];
        let exponents: [f32; 8] = [0.5, 1.0, 1.5, 2.0, 2.4, 0.1, 0.01, 3.0];
        let mut max_rel_err: f32 = 0.0;
        for &base in &bases {
            for &exp in &exponents {
                let std_val = base.powf(exp);
                let fast_val = fast_powf(base, exp);
                if std_val > 1e-6 {
                    let rel_err = ((std_val - fast_val) / std_val).abs();
                    if rel_err > max_rel_err {
                        max_rel_err = rel_err;
                    }
                }
            }
        }
        assert!(
            max_rel_err < 0.02,
            "fast_powf max relative error {max_rel_err} exceeds 2%"
        );
    }

    #[test]
    fn fast_powf_edge_cases() {
        assert_eq!(fast_powf(0.0, 2.0), 0.0);
        assert_eq!(fast_powf(-1.0, 2.0), 0.0);
        let v = fast_powf(1.0, 100.0);
        assert!((v - 1.0).abs() < 0.01, "1^100 = {v}");
    }
}
