//! Flatten AI-clipart "waviness" / bubble-noise inside flat color regions.
//!
//! AI-generated clipart (gpt-image, Imagen, …) renders nominally-flat fills with
//! a subtle low-amplitude undulation ("waviness" / bubble mottle) and banding.
//! [`ClipartFlatten`] cleans this up while keeping crisp edges and intentional
//! shading — the complement to [`BackgroundFlatten`](crate::filters::BackgroundFlatten),
//! which only touches the surrounding background.
//!
//! Two stages (all in Oklab, full-frame):
//! 1. **Guided-filter base** (He et al.): an edge-preserving, halo-free smooth
//!    of L/a/b. Low-variance waviness inside a region collapses to the local
//!    mean, while crisp edges and genuine smooth shading are kept. Easing the
//!    image toward this base removes waviness without staircasing or gradient
//!    reversal (the bilateral-filter traps). This is the default behaviour.
//! 2. **Cartoon snap** (optional, `cartoon > 0`): quantize to a small OKLab
//!    palette, label connected flat regions, and additionally pull flat-fill
//!    interiors toward their region mean for a hard flat-colour look. Genuinely
//!    shaded regions (high variance) and region boundaries are protected, so
//!    edges stay razor.
//!
//! The cleanup amount is `strength`; the flat/cartoon look is `cartoon`. Wrap in
//! [`MetricGated`](crate::metric_gate::MetricGated) for a subtlety guarantee.

use crate::access::ChannelAccess;
use crate::filter::{Filter, PlaneSemantics, ResizePhase};
use crate::filters::guided_filter::guided_filter_plane;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::prelude::*;

use super::background_flatten::chamfer_distance;
use crate::context::FilterContext;

/// Flatten clip-art waviness inside flat colour regions (see module docs).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ClipartFlatten {
    /// Master cleanup strength, `0.0`–`1.0` (how far toward the cleaned result).
    /// `0.0` is a no-op.
    pub strength: f32,
    /// Cartoon-flat amount, `0.0`–`1.0`. `0.0` = guided waviness removal only
    /// (keeps colours and shading); `1.0` = additionally snap flat regions to
    /// their mean colour for a hard flat look.
    pub cartoon: f32,
    /// Spatial scale (Gaussian sigma, px) of the waviness the guided filter
    /// removes. Typical `2.0`–`6.0`.
    pub waviness_scale: f32,
    /// Variation threshold (guided-filter `eps`, and the region-variance gate
    /// for the cartoon snap). Smaller keeps more detail; larger flattens more.
    /// Typical `0.0004`–`0.002`.
    pub flatness: f32,
    /// Width, in pixels, of the protected band along region boundaries used by
    /// the cartoon snap (keeps edges crisp). Typical `1.0`–`3.0`.
    pub edge_feather: f32,
    /// Target palette size for the cartoon-snap region segmentation. `12`–`48`.
    pub palette_size: u32,
    /// Colour distance (Oklab) below which two palette colours are merged into
    /// one (prevents a flat fill being split across several near-identical
    /// palette entries). Typical `0.03`–`0.07`.
    pub color_tolerance: f32,
}

impl Default for ClipartFlatten {
    fn default() -> Self {
        Self {
            strength: 0.85,
            cartoon: 0.0,
            waviness_scale: 3.0,
            flatness: 0.0010,
            edge_feather: 1.5,
            palette_size: 24,
            color_tolerance: 0.05,
        }
    }
}

/// Smoothstep with a degenerate-edge guard (supports descending ramps).
#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let denom = edge1 - edge0;
    if denom.abs() < 1e-12 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / denom).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dl = a[0] - b[0];
    let da = a[1] - b[1];
    let db = a[2] - b[2];
    dl * dl + da * da + db * db
}

/// Index of the nearest centroid to `c`.
#[inline]
fn nearest(c: [f32; 3], cent: &[[f32; 3]]) -> usize {
    let mut best = 0usize;
    let mut bd = f32::INFINITY;
    for (j, &k) in cent.iter().enumerate() {
        let d = dist2(c, k);
        if d < bd {
            bd = d;
            best = j;
        }
    }
    best
}

/// Quantize three planes to `k` OKLab colours via deterministic Lloyd k-means on
/// a strided training sample, then label every pixel by nearest centroid.
///
/// Centroids closer than `merge_dist` (Oklab) are merged afterwards, so an
/// image with few distinct colours yields few effective palette entries
/// regardless of `k` — otherwise k-means would split a single flat colour
/// across several centroids and fragment the flat region. Returns
/// `(labels, palette)`.
pub(crate) fn quantize_oklab_planes(
    l: &[f32],
    a: &[f32],
    b: &[f32],
    k: usize,
    iters: usize,
    merge_dist: f32,
) -> (Vec<u16>, Vec<[f32; 3]>) {
    let n = l.len();
    let k = k.clamp(1, 256);

    let stride = (n / 8192).max(1);
    let mut samples: Vec<[f32; 3]> = Vec::new();
    let mut i = 0;
    while i < n {
        samples.push([l[i], a[i], b[i]]);
        i += stride;
    }
    let k = k.min(samples.len().max(1));

    let mut cent: Vec<[f32; 3]> = (0..k)
        .map(|j| samples[(j * samples.len()) / k.max(1)])
        .collect();

    for _ in 0..iters {
        let mut sum = vec![[0.0f64; 3]; k];
        let mut cnt = vec![0u32; k];
        for s in &samples {
            let j = nearest(*s, &cent);
            sum[j][0] += s[0] as f64;
            sum[j][1] += s[1] as f64;
            sum[j][2] += s[2] as f64;
            cnt[j] += 1;
        }
        for j in 0..k {
            if cnt[j] > 0 {
                let inv = 1.0 / cnt[j] as f64;
                cent[j] = [
                    (sum[j][0] * inv) as f32,
                    (sum[j][1] * inv) as f32,
                    (sum[j][2] * inv) as f32,
                ];
            }
        }
    }

    let md2 = (merge_dist.max(0.0)) * (merge_dist.max(0.0));
    let mut rep: Vec<usize> = (0..cent.len()).collect();
    for a2 in 0..cent.len() {
        if rep[a2] != a2 {
            continue;
        }
        for b2 in (a2 + 1)..cent.len() {
            if rep[b2] == b2 && dist2(cent[a2], cent[b2]) < md2 {
                rep[b2] = a2;
            }
        }
    }
    let mut palette: Vec<[f32; 3]> = Vec::new();
    for a2 in 0..cent.len() {
        if rep[a2] == a2 {
            palette.push(cent[a2]);
        }
    }

    let mut labels = vec![0u16; n];
    for idx in 0..n {
        labels[idx] = nearest([l[idx], a[idx], b[idx]], &palette) as u16;
    }
    (labels, palette)
}

/// Convenience wrapper quantizing an [`OklabPlanes`] (used by tests).
#[cfg(test)]
pub(crate) fn quantize_oklab(
    planes: &OklabPlanes,
    k: usize,
    iters: usize,
    merge_dist: f32,
) -> (Vec<u16>, Vec<[f32; 3]>) {
    quantize_oklab_planes(&planes.l, &planes.a, &planes.b, k, iters, merge_dist)
}

/// 4-connected components on equal labels. Fills `rid` with a region id per
/// pixel and returns the number of regions.
pub(crate) fn connected_components(labels: &[u16], w: usize, h: usize, rid: &mut [u32]) -> u32 {
    const UNSET: u32 = u32::MAX;
    rid.iter_mut().for_each(|r| *r = UNSET);
    let n = w * h;
    let mut next = 0u32;
    let mut stack: Vec<u32> = Vec::new();
    for start in 0..n {
        if rid[start] != UNSET {
            continue;
        }
        let id = next;
        next += 1;
        let target = labels[start];
        rid[start] = id;
        stack.push(start as u32);
        while let Some(p) = stack.pop() {
            let p = p as usize;
            let x = p % w;
            let y = p / w;
            let visit = |ni: usize, stack: &mut Vec<u32>, rid: &mut [u32]| {
                if rid[ni] == UNSET && labels[ni] == target {
                    rid[ni] = id;
                    stack.push(ni as u32);
                }
            };
            if x > 0 {
                visit(p - 1, &mut stack, rid);
            }
            if x + 1 < w {
                visit(p + 1, &mut stack, rid);
            }
            if y > 0 {
                visit(p - w, &mut stack, rid);
            }
            if y + 1 < h {
                visit(p + w, &mut stack, rid);
            }
        }
    }
    next
}

impl Filter for ClipartFlatten {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        (self.waviness_scale * 3.0).ceil() as u32 + self.edge_feather.ceil() as u32 + 2
    }

    fn plane_semantics(&self) -> PlaneSemantics {
        PlaneSemantics::Oklab
    }

    fn resize_phase(&self) -> ResizePhase {
        ResizePhase::PreResize
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.strength <= 1e-6 {
            return;
        }
        let w = planes.width;
        let h = planes.height;
        let n = planes.pixel_count();
        if n == 0 || w < 4 || h < 4 {
            return;
        }
        let strength = self.strength.clamp(0.0, 1.0);
        let cartoon = self.cartoon.clamp(0.0, 1.0);
        let sigma = self.waviness_scale.max(0.5);
        let eps = self.flatness.max(1e-6);

        // --- Stage 1: edge-preserving guided-filter base (waviness removed) ---
        let mut gl = ctx.take_f32(n);
        let mut ga = ctx.take_f32(n);
        let mut gb = ctx.take_f32(n);
        guided_filter_plane(&planes.l, &planes.l, &mut gl, w, h, sigma, eps, ctx);
        guided_filter_plane(&planes.a, &planes.a, &mut ga, w, h, sigma, eps, ctx);
        guided_filter_plane(&planes.b, &planes.b, &mut gb, w, h, sigma, eps, ctx);

        if cartoon <= 1e-4 {
            // Gentle: ease toward the guided base.
            for i in 0..n {
                planes.l[i] += (gl[i] - planes.l[i]) * strength;
                planes.a[i] += (ga[i] - planes.a[i]) * strength;
                planes.b[i] += (gb[i] - planes.b[i]) * strength;
            }
            ctx.return_f32(gb);
            ctx.return_f32(ga);
            ctx.return_f32(gl);
            return;
        }

        // --- Stage 2: cartoon snap toward per-region mean ---
        let wu = w as usize;
        let hu = h as usize;
        let (labels, _palette) = self.quantize(&gl, &ga, &gb, w, h);
        let mut rid = vec![0u32; n];
        let num = connected_components(&labels, wu, hu, &mut rid) as usize;

        // Region mean of the guided base (clean colour).
        let mut sum = vec![[0.0f64; 3]; num];
        let mut cnt = vec![0u32; num];
        for i in 0..n {
            let r = rid[i] as usize;
            sum[r][0] += gl[i] as f64;
            sum[r][1] += ga[i] as f64;
            sum[r][2] += gb[i] as f64;
            cnt[r] += 1;
        }
        let mut mean = vec![[0.0f32; 3]; num];
        for r in 0..num {
            if cnt[r] > 0 {
                let inv = 1.0 / cnt[r] as f64;
                mean[r] = [
                    (sum[r][0] * inv) as f32,
                    (sum[r][1] * inv) as f32,
                    (sum[r][2] * inv) as f32,
                ];
            }
        }
        // Region colour variance → region flatness (keep shaded regions).
        let mut var = vec![0.0f64; num];
        for i in 0..n {
            let r = rid[i] as usize;
            var[r] += dist2([gl[i], ga[i], gb[i]], mean[r]) as f64;
        }
        let mut region_flat = vec![0.0f32; num];
        for r in 0..num {
            if cnt[r] > 0 {
                let v = (var[r] / cnt[r] as f64) as f32;
                region_flat[r] = 1.0 - smoothstep(eps * 2.0, eps * 8.0, v);
            }
        }

        // Region-boundary distance → edge protection for the snap.
        let mut boundary = ctx.take_u8(n);
        boundary.iter_mut().for_each(|b| *b = 1);
        for y in 0..hu {
            for x in 0..wu {
                let i = y * wu + x;
                let r = rid[i];
                let edge = (x > 0 && rid[i - 1] != r)
                    || (x + 1 < wu && rid[i + 1] != r)
                    || (y > 0 && rid[i - wu] != r)
                    || (y + 1 < hu && rid[i + wu] != r);
                if edge {
                    boundary[i] = 0;
                }
            }
        }
        let mut dist = ctx.take_f32(n);
        chamfer_distance(&boundary, wu, hu, &mut dist);
        let feather = self.edge_feather.max(0.25);

        // Palette colours = the per-label mean of the guided base (the effective
        // flat-fill colours). The cartoon snap pulls each pixel toward a *soft*
        // partition-of-unity blend of these — `softmax(-dist²/τ²)` over the
        // palette, with `τ` ≈ the palette spacing (`color_tolerance`). This is the
        // key to not banding a gradient: snapping each connected cell to its own
        // single mean stepped at every cell boundary, whereas the soft blend is
        // ≈ identity along a gradient (evenly-spaced palette, weights shift
        // smoothly) yet collapses an isolated flat colour to its palette entry
        // (only that colour has weight). Edges and shaded regions stay protected
        // by the same `region_flat × boundary_keep` gate as before.
        let num_labels = labels.iter().copied().max().unwrap_or(0) as usize + 1;
        let mut psum = vec![[0.0f64; 3]; num_labels];
        let mut pcnt = vec![0u32; num_labels];
        for i in 0..n {
            let k = labels[i] as usize;
            psum[k][0] += gl[i] as f64;
            psum[k][1] += ga[i] as f64;
            psum[k][2] += gb[i] as f64;
            pcnt[k] += 1;
        }
        let mut palette: Vec<[f32; 3]> = Vec::with_capacity(num_labels);
        for k in 0..num_labels {
            if pcnt[k] > 0 {
                let inv = 1.0 / pcnt[k] as f64;
                palette.push([
                    (psum[k][0] * inv) as f32,
                    (psum[k][1] * inv) as f32,
                    (psum[k][2] * inv) as f32,
                ]);
            }
        }
        let tau = self.color_tolerance.max(1e-3);
        let inv_tau2 = 1.0 / (tau * tau);

        // target = lerp(guided_base, soft_palette_blend, snap); result = lerp(orig, target, strength)
        for i in 0..n {
            let r = rid[i] as usize;
            let g = [gl[i], ga[i], gb[i]];
            // Soft partition-of-unity assignment over the palette.
            let mut wsum = 0.0f32;
            let mut acc = [0.0f32; 3];
            for &p in &palette {
                let wv = (-dist2(g, p) * inv_tau2).exp();
                wsum += wv;
                acc[0] += wv * p[0];
                acc[1] += wv * p[1];
                acc[2] += wv * p[2];
            }
            let target = if wsum > 1e-12 {
                [acc[0] / wsum, acc[1] / wsum, acc[2] / wsum]
            } else {
                g
            };
            let boundary_keep = smoothstep(0.0, feather, dist[i]);
            let snap = cartoon * region_flat[r] * boundary_keep;
            let tl = gl[i] + (target[0] - gl[i]) * snap;
            let ta = ga[i] + (target[1] - ga[i]) * snap;
            let tb = gb[i] + (target[2] - gb[i]) * snap;
            planes.l[i] += (tl - planes.l[i]) * strength;
            planes.a[i] += (ta - planes.a[i]) * strength;
            planes.b[i] += (tb - planes.b[i]) * strength;
        }

        ctx.return_f32(dist);
        ctx.return_u8(boundary);
        ctx.return_f32(gb);
        ctx.return_f32(ga);
        ctx.return_f32(gl);
    }
}

impl ClipartFlatten {
    /// Quantize for the cartoon snap. Uses the guided base (clean colours) so
    /// labels follow the intended fills, not the waviness. With the `zenquant`
    /// feature, uses the perceptual OKLab quantizer; otherwise the built-in
    /// k-means.
    fn quantize(
        &self,
        gl: &[f32],
        ga: &[f32],
        gb: &[f32],
        w: u32,
        h: u32,
    ) -> (Vec<u16>, Vec<[f32; 3]>) {
        #[cfg(feature = "zenquant")]
        if let Some(labels) = quantize_zenquant(gl, ga, gb, w, h, self.palette_size) {
            return (labels, Vec::new());
        }
        #[cfg(not(feature = "zenquant"))]
        let _ = (w, h);
        quantize_oklab_planes(
            gl,
            ga,
            gb,
            self.palette_size as usize,
            10,
            self.color_tolerance.max(1e-4),
        )
    }
}

/// Quantize the guided base via zenquant (perceptual OKLab, dither disabled so
/// labels are clean nearest-palette assignments). Returns per-pixel labels, or
/// `None` on failure (caller falls back to the built-in quantizer).
#[cfg(feature = "zenquant")]
fn quantize_zenquant(
    gl: &[f32],
    ga: &[f32],
    gb: &[f32],
    w: u32,
    h: u32,
    k: u32,
) -> Option<Vec<u16>> {
    let n = gl.len();
    // Oklab guided base → linear RGB (BT.709) → sRGB u8.
    let mut tmp = OklabPlanes::new(w, h);
    tmp.l.copy_from_slice(gl);
    tmp.a.copy_from_slice(ga);
    tmp.b.copy_from_slice(gb);
    let m1_inv = zenpixels_convert::oklab::lms_to_rgb_matrix(zenpixels::ColorPrimaries::Bt709)?;
    let mut lin = vec![0.0f32; n * 3];
    crate::gather_from_oklab(&tmp, &mut lin, 3, &m1_inv, 1.0);

    let enc = |v: f32| -> u8 {
        let v = v.clamp(0.0, 1.0);
        let s = if v <= 0.003_130_8 {
            12.92 * v
        } else {
            1.055 * crate::fast_math::fast_powf(v, 1.0 / 2.4) - 0.055
        };
        (s.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
    };
    let mut pixels: Vec<rgb::RGB<u8>> = Vec::with_capacity(n);
    for i in 0..n {
        pixels.push(rgb::RGB {
            r: enc(lin[i * 3]),
            g: enc(lin[i * 3 + 1]),
            b: enc(lin[i * 3 + 2]),
        });
    }

    let cfg = zenquant::QuantizeConfig::new(zenquant::OutputFormat::Png)
        .with_max_colors(k.clamp(2, 256))
        .with_quality(zenquant::Quality::Balanced)
        ._with_no_dither();
    let res = zenquant::quantize(&pixels, w as usize, h as usize, &cfg).ok()?;
    Some(res.indices().iter().map(|&i| i as u16).collect())
}

static CLIPART_FLATTEN_SCHEMA: FilterSchema = FilterSchema {
    name: "clipart_flatten",
    label: "Clipart Flatten",
    description: "Flatten AI-clipart waviness/bubble noise inside flat colour regions while keeping crisp edges and shading",
    group: FilterGroup::Effects,
    params: &[
        ParamDesc {
            name: "strength",
            label: "Strength",
            description: "How far toward the cleaned result (0 = off)",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.85,
                identity: 0.0,
                step: 0.01,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "cartoon",
            label: "Cartoon",
            description: "0 = waviness removal only; 1 = snap flat regions to a flat colour",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.0,
                identity: 0.0,
                step: 0.01,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "waviness_scale",
            label: "Waviness Scale",
            description: "Spatial scale of the waviness removed by the guided filter",
            kind: ParamKind::Float {
                min: 1.0,
                max: 12.0,
                default: 3.0,
                identity: 0.0,
                step: 0.5,
            },
            unit: "px",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "flatness",
            label: "Flatness",
            description: "Variation treated as waviness vs detail (guided eps + region gate)",
            kind: ParamKind::Float {
                min: 0.0002,
                max: 0.005,
                default: 0.0010,
                identity: 0.0,
                step: 0.0002,
            },
            unit: "",
            section: "Advanced",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "edge_feather",
            label: "Edge Feather",
            description: "Protected band width along region boundaries (cartoon snap)",
            kind: ParamKind::Float {
                min: 0.0,
                max: 8.0,
                default: 1.5,
                identity: 0.0,
                step: 0.5,
            },
            unit: "px",
            section: "Advanced",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "palette_size",
            label: "Palette Size",
            description: "Colours used to segment flat regions (cartoon snap)",
            kind: ParamKind::Int {
                min: 4,
                max: 64,
                default: 24,
            },
            unit: "",
            section: "Advanced",
            slider: SliderMapping::NotSlider,
        },
        ParamDesc {
            name: "color_tolerance",
            label: "Colour Merge",
            description: "Palette colours closer than this are merged (avoids over-segmentation)",
            kind: ParamKind::Float {
                min: 0.01,
                max: 0.15,
                default: 0.05,
                identity: 0.0,
                step: 0.005,
            },
            unit: "",
            section: "Advanced",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for ClipartFlatten {
    fn schema() -> &'static FilterSchema {
        &CLIPART_FLATTEN_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "strength" => Some(ParamValue::Float(self.strength)),
            "cartoon" => Some(ParamValue::Float(self.cartoon)),
            "waviness_scale" => Some(ParamValue::Float(self.waviness_scale)),
            "flatness" => Some(ParamValue::Float(self.flatness)),
            "edge_feather" => Some(ParamValue::Float(self.edge_feather)),
            "palette_size" => Some(ParamValue::Int(self.palette_size as i32)),
            "color_tolerance" => Some(ParamValue::Float(self.color_tolerance)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        match name {
            "palette_size" => {
                if let Some(v) = value.as_i32() {
                    self.palette_size = v.clamp(4, 64) as u32;
                    return true;
                }
                false
            }
            _ => {
                let v = match value.as_f32() {
                    Some(v) => v,
                    None => return false,
                };
                match name {
                    "strength" => self.strength = v.clamp(0.0, 1.0),
                    "cartoon" => self.cartoon = v.clamp(0.0, 1.0),
                    "waviness_scale" => self.waviness_scale = v.clamp(0.5, 16.0),
                    "flatness" => self.flatness = v.clamp(0.0002, 0.01),
                    "edge_feather" => self.edge_feather = v.clamp(0.0, 8.0),
                    "color_tolerance" => self.color_tolerance = v.clamp(0.01, 0.2),
                    _ => return false,
                }
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_region_wavy(w: u32, h: u32) -> OklabPlanes {
        let mut p = OklabPlanes::new(w, h);
        let wu = w as usize;
        let hu = h as usize;
        for y in 0..hu {
            for x in 0..wu {
                let i = y * wu + x;
                let wav = (((x * 5 + y * 3) % 9) as f32 / 9.0 - 0.5) * 0.03;
                if x < wu / 2 {
                    p.l[i] = 0.45 + wav;
                    p.a[i] = -0.08;
                    p.b[i] = 0.04;
                } else {
                    p.l[i] = 0.80 + wav;
                    p.a[i] = 0.02;
                    p.b[i] = 0.10;
                }
            }
        }
        p
    }

    fn region_l_variance(l: &[f32], w: usize, x0: usize, x1: usize, y0: usize, y1: usize) -> f32 {
        let mut s = 0.0f32;
        let mut c = 0usize;
        for y in y0..y1 {
            for x in x0..x1 {
                s += l[y * w + x];
                c += 1;
            }
        }
        let m = s / c as f32;
        let mut v = 0.0f32;
        for y in y0..y1 {
            for x in x0..x1 {
                let d = l[y * w + x] - m;
                v += d * d;
            }
        }
        v / c as f32
    }

    #[test]
    fn zero_strength_is_identity() {
        let mut p = two_region_wavy(48, 48);
        let orig = p.l.clone();
        ClipartFlatten {
            strength: 0.0,
            ..Default::default()
        }
        .apply(&mut p, &mut FilterContext::new());
        assert_eq!(p.l, orig);
    }

    #[test]
    fn flattens_waviness_within_regions() {
        let (w, h) = (64u32, 64u32);
        let wu = w as usize;
        let mut p = two_region_wavy(w, h);
        let vb = region_l_variance(&p.l, wu, 4, 24, 4, 60);
        ClipartFlatten::default().apply(&mut p, &mut FilterContext::new());
        let va = region_l_variance(&p.l, wu, 4, 24, 4, 60);
        assert!(
            va < vb * 0.5,
            "left-region waviness should be flattened: var {vb} -> {va}"
        );
    }

    #[test]
    fn preserves_sharp_edge() {
        let (w, h) = (64u32, 64u32);
        let wu = w as usize;
        let hu = h as usize;
        let mut p = two_region_wavy(w, h);
        let mid = wu / 2;
        let yrow = hu / 2;
        let before = (p.l[yrow * wu + mid] - p.l[yrow * wu + (mid - 1)]).abs();
        ClipartFlatten::default().apply(&mut p, &mut FilterContext::new());
        let after = (p.l[yrow * wu + mid] - p.l[yrow * wu + (mid - 1)]).abs();
        assert!(
            after >= before * 0.8,
            "edge contrast must be preserved: {before} -> {after}"
        );
    }

    #[test]
    fn preserves_shaded_region() {
        // A single smooth gradient region (high variance) must be preserved by
        // the default (guided) mode.
        let (w, h) = (48u32, 48u32);
        let wu = w as usize;
        let hu = h as usize;
        let mut p = OklabPlanes::new(w, h);
        for y in 0..hu {
            for x in 0..wu {
                let i = y * wu + x;
                p.l[i] = 0.30 + 0.40 * (x as f32 / (wu as f32 - 1.0));
            }
        }
        let orig = p.l.clone();
        ClipartFlatten::default().apply(&mut p, &mut FilterContext::new());
        let mut max_err = 0.0f32;
        for i in 0..p.l.len() {
            max_err = max_err.max((p.l[i] - orig[i]).abs());
        }
        assert!(
            max_err < 0.05,
            "smooth shaded region must be preserved, max_err={max_err}"
        );
    }

    #[test]
    fn cartoon_snaps_flatter_than_gentle() {
        // Cartoon mode should flatten a region at least as much as gentle mode.
        let (w, h) = (64u32, 64u32);
        let wu = w as usize;
        let mut gentle = two_region_wavy(w, h);
        let mut cart = gentle.clone();
        ClipartFlatten {
            cartoon: 0.0,
            ..Default::default()
        }
        .apply(&mut gentle, &mut FilterContext::new());
        ClipartFlatten {
            cartoon: 1.0,
            ..Default::default()
        }
        .apply(&mut cart, &mut FilterContext::new());
        let vg = region_l_variance(&gentle.l, wu, 4, 24, 4, 60);
        let vc = region_l_variance(&cart.l, wu, 4, 24, 4, 60);
        assert!(
            vc <= vg + 1e-6,
            "cartoon should be at least as flat as gentle: gentle {vg}, cartoon {vc}"
        );
    }

    #[test]
    fn quantize_two_colors() {
        let (w, h) = (32u32, 32u32);
        let p = two_region_wavy(w, h);
        let (labels, cent) = quantize_oklab(&p, 2, 10, 0.05);
        assert_eq!(labels.len(), (w * h) as usize);
        assert!(cent.len() >= 2);
        let wu = w as usize;
        let left = labels[(16 * wu) + 4];
        let right = labels[(16 * wu) + 28];
        assert_ne!(left, right, "the two colour regions should quantize apart");
    }

    #[test]
    fn connected_components_counts() {
        let (w, h) = (4usize, 2usize);
        let labels = vec![0u16, 0, 1, 1, 0, 0, 1, 1];
        let mut rid = vec![0u32; w * h];
        let num = connected_components(&labels, w, h, &mut rid);
        assert_eq!(num, 2, "two vertical halves → 2 regions");
        assert_eq!(rid[0], rid[1]);
        assert_ne!(rid[0], rid[2]);
    }
}
