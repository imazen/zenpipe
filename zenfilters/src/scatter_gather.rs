use crate::planes::OklabPlanes;
use crate::simd;
use zenpixels_convert::gamut::GamutMatrix;

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

    // SIMD dispatch handles the RGB→Oklab conversion
    simd::scatter_oklab(
        src,
        &mut planes.l,
        &mut planes.a,
        &mut planes.b,
        channels,
        m1,
        inv_white,
    );

    // Alpha is a straight copy — handle separately
    if ch == 4
        && let Some(alpha) = &mut planes.alpha
    {
        for i in 0..n {
            alpha[i] = src[i * ch + 3];
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

    // SIMD dispatch handles the Oklab→RGB conversion
    simd::gather_oklab(
        &planes.l,
        &planes.a,
        &planes.b,
        dst,
        channels,
        m1_inv,
        reference_white,
    );

    // Alpha is a straight copy
    if ch == 4 {
        for i in 0..n {
            dst[i * ch + 3] = planes.alpha.as_ref().map_or(1.0, |alpha| alpha[i]);
        }
    }
}

/// Convert interleaved sRGB u8 directly to planar Oklab (fused path).
///
/// Fuses sRGB→linear LUT with RGB→Oklab matrix math in one SIMD pass,
/// eliminating the intermediate linear f32 buffer (~48MB at 2048² RGB).
/// For sRGB u8 BT.709 input, this replaces the RowConverter + scatter pair.
///
/// `src` is interleaved sRGB u8 data: `[R,G,B,(A), R,G,B,(A), ...]`.
/// `channels` is 3 (RGB) or 4 (RGBA).
/// `m1` is the RGB→LMS matrix from `oklab::rgb_to_lms_matrix(primaries)`.
pub fn scatter_srgb_u8_to_oklab(
    src: &[u8],
    planes: &mut OklabPlanes,
    channels: u32,
    m1: &GamutMatrix,
) {
    let n = planes.pixel_count();
    let ch = channels as usize;
    debug_assert!(ch == 3 || ch == 4);
    debug_assert!(src.len() >= n * ch);

    simd::scatter_srgb_u8_to_oklab(
        src,
        &mut planes.l,
        &mut planes.a,
        &mut planes.b,
        channels,
        m1,
    );

    // Alpha: convert u8 to f32 (0–255 → 0.0–1.0)
    if ch == 4
        && let Some(alpha) = &mut planes.alpha
    {
        for i in 0..n {
            alpha[i] = src[i * ch + 3] as f32 / 255.0;
        }
    }
}

/// Convert planar Oklab back to interleaved sRGB u8 (fused path).
///
/// Fuses Oklab→RGB matrix math with linear→sRGB LUT in one SIMD pass,
/// eliminating the intermediate linear f32 buffer.
///
/// `dst` is interleaved sRGB u8 output buffer.
/// `m1_inv` is the LMS→RGB matrix from `oklab::lms_to_rgb_matrix(primaries)`.
pub fn gather_oklab_to_srgb_u8(
    planes: &OklabPlanes,
    dst: &mut [u8],
    channels: u32,
    m1_inv: &GamutMatrix,
) {
    let n = planes.pixel_count();
    let ch = channels as usize;
    debug_assert!(ch == 3 || ch == 4);
    debug_assert!(dst.len() >= n * ch);

    simd::gather_oklab_to_srgb_u8(&planes.l, &planes.a, &planes.b, dst, channels, m1_inv);

    // Alpha: convert f32 to u8 (0.0–1.0 → 0–255)
    if ch == 4 {
        for i in 0..n {
            let a = planes.alpha.as_ref().map_or(1.0, |alpha| alpha[i]);
            dst[i * ch + 3] = (a * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        }
    }
}

// ─── sRGB passthrough (ImageMagick-compatible) ─────────────────────

/// Scatter interleaved linear RGB f32 directly to planes (no Oklab conversion).
///
/// Maps R→L, G→a, B→b planes. Used for `WorkingSpace::Srgb` where filters
/// operate in the same color space as ImageMagick.
pub fn scatter_srgb_passthrough(src: &[f32], planes: &mut OklabPlanes, channels: u32) {
    let n = planes.pixel_count();
    let ch = channels as usize;
    debug_assert!(ch == 3 || ch == 4);
    debug_assert!(src.len() >= n * ch);

    for i in 0..n {
        planes.l[i] = src[i * ch];
        planes.a[i] = src[i * ch + 1];
        planes.b[i] = src[i * ch + 2];
    }

    if ch == 4
        && let Some(alpha) = &mut planes.alpha
    {
        for i in 0..n {
            alpha[i] = src[i * ch + 3];
        }
    }
}

/// Scatter interleaved sRGB u8 directly to f32 planes (no Oklab conversion).
///
/// Normalizes u8 [0-255] to f32 [0.0-1.0]. Maps R→L, G→a, B→b.
#[cfg(feature = "srgb-compat")]
pub fn scatter_srgb_u8_passthrough(src: &[u8], planes: &mut OklabPlanes, channels: u32) {
    let n = planes.pixel_count();
    let ch = channels as usize;
    debug_assert!(ch == 3 || ch == 4);
    debug_assert!(src.len() >= n * ch);

    let inv255 = 1.0 / 255.0;
    for i in 0..n {
        planes.l[i] = src[i * ch] as f32 * inv255;
        planes.a[i] = src[i * ch + 1] as f32 * inv255;
        planes.b[i] = src[i * ch + 2] as f32 * inv255;
    }

    if ch == 4
        && let Some(alpha) = &mut planes.alpha
    {
        for i in 0..n {
            alpha[i] = src[i * ch + 3] as f32 * inv255;
        }
    }
}

/// Gather f32 planes back to interleaved sRGB u8 (no Oklab conversion).
#[cfg(feature = "srgb-compat")]
pub fn gather_srgb_u8_passthrough(planes: &OklabPlanes, dst: &mut [u8], channels: u32) {
    let n = planes.pixel_count();
    let ch = channels as usize;
    debug_assert!(ch == 3 || ch == 4);
    debug_assert!(dst.len() >= n * ch);

    for i in 0..n {
        dst[i * ch] = (planes.l[i] * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        dst[i * ch + 1] = (planes.a[i] * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        dst[i * ch + 2] = (planes.b[i] * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    }

    if ch == 4 {
        for i in 0..n {
            let a = planes.alpha.as_ref().map_or(1.0, |alpha| alpha[i]);
            dst[i * ch + 3] = (a * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;
    use zenpixels::ColorPrimaries;
    use zenpixels_convert::oklab;

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

    /// Test that non-8-aligned pixel counts work correctly (SIMD tail handling).
    #[test]
    fn non_aligned_pixel_count() {
        let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
        let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();

        // 13 pixels — not a multiple of 8
        let src = make_test_rgb(13, 1);
        let mut planes = OklabPlanes::new(13, 1);
        scatter_to_oklab(&src, &mut planes, 3, &m1, 1.0);

        let mut dst = vec![0.0f32; src.len()];
        gather_from_oklab(&planes, &mut dst, 3, &m1_inv, 1.0);

        let mut max_err = 0.0f32;
        for i in 0..src.len() {
            max_err = max_err.max((src[i] - dst[i]).abs());
        }
        assert!(max_err < 1e-3, "non-aligned roundtrip max error: {max_err}");
    }

    fn make_test_srgb_u8(width: u32, height: u32) -> alloc::vec::Vec<u8> {
        let n = (width as usize) * (height as usize);
        let mut data = alloc::vec::Vec::with_capacity(n * 3);
        for i in 0..n {
            let t = i as f32 / n as f32;
            data.push((t * 200.0 + 30.0) as u8);
            data.push(((1.0 - t) * 180.0 + 40.0) as u8);
            data.push((t * 100.0 + 80.0) as u8);
        }
        data
    }

    #[test]
    fn fused_srgb_u8_roundtrip() {
        let (w, h) = (64, 48);
        let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
        let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();

        let src = make_test_srgb_u8(w, h);
        let mut planes = OklabPlanes::new(w, h);
        scatter_srgb_u8_to_oklab(&src, &mut planes, 3, &m1);

        // White (255, 255, 255) should give L ≈ 1.0
        // Test general range of L values
        for &l in &*planes.l {
            assert!(l >= 0.0 && l <= 1.1, "L out of range: {l}");
        }

        let mut dst = vec![0u8; src.len()];
        gather_oklab_to_srgb_u8(&planes, &mut dst, 3, &m1_inv);

        let mut max_err = 0u8;
        for (a, b) in src.iter().zip(dst.iter()) {
            let err = (*a as i16 - *b as i16).unsigned_abs() as u8;
            max_err = max_err.max(err);
        }
        assert!(
            max_err <= 1,
            "fused sRGB u8 roundtrip max error: {max_err} (should be ≤1)"
        );
    }

    #[test]
    fn fused_srgb_u8_non_aligned() {
        let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
        let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();

        // 13 pixels — not a multiple of 8
        let src = make_test_srgb_u8(13, 1);
        let mut planes = OklabPlanes::new(13, 1);
        scatter_srgb_u8_to_oklab(&src, &mut planes, 3, &m1);

        let mut dst = vec![0u8; src.len()];
        gather_oklab_to_srgb_u8(&planes, &mut dst, 3, &m1_inv);

        let mut max_err = 0u8;
        for (a, b) in src.iter().zip(dst.iter()) {
            let err = (*a as i16 - *b as i16).unsigned_abs() as u8;
            max_err = max_err.max(err);
        }
        assert!(
            max_err <= 1,
            "fused non-aligned sRGB u8 roundtrip max error: {max_err}"
        );
    }

    #[test]
    fn fused_matches_general_path() {
        // Verify fused path produces same Oklab values as general path
        let (w, h) = (32, 32);
        let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();

        let src_u8 = make_test_srgb_u8(w, h);

        // Convert u8 → linear f32 manually for the general path
        let n = (w as usize) * (h as usize);
        let mut src_f32 = alloc::vec::Vec::with_capacity(n * 3);
        for i in 0..n {
            src_f32.push(linear_srgb::default::srgb_u8_to_linear(src_u8[i * 3]));
            src_f32.push(linear_srgb::default::srgb_u8_to_linear(src_u8[i * 3 + 1]));
            src_f32.push(linear_srgb::default::srgb_u8_to_linear(src_u8[i * 3 + 2]));
        }

        // General path
        let mut planes_gen = OklabPlanes::new(w, h);
        scatter_to_oklab(&src_f32, &mut planes_gen, 3, &m1, 1.0);

        // Fused path
        let mut planes_fused = OklabPlanes::new(w, h);
        scatter_srgb_u8_to_oklab(&src_u8, &mut planes_fused, 3, &m1);

        // Compare Oklab values — should be identical (both use same LUT)
        let mut max_err = 0.0f32;
        for i in 0..n {
            max_err = max_err.max((planes_gen.l[i] - planes_fused.l[i]).abs());
            max_err = max_err.max((planes_gen.a[i] - planes_fused.a[i]).abs());
            max_err = max_err.max((planes_gen.b[i] - planes_fused.b[i]).abs());
        }
        assert!(
            max_err < 1e-5,
            "fused vs general Oklab max error: {max_err}"
        );
    }
}
