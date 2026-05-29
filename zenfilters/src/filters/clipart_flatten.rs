//! Flatten AI-clipart "waviness" / bubble-noise inside flat color regions.
//!
//! AI-generated clipart (gpt-image, Imagen, …) renders nominally-flat fills with
//! a subtle low-amplitude undulation ("waviness" / bubble mottle) and banding.
//! [`ClipartFlatten`] cleans this up while keeping crisp edges and intentional
//! shading, using a quantization-derived region mask — the complement to
//! [`BackgroundFlatten`](crate::filters::BackgroundFlatten), which only touches
//! the surrounding background.
//!
//! Pipeline (all in Oklab, full-frame):
//! 1. **Quantize** the image to a small OKLab palette (built-in k-means) and
//!    label every pixel by its nearest palette colour.
//! 2. **Connected components** per palette label → flat regions.
//! 3. Per region, the **mean colour** and the **colour variance** (spread). A
//!    low-variance region is a flat fill (flatten it); a high-variance region
//!    is genuinely shaded/detailed (keep it).
//! 4. Each pixel is eased toward its region mean by
//!    `strength × region_flatness × boundary_keep × membership`, where
//!    `region_flatness` → 0 for shaded regions,
//!    `boundary_keep` → 0 within `edge_feather` px of a region boundary (razor
//!    edges preserved), and
//!    `membership` → 0 for pixels far from their region mean (anti-aliased
//!    edges, transitions, strong local shading). Only flat-fill interiors are
//!    collapsed, removing waviness of any frequency without touching edges.
//!
//! Wrap in [`MetricGated`](crate::metric_gate::MetricGated) for a subtlety
//! guarantee, exactly like `BackgroundFlatten`.

use super::background_flatten::chamfer_distance;
use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::{Filter, PlaneSemantics, ResizePhase};
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::prelude::*;

/// Flatten clip-art waviness inside flat colour regions (see module docs).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ClipartFlatten {
    /// Master strength, `0.0`–`1.0`. `0.0` is a no-op.
    pub strength: f32,
    /// Target palette size for the region-mask quantization. Typical `12`–`48`.
    pub palette_size: u32,
    /// Region colour-variance at which a region is considered too
    /// textured/shaded to flatten (a flat fill has low variance; a shaded
    /// region has high). Typical `0.001`–`0.004`.
    pub flatness: f32,
    /// Width, in pixels, of the protected band along region boundaries (keeps
    /// edges and their anti-aliasing crisp). Typical `1.0`–`3.0`.
    pub edge_feather: f32,
    /// Colour distance (Oklab) from a pixel to its region mean beyond which the
    /// pixel is left untouched (anti-aliased edges, transitions, strong local
    /// shading). Typical `0.03`–`0.08`.
    pub color_tolerance: f32,
}

impl Default for ClipartFlatten {
    fn default() -> Self {
        Self {
            strength: 0.8,
            palette_size: 24,
            flatness: 0.0020,
            edge_feather: 1.5,
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

/// Quantize the planes to `k` OKLab colours via deterministic Lloyd k-means on
/// a strided training sample, then label every pixel by nearest centroid.
///
/// Centroids closer than `merge_dist` (Oklab) are merged afterwards, so an
/// image with few distinct colours yields few effective palette entries
/// regardless of `k` — otherwise k-means would split a single flat colour
/// across several centroids and fragment the flat region. Returns
/// `(labels, centroids)` where centroids is the merged palette.
pub(crate) fn quantize_oklab(
    planes: &OklabPlanes,
    k: usize,
    iters: usize,
    merge_dist: f32,
) -> (Vec<u16>, Vec<[f32; 3]>) {
    let n = planes.pixel_count();
    let k = k.clamp(1, 256);

    // Strided training sample (~8k points) for speed.
    let stride = (n / 8192).max(1);
    let mut samples: Vec<[f32; 3]> = Vec::new();
    let mut i = 0;
    while i < n {
        samples.push([planes.l[i], planes.a[i], planes.b[i]]);
        i += stride;
    }
    let k = k.min(samples.len().max(1));

    // Seed centroids at evenly spaced sample positions.
    let mut cent: Vec<[f32; 3]> = (0..k)
        .map(|j| samples[(j * samples.len()) / k.max(1)])
        .collect();

    // Lloyd iterations.
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

    // Merge near-duplicate centroids (greedy single-linkage) so few-colour
    // images don't fragment a flat fill across many centroids.
    let md2 = (merge_dist.max(0.0)) * (merge_dist.max(0.0));
    let mut rep: Vec<usize> = (0..cent.len()).collect();
    for a in 0..cent.len() {
        if rep[a] != a {
            continue;
        }
        for b in (a + 1)..cent.len() {
            if rep[b] == b && dist2(cent[a], cent[b]) < md2 {
                rep[b] = a;
            }
        }
    }
    let mut palette: Vec<[f32; 3]> = Vec::new();
    for a in 0..cent.len() {
        if rep[a] == a {
            palette.push(cent[a]);
        }
    }

    // Assign all pixels to the nearest merged palette colour.
    let mut labels = vec![0u16; n];
    for idx in 0..n {
        labels[idx] = nearest([planes.l[idx], planes.a[idx], planes.b[idx]], &palette) as u16;
    }
    (labels, palette)
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
        self.edge_feather.ceil() as u32 + 2
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
        let w = planes.width as usize;
        let h = planes.height as usize;
        let n = w * h;
        if n == 0 || w < 4 || h < 4 {
            return;
        }
        let strength = self.strength.clamp(0.0, 1.0);
        let flat_v = self.flatness.max(1e-6);
        let ctol = self.color_tolerance.max(1e-4);
        let feather = self.edge_feather.max(0.25);

        // --- quantize + connected components ---
        let (labels, _cent) = quantize_oklab(planes, self.palette_size as usize, 10, ctol);
        let mut rid = vec![0u32; n];
        let num = connected_components(&labels, w, h, &mut rid) as usize;

        // --- per-region mean (pass 1) ---
        let mut sum = vec![[0.0f64; 3]; num];
        let mut cnt = vec![0u32; num];
        for i in 0..n {
            let r = rid[i] as usize;
            sum[r][0] += planes.l[i] as f64;
            sum[r][1] += planes.a[i] as f64;
            sum[r][2] += planes.b[i] as f64;
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

        // --- per-region colour variance (pass 2) → region flatness ---
        let mut var = vec![0.0f64; num];
        for i in 0..n {
            let r = rid[i] as usize;
            var[r] += dist2([planes.l[i], planes.a[i], planes.b[i]], mean[r]) as f64;
        }
        let mut region_flat = vec![0.0f32; num];
        for r in 0..num {
            if cnt[r] > 0 {
                let v = (var[r] / cnt[r] as f64) as f32;
                region_flat[r] = 1.0 - smoothstep(flat_v * 0.3, flat_v, v);
            }
        }

        // --- region-boundary distance → edge protection ---
        // boundary pixels (label changes) are distance sources; interior pixels
        // gain distance, so a thin band along every edge is protected.
        let mut boundary = ctx.take_u8(n);
        boundary.iter_mut().for_each(|b| *b = 1);
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let r = rid[i];
                let edge = (x > 0 && rid[i - 1] != r)
                    || (x + 1 < w && rid[i + 1] != r)
                    || (y > 0 && rid[i - w] != r)
                    || (y + 1 < h && rid[i + w] != r);
                if edge {
                    boundary[i] = 0;
                }
            }
        }
        let mut dist = ctx.take_f32(n);
        chamfer_distance(&boundary, w, h, &mut dist);

        // --- ease flat-fill interiors toward their region mean ---
        for i in 0..n {
            let r = rid[i] as usize;
            let rflat = region_flat[r];
            if rflat <= 1e-4 {
                continue;
            }
            let m = mean[r];
            let orig = [planes.l[i], planes.a[i], planes.b[i]];
            let boundary_keep = smoothstep(0.0, feather, dist[i]);
            let d = dist2(orig, m).sqrt();
            let membership = 1.0 - smoothstep(ctol * 0.5, ctol, d);
            let wgt = strength * rflat * boundary_keep * membership;
            if wgt <= 1e-5 {
                continue;
            }
            planes.l[i] = orig[0] * (1.0 - wgt) + m[0] * wgt;
            planes.a[i] = orig[1] * (1.0 - wgt) + m[1] * wgt;
            planes.b[i] = orig[2] * (1.0 - wgt) + m[2] * wgt;
        }

        ctx.return_f32(dist);
        ctx.return_u8(boundary);
    }
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
            description: "Master effect strength (0 = off)",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.8,
                identity: 0.0,
                step: 0.01,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "palette_size",
            label: "Palette Size",
            description: "Number of colours used to segment flat regions",
            kind: ParamKind::Int {
                min: 4,
                max: 64,
                default: 24,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::NotSlider,
        },
        ParamDesc {
            name: "flatness",
            label: "Flatness",
            description: "Region variance above which a region is too textured/shaded to flatten",
            kind: ParamKind::Float {
                min: 0.0005,
                max: 0.01,
                default: 0.0020,
                identity: 0.01,
                step: 0.0005,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "edge_feather",
            label: "Edge Feather",
            description: "Width of the protected band along region boundaries",
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
            name: "color_tolerance",
            label: "Colour Tolerance",
            description: "Distance from a pixel to its region colour beyond which it is left untouched",
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
            "palette_size" => Some(ParamValue::Int(self.palette_size as i32)),
            "flatness" => Some(ParamValue::Float(self.flatness)),
            "edge_feather" => Some(ParamValue::Float(self.edge_feather)),
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
                    "flatness" => self.flatness = v.clamp(0.0005, 0.02),
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

    /// Two flat colour regions split by a sharp vertical edge, each carrying
    /// deterministic low-amplitude "waviness".
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
        let vb_left = region_l_variance(&p.l, wu, 4, 24, 4, 60);
        ClipartFlatten::default().apply(&mut p, &mut FilterContext::new());
        let va_left = region_l_variance(&p.l, wu, 4, 24, 4, 60);
        assert!(
            va_left < vb_left * 0.5,
            "left-region waviness should be flattened: var {vb_left} -> {va_left}"
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
        // A single region that is a smooth gradient (high variance) must NOT be
        // collapsed to its mean.
        let (w, h) = (48u32, 48u32);
        let wu = w as usize;
        let hu = h as usize;
        let mut p = OklabPlanes::new(w, h);
        for y in 0..hu {
            for x in 0..wu {
                let i = y * wu + x;
                p.l[i] = 0.30 + 0.40 * (x as f32 / (wu as f32 - 1.0)); // strong gradient
                p.a[i] = 0.0;
                p.b[i] = 0.0;
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
