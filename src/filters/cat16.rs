/// CAT16 chromatic adaptation transform.
///
/// Implements CIECAM16 chromatic adaptation for adapting scene illuminant
/// white points. Used to match darktable's color calibration module behavior.
///
/// The adaptation converts between illuminant white points in LMS cone
/// response space using the CAT16 matrix from the CIE CAM16 model.
///
/// Reference: darktable's `chromatic_adaptation.h` (GPL-2.0+),
/// reimplemented from the mathematical specification.

/// CIE XYZ → CAT16 LMS matrix (CIECAM16 cone responses).
///
/// From darktable's `chromatic_adaptation.h` lines 92-94.
const XYZ_TO_CAT16: [[f32; 3]; 3] = [
    [0.401288, 0.650173, -0.051461],
    [-0.250268, 1.204414, 0.045854],
    [-0.002079, 0.048952, 0.953127],
];

/// CAT16 LMS → CIE XYZ inverse matrix.
///
/// From darktable's `chromatic_adaptation.h` lines 101-103.
const CAT16_TO_XYZ: [[f32; 3]; 3] = [
    [1.862068, -1.011255, 0.149187],
    [0.387520, 0.621447, -0.008974],
    [-0.015841, -0.034123, 1.049964],
];

/// D50 white point in CAT16 LMS space.
///
/// From darktable's `chromatic_adaptation.h` `CAT16_adapt_D50`.
const D50_LMS: [f32; 3] = [0.994535, 1.000997, 0.833036];

/// D65 white point in CAT16 LMS space.
///
/// From darktable's `chromatic_adaptation.h` `CAT16_adapt_D65`.
const D65_LMS: [f32; 3] = [0.97553267, 1.01647859, 1.08483440];

/// Standard sRGB → XYZ matrix (D65 white point, IEC 61966-2-1).
const SRGB_TO_XYZ_D65: [[f32; 3]; 3] = [
    [0.4123908, 0.3575843, 0.1804808],
    [0.2126390, 0.7151687, 0.0721923],
    [0.0193308, 0.1191948, 0.9505322],
];

/// Standard XYZ → sRGB matrix (D65 white point, IEC 61966-2-1).
const XYZ_TO_SRGB_D65: [[f32; 3]; 3] = [
    [3.2404542, -1.5371385, -0.4985314],
    [-0.9692660, 1.8760108, 0.0415560],
    [0.0556434, -0.2040259, 1.0572252],
];

/// Compute the scene illuminant xy chromaticity from DNG metadata.
///
/// Tries `as_shot_white_xy` first (direct), then falls back to computing
/// from `as_shot_neutral` + `color_matrix` (inverse mapping).
///
/// Returns `None` if insufficient metadata is available.
pub fn illuminant_xy_from_dng(
    as_shot_white_xy: Option<(f64, f64)>,
    as_shot_neutral: Option<&[f64]>,
    color_matrix: Option<&[f64]>,
) -> Option<(f32, f32)> {
    // Direct path: AsShotWhiteXY tag
    if let Some((x, y)) = as_shot_white_xy {
        if y > 1e-6 {
            return Some((x as f32, y as f32));
        }
    }

    // Computed path: inv(ColorMatrix) * AsShotNeutral → XYZ → xy
    let neutral = as_shot_neutral?;
    let cm = color_matrix?;
    if neutral.len() < 3 || cm.len() < 9 {
        return None;
    }

    // ColorMatrix is XYZ→camera (3x3 stored row-major).
    // We need camera→XYZ, so invert it.
    let cm3x3 = [
        [cm[0] as f32, cm[1] as f32, cm[2] as f32],
        [cm[3] as f32, cm[4] as f32, cm[5] as f32],
        [cm[6] as f32, cm[7] as f32, cm[8] as f32],
    ];

    let inv = invert_3x3(&cm3x3)?;

    // XYZ = inv(CM) * neutral
    let n = [neutral[0] as f32, neutral[1] as f32, neutral[2] as f32];
    let xyz = mat_vec(&inv, &n);

    let sum = xyz[0] + xyz[1] + xyz[2];
    if sum.abs() < 1e-10 {
        return None;
    }

    Some((xyz[0] / sum, xyz[1] / sum))
}

/// Convert illuminant xy chromaticity to XYZ (Y=1 normalization).
fn xy_to_xyz(x: f32, y: f32) -> [f32; 3] {
    if y.abs() < 1e-10 {
        return [0.0, 1.0, 0.0];
    }
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// Compute a single 3x3 CAT16 adaptation matrix for linear sRGB data.
///
/// Adapts from `source_xy` (scene illuminant) to `target` (D50 or D65) in
/// CAT16 LMS space, with the surrounding sRGB↔XYZ conversions baked in.
///
/// The composed matrix is:
/// `XYZ_to_sRGB × CAT16_to_XYZ × diag(target_LMS / source_LMS) × XYZ_to_CAT16 × sRGB_to_XYZ`
///
/// This can be applied directly to linear sRGB triplets.
pub fn cat16_adaptation_matrix(source_xy: (f32, f32), target: AdaptationTarget) -> [[f32; 3]; 3] {
    let target_lms = match target {
        AdaptationTarget::D50 => D50_LMS,
        AdaptationTarget::D65 => D65_LMS,
    };

    // Source illuminant in XYZ and CAT16 LMS
    let source_xyz = xy_to_xyz(source_xy.0, source_xy.1);
    let source_lms = mat_vec(&XYZ_TO_CAT16, &source_xyz);

    // Adaptation diagonal: target_LMS / source_LMS
    let gain = [
        if source_lms[0].abs() > 1e-10 { target_lms[0] / source_lms[0] } else { 1.0 },
        if source_lms[1].abs() > 1e-10 { target_lms[1] / source_lms[1] } else { 1.0 },
        if source_lms[2].abs() > 1e-10 { target_lms[2] / source_lms[2] } else { 1.0 },
    ];

    // Compose: sRGB→XYZ→CAT16→scale→CAT16→XYZ→sRGB
    // Step 1: sRGB→XYZ→CAT16 = XYZ_TO_CAT16 × SRGB_TO_XYZ
    let srgb_to_lms = mat_mul(&XYZ_TO_CAT16, &SRGB_TO_XYZ_D65);

    // Step 2: Apply diagonal
    let scaled = [
        [srgb_to_lms[0][0] * gain[0], srgb_to_lms[0][1] * gain[0], srgb_to_lms[0][2] * gain[0]],
        [srgb_to_lms[1][0] * gain[1], srgb_to_lms[1][1] * gain[1], srgb_to_lms[1][2] * gain[1]],
        [srgb_to_lms[2][0] * gain[2], srgb_to_lms[2][1] * gain[2], srgb_to_lms[2][2] * gain[2]],
    ];

    // Step 3: CAT16→XYZ→sRGB = XYZ_TO_SRGB × CAT16_TO_XYZ
    let lms_to_srgb = mat_mul(&XYZ_TO_SRGB_D65, &CAT16_TO_XYZ);

    // Final: lms_to_srgb × scaled
    mat_mul(&lms_to_srgb, &scaled)
}

/// Target white point for chromatic adaptation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdaptationTarget {
    /// CIE D50 (darktable pipeline white point).
    D50,
    /// CIE D65 (sRGB standard illuminant).
    D65,
}

/// Apply CAT16 chromatic adaptation to interleaved linear sRGB f32 data in-place.
///
/// `data` is interleaved RGB triplets. `source_xy` is the scene illuminant chromaticity.
pub fn apply_cat16(data: &mut [f32], source_xy: (f32, f32), target: AdaptationTarget) {
    let m = cat16_adaptation_matrix(source_xy, target);
    let n = data.len() / 3;
    for i in 0..n {
        let base = i * 3;
        let r = data[base];
        let g = data[base + 1];
        let b = data[base + 2];
        data[base] = m[0][0] * r + m[0][1] * g + m[0][2] * b;
        data[base + 1] = m[1][0] * r + m[1][1] * g + m[1][2] * b;
        data[base + 2] = m[2][0] * r + m[2][1] * g + m[2][2] * b;
    }
}

// ── Matrix math helpers ──────────────────────────────────────────────

fn mat_vec(m: &[[f32; 3]; 3], v: &[f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn mat_mul(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    out
}

fn invert_3x3(m: &[[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);

    if det.abs() < 1e-10 {
        return None;
    }

    let inv_det = 1.0 / det;
    Some([
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv_det,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv_det,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv_det,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv_det,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv_det,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv_det,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv_det,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv_det,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv_det,
        ],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d65_to_d65_is_identity() {
        // Adapting from D65 to D65 should be near-identity
        let d65_xy = (0.3127, 0.3290);
        let m = cat16_adaptation_matrix(d65_xy, AdaptationTarget::D65);
        // Check diagonal is ~1.0 and off-diagonal is ~0.0
        for i in 0..3 {
            for j in 0..3 {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!(
                    (m[i][j] - expected).abs() < 0.01,
                    "m[{i}][{j}] = {} expected {expected}",
                    m[i][j]
                );
            }
        }
    }

    #[test]
    fn d50_to_d50_is_identity() {
        let d50_xy = (0.3457, 0.3585);
        let m = cat16_adaptation_matrix(d50_xy, AdaptationTarget::D50);
        for i in 0..3 {
            for j in 0..3 {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!(
                    (m[i][j] - expected).abs() < 0.01,
                    "m[{i}][{j}] = {} expected {expected}",
                    m[i][j]
                );
            }
        }
    }

    #[test]
    fn tungsten_to_d65_warms_blues() {
        // Tungsten (2856K) adapted to D65 should cool the image (reduce red, boost blue)
        let tungsten_xy = (0.4476, 0.4074);
        let m = cat16_adaptation_matrix(tungsten_xy, AdaptationTarget::D65);
        // Red channel gain should decrease (warm → cool)
        assert!(m[0][0] < 1.0, "red gain should decrease for tungsten→D65: {}", m[0][0]);
        // Blue channel gain should increase
        assert!(m[2][2] > 1.0, "blue gain should increase for tungsten→D65: {}", m[2][2]);
    }

    #[test]
    fn apply_preserves_neutral_at_target() {
        // A pure white pixel under D65 illuminant adapted to D65 should stay white
        let d65_xy = (0.3127, 0.3290);
        let mut data = vec![0.5f32, 0.5, 0.5]; // neutral gray
        apply_cat16(&mut data, d65_xy, AdaptationTarget::D65);
        let diff = (data[0] - 0.5).abs() + (data[1] - 0.5).abs() + (data[2] - 0.5).abs();
        assert!(diff < 0.02, "neutral shifted: {:?}", data);
    }

    #[test]
    fn illuminant_from_white_xy() {
        let xy = illuminant_xy_from_dng(Some((0.3127, 0.3290)), None, None);
        assert!(xy.is_some());
        let (x, y) = xy.unwrap();
        assert!((x - 0.3127).abs() < 1e-4);
        assert!((y - 0.3290).abs() < 1e-4);
    }

    #[test]
    fn illuminant_from_neutral_and_cm() {
        // Use a known color matrix (identity-like) and neutral to verify computation
        // With CM = identity, inv(CM) = identity, so XYZ = neutral
        // neutral = [0.95047, 1.0, 1.08883] (D65 XYZ), xy should be ~D65
        let neutral = vec![0.95047, 1.0, 1.08883];
        let cm = vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        let xy = illuminant_xy_from_dng(None, Some(&neutral), Some(&cm));
        assert!(xy.is_some());
        let (x, y) = xy.unwrap();
        // D65 xy ≈ (0.3127, 0.3290)
        assert!((x - 0.3127).abs() < 0.01, "x = {x}");
        assert!((y - 0.3290).abs() < 0.01, "y = {y}");
    }

    #[test]
    fn invert_identity() {
        let id = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let inv = invert_3x3(&id).unwrap();
        for i in 0..3 {
            for j in 0..3 {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!((inv[i][j] - expected).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn cat16_roundtrip() {
        // XYZ_TO_CAT16 × CAT16_TO_XYZ should be near identity
        let product = mat_mul(&XYZ_TO_CAT16, &CAT16_TO_XYZ);
        for i in 0..3 {
            for j in 0..3 {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!(
                    (product[i][j] - expected).abs() < 0.001,
                    "roundtrip[{i}][{j}] = {} expected {expected}",
                    product[i][j]
                );
            }
        }
    }
}
