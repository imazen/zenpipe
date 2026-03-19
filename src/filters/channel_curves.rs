use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::prelude::*;

/// Per-channel tone curves applied independently to R, G, B in sRGB space.
///
/// Unlike [`ToneCurve`](super::ToneCurve) which operates on Oklab L (preserving
/// color ratios), `ChannelCurves` operates per-channel in sRGB, enabling
/// independent tonal correction of each color channel.
///
/// Use cases:
/// - Match a target rendering by correcting per-channel tonal shape
/// - Compensate for color matrix inaccuracies
/// - Creative cross-processing effects
///
/// Each channel has its own 256-entry LUT mapping sRGB [0,1] → [0,1].
/// The filter converts Oklab → linear sRGB → apply curves → Oklab.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ChannelCurves {
    /// R, G, B LUTs, each 256 entries mapping [0,1] → [0,1].
    luts: [Vec<f32>; 3],
}

impl Default for ChannelCurves {
    fn default() -> Self {
        let id: Vec<f32> = (0..crate::LUT_SIZE)
            .map(|i| i as f32 / crate::LUT_MAX as f32)
            .collect();
        Self {
            luts: [id.clone(), id.clone(), id],
        }
    }
}

impl ChannelCurves {
    /// Create from three pre-computed 256-entry LUTs (R, G, B).
    pub fn from_luts(r: Vec<f32>, g: Vec<f32>, b: Vec<f32>) -> Self {
        Self { luts: [r, g, b] }
    }

    /// Create from control points for each channel.
    ///
    /// Points are `(input, output)` pairs in [0, 1], sorted by input.
    /// Uses monotone cubic Hermite interpolation (same as [`ToneCurve`]).
    pub fn from_points(r_pts: &[(f32, f32)], g_pts: &[(f32, f32)], b_pts: &[(f32, f32)]) -> Self {
        Self {
            luts: [
                build_channel_lut(r_pts),
                build_channel_lut(g_pts),
                build_channel_lut(b_pts),
            ],
        }
    }

    /// Create from a single set of control points applied to all channels.
    pub fn from_points_uniform(pts: &[(f32, f32)]) -> Self {
        let lut = build_channel_lut(pts);
        Self {
            luts: [lut.clone(), lut.clone(), lut],
        }
    }

    /// Create from piecewise-linear control points (4 per channel).
    ///
    /// Points at x = 0.0, 0.333, 0.667, 1.0 with specified y values.
    /// This is the compact representation used by the mobile parity optimizer.
    pub fn from_4point(r: [f32; 4], g: [f32; 4], b: [f32; 4]) -> Self {
        let to_pts = |p: [f32; 4]| -> [(f32, f32); 4] {
            [
                (0.0, p[0]),
                (1.0 / 3.0, p[1]),
                (2.0 / 3.0, p[2]),
                (1.0, p[3]),
            ]
        };
        Self::from_points(&to_pts(r), &to_pts(g), &to_pts(b))
    }

    /// Create from pre-computed LUT data of arbitrary length per channel.
    ///
    /// Each slice is resampled to 256 entries via linear interpolation.
    pub fn from_lut_data(r: &[f32], g: &[f32], b: &[f32]) -> Self {
        Self {
            luts: [resample_lut(r), resample_lut(g), resample_lut(b)],
        }
    }

    fn is_identity(&self) -> bool {
        self.luts.iter().all(|lut| {
            lut.iter()
                .enumerate()
                .all(|(i, &v)| (v - i as f32 / crate::LUT_MAX as f32).abs() < 1e-4)
        })
    }
}

impl Filter for ChannelCurves {
    fn channel_access(&self) -> ChannelAccess {
        // We need L, a, b to convert to/from sRGB
        ChannelAccess::L_AND_CHROMA
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.is_identity() {
            return;
        }

        let n = planes.l.len();
        // We need to convert Oklab → linear sRGB, apply curves, convert back.
        // Use the linear-srgb crate's conversion (available as dependency).

        for i in 0..n {
            let l = planes.l[i];
            let a = planes.a[i];
            let b = planes.b[i];

            // Oklab → linear sRGB
            let (lr, lg, lb) = oklab_to_linear_srgb(l, a, b);

            // Linear sRGB → sRGB gamma (0-1)
            let sr = linear_to_srgb(lr);
            let sg = linear_to_srgb(lg);
            let sb = linear_to_srgb(lb);

            // Apply per-channel LUT
            let mr = eval_lut(&self.luts[0], sr);
            let mg = eval_lut(&self.luts[1], sg);
            let mb = eval_lut(&self.luts[2], sb);

            // sRGB → linear sRGB
            let lr2 = srgb_to_linear(mr);
            let lg2 = srgb_to_linear(mg);
            let lb2 = srgb_to_linear(mb);

            // linear sRGB → Oklab
            let (l2, a2, b2) = linear_srgb_to_oklab(lr2, lg2, lb2);
            planes.l[i] = l2;
            planes.a[i] = a2;
            planes.b[i] = b2;
        }
    }
}

// ── LUT evaluation ──────────────────────────────────────────────────

fn eval_lut(lut: &[f32], x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let idx_f = x * crate::LUT_MAX as f32;
    let idx = idx_f as usize;
    let frac = idx_f - idx as f32;
    let lo = lut[idx.min(crate::LUT_MAX)];
    let hi = lut[(idx + 1).min(crate::LUT_MAX)];
    lo + frac * (hi - lo)
}

fn resample_lut(data: &[f32]) -> Vec<f32> {
    let mut lut = vec![0.0f32; crate::LUT_SIZE];
    if data.len() < 2 {
        for (i, v) in lut.iter_mut().enumerate() {
            *v = i as f32 / crate::LUT_MAX as f32;
        }
        return lut;
    }
    let src_max = (data.len() - 1) as f32;
    for (i, v) in lut.iter_mut().enumerate() {
        let t = i as f32 / crate::LUT_MAX as f32;
        let src_idx = t * src_max;
        let lo = src_idx as usize;
        let hi = (lo + 1).min(data.len() - 1);
        let frac = src_idx - lo as f32;
        *v = (data[lo] * (1.0 - frac) + data[hi] * frac).clamp(0.0, 1.0);
    }
    lut
}

// ── Monotone cubic Hermite LUT builder ──────────────────────────────

fn build_channel_lut(points: &[(f32, f32)]) -> Vec<f32> {
    let mut pts: Vec<(f32, f32)> = Vec::new();
    if points.is_empty() || points.len() < 2 {
        let mut lut = vec![0.0f32; crate::LUT_SIZE];
        for (i, v) in lut.iter_mut().enumerate() {
            *v = i as f32 / crate::LUT_MAX as f32;
        }
        return lut;
    }

    if points[0].0 > 0.001 {
        pts.push((0.0, 0.0));
    }
    pts.extend_from_slice(points);
    if pts.last().unwrap().0 < 0.999 {
        pts.push((1.0, 1.0));
    }

    let n = pts.len();
    let mut lut = vec![0.0f32; crate::LUT_SIZE];

    // Secants
    let mut delta = vec![0.0f32; n - 1];
    for i in 0..n - 1 {
        let dx = pts[i + 1].0 - pts[i].0;
        delta[i] = if dx.abs() > 1e-10 {
            (pts[i + 1].1 - pts[i].1) / dx
        } else {
            0.0
        };
    }

    // Tangents (Fritsch-Carlson)
    let mut m = vec![0.0f32; n];
    m[0] = delta[0];
    m[n - 1] = delta[n - 2];
    for i in 1..n - 1 {
        m[i] = if delta[i - 1] * delta[i] <= 0.0 {
            0.0
        } else {
            (delta[i - 1] + delta[i]) * 0.5
        };
    }

    // Enforce monotonicity
    for i in 0..n - 1 {
        if delta[i].abs() < 1e-10 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
        } else {
            let alpha = m[i] / delta[i];
            let beta = m[i + 1] / delta[i];
            let s = alpha * alpha + beta * beta;
            if s > 9.0 {
                let tau = 3.0 / s.sqrt();
                m[i] = tau * alpha * delta[i];
                m[i + 1] = tau * beta * delta[i];
            }
        }
    }

    // Evaluate
    for (idx, v) in lut.iter_mut().enumerate() {
        let x = idx as f32 / crate::LUT_MAX as f32;
        let seg = pts[..n - 1]
            .iter()
            .rposition(|p| x >= p.0)
            .unwrap_or_default();

        let x0 = pts[seg].0;
        let x1 = pts[seg + 1].0;
        let y0 = pts[seg].1;
        let y1 = pts[seg + 1].1;
        let dx = x1 - x0;

        if dx.abs() < 1e-10 {
            *v = y0;
            continue;
        }

        let t = ((x - x0) / dx).clamp(0.0, 1.0);
        let t2 = t * t;
        let t3 = t2 * t;
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;
        *v = (h00 * y0 + h10 * dx * m[seg] + h01 * y1 + h11 * dx * m[seg + 1]).clamp(0.0, 1.0);
    }

    lut
}

// ── Color space conversion (Oklab ↔ linear sRGB) ───────────────────
// Inlined to avoid dependency on the full color pipeline.

fn oklab_to_linear_srgb(l: f32, a: f32, b: f32) -> (f32, f32, f32) {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;

    let l3 = l_ * l_ * l_;
    let m3 = m_ * m_ * m_;
    let s3 = s_ * s_ * s_;

    let r = 4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3;
    let g = -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_38 * s3;
    let b_out = -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3;

    (r, g, b_out)
}

fn linear_srgb_to_oklab(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let l_ = 0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m_ = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s_ = 0.088_302_46 * r + 0.281_718_84 * g + 0.629_978_7 * b;

    let l_cbrt = l_.max(0.0).cbrt();
    let m_cbrt = m_.max(0.0).cbrt();
    let s_cbrt = s_.max(0.0).cbrt();

    let l = 0.210_454_26 * l_cbrt + 0.793_617_8 * m_cbrt - 0.004_072_047 * s_cbrt;
    let a = 1.977_998_5 * l_cbrt - 2.428_592_2 * m_cbrt + 0.450_593_7 * s_cbrt;
    let b_out = 0.025_904_037 * l_cbrt + 0.782_771_77 * m_cbrt - 0.808_675_77 * s_cbrt;

    (l, a, b_out)
}

fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn srgb_to_linear(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_preserves_all_channels() {
        let curves = ChannelCurves::default();
        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = (i as f32 / 16.0).min(0.99);
        }
        for v in &mut planes.a {
            *v = 0.1;
        }
        for v in &mut planes.b {
            *v = -0.05;
        }
        let l_orig = planes.l.clone();
        let a_orig = planes.a.clone();
        let b_orig = planes.b.clone();
        curves.apply(&mut planes, &mut FilterContext::new());
        // Should be very close to original (small float error from roundtrip)
        for i in 0..16 {
            assert!(
                (planes.l[i] - l_orig[i]).abs() < 0.01,
                "L[{i}]: {:.4} vs {:.4}",
                planes.l[i],
                l_orig[i]
            );
            assert!(
                (planes.a[i] - a_orig[i]).abs() < 0.01,
                "a[{i}]: {:.4} vs {:.4}",
                planes.a[i],
                a_orig[i]
            );
            assert!(
                (planes.b[i] - b_orig[i]).abs() < 0.01,
                "b[{i}]: {:.4} vs {:.4}",
                planes.b[i],
                b_orig[i]
            );
        }
    }

    #[test]
    fn red_boost_shifts_hue() {
        // Boost red channel only: R curve = gamma 0.5, G/B = identity
        let r_lut: Vec<f32> = (0..crate::LUT_SIZE)
            .map(|i| (i as f32 / crate::LUT_MAX as f32).powf(0.5))
            .collect();
        let id: Vec<f32> = (0..crate::LUT_SIZE)
            .map(|i| i as f32 / crate::LUT_MAX as f32)
            .collect();
        let curves = ChannelCurves::from_luts(r_lut, id.clone(), id);

        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.0;
        planes.b[0] = 0.0;
        let l_before = planes.l[0];

        curves.apply(&mut planes, &mut FilterContext::new());

        // Should become warmer (positive a shift toward red)
        assert!(
            planes.a[0] > 0.01,
            "boosting red should shift a positive, got {}",
            planes.a[0]
        );
        // Luminance should increase (red boost adds energy)
        assert!(
            planes.l[0] > l_before,
            "red boost should brighten, got {} vs {}",
            planes.l[0],
            l_before
        );
    }

    #[test]
    fn from_4point_identity() {
        let curves = ChannelCurves::from_4point(
            [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0],
            [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0],
            [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0],
        );
        assert!(curves.is_identity(), "4-point identity should be identity");
    }

    #[test]
    fn from_4point_contrast_boost() {
        // S-curve via 4 points: darken shadows, brighten highlights
        let curves = ChannelCurves::from_4point(
            [0.0, 0.2, 0.8, 1.0], // less mid, more extremes
            [0.0, 0.2, 0.8, 1.0],
            [0.0, 0.2, 0.8, 1.0],
        );

        let mut planes = OklabPlanes::new(2, 1);
        planes.l[0] = 0.3; // shadow
        planes.l[1] = 0.7; // highlight

        curves.apply(&mut planes, &mut FilterContext::new());

        assert!(
            planes.l[0] < 0.3,
            "S-curve should darken shadows: {}",
            planes.l[0]
        );
        assert!(
            planes.l[1] > 0.7,
            "S-curve should brighten highlights: {}",
            planes.l[1]
        );
    }

    #[test]
    fn color_space_roundtrip() {
        // Verify Oklab ↔ linear sRGB roundtrip accuracy
        let test_vals = [(0.5, 0.0, 0.0), (0.7, 0.1, -0.05), (0.3, -0.05, 0.1)];
        for (l, a, b) in test_vals {
            let (r, g, b_lin) = oklab_to_linear_srgb(l, a, b);
            let (l2, a2, b2) = linear_srgb_to_oklab(r, g, b_lin);
            assert!(
                (l - l2).abs() < 0.001 && (a - a2).abs() < 0.001 && (b - b2).abs() < 0.001,
                "roundtrip failed: ({l},{a},{b}) → ({l2},{a2},{b2})"
            );
        }
    }

    #[test]
    fn srgb_gamma_roundtrip() {
        for i in 0..=255 {
            let v = i as f32 / crate::LUT_MAX as f32;
            let rt = srgb_to_linear(linear_to_srgb(v));
            assert!(
                (v - rt).abs() < 0.001,
                "sRGB roundtrip failed at {v}: got {rt}"
            );
        }
    }
}
