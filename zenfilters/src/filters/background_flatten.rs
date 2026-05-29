//! Conservative white-background flattening for e-commerce product photos.
//!
//! [`BackgroundFlatten`] pushes a noisy / slightly-uneven near-white studio
//! background toward pure white **gently and only from the edges inward**,
//! while preserving the product and its contact shadows. It is built to run
//! fully automatically, so every stage is conservative by construction:
//!
//! 1. **Border estimate + applicability gate.** The outer border band is
//!    sampled to estimate the background's lightness, neutrality, and noise
//!    spread. If the border does not look like a bright, neutral, uniform
//!    studio background, the filter scales itself down or skips entirely
//!    (see [`BackgroundFlatten::auto_skip`]). On a normal photo this is a
//!    near-no-op.
//! 2. **Background-likeness score.** Each pixel gets a soft `[0,1]` score for
//!    "looks like the background" — bright and neutral.
//! 3. **Edge-seeded flood fill ("smart lasso").** A connected region is grown
//!    from the image border through background-like pixels. Only pixels
//!    reachable from the edge are treated as background, so a bright spot
//!    *inside* the product (not connected to the border) is never touched.
//! 4. **Feathered alpha.** A distance transform from the product silhouette
//!    feathers the effect to zero as it approaches the product, so the
//!    transition is invisible and contact shadows near the product are safe.
//! 5. **Shadow-preserving soft-knee whitening.** Background pixels are eased
//!    toward pure white with a smooth knee anchored just below the background
//!    noise floor, so true shadows (darker than the floor by
//!    [`shadow_protection`](BackgroundFlatten::shadow_protection)) are left
//!    untouched. A [`max_lift`](BackgroundFlatten::max_lift) cap keeps the
//!    change gentle.
//!
//! Later stages (gradient-background surface fit, chroma neutralization,
//! halo/fringe removal) build on this foundation.
//!
//! All math is in Oklab: `L` is perceptual lightness in `[0,1]` (pure white
//! ≈ `1.0`), and chroma is `sqrt(a² + b²)` (neutral ≈ `0`).

use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::{Filter, PlaneSemantics, ResizePhase};
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::prelude::*;

/// Conservative, automated white-background flattening (see module docs).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct BackgroundFlatten {
    /// Master strength, `0.0`–`1.0`. `0.0` is a no-op.
    pub strength: f32,
    /// Border band sampled for background estimation, as a fraction of the
    /// shorter image dimension. Typical `0.02`–`0.08`.
    pub border_frac: f32,
    /// Minimum border lightness (Oklab `L`) for the image to be treated as a
    /// white-background shot. Used by the applicability gate and as a cap on
    /// how dark a pixel may be and still count as background. Typical `0.78`–`0.9`.
    pub min_white: f32,
    /// Maximum Oklab chroma a pixel may have and still count as background.
    /// Typical `0.03`–`0.08`.
    pub chroma_tolerance: f32,
    /// Feather distance, in pixels, over which the effect ramps from zero at
    /// the product silhouette to full strength deeper into the background.
    pub feather: f32,
    /// Lightness margin (Oklab `L`) below the background noise floor that is
    /// fully protected. Pixels darker than `floor - shadow_protection` (true
    /// shadows) are never whitened. Typical `0.06`–`0.18`.
    pub shadow_protection: f32,
    /// Hard cap on how much a single pixel's `L` may be raised, in Oklab `L`
    /// units. Keeps the flattening gentle and invisible. Typical `0.1`–`0.3`.
    pub max_lift: f32,
    /// When `true`, automatically reduce strength or skip when the border does
    /// not look like a bright neutral studio background.
    pub auto_skip: bool,
    /// When `true`, fit a smooth low-order surface to the background so a
    /// gradient/uneven illumination (e.g. lighting falloff) flattens uniformly
    /// to white. When `false`, a single constant background level is used.
    pub flatten_gradient: bool,
}

impl Default for BackgroundFlatten {
    fn default() -> Self {
        Self {
            strength: 1.0,
            border_frac: 0.04,
            min_white: 0.80,
            chroma_tolerance: 0.05,
            feather: 12.0,
            shadow_protection: 0.10,
            max_lift: 0.20,
            auto_skip: true,
            flatten_gradient: true,
        }
    }
}

/// Smoothstep with a degenerate-edge guard. Supports descending ramps
/// (`edge0 > edge1`), matching the convention used across this crate.
#[inline]
pub(crate) fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let denom = edge1 - edge0;
    if denom.abs() < 1e-12 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / denom).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Robust statistics of the border band, used to decide whether (and how
/// strongly) to act, and to anchor the whitening knee.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BorderEstimate {
    /// 10th-percentile border lightness (background noise floor).
    pub l_floor: f32,
    /// Overall applicability in `[0,1]`: bright × neutral × uniform.
    pub applicability: f32,
}

/// Percentile (`q` in `[0,1]`) of a scratch slice via partial sort.
#[inline]
fn percentile(vals: &mut [f32], q: f32) -> f32 {
    if vals.is_empty() {
        return 0.0;
    }
    let k = ((vals.len() - 1) as f32 * q.clamp(0.0, 1.0)).round() as usize;
    let k = k.min(vals.len() - 1);
    vals.select_nth_unstable_by(k, |a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    vals[k]
}

/// Estimate background statistics from the border band.
pub(crate) fn estimate_border_background(
    planes: &OklabPlanes,
    border_frac: f32,
    min_white: f32,
) -> BorderEstimate {
    let w = planes.width as usize;
    let h = planes.height as usize;
    let thickness = ((border_frac.clamp(0.005, 0.45) * (w.min(h) as f32)).round() as usize).max(1);

    let mut ls: Vec<f32> = Vec::new();
    let mut cs: Vec<f32> = Vec::new();
    let is_border = |x: usize, y: usize| -> bool {
        x < thickness || x >= w.saturating_sub(thickness) || y < thickness
            || y >= h.saturating_sub(thickness)
    };
    for y in 0..h {
        let row = y * w;
        for x in 0..w {
            if !is_border(x, y) {
                continue;
            }
            let i = row + x;
            let l = planes.l[i];
            let a = planes.a[i];
            let b = planes.b[i];
            ls.push(l);
            cs.push((a * a + b * b).sqrt());
        }
    }

    if ls.is_empty() {
        return BorderEstimate {
            l_floor: 1.0,
            applicability: 0.0,
        };
    }

    let l_p50 = percentile(&mut ls, 0.50);
    let l_p10 = percentile(&mut ls, 0.10);
    let l_p90 = percentile(&mut ls, 0.90);
    let c_p50 = percentile(&mut cs, 0.50);

    // Applicability: the border must be bright, neutral, and reasonably
    // uniform to be a studio white background.
    let bright = smoothstep(min_white - 0.10, min_white, l_p50);
    let neutral = 1.0 - smoothstep(0.06, 0.12, c_p50);
    let uniform = 1.0 - smoothstep(0.18, 0.42, l_p90 - l_p10);
    let applicability = (bright * neutral * uniform).clamp(0.0, 1.0);

    BorderEstimate {
        l_floor: l_p10,
        applicability,
    }
}

/// Per-pixel "looks like the background" score in `[0,1]`: bright + neutral.
pub(crate) fn compute_bg_likeness(
    planes: &OklabPlanes,
    est: &BorderEstimate,
    params: &BackgroundFlatten,
    out: &mut [f32],
) {
    let n = planes.pixel_count();
    // Adaptive brightness floor: follow the measured background floor, but
    // never demand more than `min_white`.
    let eff_floor = (est.l_floor - 0.06).clamp(0.5, 0.97).min(params.min_white);
    let ct = params.chroma_tolerance.max(1e-4);
    for i in 0..n {
        let l = planes.l[i];
        let chroma = (planes.a[i] * planes.a[i] + planes.b[i] * planes.b[i]).sqrt();
        let l_w = smoothstep(eff_floor - 0.10, eff_floor, l);
        let c_w = 1.0 - smoothstep(ct, 2.0 * ct, chroma);
        out[i] = l_w * c_w;
    }
}

/// Flood fill from the image border through background-like pixels.
///
/// Seeds from every border pixel whose likeness ≥ `seed_thresh`, then grows
/// (4-connected) into neighbours whose likeness ≥ `keep_thresh`. Output is
/// `1` for connected-background pixels, `0` otherwise. This guarantees the
/// effect only reaches background that touches the edge — interior bright
/// regions of the product are excluded.
pub(crate) fn flood_fill_border(
    likeness: &[f32],
    w: usize,
    h: usize,
    seed_thresh: f32,
    keep_thresh: f32,
    out: &mut [u8],
) {
    out.iter_mut().for_each(|v| *v = 0);
    if w == 0 || h == 0 {
        return;
    }
    let mut stack: Vec<u32> = Vec::new();
    let push_seed = |x: usize, y: usize, out: &mut [u8], stack: &mut Vec<u32>| {
        let i = y * w + x;
        if out[i] == 0 && likeness[i] >= seed_thresh {
            out[i] = 1;
            stack.push(i as u32);
        }
    };
    for x in 0..w {
        push_seed(x, 0, out, &mut stack);
        push_seed(x, h - 1, out, &mut stack);
    }
    for y in 0..h {
        push_seed(0, y, out, &mut stack);
        push_seed(w - 1, y, out, &mut stack);
    }

    while let Some(idx) = stack.pop() {
        let i = idx as usize;
        let x = i % w;
        let y = i / w;
        let visit = |nx: usize, ny: usize, out: &mut [u8], stack: &mut Vec<u32>| {
            let ni = ny * w + nx;
            if out[ni] == 0 && likeness[ni] >= keep_thresh {
                out[ni] = 1;
                stack.push(ni as u32);
            }
        };
        if x > 0 {
            visit(x - 1, y, out, &mut stack);
        }
        if x + 1 < w {
            visit(x + 1, y, out, &mut stack);
        }
        if y > 0 {
            visit(x, y - 1, out, &mut stack);
        }
        if y + 1 < h {
            visit(x, y + 1, out, &mut stack);
        }
    }
}

/// Two-pass chamfer distance transform.
///
/// Computes, for every pixel, the approximate Euclidean distance (in pixels)
/// to the nearest pixel where `source[i] == 0`. Pixels with `source[i] == 0`
/// get distance `0`. Used here with `source = connected-background` so the
/// distance measures how far each background pixel is from the product
/// silhouette (or any non-background island).
pub(crate) fn chamfer_distance(source: &[u8], w: usize, h: usize, out: &mut [f32]) {
    const BIG: f32 = 1.0e9;
    const D1: f32 = 1.0;
    const D2: f32 = core::f32::consts::SQRT_2;

    for i in 0..w * h {
        out[i] = if source[i] == 0 { 0.0 } else { BIG };
    }
    if w == 0 || h == 0 {
        return;
    }

    // Forward pass: top-left → bottom-right.
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if out[i] == 0.0 {
                continue;
            }
            let mut best = out[i];
            if x > 0 {
                best = best.min(out[i - 1] + D1);
            }
            if y > 0 {
                best = best.min(out[i - w] + D1);
                if x > 0 {
                    best = best.min(out[i - w - 1] + D2);
                }
                if x + 1 < w {
                    best = best.min(out[i - w + 1] + D2);
                }
            }
            out[i] = best;
        }
    }
    // Backward pass: bottom-right → top-left.
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            if out[i] == 0.0 {
                continue;
            }
            let mut best = out[i];
            if x + 1 < w {
                best = best.min(out[i + 1] + D1);
            }
            if y + 1 < h {
                best = best.min(out[i + w] + D1);
                if x + 1 < w {
                    best = best.min(out[i + w + 1] + D2);
                }
                if x > 0 {
                    best = best.min(out[i + w - 1] + D2);
                }
            }
            out[i] = best;
        }
    }
}

/// A smooth low-order model of the background lightness `B(x, y)`.
///
/// Either constant (`nterms == 1`, weighted mean) or a full quadric
/// (`nterms == 6`, basis `[1, nx, ny, nx², nx·ny, ny²]` over normalized
/// coordinates `nx = x/(w-1)`, `ny = y/(h-1)`). Used to anchor the whitening
/// knee per-pixel so a gradient background flattens uniformly.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BackgroundSurface {
    coef: [f64; 6],
    nterms: usize,
    inv_w: f32,
    inv_h: f32,
}

impl BackgroundSurface {
    #[inline]
    fn basis(nx: f32, ny: f32) -> [f64; 6] {
        [
            1.0,
            nx as f64,
            ny as f64,
            (nx * nx) as f64,
            (nx * ny) as f64,
            (ny * ny) as f64,
        ]
    }

    /// Evaluate `B(x, y)`.
    #[inline]
    pub(crate) fn eval(&self, x: usize, y: usize) -> f32 {
        let nx = x as f32 * self.inv_w;
        let ny = y as f32 * self.inv_h;
        let phi = Self::basis(nx, ny);
        let mut s = 0.0f64;
        for k in 0..self.nterms {
            s += self.coef[k] * phi[k];
        }
        s as f32
    }
}

/// Solve an `n×n` linear system `m·x = rhs` via Gaussian elimination with
/// partial pivoting (`n ≤ 6`). Returns `None` if (near-)singular.
fn solve_linear(mut m: [[f64; 6]; 6], mut rhs: [f64; 6], n: usize) -> Option<[f64; 6]> {
    for col in 0..n {
        // Partial pivot.
        let mut piv = col;
        let mut best = m[col][col].abs();
        for r in (col + 1)..n {
            let v = m[r][col].abs();
            if v > best {
                best = v;
                piv = r;
            }
        }
        if best < 1e-12 {
            return None;
        }
        if piv != col {
            m.swap(col, piv);
            rhs.swap(col, piv);
        }
        let inv = 1.0 / m[col][col];
        for r in (col + 1)..n {
            let f = m[r][col] * inv;
            if f != 0.0 {
                for c in col..n {
                    m[r][c] -= f * m[col][c];
                }
                rhs[r] -= f * rhs[col];
            }
        }
    }
    // Back-substitution.
    let mut x = [0.0f64; 6];
    for i in (0..n).rev() {
        let mut s = rhs[i];
        for c in (i + 1)..n {
            s -= m[i][c] * x[c];
        }
        x[i] = s / m[i][i];
    }
    Some(x)
}

/// Fit [`BackgroundSurface`] to the connected-background pixels, weighted by
/// `weight` (typically `likeness × feather`). When `quadric` is false (or the
/// quadric system is singular) the result is the weighted-mean constant level.
pub(crate) fn fit_background_surface(
    planes: &OklabPlanes,
    weight: &[f32],
    w: usize,
    h: usize,
    quadric: bool,
) -> BackgroundSurface {
    let inv_w = 1.0 / ((w.max(2) - 1) as f32);
    let inv_h = 1.0 / ((h.max(2) - 1) as f32);

    // Weighted mean (always available as the constant fallback).
    let mut wsum = 0.0f64;
    let mut lwsum = 0.0f64;
    for y in 0..h {
        let row = y * w;
        for x in 0..w {
            let wt = weight[row + x] as f64;
            if wt > 0.0 {
                wsum += wt;
                lwsum += wt * planes.l[row + x] as f64;
            }
        }
    }
    let mean = if wsum > 1e-9 { lwsum / wsum } else { 1.0 };
    let constant = BackgroundSurface {
        coef: [mean, 0.0, 0.0, 0.0, 0.0, 0.0],
        nterms: 1,
        inv_w,
        inv_h,
    };

    if !quadric || wsum < 1e-6 {
        return constant;
    }

    // Weighted normal equations for the 6-term quadric.
    let n = 6;
    let mut m = [[0.0f64; 6]; 6];
    let mut rhs = [0.0f64; 6];
    for y in 0..h {
        let row = y * w;
        let ny = y as f32 * inv_h;
        for x in 0..w {
            let wt = weight[row + x] as f64;
            if wt <= 0.0 {
                continue;
            }
            let nx = x as f32 * inv_w;
            let phi = BackgroundSurface::basis(nx, ny);
            let l = planes.l[row + x] as f64;
            for i in 0..n {
                rhs[i] += wt * phi[i] * l;
                for j in 0..n {
                    m[i][j] += wt * phi[i] * phi[j];
                }
            }
        }
    }

    match solve_linear(m, rhs, n) {
        Some(coef) => BackgroundSurface {
            coef,
            nterms: n,
            inv_w,
            inv_h,
        },
        None => constant,
    }
}

impl Filter for BackgroundFlatten {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn is_neighborhood(&self) -> bool {
        // Needs the whole frame: border seeding, flood fill, distance transform.
        true
    }

    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        self.feather.ceil() as u32
    }

    fn plane_semantics(&self) -> PlaneSemantics {
        PlaneSemantics::Oklab
    }

    fn resize_phase(&self) -> ResizePhase {
        // Operate at full resolution so the background noise is real.
        ResizePhase::PreResize
    }

    fn scale_for_resolution(&mut self, scale: f32) {
        self.feather = (self.feather * scale).max(0.5);
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

        // Step 0: border estimate + applicability gate.
        let est = estimate_border_background(planes, self.border_frac, self.min_white);
        let applic = if self.auto_skip { est.applicability } else { 1.0 };
        if applic <= 1e-4 {
            return;
        }
        let global = (self.strength * applic).clamp(0.0, 1.0);

        // Step 1: background-likeness.
        let mut likeness = ctx.take_f32(n);
        compute_bg_likeness(planes, &est, self, &mut likeness);

        // Step 2: edge-seeded flood fill.
        let mut connected = ctx.take_u8(n);
        flood_fill_border(&likeness, w, h, 0.5, 0.35, &mut connected);

        // Step 3: distance transform from the silhouette → feather.
        let mut dist = ctx.take_f32(n);
        chamfer_distance(&connected, w, h, &mut dist);

        // Step 4: per-pixel background weight = connectivity × likeness × feather.
        // The feather ramps the effect to zero at the product silhouette.
        let feather = self.feather.max(0.5);
        let max_lift = self.max_lift.max(0.0);
        let sp = self.shadow_protection.max(0.0);
        let mut weight = ctx.take_f32(n);
        for i in 0..n {
            weight[i] = if connected[i] == 0 {
                0.0
            } else {
                likeness[i] * smoothstep(0.0, feather, dist[i])
            };
        }

        // Step 5: fit the (optionally gradient) background surface so the
        // whitening knee tracks uneven illumination, then ease background
        // pixels toward pure white with a shadow-preserving soft knee.
        let surface = fit_background_surface(planes, &weight, w, h, self.flatten_gradient);

        for y in 0..h {
            let row = y * w;
            for x in 0..w {
                let i = row + x;
                let alpha = weight[i] * global;
                if alpha <= 1e-5 {
                    continue;
                }
                let l = planes.l[i];
                // Knee anchored on the local background level `bx`: pixels at or
                // above `bx` whiten fully; pixels darker than `bx - shadow_protection`
                // (true shadows) are untouched.
                let bx = surface.eval(x, y);
                let t = smoothstep(bx - sp, bx, l);
                let mut delta = (1.0 - l) * t * alpha;
                delta = delta.clamp(-max_lift, max_lift);
                planes.l[i] = (l + delta).clamp(0.0, 1.5);
            }
        }

        ctx.return_f32(weight);
        ctx.return_f32(dist);
        ctx.return_u8(connected);
        ctx.return_f32(likeness);
    }
}

static BACKGROUND_FLATTEN_SCHEMA: FilterSchema = FilterSchema {
    name: "background_flatten",
    label: "Background Flatten",
    description: "Gently flatten a noisy near-white product-photo background to pure white, edge-in, preserving the product and its shadows",
    group: FilterGroup::Effects,
    params: &[
        ParamDesc {
            name: "strength",
            label: "Strength",
            description: "Master effect strength (0 = off)",
            kind: ParamKind::Float { min: 0.0, max: 1.0, default: 1.0, identity: 0.0, step: 0.01 },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "min_white",
            label: "Min White",
            description: "Minimum border lightness to treat the image as a white-background shot",
            kind: ParamKind::Float { min: 0.5, max: 0.99, default: 0.80, identity: 0.80, step: 0.01 },
            unit: "",
            section: "Detection",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "chroma_tolerance",
            label: "Chroma Tolerance",
            description: "Max chroma a pixel may have and still count as background",
            kind: ParamKind::Float { min: 0.01, max: 0.15, default: 0.05, identity: 0.05, step: 0.005 },
            unit: "",
            section: "Detection",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "feather",
            label: "Feather",
            description: "Edge-in feather distance",
            kind: ParamKind::Float { min: 0.0, max: 128.0, default: 12.0, identity: 0.0, step: 1.0 },
            unit: "px",
            section: "Main",
            slider: SliderMapping::SquareFromSlider,
        },
        ParamDesc {
            name: "shadow_protection",
            label: "Shadow Protection",
            description: "Lightness margin below the background floor that is never whitened",
            kind: ParamKind::Float { min: 0.0, max: 0.4, default: 0.10, identity: 0.4, step: 0.01 },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "max_lift",
            label: "Max Lift",
            description: "Hard cap on how much a pixel's lightness may be raised",
            kind: ParamKind::Float { min: 0.0, max: 1.0, default: 0.20, identity: 0.0, step: 0.01 },
            unit: "",
            section: "Advanced",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "border_frac",
            label: "Border Sample",
            description: "Border band fraction sampled for background estimation",
            kind: ParamKind::Float { min: 0.01, max: 0.2, default: 0.04, identity: 0.04, step: 0.005 },
            unit: "",
            section: "Detection",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "flatten_gradient",
            label: "Flatten Gradient",
            description: "Fit a smooth surface so an uneven/gradient background flattens uniformly",
            kind: ParamKind::Bool { default: true },
            unit: "",
            section: "Advanced",
            slider: SliderMapping::NotSlider,
        },
        ParamDesc {
            name: "auto_skip",
            label: "Auto Skip",
            description: "Reduce or skip automatically when the image is not a white-background shot",
            kind: ParamKind::Bool { default: true },
            unit: "",
            section: "Detection",
            slider: SliderMapping::NotSlider,
        },
    ],
};

impl Describe for BackgroundFlatten {
    fn schema() -> &'static FilterSchema {
        &BACKGROUND_FLATTEN_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "strength" => Some(ParamValue::Float(self.strength)),
            "min_white" => Some(ParamValue::Float(self.min_white)),
            "chroma_tolerance" => Some(ParamValue::Float(self.chroma_tolerance)),
            "feather" => Some(ParamValue::Float(self.feather)),
            "shadow_protection" => Some(ParamValue::Float(self.shadow_protection)),
            "max_lift" => Some(ParamValue::Float(self.max_lift)),
            "border_frac" => Some(ParamValue::Float(self.border_frac)),
            "auto_skip" => Some(ParamValue::Bool(self.auto_skip)),
            "flatten_gradient" => Some(ParamValue::Bool(self.flatten_gradient)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        match name {
            "auto_skip" | "flatten_gradient" => {
                if let ParamValue::Bool(b) = value {
                    match name {
                        "auto_skip" => self.auto_skip = b,
                        _ => self.flatten_gradient = b,
                    }
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
                    "min_white" => self.min_white = v.clamp(0.5, 0.99),
                    "chroma_tolerance" => self.chroma_tolerance = v.clamp(0.001, 0.2),
                    "feather" => self.feather = v.max(0.0),
                    "shadow_protection" => self.shadow_protection = v.clamp(0.0, 0.5),
                    "max_lift" => self.max_lift = v.clamp(0.0, 1.0),
                    "border_frac" => self.border_frac = v.clamp(0.005, 0.45),
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

    /// Build planes: near-white noisy background with a dark product square in
    /// the center and a soft contact-shadow band just below it.
    fn product_on_white(w: u32, h: u32) -> OklabPlanes {
        let mut p = OklabPlanes::new(w, h);
        let wu = w as usize;
        let hu = h as usize;
        for y in 0..hu {
            for x in 0..wu {
                let i = y * wu + x;
                // Deterministic pseudo-noise around 0.95.
                let noise =
                    (((x * 31 + y * 17) % 13) as f32 / 13.0 - 0.5) * 0.03;
                p.l[i] = 0.95 + noise;
                p.a[i] = 0.0;
                p.b[i] = 0.0;
            }
        }
        // Dark product square in the middle third.
        let x0 = wu / 3;
        let x1 = 2 * wu / 3;
        let y0 = hu / 3;
        let y1 = 2 * hu / 3;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = y * wu + x;
                p.l[i] = 0.25;
                p.a[i] = 0.04;
                p.b[i] = -0.02;
            }
        }
        // Soft contact shadow: a band just below the product (darker than the
        // background by more than shadow_protection).
        for y in y1..(y1 + hu / 12).min(hu) {
            for x in x0..x1 {
                let i = y * wu + x;
                p.l[i] = 0.70;
            }
        }
        p
    }

    fn mean(slice: &[f32]) -> f32 {
        slice.iter().sum::<f32>() / slice.len() as f32
    }

    #[test]
    fn zero_strength_is_identity() {
        let mut p = product_on_white(64, 64);
        let orig = p.l.clone();
        BackgroundFlatten {
            strength: 0.0,
            ..Default::default()
        }
        .apply(&mut p, &mut FilterContext::new());
        assert_eq!(p.l, orig);
    }

    #[test]
    fn whitens_background_noise() {
        let (w, h) = (96u32, 96u32);
        let mut p = product_on_white(w, h);
        let orig = p.clone();
        BackgroundFlatten::default().apply(&mut p, &mut FilterContext::new());

        // A corner pixel (deep background) should be pushed toward 1.0.
        let corner = p.l[0];
        assert!(
            corner > orig.l[0] && corner > 0.985,
            "corner background should approach white: {} -> {}",
            orig.l[0],
            corner
        );

        // Background variance should drop (noise flattened). Sample top strip.
        let strip = (w as usize) * 4;
        let var_before: f32 = {
            let m = mean(&orig.l[..strip]);
            orig.l[..strip].iter().map(|v| (v - m) * (v - m)).sum::<f32>() / strip as f32
        };
        let var_after: f32 = {
            let m = mean(&p.l[..strip]);
            p.l[..strip].iter().map(|v| (v - m) * (v - m)).sum::<f32>() / strip as f32
        };
        assert!(
            var_after < var_before,
            "background noise variance should drop: {var_before} -> {var_after}"
        );
    }

    #[test]
    fn preserves_product_and_shadow() {
        let (w, h) = (96u32, 96u32);
        let mut p = product_on_white(w, h);
        let orig = p.clone();
        BackgroundFlatten::default().apply(&mut p, &mut FilterContext::new());

        let wu = w as usize;
        let hu = h as usize;
        // Product center unchanged.
        let ci = (hu / 2) * wu + wu / 2;
        assert!(
            (p.l[ci] - orig.l[ci]).abs() < 1e-4,
            "product center must be untouched: {} -> {}",
            orig.l[ci],
            p.l[ci]
        );
        // Shadow band (just below product) largely preserved (not whitened).
        let sy = (2 * hu / 3) + hu / 24;
        let si = sy * wu + wu / 2;
        assert!(
            p.l[si] < 0.80,
            "contact shadow must be preserved, got {}",
            p.l[si]
        );
    }

    #[test]
    fn interior_bright_island_not_whitened() {
        // A bright neutral spot fully enclosed by the dark product, not
        // connected to the border, must NOT be whitened.
        let (w, h) = (96u32, 96u32);
        let mut p = product_on_white(w, h);
        let wu = w as usize;
        let hu = h as usize;
        let cx = wu / 2;
        let cy = hu / 2;
        // 4x4 bright island in the product center.
        for y in (cy - 2)..(cy + 2) {
            for x in (cx - 2)..(cx + 2) {
                p.l[y * wu + x] = 0.95;
            }
        }
        let orig = p.clone();
        BackgroundFlatten::default().apply(&mut p, &mut FilterContext::new());
        let ii = cy * wu + cx;
        assert!(
            (p.l[ii] - orig.l[ii]).abs() < 1e-4,
            "interior bright island (not border-connected) must be untouched: {} -> {}",
            orig.l[ii],
            p.l[ii]
        );
    }

    #[test]
    fn near_noop_on_non_white_background() {
        // A mid-gray, colorful image is not a white-background shot; auto_skip
        // should make this a near-no-op.
        let (w, h) = (64u32, 64u32);
        let mut p = OklabPlanes::new(w, h);
        for i in 0..(w * h) as usize {
            p.l[i] = 0.45 + 0.1 * ((i % 7) as f32 / 7.0);
            p.a[i] = 0.10;
            p.b[i] = 0.08;
        }
        let orig = p.clone();
        BackgroundFlatten::default().apply(&mut p, &mut FilterContext::new());
        let mut max_err = 0.0f32;
        for i in 0..p.l.len() {
            max_err = max_err.max((p.l[i] - orig.l[i]).abs());
        }
        assert!(max_err < 1e-4, "non-white-bg image should be a no-op, max_err={max_err}");
    }

    #[test]
    fn flood_fill_respects_connectivity() {
        // 5x5: border all bg-like, center 3x3 hole of non-bg with a bg pixel
        // dead center → center must NOT be connected.
        let (w, h) = (5usize, 5usize);
        let mut like = vec![1.0f32; w * h];
        // carve a ring of non-bg around the center pixel
        for &(x, y) in &[
            (1, 1),
            (2, 1),
            (3, 1),
            (1, 2),
            (3, 2),
            (1, 3),
            (2, 3),
            (3, 3),
        ] {
            like[y * w + x] = 0.0;
        }
        // center stays 1.0 but is enclosed
        let mut out = vec![0u8; w * h];
        flood_fill_border(&like, w, h, 0.5, 0.35, &mut out);
        assert_eq!(out[0], 1, "corner should be connected");
        assert_eq!(out[2 * w + 2], 0, "enclosed center must not be connected");
    }

    #[test]
    fn surface_fit_recovers_plane() {
        let (w, h) = (40usize, 30usize);
        let mut p = OklabPlanes::new(w as u32, h as u32);
        let plane = |x: usize, y: usize| {
            let nx = x as f32 / (w as f32 - 1.0);
            let ny = y as f32 / (h as f32 - 1.0);
            0.80 + 0.15 * nx + 0.04 * ny
        };
        for y in 0..h {
            for x in 0..w {
                p.l[y * w + x] = plane(x, y);
            }
        }
        let weight = vec![1.0f32; w * h];
        let surf = fit_background_surface(&p, &weight, w, h, true);
        for &(x, y) in &[(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1), (w / 2, h / 2)] {
            let got = surf.eval(x, y);
            let expect = plane(x, y);
            assert!(
                (got - expect).abs() < 1e-3,
                "surface at ({x},{y}): {got} vs {expect}"
            );
        }
    }

    /// Vertical gradient background (top darker, bottom brighter) + center
    /// product. With `flatten_gradient` the whole background reaches white;
    /// without it, the darker (top) end is left visibly under-whitened.
    fn gradient_on_white(w: u32, h: u32) -> OklabPlanes {
        let mut p = OklabPlanes::new(w, h);
        let wu = w as usize;
        let hu = h as usize;
        for y in 0..hu {
            for x in 0..wu {
                let i = y * wu + x;
                let g = 0.90 + 0.09 * (y as f32 / (hu as f32 - 1.0));
                let noise = (((x * 7 + y * 13) % 11) as f32 / 11.0 - 0.5) * 0.02;
                p.l[i] = g + noise;
            }
        }
        let (x0, x1, y0, y1) = (wu / 3, 2 * wu / 3, hu / 3, 2 * hu / 3);
        for y in y0..y1 {
            for x in x0..x1 {
                p.l[y * wu + x] = 0.25;
                p.a[y * wu + x] = 0.03;
            }
        }
        p
    }

    #[test]
    fn flattens_gradient_uniformly() {
        let (w, h) = (96u32, 96u32);
        let wu = w as usize;
        let top = 4 * wu + 48; // background, above product
        let bot = 92 * wu + 48; // background, below product

        let mut on = gradient_on_white(w, h);
        BackgroundFlatten {
            flatten_gradient: true,
            ..Default::default()
        }
        .apply(&mut on, &mut FilterContext::new());

        let mut off = gradient_on_white(w, h);
        BackgroundFlatten {
            flatten_gradient: false,
            ..Default::default()
        }
        .apply(&mut off, &mut FilterContext::new());

        // With gradient fit, both the dark (top) and bright (bottom) ends of
        // the background reach near-white.
        assert!(
            on.l[top] > 0.985 && on.l[bot] > 0.985,
            "gradient fit should whiten both ends: top={}, bot={}",
            on.l[top],
            on.l[bot]
        );
        // Without it, the constant knee leaves the darker (top) end visibly
        // less white than the gradient-aware result.
        assert!(
            off.l[top] < on.l[top] - 0.02,
            "constant knee should under-whiten the dark end: off_top={}, on_top={}",
            off.l[top],
            on.l[top]
        );
    }

    #[test]
    fn chamfer_distance_basic() {
        // Single source at center; corners should have larger distance.
        let (w, h) = (9usize, 9usize);
        let mut src = vec![1u8; w * h];
        src[4 * w + 4] = 0; // source
        let mut dist = vec![0.0f32; w * h];
        chamfer_distance(&src, w, h, &mut dist);
        assert_eq!(dist[4 * w + 4], 0.0);
        let adjacent = dist[4 * w + 5];
        let corner = dist[0];
        assert!((adjacent - 1.0).abs() < 1e-4, "adjacent dist ~1, got {adjacent}");
        assert!(corner > adjacent, "corner farther than adjacent");
    }
}
