use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::fast_math::{fast_atan2, fast_sincos};
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::prelude::*;

/// Resolution of hue-indexed LUTs (360 entries = 1° per bin).
const HUE_LUT_SIZE: usize = 360;
/// Resolution of luminance-indexed LUTs.
const LUM_LUT_SIZE: usize = 256;

/// DaVinci Resolve-style hue-qualified curves.
///
/// Four independent 1D curves that give per-hue and per-luminance control
/// over color properties. This is the tool that separates professional
/// color grading from basic adjustments.
///
/// All curves operate in Oklab polar coordinates where hue is perceptually
/// uniform — a huge advantage over Resolve's internal HSL space. No hue
/// skew artifacts, no unexpected interactions.
///
/// ## Curves
///
/// - **Hue vs Saturation**: For each hue angle, a chroma multiplier.
///   Example: desaturate only the greens, boost only reds.
///
/// - **Hue vs Hue**: For each hue angle, a hue offset in degrees.
///   Example: shift cyan toward blue, push orange toward red.
///
/// - **Hue vs Luminance**: For each hue angle, a luminance offset.
///   Example: darken blues, brighten yellows.
///
/// - **Luminance vs Saturation**: For each luminance level, a chroma multiplier.
///   Example: desaturate shadows (cinematic look), boost midtone color.
///
/// Each curve is defined by control points and built into a LUT via
/// monotone cubic Hermite interpolation (same as ToneCurve).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct HueCurves {
    /// Hue vs Saturation: 360 entries indexed by hue degree, each a chroma multiplier.
    /// 1.0 = no change. Wrapped: entry 0 = 0°, entry 359 = 359°.
    hue_sat: Vec<f32>,
    /// Hue vs Hue: 360 entries, each a hue offset in degrees.
    /// 0.0 = no change.
    hue_hue: Vec<f32>,
    /// Hue vs Luminance: 360 entries, each a luminance offset.
    /// 0.0 = no change.
    hue_lum: Vec<f32>,
    /// Luminance vs Saturation: 256 entries indexed by L [0,1], each a chroma multiplier.
    /// 1.0 = no change.
    lum_sat: Vec<f32>,
}

impl Default for HueCurves {
    fn default() -> Self {
        Self {
            hue_sat: vec![1.0; HUE_LUT_SIZE],
            hue_hue: vec![0.0; HUE_LUT_SIZE],
            hue_lum: vec![0.0; HUE_LUT_SIZE],
            lum_sat: vec![1.0; LUM_LUT_SIZE],
        }
    }
}

static HUE_CURVES_SCHEMA: FilterSchema = FilterSchema {
    name: "hue_curves",
    label: "Hue Curves",
    description: "Per-hue and per-luminance curves for targeted color grading",
    group: FilterGroup::Color,
    params: &[],
};

impl Describe for HueCurves {
    fn schema() -> &'static FilterSchema {
        &HUE_CURVES_SCHEMA
    }

    fn get_param(&self, _name: &str) -> Option<ParamValue> {
        // Curves are set programmatically, not via scalar params
        None
    }

    fn set_param(&mut self, _name: &str, _value: ParamValue) -> bool {
        false
    }
}

impl HueCurves {
    /// Build the Hue vs Saturation curve from control points.
    ///
    /// Points are `(hue_degrees, multiplier)` pairs. Hue wraps at 360°.
    /// The multiplier is a chroma scale: 1.0 = unchanged, 0.0 = desaturate,
    /// 2.0 = double saturation.
    ///
    /// Points should cover [0, 360). Intermediate values are interpolated
    /// via monotone cubic spline. The curve wraps smoothly around 360°→0°.
    pub fn set_hue_sat(&mut self, points: &[(f32, f32)]) {
        self.hue_sat = build_wrapped_hue_lut(points, 1.0);
    }

    /// Build the Hue vs Hue curve from control points.
    ///
    /// Points are `(hue_degrees, offset_degrees)`. Positive offset shifts
    /// hue clockwise (in Oklab); negative shifts counterclockwise.
    pub fn set_hue_hue(&mut self, points: &[(f32, f32)]) {
        self.hue_hue = build_wrapped_hue_lut(points, 0.0);
    }

    /// Build the Hue vs Luminance curve from control points.
    ///
    /// Points are `(hue_degrees, L_offset)`. Positive = brighten that hue.
    pub fn set_hue_lum(&mut self, points: &[(f32, f32)]) {
        self.hue_lum = build_wrapped_hue_lut(points, 0.0);
    }

    /// Build the Luminance vs Saturation curve from control points.
    ///
    /// Points are `(L_value, multiplier)` where L is in [0, 1].
    /// 1.0 = no change.
    pub fn set_lum_sat(&mut self, points: &[(f32, f32)]) {
        self.lum_sat = build_linear_lut(points, 1.0, LUM_LUT_SIZE);
    }

    /// Set Hue vs Saturation from a raw 360-entry LUT.
    pub fn set_hue_sat_lut(&mut self, lut: Vec<f32>) {
        debug_assert_eq!(lut.len(), HUE_LUT_SIZE);
        self.hue_sat = lut;
    }

    /// Set Hue vs Hue from a raw 360-entry LUT.
    pub fn set_hue_hue_lut(&mut self, lut: Vec<f32>) {
        debug_assert_eq!(lut.len(), HUE_LUT_SIZE);
        self.hue_hue = lut;
    }

    /// Set Hue vs Luminance from a raw 360-entry LUT.
    pub fn set_hue_lum_lut(&mut self, lut: Vec<f32>) {
        debug_assert_eq!(lut.len(), HUE_LUT_SIZE);
        self.hue_lum = lut;
    }

    /// Set Luminance vs Saturation from a raw 256-entry LUT.
    pub fn set_lum_sat_lut(&mut self, lut: Vec<f32>) {
        debug_assert_eq!(lut.len(), LUM_LUT_SIZE);
        self.lum_sat = lut;
    }

    fn is_identity(&self) -> bool {
        self.hue_sat.iter().all(|&v| (v - 1.0).abs() < 1e-6)
            && self.hue_hue.iter().all(|&v| v.abs() < 1e-6)
            && self.hue_lum.iter().all(|&v| v.abs() < 1e-6)
            && self.lum_sat.iter().all(|&v| (v - 1.0).abs() < 1e-6)
    }
}

/// Evaluate a hue-indexed LUT with linear interpolation and wrapping.
#[inline]
fn eval_hue_lut(lut: &[f32], hue_deg: f32) -> f32 {
    let h = hue_deg.rem_euclid(360.0);
    let idx_f = h * (HUE_LUT_SIZE as f32 / 360.0);
    let idx = idx_f as usize;
    let frac = idx_f - idx as f32;
    let lo = lut[idx % HUE_LUT_SIZE];
    let hi = lut[(idx + 1) % HUE_LUT_SIZE];
    lo + frac * (hi - lo)
}

/// Evaluate a luminance-indexed LUT with linear interpolation.
#[inline]
fn eval_lum_lut(lut: &[f32], l: f32) -> f32 {
    let l = l.clamp(0.0, 1.0);
    let max = LUM_LUT_SIZE - 1;
    let idx_f = l * max as f32;
    let idx = idx_f as usize;
    let frac = idx_f - idx as f32;
    let lo = lut[idx.min(max)];
    let hi = lut[(idx + 1).min(max)];
    lo + frac * (hi - lo)
}

impl Filter for HueCurves {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.is_identity() {
            return;
        }

        let n = planes.pixel_count();

        // Check which curves are active to skip unnecessary work
        let has_hue_sat = self.hue_sat.iter().any(|&v| (v - 1.0).abs() > 1e-6);
        let has_hue_hue = self.hue_hue.iter().any(|&v| v.abs() > 1e-6);
        let has_hue_lum = self.hue_lum.iter().any(|&v| v.abs() > 1e-6);
        let has_lum_sat = self.lum_sat.iter().any(|&v| (v - 1.0).abs() > 1e-6);
        let needs_hue = has_hue_sat || has_hue_hue || has_hue_lum;

        for i in 0..n {
            let a = planes.a[i];
            let b = planes.b[i];
            let l = planes.l[i];
            let chroma = (a * a + b * b).sqrt();

            // Lum vs Sat applies to all pixels (even neutral)
            let lum_sat_factor = if has_lum_sat {
                eval_lum_lut(&self.lum_sat, l)
            } else {
                1.0
            };

            // Hue-based curves need chroma > 0
            if needs_hue && chroma > 1e-5 {
                let hue_rad = fast_atan2(b, a);
                let mut hue_deg = hue_rad.to_degrees();
                if hue_deg < 0.0 {
                    hue_deg += 360.0;
                }

                // Hue vs Saturation
                let hue_sat_factor = if has_hue_sat {
                    eval_hue_lut(&self.hue_sat, hue_deg)
                } else {
                    1.0
                };

                // Hue vs Hue
                let hue_offset = if has_hue_hue {
                    eval_hue_lut(&self.hue_hue, hue_deg)
                } else {
                    0.0
                };

                // Hue vs Luminance
                let lum_offset = if has_hue_lum {
                    eval_hue_lut(&self.hue_lum, hue_deg)
                } else {
                    0.0
                };

                // Apply combined chroma scaling
                let new_chroma = (chroma * hue_sat_factor * lum_sat_factor).max(0.0);

                // Apply hue shift
                let new_hue_rad = hue_rad + hue_offset.to_radians();

                let (sin_h, cos_h) = fast_sincos(new_hue_rad);
                planes.a[i] = new_chroma * cos_h;
                planes.b[i] = new_chroma * sin_h;

                // Apply luminance offset
                if lum_offset.abs() > 1e-6 {
                    planes.l[i] = (l + lum_offset).max(0.0);
                }
            } else if has_lum_sat && chroma > 1e-5 {
                // Only lum_sat active, no hue curves — just scale chroma
                let new_chroma = chroma * lum_sat_factor;
                let scale = new_chroma / chroma;
                planes.a[i] = a * scale;
                planes.b[i] = b * scale;
            }
        }
    }
}

// ── LUT builders ──────────────────────────────────────────────────────

/// Build a 360-entry hue LUT from control points with wrapping.
///
/// Points are `(hue_degrees, value)`. The LUT wraps: 360° = 0°.
/// Monotone cubic interpolation ensures smooth transitions.
fn build_wrapped_hue_lut(points: &[(f32, f32)], identity: f32) -> Vec<f32> {
    if points.len() < 2 {
        return vec![identity; HUE_LUT_SIZE];
    }

    // Normalize hue values to [0, 360) and sort
    let mut pts: Vec<(f32, f32)> = points
        .iter()
        .map(|&(h, v)| (h.rem_euclid(360.0), v))
        .collect();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < 0.01);

    if pts.len() < 2 {
        return vec![identity; HUE_LUT_SIZE];
    }

    // For wrapping: extend with ghost points from the other end
    let n = pts.len();
    let mut extended = Vec::with_capacity(n + 2);
    // Ghost point before 0: last point shifted by -360
    extended.push((pts[n - 1].0 - 360.0, pts[n - 1].1));
    extended.extend_from_slice(&pts);
    // Ghost point after 360: first point shifted by +360
    extended.push((pts[0].0 + 360.0, pts[0].1));

    // Build LUT via piecewise cubic Hermite on the extended points
    let en = extended.len();

    // Compute secants
    let mut delta = vec![0.0f32; en - 1];
    for i in 0..en - 1 {
        let dx = extended[i + 1].0 - extended[i].0;
        delta[i] = if dx.abs() > 1e-10 {
            (extended[i + 1].1 - extended[i].1) / dx
        } else {
            0.0
        };
    }

    // Compute tangents (Fritsch-Carlson)
    let mut m = vec![0.0f32; en];
    m[0] = delta[0];
    m[en - 1] = delta[en - 2];
    for i in 1..en - 1 {
        if delta[i - 1] * delta[i] <= 0.0 {
            m[i] = 0.0;
        } else {
            m[i] = (delta[i - 1] + delta[i]) * 0.5;
        }
    }

    // Enforce monotonicity
    for i in 0..en - 1 {
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

    // Evaluate at each degree
    let mut lut = vec![identity; HUE_LUT_SIZE];
    for (deg, v) in lut.iter_mut().enumerate() {
        let x = deg as f32;

        // Find interval in extended array
        let seg = extended[..en - 1]
            .iter()
            .rposition(|p| x >= p.0)
            .unwrap_or(0);

        let x0 = extended[seg].0;
        let x1 = extended[seg + 1].0;
        let y0 = extended[seg].1;
        let y1 = extended[seg + 1].1;
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
        *v = h00 * y0 + h10 * dx * m[seg] + h01 * y1 + h11 * dx * m[seg + 1];
    }

    lut
}

/// Build a linear-indexed LUT from control points (for luminance-based curves).
fn build_linear_lut(points: &[(f32, f32)], identity: f32, size: usize) -> Vec<f32> {
    if points.len() < 2 {
        return vec![identity; size];
    }

    let max = (size - 1) as f32;

    let mut pts: Vec<(f32, f32)> = Vec::new();
    if points[0].0 > 0.001 {
        pts.push((0.0, identity));
    }
    pts.extend_from_slice(points);
    if pts.last().unwrap().0 < 0.999 {
        pts.push((1.0, identity));
    }

    let n = pts.len();
    if n < 2 {
        return vec![identity; size];
    }

    // Compute secants
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
        if delta[i - 1] * delta[i] <= 0.0 {
            m[i] = 0.0;
        } else {
            m[i] = (delta[i - 1] + delta[i]) * 0.5;
        }
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
    let mut lut = vec![identity; size];
    for (idx, v) in lut.iter_mut().enumerate() {
        let x = idx as f32 / max;

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
        *v = h00 * y0 + h10 * dx * m[seg] + h01 * y1 + h11 * dx * m[seg + 1];
    }

    lut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_identity() {
        let curves = HueCurves::default();
        assert!(curves.is_identity());

        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = 0.3 + (i as f32) * 0.01;
        }
        for v in &mut planes.a {
            *v = 0.1;
        }
        for v in &mut planes.b {
            *v = 0.05;
        }
        let l_orig = planes.l.clone();
        let a_orig = planes.a.clone();
        let b_orig = planes.b.clone();
        curves.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, l_orig);
        assert_eq!(planes.a, a_orig);
        assert_eq!(planes.b, b_orig);
    }

    #[test]
    fn hue_vs_sat_desaturates_reds() {
        let mut curves = HueCurves::default();
        // Red in Oklab is around hue 30° (positive a, slightly positive b)
        // Desaturate the red region: set multiplier to 0.0 around 30°
        curves.set_hue_sat(&[
            (0.0, 0.0),
            (60.0, 0.0),
            (90.0, 1.0),
            (270.0, 1.0),
            (330.0, 0.0),
        ]);

        let mut planes = OklabPlanes::new(2, 1);
        // Red pixel: strong positive a
        planes.l[0] = 0.5;
        planes.a[0] = 0.15;
        planes.b[0] = 0.05;
        // Blue pixel: negative b
        planes.l[1] = 0.5;
        planes.a[1] = -0.02;
        planes.b[1] = -0.15;

        let c_red_before = (planes.a[0] * planes.a[0] + planes.b[0] * planes.b[0]).sqrt();
        let c_blue_before = (planes.a[1] * planes.a[1] + planes.b[1] * planes.b[1]).sqrt();

        curves.apply(&mut planes, &mut FilterContext::new());

        let c_red_after = (planes.a[0] * planes.a[0] + planes.b[0] * planes.b[0]).sqrt();
        let c_blue_after = (planes.a[1] * planes.a[1] + planes.b[1] * planes.b[1]).sqrt();

        assert!(
            c_red_after < c_red_before * 0.5,
            "red should be desaturated: {c_red_before} → {c_red_after}"
        );
        // Blue should be mostly unchanged (hue ~255° is in the 1.0 zone)
        assert!(
            (c_blue_after - c_blue_before).abs() < c_blue_before * 0.3,
            "blue should be mostly unchanged: {c_blue_before} → {c_blue_after}"
        );
    }

    #[test]
    fn hue_vs_hue_shifts_color() {
        let mut curves = HueCurves::default();
        // Shift hue around 30° by +60°
        curves.set_hue_hue(&[(0.0, 60.0), (60.0, 60.0), (90.0, 0.0), (270.0, 0.0)]);

        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.15;
        planes.b[0] = 0.05;
        let hue_before = planes.b[0].atan2(planes.a[0]).to_degrees();

        curves.apply(&mut planes, &mut FilterContext::new());

        let hue_after = planes.b[0].atan2(planes.a[0]).to_degrees();
        let diff = (hue_after - hue_before).abs();
        assert!(
            diff > 30.0,
            "hue should shift significantly: {hue_before:.1}° → {hue_after:.1}° (diff={diff:.1}°)"
        );
    }

    #[test]
    fn hue_vs_lum_brightens_yellows() {
        let mut curves = HueCurves::default();
        // Yellow in Oklab is around hue ~90-110°
        curves.set_hue_lum(&[(80.0, 0.1), (120.0, 0.1), (150.0, 0.0), (300.0, 0.0)]);

        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        // Yellowish pixel
        planes.a[0] = 0.02;
        planes.b[0] = 0.15;

        curves.apply(&mut planes, &mut FilterContext::new());

        assert!(planes.l[0] > 0.5, "yellow should brighten: {}", planes.l[0]);
    }

    #[test]
    fn lum_vs_sat_desaturates_shadows() {
        let mut curves = HueCurves::default();
        // Desaturate shadows (L < 0.3), full sat for midtones and highlights
        curves.set_lum_sat(&[(0.0, 0.0), (0.2, 0.2), (0.4, 1.0), (1.0, 1.0)]);

        let mut planes = OklabPlanes::new(2, 1);
        // Shadow pixel
        planes.l[0] = 0.1;
        planes.a[0] = 0.1;
        planes.b[0] = 0.05;
        // Midtone pixel
        planes.l[1] = 0.6;
        planes.a[1] = 0.1;
        planes.b[1] = 0.05;

        let c_shadow_before = (planes.a[0] * planes.a[0] + planes.b[0] * planes.b[0]).sqrt();
        let c_mid_before = (planes.a[1] * planes.a[1] + planes.b[1] * planes.b[1]).sqrt();

        curves.apply(&mut planes, &mut FilterContext::new());

        let c_shadow_after = (planes.a[0] * planes.a[0] + planes.b[0] * planes.b[0]).sqrt();
        let c_mid_after = (planes.a[1] * planes.a[1] + planes.b[1] * planes.b[1]).sqrt();

        assert!(
            c_shadow_after < c_shadow_before * 0.5,
            "shadows should be desaturated: {c_shadow_before} → {c_shadow_after}"
        );
        assert!(
            (c_mid_after - c_mid_before).abs() < c_mid_before * 0.1,
            "midtones should be unchanged: {c_mid_before} → {c_mid_after}"
        );
    }

    #[test]
    fn neutral_pixels_unaffected_by_hue_curves() {
        let mut curves = HueCurves::default();
        curves.set_hue_sat(&[(0.0, 2.0), (180.0, 0.0)]);
        curves.set_hue_hue(&[(0.0, 90.0), (180.0, -90.0)]);

        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.0;
        planes.b[0] = 0.0;

        curves.apply(&mut planes, &mut FilterContext::new());

        assert!(
            planes.a[0].abs() < 1e-5,
            "neutral should stay neutral: a={}",
            planes.a[0]
        );
        assert!(
            planes.b[0].abs() < 1e-5,
            "neutral should stay neutral: b={}",
            planes.b[0]
        );
    }

    #[test]
    fn hue_lut_wraps_smoothly() {
        // Points at 350° and 10° should interpolate through 0°
        let lut = build_wrapped_hue_lut(&[(350.0, 2.0), (10.0, 2.0), (180.0, 1.0)], 1.0);

        // At 0° (between 350° and 10°) should be close to 2.0
        assert!(
            lut[0] > 1.5,
            "hue 0° should be close to 2.0 (between 350° and 10°): {}",
            lut[0]
        );
        // At 180° should be close to 1.0
        assert!(
            (lut[180] - 1.0).abs() < 0.3,
            "hue 180° should be close to 1.0: {}",
            lut[180]
        );
    }

    #[test]
    fn combined_curves() {
        let mut curves = HueCurves::default();
        // Boost red saturation + darken shadows
        curves.set_hue_sat(&[(0.0, 1.5), (60.0, 1.5), (90.0, 1.0), (270.0, 1.0)]);
        curves.set_lum_sat(&[(0.0, 0.5), (0.3, 1.0), (1.0, 1.0)]);

        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.15;
        planes.b[0] = 0.05;
        let c_before = (planes.a[0] * planes.a[0] + planes.b[0] * planes.b[0]).sqrt();

        curves.apply(&mut planes, &mut FilterContext::new());

        let c_after = (planes.a[0] * planes.a[0] + planes.b[0] * planes.b[0]).sqrt();
        // Hue sat boosts by 1.5, lum sat at L=0.5 should be ~1.0
        assert!(
            c_after > c_before * 1.2,
            "combined should boost red chroma: {c_before} → {c_after}"
        );
    }
}
