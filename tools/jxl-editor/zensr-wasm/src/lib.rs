#![forbid(unsafe_code)]

use std::cell::RefCell;

use serde::{Deserialize, Serialize};
use zensr_micro::adopted::AdoptedModel;
use zensr_micro::decode_all_f16;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

const MAX_OUTPUT_PIXELS: u64 = 64_000_000;
const MAX_OUTPUT_DIM: u32 = 16384;

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct ZensrOutput {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl ZensrOutput {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ZensrConfig {
    pub arch: String,
    pub nf: usize,
    pub nc: usize,
    pub scale: usize,
    pub strength: f32,
    pub tile: usize,
}

impl Default for ZensrConfig {
    fn default() -> Self {
        Self {
            arch: "compact".into(),
            nf: 24,
            nc: 8,
            scale: 1,
            strength: 1.0,
            tile: 128,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelCatalogEntry {
    id: &'static str,
    label: &'static str,
    description: &'static str,
    arch: &'static str,
    nf: usize,
    nc: usize,
    scale: usize,
    weights_url: &'static str,
    weights_sha256: &'static str,
    weights_bytes: usize,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn zensr_catalog_json() -> String {
    let entries = [
        ModelCatalogEntry {
            id: "dejpeg_rt24g",
            label: "ZenSR De-JPEG Realtime (rt24g · 1× · 84 KB)",
            description: "43k-param pure-Rust neural JPEG artifact restoration (1×, f16).",
            arch: "compact",
            nf: 24,
            nc: 8,
            scale: 1,
            weights_url: "./models/dejpeg_rt24g.f16.raw",
            weights_sha256: "0de929e1bee7bde444d1d8ab46a8fd0d0ff4fef47daf561095d1661008798b80",
            weights_bytes: 86_406,
        },
        ModelCatalogEntry {
            id: "rtc_distill_2x",
            label: "ZenSR Realtime 2× Upscale (rtc24 · 2× · 88 KB)",
            description: "45k-param distilled neural 2× super-resolution (fast in browser WASM SIMD).",
            arch: "compact",
            nf: 24,
            nc: 8,
            scale: 2,
            weights_url: "./models/rtc_distill_2x.f16.raw",
            weights_sha256: "2b5173ac26b6c1aab53b4b4d34320096421ee2588fa5021f1c9393f1120277b4",
            weights_bytes: 90_312,
        },
        ModelCatalogEntry {
            id: "nomosuni_span_2x",
            label: "ZenSR Quality 2× Upscale (SPAN-48 · 2× · 802 KB)",
            description: "410k-param SPAN-48 neural 2× super-resolution for photos & graphics.",
            arch: "span48",
            nf: 48,
            nc: 6,
            scale: 2,
            weights_url: "./models/nomosuni_span_2x.f16.raw",
            weights_sha256: "b1e56f5f20df3cbcd8c05416bd10802b3b86fcf67abb350ed92fa31319417481",
            weights_bytes: 821_400,
        },
        ModelCatalogEntry {
            id: "nomosuni_span_4x",
            label: "ZenSR Quality 4× Upscale (SPAN-48 · 4× · 833 KB)",
            description: "426k-param SPAN-48 neural 4× super-resolution for detail enlargement.",
            arch: "span48",
            nf: 48,
            nc: 6,
            scale: 4,
            weights_url: "./models/nomosuni_span_4x.f16.raw",
            weights_sha256: "5006aa39926af84bd7e019e1fd8cbf5eb05fefb614badcee53a32f9a8a349a93",
            weights_bytes: 852_576,
        },
    ];
    serde_json::to_string(&entries).unwrap_or_else(|_| "[]".into())
}

fn expected_floats(cfg: &ZensrConfig) -> Result<usize, String> {
    let s2 = cfg
        .scale
        .checked_mul(cfg.scale)
        .ok_or("Invalid ZenSR scale.")?;
    match cfg.arch.as_str() {
        "compact" => {
            if cfg.nf < 4 || !cfg.nf.is_multiple_of(4) || cfg.nf > 64 {
                return Err(format!(
                    "Compact nf={} must be a multiple of 4 in 4..=64.",
                    cfg.nf
                ));
            }
            if !(1..=4).contains(&cfg.scale) {
                return Err(format!("Compact scale={} must be in 1..=4.", cfg.scale));
            }
            let nf = cfg.nf;
            Ok(
                (3 * nf * 9 + 2 * nf)
                    + cfg.nc * (nf * nf * 9 + 2 * nf)
                    + (nf * 3 * s2 * 9 + 3 * s2),
            )
        }
        "span48" => {
            if cfg.scale != 2 && cfg.scale != 4 {
                return Err(format!("SPAN-48 scale={} must be 2 or 4.", cfg.scale));
            }
            let fc = 48usize;
            Ok((3 * fc * 9 + fc)
                + 18 * (fc * fc * 9 + fc)
                + (fc * fc * 9 + fc)
                + (4 * fc * fc + fc)
                + (fc * 3 * s2 * 9 + 3 * s2))
        }
        other => Err(format!("Unsupported ZenSR architecture '{other}'.")),
    }
}

fn decode_weights(weights: &[u8], expected: usize) -> Result<Vec<f32>, String> {
    if weights.len() == expected * 2 {
        Ok(decode_all_f16(weights))
    } else if weights.len() == expected * 4 {
        Ok(weights
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect())
    } else {
        Err(format!(
            "ZenSR weights size {} B does not match expected {} f16 ({} B) or f32 ({} B) floats.",
            weights.len(),
            expected,
            expected * 2,
            expected * 4
        ))
    }
}

fn fnv1a_key(weights: &[u8], cfg: &ZensrConfig) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in cfg.arch.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    for n in [cfg.nf, cfg.nc, cfg.scale, weights.len()] {
        h ^= n as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    // Sample every 64th byte plus head/tail for fast cache keying
    let step = (weights.len() / 512).max(1);
    for &b in weights.iter().step_by(step) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    for &b in weights.iter().rev().take(16) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

thread_local! {
    static MODEL_CACHE: RefCell<Option<(u64, AdoptedModel)>> = const { RefCell::new(None) };
}

fn with_cached_model<R>(
    weights: &[u8],
    cfg: &ZensrConfig,
    f: impl FnOnce(&AdoptedModel) -> R,
) -> Result<R, String> {
    let key = fnv1a_key(weights, cfg);
    let hit = MODEL_CACHE.with(|cell| {
        cell.borrow()
            .as_ref()
            .is_some_and(|(cached_key, _)| *cached_key == key)
    });
    if !hit {
        let expected = expected_floats(cfg)?;
        let raw = decode_weights(weights, expected)?;
        let model = match cfg.arch.as_str() {
            "compact" => AdoptedModel::load_compact(&raw, cfg.nf, cfg.nc, cfg.scale)?,
            "span48" => AdoptedModel::load_span48(&raw, cfg.scale)?,
            other => return Err(format!("Unsupported ZenSR architecture '{other}'.")),
        };
        MODEL_CACHE.with(|cell| {
            *cell.borrow_mut() = Some((key, model));
        });
    }
    Ok(MODEL_CACHE.with(|cell| {
        let borrow = cell.borrow();
        let (_, model) = borrow.as_ref().expect("model cached");
        f(model)
    }))
}

#[inline]
fn bilinear_alpha(
    alpha: &[u8],
    w: usize,
    h: usize,
    ox: usize,
    oy: usize,
    ow: usize,
    oh: usize,
) -> u8 {
    if w == ow && h == oh {
        return alpha[oy * w + ox];
    }
    let sx = ((ox as f32 + 0.5) * (w as f32 / ow as f32) - 0.5).clamp(0.0, (w - 1) as f32);
    let sy = ((oy as f32 + 0.5) * (h as f32 / oh as f32) - 0.5).clamp(0.0, (h - 1) as f32);
    let x0 = sx.floor() as usize;
    let y0 = sy.floor() as usize;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = sx - x0 as f32;
    let fy = sy - y0 as f32;
    let a00 = f32::from(alpha[y0 * w + x0]);
    let a10 = f32::from(alpha[y0 * w + x1]);
    let a01 = f32::from(alpha[y1 * w + x0]);
    let a11 = f32::from(alpha[y1 * w + x1]);
    let top = a00 + (a10 - a00) * fx;
    let bot = a01 + (a11 - a01) * fx;
    (top + (bot - top) * fy).round().clamp(0.0, 255.0) as u8
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn zensr_run_rgba(
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    weights: &[u8],
    config_json: &str,
) -> Result<ZensrOutput, String> {
    if width == 0
        || height == 0
        || width > MAX_OUTPUT_DIM
        || height > MAX_OUTPUT_DIM
        || pixels.len() != width as usize * height as usize * 4
    {
        return Err("Invalid RGBA dimensions for ZenSR.".into());
    }
    let cfg: ZensrConfig =
        serde_json::from_str(config_json).map_err(|e| format!("Invalid ZenSR config JSON: {e}"))?;
    if !cfg.strength.is_finite() || !(0.0..=1.0).contains(&cfg.strength) {
        return Err("ZenSR strength must be within 0..=1.".into());
    }
    let out_w = width
        .checked_mul(cfg.scale as u32)
        .ok_or("ZenSR output width overflow.")?;
    let out_h = height
        .checked_mul(cfg.scale as u32)
        .ok_or("ZenSR output height overflow.")?;
    if out_w > MAX_OUTPUT_DIM
        || out_h > MAX_OUTPUT_DIM
        || u64::from(out_w) * u64::from(out_h) > MAX_OUTPUT_PIXELS
    {
        return Err(format!(
            "ZenSR output {out_w}×{out_h} exceeds the 16,384 px / 64 MP editor limit."
        ));
    }
    if cfg.scale == 1 && cfg.strength <= 1e-5 {
        return Ok(ZensrOutput {
            width,
            height,
            pixels,
        });
    }

    let w = width as usize;
    let h = height as usize;
    let plane = w * h;
    let mut planar = vec![0.0f32; 3 * plane];
    let mut alpha = vec![255u8; plane];
    for i in 0..plane {
        let p = &pixels[i * 4..i * 4 + 4];
        planar[i] = f32::from(p[0]) / 255.0;
        planar[plane + i] = f32::from(p[1]) / 255.0;
        planar[2 * plane + i] = f32::from(p[2]) / 255.0;
        alpha[i] = p[3];
    }

    let tile = if cfg.tile == 0 {
        128
    } else {
        cfg.tile.clamp(32, 512)
    };
    // threads = 1 for deterministic single-threaded WebAssembly execution
    let out_planar = with_cached_model(weights, &cfg, |model| {
        model.upscale_tiled(&planar, h, w, 1, tile)
    })?;

    let ow = out_w as usize;
    let oh = out_h as usize;
    let oplane = ow * oh;
    let mut out_rgba = vec![0u8; oplane * 4];
    let strength = cfg.strength;

    if cfg.scale == 1 {
        for i in 0..plane {
            let r_sr = out_planar[i].clamp(0.0, 1.0) * 255.0;
            let g_sr = out_planar[plane + i].clamp(0.0, 1.0) * 255.0;
            let b_sr = out_planar[2 * plane + i].clamp(0.0, 1.0) * 255.0;
            let p = &pixels[i * 4..i * 4 + 4];
            let dst = &mut out_rgba[i * 4..i * 4 + 4];
            dst[0] = (f32::from(p[0]) + (r_sr - f32::from(p[0])) * strength)
                .round()
                .clamp(0.0, 255.0) as u8;
            dst[1] = (f32::from(p[1]) + (g_sr - f32::from(p[1])) * strength)
                .round()
                .clamp(0.0, 255.0) as u8;
            dst[2] = (f32::from(p[2]) + (b_sr - f32::from(p[2])) * strength)
                .round()
                .clamp(0.0, 255.0) as u8;
            dst[3] = alpha[i];
        }
    } else {
        for oy in 0..oh {
            for ox in 0..ow {
                let idx = oy * ow + ox;
                let dst = &mut out_rgba[idx * 4..idx * 4 + 4];
                dst[0] = (out_planar[idx].clamp(0.0, 1.0) * 255.0).round() as u8;
                dst[1] = (out_planar[oplane + idx].clamp(0.0, 1.0) * 255.0).round() as u8;
                dst[2] = (out_planar[2 * oplane + idx].clamp(0.0, 1.0) * 255.0).round() as u8;
                dst[3] = bilinear_alpha(&alpha, w, h, ox, oy, ow, oh);
            }
        }
    }

    Ok(ZensrOutput {
        width: out_w,
        height: out_h,
        pixels: out_rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f32_to_le_bytes(v: &[f32]) -> Vec<u8> {
        let mut out = Vec::with_capacity(v.len() * 4);
        for x in v {
            out.extend_from_slice(&x.to_le_bytes());
        }
        out
    }

    #[test]
    fn compact_scale1_preserves_alpha_and_strength_blend() {
        let cfg = ZensrConfig {
            arch: "compact".into(),
            nf: 8,
            nc: 1,
            scale: 1,
            strength: 1.0,
            tile: 32,
        };
        let n = expected_floats(&cfg).unwrap();
        // Zero weights + non-zero final bias shifts RGB slightly upward from nearest_add residual
        let mut raw = vec![0.0f32; n];
        raw[n - 3] = 0.2;
        raw[n - 2] = 0.1;
        raw[n - 1] = -0.1;
        let bytes = f32_to_le_bytes(&raw);

        let input = vec![
            100, 120, 140, 77, 80, 90, 100, 255, 60, 70, 80, 0, 120, 130, 140, 200,
        ];
        let out_full = zensr_run_rgba(
            input.clone(),
            2,
            2,
            &bytes,
            r#"{"arch":"compact","nf":8,"nc":1,"scale":1,"strength":1.0,"tile":32}"#,
        )
        .unwrap();
        assert_eq!((out_full.width(), out_full.height()), (2, 2));
        let px_full = out_full.into_pixels();
        assert!(px_full[0] > 140 && px_full[0] < 160);
        assert_eq!(px_full[3], 77);
        assert_eq!(px_full[11], 0);

        let out_zero = zensr_run_rgba(
            input.clone(),
            2,
            2,
            &bytes,
            r#"{"arch":"compact","nf":8,"nc":1,"scale":1,"strength":0.0,"tile":32}"#,
        )
        .unwrap();
        assert_eq!(out_zero.into_pixels(), input);
    }

    #[test]
    fn compact_scale2_upscales_rgb_and_alpha() {
        let cfg = ZensrConfig {
            arch: "compact".into(),
            nf: 8,
            nc: 1,
            scale: 2,
            strength: 1.0,
            tile: 32,
        };
        let n = expected_floats(&cfg).unwrap();
        let bytes = f32_to_le_bytes(&vec![0.0f32; n]);
        let input = vec![50, 100, 150, 128, 200, 150, 100, 255];
        let out = zensr_run_rgba(
            input,
            2,
            1,
            &bytes,
            r#"{"arch":"compact","nf":8,"nc":1,"scale":2,"strength":1.0,"tile":32}"#,
        )
        .unwrap();
        assert_eq!((out.width(), out.height()), (4, 2));
        let px = out.into_pixels();
        assert_eq!(px.len(), 4 * 2 * 4);
        assert_eq!(px[0], 50);
        assert_eq!(px[3], 128);
        assert_eq!(px[3 * 4], 200);
        assert_eq!(px[3 * 4 + 3], 255);
    }

    #[test]
    fn rejects_invalid_weights_or_config() {
        assert!(zensr_run_rgba(vec![0; 16], 2, 2, &[1, 2, 3], "{}").is_err());
        assert!(zensr_run_rgba(vec![0; 16], 2, 2, &[], r#"{"arch":"unknown"}"#).is_err());
    }
}
