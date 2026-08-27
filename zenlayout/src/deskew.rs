//! Content-adaptive skew detection (zenpipe#27).
//!
//! Estimates the angle of the dominant line structure in a grayscale image
//! — text lines on a scanned document, a horizon, table rules — so an
//! [`AutoDeskewEffect`](crate::dimension::AutoDeskewEffect) can turn it into
//! a concrete [`RotateEffect`](crate::dimension::RotateEffect).
//!
//! # Method: gradient moment (structure tensor)
//!
//! One pass over the image accumulates the structure tensor
//! `J = Σ [Ix²  IxIy; IxIy  Iy²]` from central-difference gradients. Its
//! principal eigenvector is the dominant *gradient* direction; the dominant
//! *edge* direction is perpendicular to it. Text rows and rules produce
//! strong, coherent gradients across the lines, so the edge direction is
//! the skew. `O(N)`, no allocation, and only every `step`-th pixel is
//! visited on large images (`step = ceil(max(w, h) / 1000)` — the angle is
//! scale-invariant).
//!
//! Biased toward high-contrast content: a photo with a strong diagonal
//! (a road, a roof) reports that diagonal. Pair with a small `max_angle`.
//! It is also a *coarse* estimator: on thin rulings the discretized
//! gradients carry an anisotropic bias of roughly ±10–15% of the angle
//! (measured on the anti-aliased fixtures in the tests: Sobel under-,
//! central differences over-estimate), so it is not accurate to 0.2°.
//!
//! # Method: projection variance
//!
//! [`detect_skew_projection_variance`] rotates the projection axis instead
//! of the image: for a candidate angle θ every pixel's darkness is binned by
//! its coordinate perpendicular to the line direction `(cos θ, sin θ)`.
//! When θ matches the ruling, dark rows land in few bins and the histogram
//! variance peaks. A 1° sweep over `[-max, +max]` is refined at 0.1° around
//! the best bin — accurate to the grid on documents and rulings, at
//! `O(N × angles)`. This is the method [`AutoDeskewEffect::new`] picks.
//!
//! [`AutoDeskewEffect::new`]: crate::dimension::AutoDeskewEffect::new
//!
//! # Angle convention
//!
//! Degrees, in the image coordinate system (y down), matching
//! [`RotateEffect`](crate::dimension::RotateEffect): a horizontal line
//! rotated by `RotateEffect::from_degrees(a, ..)` is detected as `a`, so
//! `RotateEffect::from_degrees(-detected, ..)` straightens it. Angles are
//! folded modulo 90° into `(-45°, 45°]` — vertical structure counts as
//! "straight", the same as horizontal — and then rejected (`None`) when
//! `|angle| > max_angle_deg`.

#[allow(unused_imports)]
use crate::float_math::Float;

/// Minimum structure-tensor coherence (0 = isotropic, 1 = a single
/// direction) below which no dominant orientation is reported.
const MIN_COHERENCE: f64 = 0.05;

/// Detect the skew angle (degrees) of the dominant line structure in an
/// 8-bit grayscale image via the structure tensor.
///
/// `luma` is `h` rows of `w` samples at `stride` bytes per row. Returns
/// `None` when the image is smaller than 3×3, has no coherent orientation
/// (uniform, noise, isotropic texture), or the folded angle exceeds
/// `max_angle_deg` (clamped to `(0, 45]`). See the [module docs](self)
/// for the convention.
pub fn detect_skew_gradient_moment(
    luma: &[u8],
    w: u32,
    h: u32,
    stride: usize,
    max_angle_deg: f32,
) -> Option<f32> {
    if w < 3 || h < 3 {
        return None;
    }
    let (w, h) = (w as usize, h as usize);
    if stride < w || luma.len() < (h - 1) * stride + w {
        return None;
    }
    let max_angle = f64::from(max_angle_deg).clamp(f64::EPSILON, 45.0);

    // Visit every `step`-th sample on big images; gradients are still
    // central differences at full resolution.
    let step = w.max(h).div_ceil(1000).max(1);

    let (mut jxx, mut jxy, mut jyy) = (0.0f64, 0.0f64, 0.0f64);
    let mut y = 1;
    while y + 1 < h {
        let row = &luma[y * stride..y * stride + w];
        let up = &luma[(y - 1) * stride..(y - 1) * stride + w];
        let down = &luma[(y + 1) * stride..(y + 1) * stride + w];
        let mut x = 1;
        while x + 1 < w {
            // Sobel 3×3: the smoothing along the derivative's perpendicular
            // damps rasterization phase jitter that plain central
            // differences turn into spurious cross-axis energy.
            let ix = (f64::from(row[x + 1]) - f64::from(row[x - 1])) * 2.0
                + (f64::from(up[x + 1]) - f64::from(up[x - 1]))
                + (f64::from(down[x + 1]) - f64::from(down[x - 1]));
            let iy = (f64::from(down[x]) - f64::from(up[x])) * 2.0
                + (f64::from(down[x - 1]) - f64::from(up[x - 1]))
                + (f64::from(down[x + 1]) - f64::from(up[x + 1]));
            jxx += ix * ix;
            jxy += ix * iy;
            jyy += iy * iy;
            x += step;
        }
        y += step;
    }

    let trace = jxx + jyy;
    if trace <= 0.0 {
        return None;
    }
    let diff = jxx - jyy;
    let coherence = (diff * diff + 4.0 * jxy * jxy).sqrt() / trace;
    if coherence < MIN_COHERENCE {
        return None;
    }

    // Dominant gradient orientation; edges run perpendicular to it.
    let gradient_deg = (0.5 * (2.0 * jxy).atan2(diff)).to_degrees();
    let mut edge_deg = gradient_deg + 90.0;
    // Fold modulo 90° into (-45, 45]: axis-aligned structure is "straight".
    // (`%` then fix-up: `f64::rem_euclid` is std-only.)
    edge_deg %= 90.0;
    if edge_deg < 0.0 {
        edge_deg += 90.0;
    }
    if edge_deg > 45.0 {
        edge_deg -= 90.0;
    }
    if edge_deg.abs() > max_angle {
        return None;
    }
    Some(edge_deg as f32)
}

/// Detect the skew angle (degrees) of the dominant line structure by
/// maximizing the variance of the perpendicular projection histogram.
///
/// Same input contract and angle convention as
/// [`detect_skew_gradient_moment`]. Sweeps `[-max, +max]` in 1° steps and
/// refines ±1° around the best candidate in 0.1° steps. Returns `None` for
/// degenerate input or when the best projection is no sharper than an
/// isotropic one (no line structure). `max_angle_deg` is clamped to
/// `(0, 45]`.
pub fn detect_skew_projection_variance(
    luma: &[u8],
    w: u32,
    h: u32,
    stride: usize,
    max_angle_deg: f32,
) -> Option<f32> {
    if w < 3 || h < 3 {
        return None;
    }
    let (wu, hu) = (w as usize, h as usize);
    if stride < wu || luma.len() < (hu - 1) * stride + wu {
        return None;
    }
    let max_angle = f64::from(max_angle_deg).clamp(f64::EPSILON, 45.0);
    let step = wu.max(hu).div_ceil(1000).max(1);

    // Bins: the projection coordinate spans at most the image diagonal.
    let diag = ((wu * wu + hu * hu) as f64).sqrt();
    let n_bins = (diag / step as f64).ceil() as usize + 2;
    let mut sums = alloc::vec![0.0f64; n_bins];
    let mut counts = alloc::vec![0u32; n_bins];
    let (cx, cy) = (wu as f64 / 2.0, hu as f64 / 2.0);
    let half = diag / 2.0;

    // Score = variance of per-bin mean darkness over well-populated bins.
    let mut score = |theta_deg: f64| -> f64 {
        sums.iter_mut().for_each(|v| *v = 0.0);
        counts.iter_mut().for_each(|v| *v = 0);
        let (sn, cs) = theta_deg.to_radians().sin_cos();
        let mut y = 0;
        while y < hu {
            let row = &luma[y * stride..y * stride + wu];
            let dy = y as f64 - cy;
            let mut x = 0;
            while x < wu {
                let dx = x as f64 - cx;
                let p = (dy * cs - dx * sn + half) / step as f64;
                let b = (p as usize).min(n_bins - 1);
                sums[b] += f64::from(255 - row[x]);
                counts[b] += 1;
                x += step;
            }
            y += step;
        }
        let max_count = counts.iter().copied().max().unwrap_or(0);
        if max_count == 0 {
            return 0.0;
        }
        let min_count = max_count / 2;
        let (mut n, mut mean, mut m2) = (0.0f64, 0.0f64, 0.0f64);
        for (s, c) in sums.iter().zip(&counts) {
            if *c < min_count.max(1) {
                continue;
            }
            let v = s / f64::from(*c);
            n += 1.0;
            let delta = v - mean;
            mean += delta / n;
            m2 += delta * (v - mean);
        }
        if n < 2.0 { 0.0 } else { m2 / (n - 1.0) }
    };

    // Coarse 1° sweep.
    let coarse_steps = max_angle.floor() as i64;
    let mut best = (0.0f64, f64::NEG_INFINITY);
    let mut k = -coarse_steps;
    while k <= coarse_steps {
        let t = k as f64;
        let sc = score(t);
        if sc > best.1 {
            best = (t, sc);
        }
        k += 1;
    }
    // Isotropic content: every angle scores about the same. Compare the
    // peak against the sweep's baseline.
    let baseline = score(
        if best.0 > 0.0 {
            best.0 - max_angle
        } else {
            best.0 + max_angle
        }
        .clamp(-max_angle, max_angle),
    );
    if best.1.is_nan() || best.1 <= baseline * 1.05 || best.1 <= 0.0 {
        return None;
    }
    // Fine 0.1° refinement around the coarse peak.
    let mut fine = best;
    let mut j = -10i64;
    while j <= 10 {
        let t = best.0 + j as f64 * 0.1;
        if t.abs() <= max_angle {
            let sc = score(t);
            if sc > fine.1 {
                fine = (t, sc);
            }
        }
        j += 1;
    }
    Some(fine.0 as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    /// Parallel dark lines (3 px thick, 16 px apart) with direction
    /// `(cos a, sin a)` in image coordinates — a horizontal ruling rotated
    /// by `a` degrees in `RotateEffect`'s convention. Edges are anti-aliased
    /// (1 px coverage ramp) like any resampled scan; hard-edged staircase
    /// lines bias a gradient estimator by ~10%.
    fn ruled(w: u32, h: u32, angle_deg: f32) -> Vec<u8> {
        let (s, c) = angle_deg.to_radians().sin_cos();
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        let mut px = vec![255u8; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let p = (y as f32 - cy) * c - (x as f32 - cx) * s;
                let d = ((p % 16.0) + 16.0) % 16.0;
                // Coverage of the [0, 3) band with a 1 px linear ramp.
                let cov = (d + 0.5).min(3.5 - d).clamp(0.0, 1.0);
                px[(y * w + x) as usize] = (255.0 * (1.0 - cov)).round() as u8;
            }
        }
        px
    }

    /// Gradient moment is the coarse seed: within ~15% of the angle
    /// (+0.3° floor) on thin anti-aliased rulings, sign always right.
    #[test]
    fn gradient_moment_is_a_coarse_estimate_with_the_right_sign() {
        for a in [-9.0f32, -5.0, -2.5, 0.0, 1.3, 3.0, 6.5, 9.9] {
            let img = ruled(256, 256, a);
            let got = detect_skew_gradient_moment(&img, 256, 256, 256, 15.0)
                .unwrap_or_else(|| panic!("angle {a}: no detection"));
            let tol = 0.3 + 0.15 * a.abs();
            assert!(
                (got - a).abs() <= tol,
                "angle {a}: detected {got} (tol {tol})"
            );
        }
    }

    #[test]
    fn projection_variance_recovers_known_angles_within_0_2_degrees() {
        for a in [-9.0f32, -5.0, -2.5, -0.7, 0.0, 1.3, 3.0, 6.5, 9.9] {
            let img = ruled(256, 256, a);
            let got = detect_skew_projection_variance(&img, 256, 256, 256, 15.0)
                .unwrap_or_else(|| panic!("angle {a}: no detection"));
            assert!((got - a).abs() <= 0.2, "angle {a}: detected {got}");
        }
    }

    #[test]
    fn vertical_structure_counts_as_straight() {
        let img = ruled(200, 200, 90.0);
        let got = detect_skew_gradient_moment(&img, 200, 200, 200, 10.0).unwrap();
        assert!(got.abs() <= 0.5, "vertical lines (gradient moment): {got}");
        let img = ruled(200, 200, 93.0);
        let got = detect_skew_gradient_moment(&img, 200, 200, 200, 10.0).unwrap();
        assert!(
            (got - 3.0).abs() <= 0.8,
            "vertical lines +3 (gradient moment): {got}"
        );
    }

    #[test]
    fn rejects_uniform_and_out_of_range() {
        let flat = vec![128u8; 64 * 64];
        assert_eq!(detect_skew_gradient_moment(&flat, 64, 64, 64, 10.0), None);
        assert_eq!(
            detect_skew_projection_variance(&flat, 64, 64, 64, 10.0),
            None
        );
        // 20° skew with a 10° budget → None from the tensor, not a clamped guess.
        let img = ruled(256, 256, 20.0);
        assert_eq!(detect_skew_gradient_moment(&img, 256, 256, 256, 10.0), None);
        // Degenerate sizes.
        assert_eq!(detect_skew_gradient_moment(&flat, 2, 2, 2, 10.0), None);
        assert_eq!(detect_skew_projection_variance(&flat, 2, 2, 2, 10.0), None);
        assert_eq!(
            detect_skew_gradient_moment(&flat[..10], 64, 64, 64, 10.0),
            None
        );
        assert_eq!(
            detect_skew_projection_variance(&flat[..10], 64, 64, 64, 10.0),
            None
        );
    }

    #[test]
    fn strided_and_subsampled_inputs_agree() {
        let img = ruled(1400, 900, 4.0);
        let full = detect_skew_projection_variance(&img, 1400, 900, 1400, 10.0).unwrap();
        assert!((full - 4.0).abs() <= 0.2, "{full}");
        // Same pixels behind a padded stride.
        let stride = 1500;
        let mut padded = vec![7u8; stride * 900];
        for y in 0..900 {
            padded[y * stride..y * stride + 1400].copy_from_slice(&img[y * 1400..(y + 1) * 1400]);
        }
        let strided = detect_skew_projection_variance(&padded, 1400, 900, stride, 10.0).unwrap();
        assert_eq!(strided, full);
        let gm = detect_skew_gradient_moment(&padded, 1400, 900, stride, 10.0).unwrap();
        assert_eq!(
            gm,
            detect_skew_gradient_moment(&img, 1400, 900, 1400, 10.0).unwrap()
        );
    }
}
