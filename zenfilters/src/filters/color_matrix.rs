use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use zenpixels_convert::oklab;

/// 5×5 color matrix applied in linear RGB space.
///
/// The matrix transforms `[R, G, B, A, 1]` → `[R', G', B', A', 1]` using
/// row-major 5×5 layout (25 elements). The 5th column is the bias/offset.
///
/// Unlike imageflow4 which applies the matrix in sRGB gamma space, this
/// filter converts each pixel from Oklab → linear RGB, applies the matrix,
/// then converts back to Oklab. This avoids the perceptual non-linearity
/// of gamma-space matrix operations.
///
/// The matrix uses BT.709 primaries for the Oklab↔RGB conversion since
/// the matrix coefficients are defined relative to standard RGB.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ColorMatrix {
    /// Row-major 5×5 matrix (25 elements).
    /// `[R',G',B',A',1] = M × [R,G,B,A,1]`
    pub matrix: [f32; 25],
}

impl Default for ColorMatrix {
    fn default() -> Self {
        Self {
            matrix: Self::IDENTITY,
        }
    }
}

impl ColorMatrix {
    /// Identity matrix (no-op).
    pub const IDENTITY: [f32; 25] = [
        1.0, 0.0, 0.0, 0.0, 0.0, // R' = R
        0.0, 1.0, 0.0, 0.0, 0.0, // G' = G
        0.0, 0.0, 1.0, 0.0, 0.0, // B' = B
        0.0, 0.0, 0.0, 1.0, 0.0, // A' = A
        0.0, 0.0, 0.0, 0.0, 1.0, // 1' = 1
    ];
}

impl Filter for ColorMatrix {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::ALL
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        // We need BT.709 matrices for the Oklab↔RGB round-trip.
        // These are the same matrices used in scatter_gather for BT.709.
        let m1_inv = oklab::lms_to_rgb_matrix(zenpixels::ColorPrimaries::Bt709)
            .expect("BT.709 always supported");
        let m1 = oklab::rgb_to_lms_matrix(zenpixels::ColorPrimaries::Bt709)
            .expect("BT.709 always supported");

        let m = &self.matrix;
        let n = planes.pixel_count();

        for i in 0..n {
            // Oklab → linear RGB
            let [r, g, b] = oklab::oklab_to_rgb(planes.l[i], planes.a[i], planes.b[i], &m1_inv);
            let a = planes.alpha.as_ref().map_or(1.0, |alpha| alpha[i]);

            // Apply 5×5 matrix
            let nr = m[0] * r + m[1] * g + m[2] * b + m[3] * a + m[4];
            let ng = m[5] * r + m[6] * g + m[7] * b + m[8] * a + m[9];
            let nb = m[10] * r + m[11] * g + m[12] * b + m[13] * a + m[14];
            let na = m[15] * r + m[16] * g + m[17] * b + m[18] * a + m[19];

            // Linear RGB → Oklab
            let [l, oa, ob] = oklab::rgb_to_oklab(nr.max(0.0), ng.max(0.0), nb.max(0.0), &m1);
            planes.l[i] = l;
            planes.a[i] = oa;
            planes.b[i] = ob;

            if let Some(alpha) = &mut planes.alpha {
                alpha[i] = na.clamp(0.0, 1.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_matrix_is_noop() {
        let mut planes = OklabPlanes::new(8, 8);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = 0.3 + (i as f32) * 0.005;
        }
        for (i, v) in planes.a.iter_mut().enumerate() {
            *v = (i as f32 - 32.0) * 0.001;
        }
        for (i, v) in planes.b.iter_mut().enumerate() {
            *v = (32.0 - i as f32) * 0.001;
        }
        let orig_l = planes.l.clone();
        let orig_a = planes.a.clone();
        let orig_b = planes.b.clone();

        ColorMatrix {
            matrix: ColorMatrix::IDENTITY,
        }
        .apply(&mut planes, &mut FilterContext::new());

        for i in 0..planes.pixel_count() {
            assert!(
                (planes.l[i] - orig_l[i]).abs() < 1e-4,
                "L[{i}]: {} vs {}",
                planes.l[i],
                orig_l[i]
            );
            assert!(
                (planes.a[i] - orig_a[i]).abs() < 1e-4,
                "a[{i}]: {} vs {}",
                planes.a[i],
                orig_a[i]
            );
            assert!(
                (planes.b[i] - orig_b[i]).abs() < 1e-4,
                "b[{i}]: {} vs {}",
                planes.b[i],
                orig_b[i]
            );
        }
    }

    #[test]
    fn grayscale_via_matrix() {
        // BT.709 luma matrix (sums to ~1.0)
        let mut m = [0.0f32; 25];
        // R' = 0.2126*R + 0.7152*G + 0.0722*B
        m[0] = 0.2126;
        m[1] = 0.7152;
        m[2] = 0.0722;
        // G' = same
        m[5] = 0.2126;
        m[6] = 0.7152;
        m[7] = 0.0722;
        // B' = same
        m[10] = 0.2126;
        m[11] = 0.7152;
        m[12] = 0.0722;
        // A' = A
        m[18] = 1.0;
        // 1' = 1
        m[24] = 1.0;

        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.l {
            *v = 0.5;
        }
        for v in &mut planes.a {
            *v = 0.05;
        }
        for v in &mut planes.b {
            *v = -0.03;
        }

        ColorMatrix { matrix: m }.apply(&mut planes, &mut FilterContext::new());

        // After grayscale matrix, chroma should be near zero
        for i in 0..planes.pixel_count() {
            assert!(
                planes.a[i].abs() < 0.01,
                "a[{i}] should be near zero: {}",
                planes.a[i]
            );
            assert!(
                planes.b[i].abs() < 0.01,
                "b[{i}] should be near zero: {}",
                planes.b[i]
            );
        }
    }

    #[test]
    fn brightness_via_matrix_bias() {
        // Add 0.1 to all RGB channels via the bias column
        let mut m = ColorMatrix::IDENTITY;
        m[4] = 0.1; // R bias
        m[9] = 0.1; // G bias
        m[14] = 0.1; // B bias

        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.l {
            *v = 0.5;
        }
        let orig_l = planes.l[0];

        ColorMatrix { matrix: m }.apply(&mut planes, &mut FilterContext::new());

        // L should increase (brighter)
        assert!(
            planes.l[0] > orig_l,
            "brightness should increase: {} vs {}",
            planes.l[0],
            orig_l
        );
    }
}
