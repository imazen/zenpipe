use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MaskSpec {
    pub enabled: bool,
    pub mode: String,
    pub shape: String,
    pub invert: bool,
    pub density: f32,
    pub feather: f32,
    // Linear gradient params
    pub angle: f32,
    pub position: f32,
    pub spread: f32,
    // Radial / ellipse params
    pub cx: f32,
    pub cy: f32,
    pub rx: f32,
    pub ry: f32,
    pub softness: f32,
    // Rounded rect params
    pub inset: f32,
    pub radius: f32,
    // Luminance range params
    pub low: f32,
    pub high: f32,
    pub luma_feather: f32,
}

impl Default for MaskSpec {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: "parametric".into(),
            shape: "none".into(),
            invert: false,
            density: 1.0,
            feather: 0.0,
            angle: 0.0,
            position: 0.5,
            spread: 0.35,
            cx: 0.5,
            cy: 0.5,
            rx: 0.38,
            ry: 0.38,
            softness: 0.3,
            inset: 0.08,
            radius: 0.12,
            low: 0.25,
            high: 1.0,
            luma_feather: 0.12,
        }
    }
}

#[inline]
pub(crate) fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let denom = edge1 - edge0;
    if denom.abs() < 1e-6 {
        return if x >= edge0 { 1.0 } else { 0.0 };
    }
    let t = ((x - edge0) / denom).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn srgb_to_linear_approx(u: u8) -> f32 {
    let x = f32::from(u) / 255.0;
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
pub(crate) fn oklab_l_from_srgb(r: u8, g: u8, b: u8) -> f32 {
    let lr = srgb_to_linear_approx(r);
    let lg = srgb_to_linear_approx(g);
    let lb = srgb_to_linear_approx(b);
    let l = (0.412_221_46 * lr + 0.536_332_55 * lg + 0.051_445_995 * lb).cbrt();
    let m = (0.211_903_5 * lr + 0.680_699_5 * lg + 0.107_396_96 * lb).cbrt();
    let s = (0.088_302_46 * lr + 0.281_718_85 * lg + 0.629_978_7 * lb).cbrt();
    (0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s).clamp(0.0, 1.0)
}

fn evaluate_parametric(spec: &MaskSpec, pixels: &[u8], w: usize, h: usize) -> Vec<f32> {
    let n = w * h;
    let mut out = vec![1.0f32; n];
    let inv_w = if w > 1 { 1.0 / (w - 1) as f32 } else { 0.0 };
    let inv_h = if h > 1 { 1.0 / (h - 1) as f32 } else { 0.0 };
    match spec.shape.as_str() {
        "linear" => {
            let rad = spec.angle.to_radians();
            let dx = rad.cos();
            let dy = rad.sin();
            let half = (spec.spread.clamp(0.01, 1.0)) * 0.5;
            let center = spec.position.clamp(0.0, 1.0) - 0.5;
            for y in 0..h {
                let ny = y as f32 * inv_h - 0.5;
                for x in 0..w {
                    let nx = x as f32 * inv_w - 0.5;
                    let proj = nx * dx + ny * dy;
                    out[y * w + x] = smoothstep(center - half, center + half, proj);
                }
            }
        }
        "radial" => {
            let cx = spec.cx.clamp(0.0, 1.0);
            let cy = spec.cy.clamp(0.0, 1.0);
            let rx = spec.rx.clamp(0.02, 1.5);
            let ry = spec.ry.clamp(0.02, 1.5);
            let soft = spec.softness.clamp(0.0, 0.99);
            let inner = (1.0 - soft).max(0.0);
            for y in 0..h {
                let ny = y as f32 * inv_h;
                let dy = (ny - cy) / ry;
                for x in 0..w {
                    let nx = x as f32 * inv_w;
                    let dx = (nx - cx) / rx;
                    let d = (dx * dx + dy * dy).sqrt();
                    out[y * w + x] = 1.0 - smoothstep(inner, 1.0, d);
                }
            }
        }
        "rounded_rect" => {
            let inset = spec.inset.clamp(0.0, 0.48);
            let hx = (0.5 - inset).max(0.01);
            let hy = (0.5 - inset).max(0.01);
            let r = spec.radius.clamp(0.0, hx.min(hy));
            let soft = spec.softness.clamp(0.0, 0.4);
            let min_side = (w.min(h) as f32).max(1.0);
            let aa = 1.0 / min_side;
            let edge = soft.max(aa);
            for y in 0..h {
                let py = (y as f32 * inv_h - 0.5).abs();
                for x in 0..w {
                    let px = (x as f32 * inv_w - 0.5).abs();
                    let qx = px - (hx - r);
                    let qy = py - (hy - r);
                    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
                    let inside = qx.max(qy).min(0.0);
                    let sdf = outside + inside - r;
                    out[y * w + x] = 1.0 - smoothstep(-edge * 0.5, edge * 0.5, sdf);
                }
            }
        }
        "luminance" => {
            let low = spec.low.clamp(0.0, 1.0);
            let high = spec.high.clamp(low, 1.0);
            let feather = spec.luma_feather.clamp(0.0, 0.5);
            for i in 0..n {
                let px = &pixels[i * 4..i * 4 + 4];
                let l = oklab_l_from_srgb(px[0], px[1], px[2]);
                let lo_ramp = if low <= 0.0 {
                    1.0
                } else {
                    smoothstep(low - feather, low + feather * 0.5, l)
                };
                let hi_ramp = if high >= 1.0 {
                    1.0
                } else {
                    1.0 - smoothstep(high - feather * 0.5, high + feather, l)
                };
                out[i] = (lo_ramp * hi_ramp).clamp(0.0, 1.0);
            }
        }
        _ => {}
    }
    out
}

fn gaussian_blur_plane(plane: &mut [f32], w: usize, h: usize, sigma: f32) {
    let radius = (sigma * 2.5).ceil().clamp(1.0, 64.0) as usize;
    let mut kernel = Vec::with_capacity(radius * 2 + 1);
    let inv2s2 = 1.0 / (2.0 * sigma * sigma);
    let mut sum = 0.0f32;
    for i in 0..=(radius * 2) {
        let d = i as f32 - radius as f32;
        let k = (-d * d * inv2s2).exp();
        kernel.push(k);
        sum += k;
    }
    for k in &mut kernel {
        *k /= sum;
    }

    let mut tmp = vec![0.0f32; w * h];
    for y in 0..h {
        let row = y * w;
        for x in 0..w {
            let mut acc = 0.0f32;
            for (ki, &kv) in kernel.iter().enumerate() {
                let sx =
                    (x as isize + ki as isize - radius as isize).clamp(0, w as isize - 1) as usize;
                acc += plane[row + sx] * kv;
            }
            tmp[row + x] = acc;
        }
    }
    for y in 0..h {
        let row = y * w;
        for x in 0..w {
            let mut acc = 0.0f32;
            for (ki, &kv) in kernel.iter().enumerate() {
                let sy =
                    (y as isize + ki as isize - radius as isize).clamp(0, h as isize - 1) as usize;
                acc += tmp[sy * w + x] * kv;
            }
            plane[row + x] = acc.clamp(0.0, 1.0);
        }
    }
}

pub fn apply_layer_mask_rgba_impl(
    mut pixels: Vec<u8>,
    width: u32,
    height: u32,
    spec_json: &str,
    raster_mask: &[u8],
    overlay_ruby: bool,
) -> Result<Vec<u8>, String> {
    if width == 0
        || height == 0
        || width > 16384
        || height > 16384
        || u64::from(width) * u64::from(height) > 64_000_000
        || pixels.len() != width as usize * height as usize * 4
    {
        return Err("Invalid dimensions or RGBA buffer for layer mask.".into());
    }
    let spec: MaskSpec =
        serde_json::from_str(spec_json).map_err(|e| format!("Invalid mask JSON: {e}"))?;
    if !spec.enabled {
        return Ok(pixels);
    }

    let w = width as usize;
    let h = height as usize;
    let n = w * h;
    if !raster_mask.is_empty() && raster_mask.len() != n {
        return Err(format!(
            "Raster mask length {} does not match {}x{} pixels.",
            raster_mask.len(),
            w,
            h
        ));
    }

    let use_parametric =
        matches!(spec.mode.as_str(), "parametric" | "combined") && spec.shape != "none";
    let use_raster = matches!(spec.mode.as_str(), "raster" | "combined") && raster_mask.len() == n;

    let mut mask = if use_parametric {
        evaluate_parametric(&spec, &pixels, w, h)
    } else {
        vec![1.0f32; n]
    };

    if use_raster {
        for (dst, &rm) in mask.iter_mut().zip(raster_mask.iter()) {
            *dst *= f32::from(rm) / 255.0;
        }
    }

    let feather = spec.feather.clamp(0.0, 40.0);
    if feather >= 0.25 && (use_parametric || use_raster) {
        gaussian_blur_plane(&mut mask, w, h, feather);
    }

    let density = spec.density.clamp(0.0, 1.0);
    for (i, m_val) in mask.iter().copied().enumerate() {
        let m = if spec.invert { 1.0 - m_val } else { m_val };
        let eff = (1.0 - density * (1.0 - m)).clamp(0.0, 1.0);
        let px = &mut pixels[i * 4..i * 4 + 4];
        if overlay_ruby {
            let tint = (1.0 - eff) * 0.58;
            px[0] = (f32::from(px[0]) * (1.0 - tint) + 224.0 * tint)
                .round()
                .clamp(0.0, 255.0) as u8;
            px[1] = (f32::from(px[1]) * (1.0 - tint) + 38.0 * tint)
                .round()
                .clamp(0.0, 255.0) as u8;
            px[2] = (f32::from(px[2]) * (1.0 - tint) + 64.0 * tint)
                .round()
                .clamp(0.0, 255.0) as u8;
        } else {
            px[3] = (f32::from(px[3]) * eff).round().clamp(0.0, 255.0) as u8;
        }
    }
    Ok(pixels)
}
