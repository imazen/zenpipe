use core::f32::consts::{FRAC_1_SQRT_2, PI, SQRT_2};

use jxl_encoder::{Spline, SplinePoint};
use serde::{Deserialize, Serialize};

use crate::limits;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

const OPSIN_MATRIX: [[f32; 3]; 3] = [
    [0.30, 0.622, 0.078],
    [0.23, 0.692, 0.078],
    [0.243_422_69, 0.204_767_45, 0.551_809_87],
];

#[allow(clippy::excessive_precision)]
const INV_OPSIN: [[f32; 3]; 3] = [
    [11.031_566_9, -9.866_943_9, -0.164_622_996_470_588_26],
    [-3.254_147_4, 4.418_770_4, -0.164_622_996_470_588_26],
    [-3.658_851_3, 2.712_923, 1.945_928_2],
];

const OPSIN_BIAS: [f32; 3] = [0.003_793_073_4; 3];
const NEG_CBRT_BIAS: [f32; 3] = [-0.155_954_2; 3];

const CHANNEL_WEIGHT: [f32; 4] = [0.0042, 0.075, 0.07, 0.3333];
const DESIRED_RENDERING_DISTANCE: f32 = 1.0;
const ONE_OVER_2S2: f32 = 0.353_553_38;
const DISTANCE_EXP: f32 = 3.0;
const NUM_POINTS_PER_SEGMENT: usize = 16;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplineSpec {
    pub control_points: Vec<[f32; 2]>,
    pub color_dct: [[f32; 32]; 3],
    pub sigma_dct: [f32; 32],
}

impl SplineSpec {
    pub fn to_encoder_spline(&self) -> Result<Spline, String> {
        if self.control_points.is_empty() {
            return Err("A spline requires at least one control point.".into());
        }
        if self.control_points.len() > 256 {
            return Err("A spline supports at most 256 control points.".into());
        }
        let mut points = Vec::with_capacity(self.control_points.len());
        let mut prev_rounded: Option<(i64, i64)> = None;
        for &[x, y] in &self.control_points {
            if !x.is_finite() || !y.is_finite() || x.abs() > 32768.0 || y.abs() > 32768.0 {
                return Err("Spline control points must be finite coordinates.".into());
            }
            let rounded = (x.round() as i64, y.round() as i64);
            if prev_rounded == Some(rounded) {
                return Err(
                    "Consecutive spline control points must differ after integer rounding.".into(),
                );
            }
            prev_rounded = Some(rounded);
            points.push(SplinePoint::new(x, y));
        }
        if !self
            .color_dct
            .iter()
            .flatten()
            .chain(&self.sigma_dct)
            .all(|v| v.is_finite())
        {
            return Err("Spline DCT coefficients must be finite.".into());
        }
        Ok(Spline {
            control_points: points,
            color_dct: self.color_dct,
            sigma_dct: self.sigma_dct,
        })
    }
}

fn srgb_u8_to_linear(v: u8) -> f32 {
    let x = f32::from(v) / 255.0;
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb_u8(v: f32) -> u8 {
    let c = v.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s.clamp(0.0, 1.0) * 255.0).round() as u8
}

pub(crate) fn srgb_to_xyb(r: u8, g: u8, b: u8) -> [f32; 3] {
    let rl = srgb_u8_to_linear(r);
    let gl = srgb_u8_to_linear(g);
    let bl = srgb_u8_to_linear(b);
    let m0 = (OPSIN_MATRIX[0][0] * rl + OPSIN_MATRIX[0][1] * gl + OPSIN_MATRIX[0][2] * bl).max(0.0);
    let m1 = (OPSIN_MATRIX[1][0] * rl + OPSIN_MATRIX[1][1] * gl + OPSIN_MATRIX[1][2] * bl).max(0.0);
    let m2 = (OPSIN_MATRIX[2][0] * rl + OPSIN_MATRIX[2][1] * gl + OPSIN_MATRIX[2][2] * bl).max(0.0);
    let l = (m0 + OPSIN_BIAS[0]).cbrt() + NEG_CBRT_BIAS[0];
    let m = (m1 + OPSIN_BIAS[1]).cbrt() + NEG_CBRT_BIAS[1];
    let s = (m2 + OPSIN_BIAS[2]).cbrt() + NEG_CBRT_BIAS[2];
    [0.5 * (l - m), 0.5 * (l + m), s]
}

pub(crate) fn xyb_to_srgb(x: f32, y: f32, b: f32) -> [u8; 3] {
    let l_b = (y + x) - NEG_CBRT_BIAS[0];
    let m_b = (y - x) - NEG_CBRT_BIAS[1];
    let s_b = b - NEG_CBRT_BIAS[2];
    let m0 = l_b * l_b * l_b - OPSIN_BIAS[0];
    let m1 = m_b * m_b * m_b - OPSIN_BIAS[1];
    let m2 = s_b * s_b * s_b - OPSIN_BIAS[2];
    let rl = INV_OPSIN[0][0] * m0 + INV_OPSIN[0][1] * m1 + INV_OPSIN[0][2] * m2;
    let gl = INV_OPSIN[1][0] * m0 + INV_OPSIN[1][1] * m1 + INV_OPSIN[1][2] * m2;
    let bl = INV_OPSIN[2][0] * m0 + INV_OPSIN[2][1] * m1 + INV_OPSIN[2][2] * m2;
    [
        linear_to_srgb_u8(rl),
        linear_to_srgb_u8(gl),
        linear_to_srgb_u8(bl),
    ]
}

fn fast_erf(x: f32) -> f32 {
    let absx = x.abs();
    let d1 = absx * 7.773_943_7e-2 + 2.052_600_2e-4;
    let d2 = d1 * absx + 2.321_202_2e-1;
    let d3 = d2 * absx + 2.778_208e-1;
    let d4 = d3 * absx + 1.0;
    let d5 = d4 * d4;
    let inv = 1.0 / d5;
    (-inv * inv + 1.0).copysign(x)
}

fn fast_cos(x: f32) -> f32 {
    let pi2 = PI * 2.0;
    let npi2 = (x * (0.5 / PI)).floor() * pi2;
    let xmodpi2 = x - npi2;
    let x_pi = xmodpi2.min(pi2 - xmodpi2);
    let above_pihalf = x_pi >= PI * 0.5;
    let x_pihalf = if above_pihalf { PI - x_pi } else { x_pi };
    let xs = x_pihalf * 0.25;
    let x2 = xs * xs;
    let x4 = x2 * x2;
    let prescaling = x4 * 0.069_604_38 + (x2 * -0.840_873_73 + 1.681_792_7);
    let scale1 = prescaling * prescaling - SQRT_2;
    let scale2 = scale1 * scale1 - 1.0;
    if above_pihalf { -scale2 } else { scale2 }
}

fn continuous_idct(dct: &[f32; 32], cosines: &[f32; 32]) -> f32 {
    dct.iter()
        .zip(cosines.iter())
        .map(|(&c, &cos)| c * cos)
        .sum::<f32>()
        * SQRT_2
}

#[derive(Clone, Copy, Default)]
struct Pt {
    x: f32,
    y: f32,
}

impl Pt {
    fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    fn abs(self) -> f32 {
        self.x.hypot(self.y)
    }
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y)
    }
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y)
    }
    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s)
    }
}

fn draw_centripetal_catmull_rom(points: &[Pt]) -> Vec<Pt> {
    if points.len() <= 1 {
        return points.to_vec();
    }
    let first_extra = points[0].add(points[0].sub(points[1]));
    let last_extra =
        points[points.len() - 1].add(points[points.len() - 1].sub(points[points.len() - 2]));
    let mut extended = Vec::with_capacity(points.len() + 2);
    extended.push(first_extra);
    extended.extend_from_slice(points);
    extended.push(last_extra);

    let mut dists = Vec::with_capacity(extended.len() - 1);
    for i in 0..extended.len() - 1 {
        dists.push(extended[i + 1].sub(extended[i]).abs().sqrt());
    }
    let num_windows = extended.len() - 3;
    let mut result = Vec::with_capacity(num_windows * NUM_POINTS_PER_SEGMENT + 1);
    for w in 0..num_windows {
        let p = [
            extended[w],
            extended[w + 1],
            extended[w + 2],
            extended[w + 3],
        ];
        let d = [dists[w], dists[w + 1], dists[w + 2]];
        let t = [0.0, d[0], d[0] + d[1], d[0] + d[1] + d[2]];
        result.push(p[1]);
        if d[1] < 1e-10 {
            continue;
        }
        for i in 1..NUM_POINTS_PER_SEGMENT {
            let tt = d[0] + (i as f32 / NUM_POINTS_PER_SEGMENT as f32) * d[1];
            let mut a = [Pt::default(); 3];
            for k in 0..3 {
                let denom = d[k].max(1e-10);
                a[k] = p[k].add(p[k + 1].sub(p[k]).mul((tt - t[k]) / denom));
            }
            let mut b = [Pt::default(); 2];
            for k in 0..2 {
                let denom = (d[k] + d[k + 1]).max(1e-10);
                b[k] = a[k].add(a[k + 1].sub(a[k]).mul((tt - t[k]) / denom));
            }
            result.push(b[0].add(b[1].sub(b[0]).mul((tt - t[1]) / d[1])));
        }
    }
    result.push(points[points.len() - 1]);
    result
}

fn resample_equally(points: &[Pt], desired_distance: f32) -> Vec<(Pt, f32)> {
    if points.is_empty() {
        return vec![];
    }
    let mut result = vec![(points[0], desired_distance)];
    if points.len() == 1 {
        return result;
    }
    let mut acc = 0.0f32;
    for index in 0..points.len() - 1 {
        let mut cur = points[index];
        let next = points[index + 1];
        let seg = next.sub(cur);
        let len = seg.abs();
        if len < 1e-10 {
            continue;
        }
        let step = seg.mul(1.0 / len);
        if acc + len >= desired_distance {
            cur = cur.add(step.mul(desired_distance - acc));
            result.push((cur, desired_distance));
            acc -= desired_distance;
        }
        acc += len;
        while acc >= desired_distance {
            cur = cur.add(step.mul(desired_distance));
            result.push((cur, desired_distance));
            acc -= desired_distance;
        }
    }
    result.push((points[points.len() - 1], acc));
    result
}

/// Quantize and dequantize a spline identically to `jxl-encoder`'s `VarDctEncoder`
/// (`quantization_adjustment = 0`, `y_to_x = 0.0`, `y_to_b = 1.0`).
fn quantize_roundtrip(spline: &Spline) -> Result<(Vec<Pt>, [[f32; 32]; 3], [f32; 32]), String> {
    let y_to_x = 0.0f32;
    let y_to_b = 1.0f32;
    let mut pts = Vec::with_capacity(spline.control_points.len());
    for p in &spline.control_points {
        pts.push(Pt::new(p.x.round(), p.y.round()));
    }
    let mut q_color = [[0i32; 32]; 3];
    for i in 0..32 {
        let f = if i == 0 { SQRT_2 } else { 1.0 };
        let v = (spline.color_dct[1][i] * f / CHANNEL_WEIGHT[1]).round();
        if !v.is_finite() || v < i32::MIN as f32 || v > i32::MAX as f32 {
            return Err("Spline Y color coefficient exceeds quantized range.".into());
        }
        q_color[1][i] = v as i32;
    }
    let mut restored_y = [0.0f32; 32];
    for i in 0..32 {
        let inv_f = if i == 0 { FRAC_1_SQRT_2 } else { 1.0 };
        restored_y[i] = q_color[1][i] as f32 * inv_f * CHANNEL_WEIGHT[1];
    }
    for c in [0usize, 2] {
        let cfl = if c == 0 { y_to_x } else { y_to_b };
        for i in 0..32 {
            let f = if i == 0 { SQRT_2 } else { 1.0 };
            let dec = spline.color_dct[c][i] - cfl * restored_y[i];
            let v = (dec * f / CHANNEL_WEIGHT[c]).round();
            if !v.is_finite() || v < i32::MIN as f32 || v > i32::MAX as f32 {
                return Err("Spline chroma coefficient exceeds quantized range.".into());
            }
            q_color[c][i] = v as i32;
        }
    }
    let mut q_sigma = [0i32; 32];
    for i in 0..32 {
        let f = if i == 0 { SQRT_2 } else { 1.0 };
        let v = (spline.sigma_dct[i] * f / CHANNEL_WEIGHT[3]).round();
        if !v.is_finite() || v < i32::MIN as f32 || v > i32::MAX as f32 {
            return Err("Spline sigma coefficient exceeds quantized range.".into());
        }
        q_sigma[i] = v as i32;
    }

    let mut color_dct = [[0.0f32; 32]; 3];
    for c in 0..3 {
        for i in 0..32 {
            let inv_f = if i == 0 { FRAC_1_SQRT_2 } else { 1.0 };
            color_dct[c][i] = q_color[c][i] as f32 * inv_f * CHANNEL_WEIGHT[c];
        }
    }
    for i in 0..32 {
        color_dct[0][i] += y_to_x * color_dct[1][i];
        color_dct[2][i] += y_to_b * color_dct[1][i];
    }
    let mut sigma_dct = [0.0f32; 32];
    for i in 0..32 {
        let inv_f = if i == 0 { FRAC_1_SQRT_2 } else { 1.0 };
        sigma_dct[i] = q_sigma[i] as f32 * inv_f * CHANNEL_WEIGHT[3];
    }
    Ok((pts, color_dct, sigma_dct))
}

/// Add the exact quantized JPEG XL Gaussian spline contribution onto an RGBA8 canvas.
///
/// Because `jxl-encoder`'s `LossyConfig::with_splines` subtracts splines from the
/// source XYB planes before VarDCT encoding (and the decoder adds them back),
/// baking the authored spline onto the canvas ensures the final decoded image
/// visibly contains the spline while VarDCT encodes the residual cleanly.
pub(crate) fn render_splines_onto_rgba(
    base_rgba: &[u8],
    width: u32,
    height: u32,
    splines: &[Spline],
) -> Result<Vec<u8>, String> {
    limits()
        .check_dimensions(width, height)
        .map_err(|e| e.to_string())?;
    let w = width as usize;
    let h = height as usize;
    let count = w
        .checked_mul(h)
        .ok_or_else(|| "Spline canvas size overflow.".to_string())?;
    let expected_bytes = count
        .checked_mul(4)
        .ok_or_else(|| "Spline canvas byte overflow.".to_string())?;
    if base_rgba.len() != expected_bytes {
        return Err("Spline canvas RGBA byte count does not match dimensions.".into());
    }
    if splines.is_empty() {
        return Ok(base_rgba.to_vec());
    }

    let mut xyb = [
        vec![0.0f32; count],
        vec![0.0f32; count],
        vec![0.0f32; count],
    ];
    for (i, px) in base_rgba.as_chunks::<4>().0.iter().enumerate() {
        let [x, y, b] = srgb_to_xyb(px[0], px[1], px[2]);
        xyb[0][i] = x;
        xyb[1][i] = y;
        xyb[2][i] = b;
    }

    for spline in splines {
        let (pts, color_dct, sigma_dct) = quantize_roundtrip(spline)?;
        let curve = draw_centripetal_catmull_rom(&pts);
        let samples = resample_equally(&curve, DESIRED_RENDERING_DISTANCE);
        if samples.len() < 2 {
            continue;
        }
        let total_len = (samples.len() as isize - 2) as f32 * DESIRED_RENDERING_DISTANCE
            + samples.last().unwrap().1;
        if total_len <= 0.0 {
            continue;
        }
        let inv_len = 1.0 / total_len;
        for (idx, (pt, intensity)) in samples.iter().enumerate() {
            let progress = (idx as f32 * DESIRED_RENDERING_DISTANCE * inv_len).min(1.0);
            let t = 31.0 * progress;
            let tandhalf = t + 0.5;
            let cosines: [f32; 32] =
                core::array::from_fn(|i| fast_cos(PI / 32.0 * i as f32 * tandhalf));
            let color = [
                continuous_idct(&color_dct[0], &cosines),
                continuous_idct(&color_dct[1], &cosines),
                continuous_idct(&color_dct[2], &cosines),
            ];
            let sigma = continuous_idct(&sigma_dct, &cosines);
            if !sigma.is_finite()
                || sigma == 0.0
                || !(1.0 / sigma).is_finite()
                || !intensity.is_finite()
            {
                continue;
            }
            let max_color = [0.01f32, color[0].abs(), color[1].abs(), color[2].abs()]
                .into_iter()
                .map(|c| (c * *intensity).abs())
                .fold(0.0f32, f32::max);
            let max_dist =
                (-2.0 * sigma * sigma * (0.1f32.ln() * DISTANCE_EXP - max_color.ln())).sqrt();
            if !max_dist.is_finite() || max_dist <= 0.0 {
                continue;
            }
            let inv_sigma = 1.0 / sigma;
            let scale = 0.25 * sigma * *intensity;
            let y0 = (pt.y - max_dist).round().clamp(0.0, h as f32) as usize;
            let y1 = ((pt.y + max_dist).round().clamp(0.0, h as f32) as usize)
                .saturating_add(1)
                .min(h);
            let x0 = (pt.x - max_dist).round().clamp(0.0, w as f32) as usize;
            let x1 = ((pt.x + max_dist).round().clamp(0.0, w as f32) as usize)
                .saturating_add(1)
                .min(w);
            for py in y0..y1 {
                let dy = py as f32 - pt.y;
                for px in x0..x1 {
                    let dx = px as f32 - pt.x;
                    let dist = (dx * dx + dy * dy).sqrt();
                    let one_dim = fast_erf((dist * 0.5 + ONE_OVER_2S2) * inv_sigma)
                        - fast_erf((dist * 0.5 - ONE_OVER_2S2) * inv_sigma);
                    let local = scale * one_dim * one_dim;
                    let off = py * w + px;
                    xyb[0][off] += color[0] * local;
                    xyb[1][off] += color[1] * local;
                    xyb[2][off] += color[2] * local;
                }
            }
        }
    }

    let mut out = vec![0u8; expected_bytes];
    for (i, (dst, src)) in out
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(base_rgba.as_chunks::<4>().0.iter())
        .enumerate()
    {
        let [r, g, b] = xyb_to_srgb(xyb[0][i], xyb[1][i], xyb[2][i]);
        *dst = [r, g, b, src[3]];
    }
    Ok(out)
}

/// Compute constant-spline XYB DCT[0] coefficients (`[x0, y0, b0]`) and sigma DCT[0]
/// that produce a visible stroke of approximately `(fg_r, fg_g, fg_b)` over
/// `(bg_r, bg_g, bg_b)` at the requested Gaussian `sigma`.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn spline_color_dct_from_srgb(
    fg_r: u8,
    fg_g: u8,
    fg_b: u8,
    bg_r: u8,
    bg_g: u8,
    bg_b: u8,
    sigma: f32,
) -> Result<Vec<f32>, String> {
    if !sigma.is_finite() || !(0.5..=16.0).contains(&sigma) {
        return Err("Spline sigma must be between 0.5 and 16.0.".into());
    }
    let fg = srgb_to_xyb(fg_r, fg_g, fg_b);
    let bg = srgb_to_xyb(bg_r, bg_g, bg_b);
    // Estimate centerline gain of unit-spaced Gaussian splats along a straight curve.
    let inv_sigma = 1.0 / sigma;
    let mut gain = 0.0f32;
    let radius = (sigma * 5.0).ceil() as i32;
    for k in -radius..=radius {
        let d = (k as f32).abs();
        let one_dim = fast_erf((d * 0.5 + ONE_OVER_2S2) * inv_sigma)
            - fast_erf((d * 0.5 - ONE_OVER_2S2) * inv_sigma);
        gain += 0.25 * sigma * one_dim * one_dim;
    }
    let norm = if gain > 1e-4 { 1.0 / gain } else { 1.0 };
    // continuous_idct multiplies dct[0] by SQRT_2, so divide by SQRT_2 here.
    Ok(vec![
        (fg[0] - bg[0]) * norm * FRAC_1_SQRT_2,
        (fg[1] - bg[1]) * norm * FRAC_1_SQRT_2,
        (fg[2] - bg[2]) * norm * FRAC_1_SQRT_2,
        sigma * FRAC_1_SQRT_2,
    ])
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn render_splines_rgba(
    pixels: &[u8],
    width: u32,
    height: u32,
    splines_json: &str,
) -> Result<Vec<u8>, String> {
    let specs: Vec<SplineSpec> =
        serde_json::from_str(splines_json).map_err(|e| format!("Invalid spline JSON: {e}"))?;
    let splines: Vec<Spline> = specs
        .iter()
        .map(SplineSpec::to_encoder_spline)
        .collect::<Result<_, _>>()?;
    render_splines_onto_rgba(pixels, width, height, &splines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xyb_roundtrip_matches_srgb_within_one_code() {
        for &(r, g, b) in &[
            (0, 0, 0),
            (255, 255, 255),
            (255, 0, 0),
            (0, 255, 0),
            (0, 0, 255),
            (40, 120, 210),
            (230, 180, 35),
        ] {
            let [x, y, bv] = srgb_to_xyb(r, g, b);
            let [rr, gg, bb] = xyb_to_srgb(x, y, bv);
            assert!(rr.abs_diff(r) <= 1, "r: {r} -> {rr}");
            assert!(gg.abs_diff(g) <= 1, "g: {g} -> {gg}");
            assert!(bb.abs_diff(b) <= 1, "b: {b} -> {bb}");
        }
    }

    #[test]
    fn spline_render_changes_pixels_along_curve() {
        let w = 64u32;
        let h = 48u32;
        let base = [24u8, 30, 44, 255].repeat((w * h) as usize);
        let dct = spline_color_dct_from_srgb(245, 220, 60, 24, 30, 44, 2.0).unwrap();
        let mut color_dct = [[0.0f32; 32]; 3];
        color_dct[0][0] = dct[0];
        color_dct[1][0] = dct[1];
        color_dct[2][0] = dct[2];
        let mut sigma_dct = [0.0f32; 32];
        sigma_dct[0] = dct[3];
        let spline = Spline {
            control_points: vec![
                SplinePoint::new(8.0, 24.0),
                SplinePoint::new(32.0, 14.0),
                SplinePoint::new(56.0, 24.0),
            ],
            color_dct,
            sigma_dct,
        };
        let rendered = render_splines_onto_rgba(&base, w, h, &[spline]).unwrap();
        // Midpoint near (32, 14) must brighten dramatically toward yellow.
        let mid = ((14 * w + 32) * 4) as usize;
        assert!(
            rendered[mid] > 180 && rendered[mid + 1] > 160,
            "mid={:?}",
            &rendered[mid..mid + 4]
        );
        // Far corner (2, 2) must stay close to background.
        let corner = ((2 * w + 2) * 4) as usize;
        assert!(rendered[corner].abs_diff(24) <= 1);
    }
}
