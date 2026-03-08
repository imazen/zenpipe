use crate::planes::OklabPlanes;
use zenpixels_convert::gamut::GamutMatrix;
use zenpixels_convert::oklab;

/// Convert interleaved linear RGB f32 to planar Oklab.
///
/// `src` is interleaved linear RGB(A) f32 data: `[R,G,B,(A), R,G,B,(A), ...]`.
/// `channels` is 3 (RGB) or 4 (RGBA).
/// `m1` is the RGB→LMS matrix from `oklab::rgb_to_lms_matrix(primaries)`.
/// `reference_white` normalizes HDR values (1.0 for SDR, 203.0 for PQ).
///
/// Populates `planes.l`, `planes.a`, `planes.b`, and optionally `planes.alpha`.
pub fn scatter_to_oklab(
    src: &[f32],
    planes: &mut OklabPlanes,
    channels: u32,
    m1: &GamutMatrix,
    reference_white: f32,
) {
    let n = planes.pixel_count();
    let ch = channels as usize;
    debug_assert!(ch == 3 || ch == 4);
    debug_assert!(src.len() >= n * ch);
    debug_assert!(planes.l.len() == n);

    let inv_white = 1.0 / reference_white;

    for i in 0..n {
        let base = i * ch;
        let r = src[base] * inv_white;
        let g = src[base + 1] * inv_white;
        let b = src[base + 2] * inv_white;

        let [l, a, ob] = oklab::rgb_to_oklab(r, g, b, m1);
        planes.l[i] = l;
        planes.a[i] = a;
        planes.b[i] = ob;

        if ch == 4
            && let Some(alpha) = &mut planes.alpha
        {
            alpha[i] = src[base + 3];
        }
    }
}

/// Convert planar Oklab back to interleaved linear RGB f32.
///
/// `dst` is interleaved linear RGB(A) f32 output buffer.
/// `m1_inv` is the LMS→RGB matrix from `oklab::lms_to_rgb_matrix(primaries)`.
/// `reference_white` denormalizes HDR values.
///
/// The output is clamped to `[0.0, ∞)` — negative values from out-of-gamut
/// colors are clipped. Use `GamutMapping::ChromaReduce` for hue-preserving
/// gamut mapping before calling this.
pub fn gather_from_oklab(
    planes: &OklabPlanes,
    dst: &mut [f32],
    channels: u32,
    m1_inv: &GamutMatrix,
    reference_white: f32,
) {
    let n = planes.pixel_count();
    let ch = channels as usize;
    debug_assert!(ch == 3 || ch == 4);
    debug_assert!(dst.len() >= n * ch);

    for i in 0..n {
        let [r, g, b] = oklab::oklab_to_rgb(planes.l[i], planes.a[i], planes.b[i], m1_inv);
        let base = i * ch;
        dst[base] = (r * reference_white).max(0.0);
        dst[base + 1] = (g * reference_white).max(0.0);
        dst[base + 2] = (b * reference_white).max(0.0);

        if ch == 4 {
            dst[base + 3] = planes.alpha.as_ref().map_or(1.0, |alpha| alpha[i]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenpixels::ColorPrimaries;

    fn make_test_rgb(width: u32, height: u32) -> Vec<f32> {
        let n = (width as usize) * (height as usize);
        let mut data = Vec::with_capacity(n * 3);
        for i in 0..n {
            let t = i as f32 / n as f32;
            data.push((t * 0.8 + 0.1).clamp(0.001, 1.0));
            data.push(((1.0 - t) * 0.7 + 0.15).clamp(0.001, 1.0));
            data.push((t * 0.5 + 0.2).clamp(0.001, 1.0));
        }
        data
    }

    #[test]
    fn scatter_gather_roundtrip() {
        let (w, h) = (64, 48);
        let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
        let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();

        let src = make_test_rgb(w, h);
        let mut planes = OklabPlanes::new(w, h);
        scatter_to_oklab(&src, &mut planes, 3, &m1, 1.0);

        let mut dst = vec![0.0f32; src.len()];
        gather_from_oklab(&planes, &mut dst, 3, &m1_inv, 1.0);

        let mut max_err = 0.0f32;
        for i in 0..src.len() {
            let err = (src[i] - dst[i]).abs();
            max_err = max_err.max(err);
        }
        assert!(
            max_err < 1e-3,
            "scatter/gather roundtrip max error: {max_err}"
        );
    }

    #[test]
    fn scatter_gather_roundtrip_rgba() {
        let (w, h) = (32, 32);
        let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
        let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();

        let n = (w as usize) * (h as usize);
        let mut src = Vec::with_capacity(n * 4);
        for i in 0..n {
            let t = i as f32 / n as f32;
            src.push(t * 0.8 + 0.1);
            src.push((1.0 - t) * 0.7 + 0.15);
            src.push(t * 0.5 + 0.2);
            src.push(0.8); // alpha
        }

        let mut planes = OklabPlanes::with_alpha(w, h);
        scatter_to_oklab(&src, &mut planes, 4, &m1, 1.0);

        // Verify alpha was captured
        for &a in planes.alpha.as_ref().unwrap() {
            assert!((a - 0.8).abs() < 1e-6);
        }

        let mut dst = vec![0.0f32; src.len()];
        gather_from_oklab(&planes, &mut dst, 4, &m1_inv, 1.0);

        let mut max_err = 0.0f32;
        for i in 0..n {
            for c in 0..3 {
                let err = (src[i * 4 + c] - dst[i * 4 + c]).abs();
                max_err = max_err.max(err);
            }
            // Alpha should be exact
            assert!((dst[i * 4 + 3] - 0.8).abs() < 1e-6);
        }
        assert!(
            max_err < 1e-3,
            "RGBA scatter/gather roundtrip max error: {max_err}"
        );
    }

    #[test]
    fn white_produces_l_near_one() {
        let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
        let src = vec![1.0f32; 3]; // single white pixel
        let mut planes = OklabPlanes::new(1, 1);
        scatter_to_oklab(&src, &mut planes, 3, &m1, 1.0);
        assert!(
            (planes.l[0] - 1.0).abs() < 5e-4,
            "white L = {}",
            planes.l[0]
        );
        assert!(planes.a[0].abs() < 5e-4, "white a = {}", planes.a[0]);
        assert!(planes.b[0].abs() < 5e-4, "white b = {}", planes.b[0]);
    }

    #[test]
    fn black_produces_l_near_zero() {
        let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
        let src = vec![0.0f32; 3];
        let mut planes = OklabPlanes::new(1, 1);
        scatter_to_oklab(&src, &mut planes, 3, &m1, 1.0);
        assert!(planes.l[0].abs() < 1e-6, "black L = {}", planes.l[0]);
    }

    #[test]
    fn hdr_normalization() {
        let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
        let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();

        // Simulate PQ reference white at 203 nits
        let ref_white = 203.0;
        let src = vec![ref_white; 3]; // reference white in absolute luminance
        let mut planes = OklabPlanes::new(1, 1);
        scatter_to_oklab(&src, &mut planes, 3, &m1, ref_white);

        // After normalization, reference white should be L ≈ 1.0
        assert!(
            (planes.l[0] - 1.0).abs() < 5e-4,
            "HDR ref white L = {}",
            planes.l[0]
        );

        let mut dst = vec![0.0f32; 3];
        gather_from_oklab(&planes, &mut dst, 3, &m1_inv, ref_white);
        for c in 0..3 {
            assert!(
                (dst[c] - ref_white).abs() < 0.1,
                "HDR roundtrip channel {c}: {} vs {ref_white}",
                dst[c]
            );
        }
    }
}
