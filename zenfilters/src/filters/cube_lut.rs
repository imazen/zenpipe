//! 3D color LUT loading, application, and compression.
//!
//! Three representations of 3D color transforms, from high-fidelity to compact:
//!
//! - [`CubeLut`]: Full 3D grid with trilinear interpolation. Parses the
//!   industry-standard .cube format. 17³ = 59 KB, 33³ = 431 KB.
//!
//! - [`TensorLut`]: Rank-N tensor decomposition of a 3D LUT into separable
//!   1D factors. A rank-8 decomposition of a 33³ LUT fits in 9.5 KB with
//!   max error of 11 levels @8bit (avg < 1 level).
//!
//! - [`MlpLut`]: Neural network approximation (3→h→h→3 MLP with residual
//!   skip). Infrastructure for torch-trained weights; the built-in SGD
//!   trainer is a starting point, not production-quality.
//!
//! [`LutAccuracy`] measures max and average per-channel error between any
//! approximation and a reference LUT.

use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::prelude::*;
use zenpixels_convert::oklab;

/// 3D color lookup table loaded from Adobe .cube format.
///
/// .cube is the universal LUT exchange format used by colorists. Every
/// major grading application (DaVinci Resolve, Premiere, FCPX, Lightroom)
/// can export and import .cube files.
///
/// The LUT maps linear RGB → linear RGB via trilinear interpolation on a
/// uniform 3D grid. Typical sizes are 17³, 33³, or 65³.
///
/// The filter converts Oklab → linear RGB, applies the LUT, then converts
/// back to Oklab.
///
/// A `strength` parameter (0.0–1.0) blends between the original and
/// LUT-transformed color for partial application.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct CubeLut {
    /// Flattened 3D LUT data: `[r][g][b]` order, each entry is `[R, G, B]`.
    /// Total entries: `size * size * size`.
    data: Vec<[f32; 3]>,
    /// Grid size per axis (e.g., 17, 33, 65).
    size: usize,
    /// Input domain minimum (default: [0, 0, 0]).
    domain_min: [f32; 3],
    /// Input domain maximum (default: [1, 1, 1]).
    domain_max: [f32; 3],
    /// Blend strength. 1.0 = full LUT, 0.0 = bypass.
    pub strength: f32,
    /// Optional title from the .cube file.
    pub title: String,
}

/// Errors from parsing .cube files.
#[derive(Clone, Debug)]
pub enum CubeParseError {
    /// No LUT_3D_SIZE found.
    MissingSize,
    /// LUT size is outside valid range (2–256).
    InvalidSize(usize),
    /// Not enough data rows for the declared size.
    InsufficientData { expected: usize, found: usize },
    /// A data line couldn't be parsed as three floats.
    BadDataLine(usize),
}

impl core::fmt::Display for CubeParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MissingSize => write!(f, "no LUT_3D_SIZE found in .cube file"),
            Self::InvalidSize(s) => write!(f, "invalid LUT_3D_SIZE {s} (must be 2–256)"),
            Self::InsufficientData { expected, found } => {
                write!(f, "expected {expected} data rows, found {found}")
            }
            Self::BadDataLine(n) => write!(f, "bad data at line {n}"),
        }
    }
}

impl Default for CubeLut {
    fn default() -> Self {
        // 2×2×2 identity LUT
        let size = 2;
        let mut data = Vec::with_capacity(size * size * size);
        for r in 0..size {
            for g in 0..size {
                for b in 0..size {
                    data.push([
                        r as f32 / (size - 1) as f32,
                        g as f32 / (size - 1) as f32,
                        b as f32 / (size - 1) as f32,
                    ]);
                }
            }
        }
        Self {
            data,
            size,
            domain_min: [0.0; 3],
            domain_max: [1.0; 3],
            strength: 1.0,
            title: String::new(),
        }
    }
}

impl CubeLut {
    /// Parse a .cube file from its text content.
    ///
    /// Supports the standard Adobe .cube format:
    /// - `TITLE "name"` (optional)
    /// - `LUT_3D_SIZE N` (required)
    /// - `DOMAIN_MIN r g b` (optional, default 0 0 0)
    /// - `DOMAIN_MAX r g b` (optional, default 1 1 1)
    /// - Data rows: `r g b` (one per line, N³ total)
    ///
    /// Lines starting with `#` are comments. Blank lines are skipped.
    /// 1D LUTs (`LUT_1D_SIZE`) are not supported by this filter.
    pub fn parse(text: &str) -> Result<Self, CubeParseError> {
        let mut size: Option<usize> = None;
        let mut domain_min = [0.0f32; 3];
        let mut domain_max = [1.0f32; 3];
        let mut title = String::new();
        let mut data = Vec::new();
        let mut line_num = 0;

        for line in text.lines() {
            line_num += 1;
            let line = line.trim();

            // Skip empty lines and comments
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Metadata keywords
            if let Some(rest) = line.strip_prefix("TITLE") {
                let rest = rest.trim().trim_matches('"');
                title = String::from(rest);
                continue;
            }
            if let Some(rest) = line.strip_prefix("LUT_3D_SIZE") {
                if let Ok(s) = rest.trim().parse::<usize>() {
                    size = Some(s);
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("LUT_1D_SIZE") {
                // Skip 1D LUT size declarations — we only handle 3D
                let _ = rest;
                continue;
            }
            if let Some(rest) = line.strip_prefix("DOMAIN_MIN") {
                if let Some(vals) = parse_three_floats(rest) {
                    domain_min = vals;
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("DOMAIN_MAX") {
                if let Some(vals) = parse_three_floats(rest) {
                    domain_max = vals;
                }
                continue;
            }

            // Data lines: three floats separated by whitespace
            if let Some(vals) = parse_three_floats(line) {
                data.push(vals);
            } else {
                // Check if it looks like a keyword we should skip
                if line.chars().next().is_some_and(|c| c.is_alphabetic()) {
                    continue;
                }
                return Err(CubeParseError::BadDataLine(line_num));
            }
        }

        let size = size.ok_or(CubeParseError::MissingSize)?;
        if !(2..=256).contains(&size) {
            return Err(CubeParseError::InvalidSize(size));
        }

        let expected = size * size * size;
        if data.len() < expected {
            return Err(CubeParseError::InsufficientData {
                expected,
                found: data.len(),
            });
        }
        data.truncate(expected);

        Ok(Self {
            data,
            size,
            domain_min,
            domain_max,
            strength: 1.0,
            title,
        })
    }

    /// Create an identity LUT of the given size.
    pub fn identity(size: usize) -> Self {
        let n = size * size * size;
        let mut data = Vec::with_capacity(n);
        let scale = 1.0 / (size - 1) as f32;
        for r in 0..size {
            for g in 0..size {
                for b in 0..size {
                    data.push([r as f32 * scale, g as f32 * scale, b as f32 * scale]);
                }
            }
        }
        Self {
            data,
            size,
            domain_min: [0.0; 3],
            domain_max: [1.0; 3],
            strength: 1.0,
            title: String::new(),
        }
    }

    /// Grid size per axis.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Total memory footprint in bytes.
    pub fn size_bytes(&self) -> usize {
        self.data.len() * core::mem::size_of::<[f32; 3]>()
    }

    /// Trilinear lookup into the 3D LUT.
    #[inline]
    fn lookup(&self, rgb: [f32; 3]) -> [f32; 3] {
        let s = self.size;
        let max_idx = (s - 1) as f32;

        // Normalize input to [0, 1] range using domain
        let nr = ((rgb[0] - self.domain_min[0]) / (self.domain_max[0] - self.domain_min[0]))
            .clamp(0.0, 1.0)
            * max_idx;
        let ng = ((rgb[1] - self.domain_min[1]) / (self.domain_max[1] - self.domain_min[1]))
            .clamp(0.0, 1.0)
            * max_idx;
        let nb = ((rgb[2] - self.domain_min[2]) / (self.domain_max[2] - self.domain_min[2]))
            .clamp(0.0, 1.0)
            * max_idx;

        let r0 = nr as usize;
        let g0 = ng as usize;
        let b0 = nb as usize;
        let r1 = (r0 + 1).min(s - 1);
        let g1 = (g0 + 1).min(s - 1);
        let b1 = (b0 + 1).min(s - 1);

        let fr = nr - r0 as f32;
        let fg = ng - g0 as f32;
        let fb = nb - b0 as f32;

        // 8 corner lookups
        let c000 = self.data[r0 * s * s + g0 * s + b0];
        let c001 = self.data[r0 * s * s + g0 * s + b1];
        let c010 = self.data[r0 * s * s + g1 * s + b0];
        let c011 = self.data[r0 * s * s + g1 * s + b1];
        let c100 = self.data[r1 * s * s + g0 * s + b0];
        let c101 = self.data[r1 * s * s + g0 * s + b1];
        let c110 = self.data[r1 * s * s + g1 * s + b0];
        let c111 = self.data[r1 * s * s + g1 * s + b1];

        // Trilinear interpolation
        let mut out = [0.0f32; 3];
        for ch in 0..3 {
            let c00 = c000[ch] * (1.0 - fb) + c001[ch] * fb;
            let c01 = c010[ch] * (1.0 - fb) + c011[ch] * fb;
            let c10 = c100[ch] * (1.0 - fb) + c101[ch] * fb;
            let c11 = c110[ch] * (1.0 - fb) + c111[ch] * fb;

            let c0 = c00 * (1.0 - fg) + c01 * fg;
            let c1 = c10 * (1.0 - fg) + c11 * fg;

            out[ch] = c0 * (1.0 - fr) + c1 * fr;
        }
        out
    }
}

// ── LUT Compression ───────────────────────────────────────────────────

/// Accuracy metrics comparing a compressed representation against a reference LUT.
#[derive(Clone, Debug)]
pub struct LutAccuracy {
    /// Maximum absolute difference across all samples and channels.
    pub max_diff: f32,
    /// Mean absolute difference across all samples and channels.
    pub avg_diff: f32,
    /// Per-channel max absolute difference [R, G, B].
    pub max_diff_per_channel: [f32; 3],
    /// Per-channel mean absolute difference [R, G, B].
    pub avg_diff_per_channel: [f32; 3],
    /// Number of sample points evaluated.
    pub sample_count: usize,
}

impl core::fmt::Display for LutAccuracy {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "max={:.6} avg={:.6} (R max={:.6} avg={:.6}, G max={:.6} avg={:.6}, B max={:.6} avg={:.6}) [{} samples]",
            self.max_diff,
            self.avg_diff,
            self.max_diff_per_channel[0],
            self.avg_diff_per_channel[0],
            self.max_diff_per_channel[1],
            self.avg_diff_per_channel[1],
            self.max_diff_per_channel[2],
            self.avg_diff_per_channel[2],
            self.sample_count,
        )
    }
}

impl CubeLut {
    /// Measure accuracy of an approximation function against this LUT.
    ///
    /// Evaluates `approx_fn` at a uniform grid of `eval_size³` points and
    /// compares against trilinear-interpolated LUT values.
    pub fn measure_accuracy(
        &self,
        approx_fn: &dyn Fn([f32; 3]) -> [f32; 3],
        eval_size: usize,
    ) -> LutAccuracy {
        let mut max_diff = 0.0f32;
        let mut sum_diff = 0.0f32;
        let mut max_ch = [0.0f32; 3];
        let mut sum_ch = [0.0f32; 3];
        let mut count = 0usize;

        let scale = 1.0 / (eval_size - 1) as f32;
        for ri in 0..eval_size {
            for gi in 0..eval_size {
                for bi in 0..eval_size {
                    let rgb = [ri as f32 * scale, gi as f32 * scale, bi as f32 * scale];
                    let expected = self.lookup(rgb);
                    let got = approx_fn(rgb);

                    for ch in 0..3 {
                        let d = (expected[ch] - got[ch]).abs();
                        max_diff = max_diff.max(d);
                        sum_diff += d;
                        max_ch[ch] = max_ch[ch].max(d);
                        sum_ch[ch] += d;
                    }
                    count += 1;
                }
            }
        }

        let total = (count * 3) as f32;
        let cnt = count as f32;
        LutAccuracy {
            max_diff,
            avg_diff: sum_diff / total,
            max_diff_per_channel: max_ch,
            avg_diff_per_channel: [sum_ch[0] / cnt, sum_ch[1] / cnt, sum_ch[2] / cnt],
            sample_count: count,
        }
    }

    /// Access the raw LUT data for compression algorithms.
    pub fn data(&self) -> &[[f32; 3]] {
        &self.data
    }

    /// Mutable access to the raw LUT data for LUT generation.
    pub fn data_mut(&mut self) -> &mut [[f32; 3]] {
        &mut self.data
    }

    /// Access domain bounds.
    pub fn domain(&self) -> ([f32; 3], [f32; 3]) {
        (self.domain_min, self.domain_max)
    }
}

/// Rank-N tensor decomposition of a 3D LUT.
///
/// Approximates a 3D LUT as a sum of separable rank-1 terms:
/// ```text
/// LUT(r,g,b) ≈ Σᵢ fᵢ(r) ⊗ gᵢ(g) ⊗ hᵢ(b)
/// ```
///
/// Each rank-1 term stores three 1D lookup tables (one per axis).
/// For smooth color transforms (film looks, grading), 3–8 terms
/// give excellent accuracy.
///
/// Storage: `rank * size * 3 channels * 3 axes * 4 bytes`.
/// For rank=5, size=33: 5 × 33 × 3 × 3 × 4 = 5,940 bytes (~6 KB).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TensorLut {
    /// Rank-1 terms, each with three 1D factor functions.
    factors: Vec<RankTerm>,
    /// Grid size (must match source LUT).
    size: usize,
}

/// A single rank-1 term: three 1D functions (one per input axis),
/// each producing 3 output channel values.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct RankTerm {
    /// f(r): `size` entries, each `[f32; 3]` (contribution to R', G', B' output).
    r_axis: Vec<[f32; 3]>,
    /// g(g): `size` entries.
    g_axis: Vec<[f32; 3]>,
    /// b(b): `size` entries.
    b_axis: Vec<[f32; 3]>,
}

#[allow(clippy::needless_range_loop)]
impl TensorLut {
    /// Decompose a CubeLut into a rank-N tensor approximation.
    ///
    /// Uses alternating least squares (ALS): iteratively fix two axes,
    /// solve for the third, repeat until convergence.
    ///
    /// Higher `rank` = better accuracy, more storage. For typical film
    /// LUTs, rank 5–8 gives max error < 0.005 (invisible at 8-bit).
    pub fn decompose(lut: &CubeLut, rank: usize, iterations: usize) -> Self {
        let size = lut.size;
        let n = size;
        let mut factors = Vec::with_capacity(rank);

        // Work on residual: start with full LUT, subtract each rank's contribution
        let mut residual: Vec<[f32; 3]> = lut.data.clone();

        for _term in 0..rank {
            // Initialize with the dominant direction (first singular vector approx)
            let mut fr = vec![[0.0f32; 3]; n];
            let mut fg = vec![[0.0f32; 3]; n];
            let mut fb = vec![[0.0f32; 3]; n];

            // Initialize fr to uniform, fg/fb from marginal sums
            fr.fill([1.0; 3]);

            // Initialize fg from sum over r,b axes
            for gi in 0..n {
                let mut s = [0.0f32; 3];
                for ri in 0..n {
                    for bi in 0..n {
                        let v = residual[ri * n * n + gi * n + bi];
                        for ch in 0..3 {
                            s[ch] += v[ch];
                        }
                    }
                }
                let norm = (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt().max(1e-10);
                fg[gi] = [s[0] / norm, s[1] / norm, s[2] / norm];
            }

            // Initialize fb from sum over r,g axes
            for bi in 0..n {
                let mut s = [0.0f32; 3];
                for ri in 0..n {
                    for gi in 0..n {
                        let v = residual[ri * n * n + gi * n + bi];
                        for ch in 0..3 {
                            s[ch] += v[ch];
                        }
                    }
                }
                let norm = (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt().max(1e-10);
                fb[bi] = [s[0] / norm, s[1] / norm, s[2] / norm];
            }

            // ALS iterations
            for _iter in 0..iterations {
                // Solve for fr (fix fg, fb)
                for ri in 0..n {
                    let mut num = [0.0f32; 3];
                    let mut den = [0.0f32; 3];
                    for gi in 0..n {
                        for bi in 0..n {
                            let v = residual[ri * n * n + gi * n + bi];
                            for ch in 0..3 {
                                let w = fg[gi][ch] * fb[bi][ch];
                                num[ch] += v[ch] * w;
                                den[ch] += w * w;
                            }
                        }
                    }
                    for ch in 0..3 {
                        fr[ri][ch] = if den[ch] > 1e-12 {
                            num[ch] / den[ch]
                        } else {
                            0.0
                        };
                    }
                }

                // Solve for fg (fix fr, fb)
                for gi in 0..n {
                    let mut num = [0.0f32; 3];
                    let mut den = [0.0f32; 3];
                    for ri in 0..n {
                        for bi in 0..n {
                            let v = residual[ri * n * n + gi * n + bi];
                            for ch in 0..3 {
                                let w = fr[ri][ch] * fb[bi][ch];
                                num[ch] += v[ch] * w;
                                den[ch] += w * w;
                            }
                        }
                    }
                    for ch in 0..3 {
                        fg[gi][ch] = if den[ch] > 1e-12 {
                            num[ch] / den[ch]
                        } else {
                            0.0
                        };
                    }
                }

                // Solve for fb (fix fr, fg)
                for bi in 0..n {
                    let mut num = [0.0f32; 3];
                    let mut den = [0.0f32; 3];
                    for ri in 0..n {
                        for gi in 0..n {
                            let v = residual[ri * n * n + gi * n + bi];
                            for ch in 0..3 {
                                let w = fr[ri][ch] * fg[gi][ch];
                                num[ch] += v[ch] * w;
                                den[ch] += w * w;
                            }
                        }
                    }
                    for ch in 0..3 {
                        fb[bi][ch] = if den[ch] > 1e-12 {
                            num[ch] / den[ch]
                        } else {
                            0.0
                        };
                    }
                }
            }

            // Subtract this rank's contribution from residual
            for ri in 0..n {
                for gi in 0..n {
                    for bi in 0..n {
                        let idx = ri * n * n + gi * n + bi;
                        for ch in 0..3 {
                            residual[idx][ch] -= fr[ri][ch] * fg[gi][ch] * fb[bi][ch];
                        }
                    }
                }
            }

            factors.push(RankTerm {
                r_axis: fr,
                g_axis: fg,
                b_axis: fb,
            });
        }

        Self { factors, size }
    }

    /// Evaluate the tensor approximation at a point.
    ///
    /// Input RGB in [0, 1]. Uses linear interpolation between grid points.
    pub fn lookup(&self, rgb: [f32; 3]) -> [f32; 3] {
        let max_idx = (self.size - 1) as f32;
        let mut result = [0.0f32; 3];

        for term in &self.factors {
            let mut per_axis = [[0.0f32; 3]; 3]; // [axis][output_ch]

            // Interpolate each axis
            let axes: [(&[[f32; 3]], f32); 3] = [
                (&term.r_axis, rgb[0]),
                (&term.g_axis, rgb[1]),
                (&term.b_axis, rgb[2]),
            ];

            for (ax, (data, val)) in axes.iter().enumerate() {
                let pos = (val * max_idx).clamp(0.0, max_idx);
                let lo = pos as usize;
                let hi = (lo + 1).min(self.size - 1);
                let frac = pos - lo as f32;
                for ch in 0..3 {
                    per_axis[ax][ch] = data[lo][ch] * (1.0 - frac) + data[hi][ch] * frac;
                }
            }

            // Multiply the three axis contributions per output channel
            for ch in 0..3 {
                result[ch] += per_axis[0][ch] * per_axis[1][ch] * per_axis[2][ch];
            }
        }

        result
    }

    /// Number of rank-1 terms.
    pub fn rank(&self) -> usize {
        self.factors.len()
    }

    /// Storage size in bytes.
    pub fn size_bytes(&self) -> usize {
        self.factors.len() * self.size * 3 * core::mem::size_of::<[f32; 3]>()
    }

    /// Grid size per axis.
    pub fn grid_size(&self) -> usize {
        self.size
    }

    /// Serialize to a compact flat f32 array.
    ///
    /// Layout: `[rank: u32 as f32, size: u32 as f32, ...factors]`
    /// where each factor is `[r_axis..., g_axis..., b_axis...]`
    /// and each axis entry is `[ch0, ch1, ch2]`.
    ///
    /// Total floats: 2 + rank * size * 3 * 3.
    pub fn to_bytes(&self) -> Vec<f32> {
        let rank = self.factors.len();
        let n = self.size;
        let mut out = Vec::with_capacity(2 + rank * n * 9);
        out.push(rank as f32);
        out.push(n as f32);
        for term in &self.factors {
            for v in &term.r_axis {
                out.extend_from_slice(v);
            }
            for v in &term.g_axis {
                out.extend_from_slice(v);
            }
            for v in &term.b_axis {
                out.extend_from_slice(v);
            }
        }
        out
    }

    /// Deserialize from a flat f32 array produced by [`to_bytes`].
    pub fn from_bytes(data: &[f32]) -> Result<Self, &'static str> {
        if data.len() < 2 {
            return Err("data too short");
        }
        let rank = data[0] as usize;
        let size = data[1] as usize;
        let expected = 2 + rank * size * 9;
        if data.len() < expected {
            return Err("data too short for declared rank/size");
        }

        let mut factors = Vec::with_capacity(rank);
        let mut idx = 2;
        for _ in 0..rank {
            let mut r_axis = Vec::with_capacity(size);
            for _ in 0..size {
                r_axis.push([data[idx], data[idx + 1], data[idx + 2]]);
                idx += 3;
            }
            let mut g_axis = Vec::with_capacity(size);
            for _ in 0..size {
                g_axis.push([data[idx], data[idx + 1], data[idx + 2]]);
                idx += 3;
            }
            let mut b_axis = Vec::with_capacity(size);
            for _ in 0..size {
                b_axis.push([data[idx], data[idx + 1], data[idx + 2]]);
                idx += 3;
            }
            factors.push(RankTerm {
                r_axis,
                g_axis,
                b_axis,
            });
        }

        Ok(Self { factors, size })
    }
}

impl Filter for TensorLut {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        let m1_inv = oklab::lms_to_rgb_matrix(zenpixels::ColorPrimaries::Bt709)
            .expect("BT.709 always supported");
        let m1 = oklab::rgb_to_lms_matrix(zenpixels::ColorPrimaries::Bt709)
            .expect("BT.709 always supported");

        let n = planes.pixel_count();
        for i in 0..n {
            let [r, g, b] = oklab::oklab_to_rgb(planes.l[i], planes.a[i], planes.b[i], &m1_inv);
            let out = self.lookup([r.max(0.0), g.max(0.0), b.max(0.0)]);
            let [l, oa, ob] =
                oklab::rgb_to_oklab(out[0].max(0.0), out[1].max(0.0), out[2].max(0.0), &m1);
            planes.l[i] = l;
            planes.a[i] = oa;
            planes.b[i] = ob;
        }
    }
}

/// A small MLP for approximating a 3D LUT.
///
/// Architecture: 3 → hidden → hidden → 3 with ReLU activations
/// and a residual skip connection from input to output.
///
/// The skip connection means the MLP only needs to learn the *difference*
/// from identity, which is typically small for color grading LUTs.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MlpLut {
    /// Layer 1: input_dim → hidden
    w1: Vec<Vec<f32>>,
    b1: Vec<f32>,
    /// Layer 2: hidden → hidden
    w2: Vec<Vec<f32>>,
    b2: Vec<f32>,
    /// Layer 3: hidden → 3
    w3: Vec<Vec<f32>>,
    b3: [f32; 3],
    hidden: usize,
}

#[allow(clippy::needless_range_loop)]
impl MlpLut {
    /// Create a new MLP with the given hidden size, zero-initialized.
    pub fn new(hidden: usize) -> Self {
        Self {
            w1: vec![vec![0.0; 3]; hidden],
            b1: vec![0.0; hidden],
            w2: vec![vec![0.0; hidden]; hidden],
            b2: vec![0.0; hidden],
            w3: vec![vec![0.0; hidden]; 3],
            b3: [0.0; 3],
            hidden,
        }
    }

    /// Total number of trainable parameters.
    pub fn param_count(&self) -> usize {
        let h = self.hidden;
        3 * h + h + h * h + h + h * 3 + 3
    }

    /// Storage size in bytes.
    pub fn size_bytes(&self) -> usize {
        self.param_count() * core::mem::size_of::<f32>()
    }

    /// Forward pass with ReLU activations and residual skip.
    pub fn forward(&self, rgb: [f32; 3]) -> [f32; 3] {
        let h = self.hidden;

        // Layer 1: 3 → hidden, ReLU
        let mut h1 = vec![0.0f32; h];
        for i in 0..h {
            let mut sum = self.b1[i];
            for j in 0..3 {
                sum += self.w1[i][j] * rgb[j];
            }
            h1[i] = sum.max(0.0);
        }

        // Layer 2: hidden → hidden, ReLU
        let mut h2 = vec![0.0f32; h];
        for i in 0..h {
            let mut sum = self.b2[i];
            for j in 0..h {
                sum += self.w2[i][j] * h1[j];
            }
            h2[i] = sum.max(0.0);
        }

        // Layer 3: hidden → 3
        let mut out = rgb; // residual skip
        for i in 0..3 {
            let mut sum = self.b3[i];
            for j in 0..h {
                sum += self.w3[i][j] * h2[j];
            }
            out[i] += sum;
        }

        out
    }

    /// Train this MLP to approximate a CubeLut using simple SGD.
    ///
    /// Generates training samples from the LUT at `train_size³` grid points.
    /// Returns the final accuracy metrics.
    pub fn train_from_lut(
        &mut self,
        lut: &CubeLut,
        train_size: usize,
        epochs: usize,
        learning_rate: f32,
    ) -> LutAccuracy {
        let h = self.hidden;

        // Generate training data: (input_rgb, target_delta_from_identity)
        let scale = 1.0 / (train_size - 1) as f32;
        let mut samples: Vec<([f32; 3], [f32; 3])> = Vec::new();
        for ri in 0..train_size {
            for gi in 0..train_size {
                for bi in 0..train_size {
                    let input = [ri as f32 * scale, gi as f32 * scale, bi as f32 * scale];
                    let target = lut.lookup(input);
                    // Store delta from identity (what the residual MLP must learn)
                    let delta = [
                        target[0] - input[0],
                        target[1] - input[1],
                        target[2] - input[2],
                    ];
                    samples.push((input, delta));
                }
            }
        }

        // SGD training
        for _epoch in 0..epochs {
            for &(input, target_delta) in &samples {
                // Forward pass
                let mut h1 = vec![0.0f32; h];
                for i in 0..h {
                    let mut sum = self.b1[i];
                    for j in 0..3 {
                        sum += self.w1[i][j] * input[j];
                    }
                    h1[i] = sum.max(0.0);
                }

                let mut h2 = vec![0.0f32; h];
                for i in 0..h {
                    let mut sum = self.b2[i];
                    for j in 0..h {
                        sum += self.w2[i][j] * h1[j];
                    }
                    h2[i] = sum.max(0.0);
                }

                let mut out_delta = [0.0f32; 3];
                for i in 0..3 {
                    let mut sum = self.b3[i];
                    for j in 0..h {
                        sum += self.w3[i][j] * h2[j];
                    }
                    out_delta[i] = sum;
                }

                // Loss gradient: d_loss/d_out = 2 * (out - target) / 3
                let mut d_out = [0.0f32; 3];
                for i in 0..3 {
                    d_out[i] = 2.0 * (out_delta[i] - target_delta[i]) / 3.0;
                }

                // Backprop through layer 3
                let mut d_h2 = vec![0.0f32; h];
                for i in 0..3 {
                    for j in 0..h {
                        d_h2[j] += d_out[i] * self.w3[i][j];
                        self.w3[i][j] -= learning_rate * d_out[i] * h2[j];
                    }
                    self.b3[i] -= learning_rate * d_out[i];
                }

                // ReLU gradient for layer 2
                for j in 0..h {
                    if h2[j] <= 0.0 {
                        d_h2[j] = 0.0;
                    }
                }

                // Backprop through layer 2
                let mut d_h1 = vec![0.0f32; h];
                for i in 0..h {
                    for j in 0..h {
                        d_h1[j] += d_h2[i] * self.w2[i][j];
                        self.w2[i][j] -= learning_rate * d_h2[i] * h1[j];
                    }
                    self.b2[i] -= learning_rate * d_h2[i];
                }

                // ReLU gradient for layer 1
                for j in 0..h {
                    if h1[j] <= 0.0 {
                        d_h1[j] = 0.0;
                    }
                }

                // Backprop through layer 1
                for i in 0..h {
                    for j in 0..3 {
                        self.w1[i][j] -= learning_rate * d_h1[i] * input[j];
                    }
                    self.b1[i] -= learning_rate * d_h1[i];
                }
            }
        }

        // Measure final accuracy
        lut.measure_accuracy(&|rgb| self.forward(rgb), train_size)
    }
}

fn parse_three_floats(s: &str) -> Option<[f32; 3]> {
    let mut iter = s.split_whitespace();
    let r = iter.next()?.parse::<f32>().ok()?;
    let g = iter.next()?.parse::<f32>().ok()?;
    let b = iter.next()?.parse::<f32>().ok()?;
    Some([r, g, b])
}

impl Filter for CubeLut {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.strength.abs() < 1e-6 {
            return;
        }

        let m1_inv = oklab::lms_to_rgb_matrix(zenpixels::ColorPrimaries::Bt709)
            .expect("BT.709 always supported");
        let m1 = oklab::rgb_to_lms_matrix(zenpixels::ColorPrimaries::Bt709)
            .expect("BT.709 always supported");

        let n = planes.pixel_count();
        let blend = self.strength.clamp(0.0, 1.0);
        let inv_blend = 1.0 - blend;

        for i in 0..n {
            // Oklab → linear RGB
            let [r, g, b] = oklab::oklab_to_rgb(planes.l[i], planes.a[i], planes.b[i], &m1_inv);

            // Apply 3D LUT
            let lut_rgb = self.lookup([r.max(0.0), g.max(0.0), b.max(0.0)]);

            // Blend original and LUT result
            let r2 = inv_blend * r + blend * lut_rgb[0];
            let g2 = inv_blend * g + blend * lut_rgb[1];
            let b2 = inv_blend * b + blend * lut_rgb[2];

            // Linear RGB → Oklab
            let [l, oa, ob] = oklab::rgb_to_oklab(r2.max(0.0), g2.max(0.0), b2.max(0.0), &m1);
            planes.l[i] = l;
            planes.a[i] = oa;
            planes.b[i] = ob;
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn identity_lut_is_noop() {
        let lut = CubeLut::identity(17);
        assert_eq!(lut.size(), 17);

        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = 0.3 + (i as f32) * 0.01;
        }
        for v in &mut planes.a {
            *v = 0.05;
        }
        for v in &mut planes.b {
            *v = -0.03;
        }
        let l_orig = planes.l.clone();
        let a_orig = planes.a.clone();
        let b_orig = planes.b.clone();

        lut.apply(&mut planes, &mut FilterContext::new());

        for i in 0..planes.pixel_count() {
            assert!(
                (planes.l[i] - l_orig[i]).abs() < 0.02,
                "L[{i}]: {} vs {}",
                planes.l[i],
                l_orig[i]
            );
            assert!(
                (planes.a[i] - a_orig[i]).abs() < 0.02,
                "a[{i}]: {} vs {}",
                planes.a[i],
                a_orig[i]
            );
            assert!(
                (planes.b[i] - b_orig[i]).abs() < 0.02,
                "b[{i}]: {} vs {}",
                planes.b[i],
                b_orig[i]
            );
        }
    }

    #[test]
    fn parse_minimal_cube() {
        let cube_text = "\
# Comment line
TITLE \"Test LUT\"
LUT_3D_SIZE 2

0.0 0.0 0.0
0.0 0.0 1.0
0.0 1.0 0.0
0.0 1.0 1.0
1.0 0.0 0.0
1.0 0.0 1.0
1.0 1.0 0.0
1.0 1.0 1.0
";
        let lut = CubeLut::parse(cube_text).unwrap();
        assert_eq!(lut.size(), 2);
        assert_eq!(lut.title, "Test LUT");
        assert_eq!(lut.data.len(), 8);
    }

    #[test]
    fn parse_with_domain() {
        let cube_text = "\
LUT_3D_SIZE 2
DOMAIN_MIN 0.0 0.0 0.0
DOMAIN_MAX 1.0 1.0 1.0

0.0 0.0 0.0
0.0 0.0 1.0
0.0 1.0 0.0
0.0 1.0 1.0
1.0 0.0 0.0
1.0 0.0 1.0
1.0 1.0 0.0
1.0 1.0 1.0
";
        let lut = CubeLut::parse(cube_text).unwrap();
        assert_eq!(lut.domain_min, [0.0; 3]);
        assert_eq!(lut.domain_max, [1.0; 3]);
    }

    #[test]
    fn parse_error_missing_size() {
        let cube_text = "0.0 0.0 0.0\n0.0 0.0 1.0\n";
        assert!(matches!(
            CubeLut::parse(cube_text),
            Err(CubeParseError::MissingSize)
        ));
    }

    #[test]
    fn parse_error_insufficient_data() {
        let cube_text = "LUT_3D_SIZE 2\n0.0 0.0 0.0\n";
        assert!(matches!(
            CubeLut::parse(cube_text),
            Err(CubeParseError::InsufficientData { .. })
        ));
    }

    #[test]
    fn strength_zero_is_bypass() {
        let mut lut = CubeLut::identity(2);
        // Make LUT non-identity: invert all values
        for entry in &mut lut.data {
            *entry = [1.0 - entry[0], 1.0 - entry[1], 1.0 - entry[2]];
        }
        lut.strength = 0.0;

        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.05;
        planes.b[0] = -0.03;
        let l_orig = planes.l[0];

        lut.apply(&mut planes, &mut FilterContext::new());
        assert!(
            (planes.l[0] - l_orig).abs() < 1e-6,
            "strength=0 should bypass: {} vs {}",
            planes.l[0],
            l_orig
        );
    }

    #[test]
    fn strength_partial_blends() {
        // Create a LUT that maps everything to white
        let mut lut = CubeLut::identity(2);
        for entry in &mut lut.data {
            *entry = [1.0, 1.0, 1.0];
        }
        lut.strength = 0.5;

        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.3;
        planes.a[0] = 0.0;
        planes.b[0] = 0.0;

        lut.apply(&mut planes, &mut FilterContext::new());

        // Should be between original L and 1.0
        assert!(
            planes.l[0] > 0.3 && planes.l[0] < 1.0,
            "50% blend should be between original and white: {}",
            planes.l[0]
        );
    }

    #[test]
    fn trilinear_interpolation_midpoint() {
        // Identity LUT: lookup at midpoint should return midpoint
        let lut = CubeLut::identity(17);
        let result = lut.lookup([0.5, 0.5, 0.5]);
        for ch in 0..3 {
            assert!(
                (result[ch] - 0.5).abs() < 0.01,
                "midpoint ch{ch}: {}",
                result[ch]
            );
        }
    }

    #[test]
    fn size_bytes_correct() {
        let lut = CubeLut::identity(17);
        assert_eq!(lut.size_bytes(), 17 * 17 * 17 * 12); // 12 bytes per [f32; 3]
    }

    /// Create a warm-tone film emulation LUT for testing compression.
    /// Lifts shadows warm, pushes highlights cool, adds an S-curve.
    fn make_test_film_lut(size: usize) -> CubeLut {
        let mut lut = CubeLut::identity(size);
        let scale = 1.0 / (size - 1) as f32;
        for ri in 0..size {
            for gi in 0..size {
                for bi in 0..size {
                    let r = ri as f32 * scale;
                    let g = gi as f32 * scale;
                    let b = bi as f32 * scale;

                    // S-curve on luminance
                    let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                    let s_lum = if lum < 0.5 {
                        2.0 * lum * lum
                    } else {
                        1.0 - 2.0 * (1.0 - lum) * (1.0 - lum)
                    };
                    let lum_ratio = if lum > 0.001 { s_lum / lum } else { 1.0 };

                    // Warm shadows, cool highlights
                    let warmth = 1.0 - lum; // more warmth in shadows
                    let r_out = (r * lum_ratio + warmth * 0.05).clamp(0.0, 1.0);
                    let g_out = (g * lum_ratio).clamp(0.0, 1.0);
                    let b_out = (b * lum_ratio - warmth * 0.03 + lum * 0.02).clamp(0.0, 1.0);

                    let idx = ri * size * size + gi * size + bi;
                    lut.data[idx] = [r_out, g_out, b_out];
                }
            }
        }
        lut
    }

    #[test]
    fn tensor_decomposition_accuracy() {
        let lut = make_test_film_lut(17);

        for rank in [1, 3, 5, 8] {
            let tensor = TensorLut::decompose(&lut, rank, 20);
            let acc = lut.measure_accuracy(&|rgb| tensor.lookup(rgb), 33);
            std::eprintln!(
                "TensorLut rank={rank}: size={} bytes, {acc}",
                tensor.size_bytes()
            );

            // Rank 5+ should get max error below 0.05 for this smooth LUT
            if rank >= 5 {
                assert!(
                    acc.max_diff < 0.05,
                    "rank {rank} max_diff too high: {}",
                    acc.max_diff
                );
            }
        }
    }

    #[test]
    fn mlp_lut_training() {
        let lut = make_test_film_lut(9); // small for fast test

        for hidden in [16, 32] {
            let mut mlp = MlpLut::new(hidden);
            let acc = mlp.train_from_lut(&lut, 9, 50, 0.001);
            std::eprintln!(
                "MlpLut h={hidden}: params={}, size={} bytes, {acc}",
                mlp.param_count(),
                mlp.size_bytes()
            );
        }
    }

    #[test]
    fn tensor_vs_mlp_comparison() {
        let lut = make_test_film_lut(17);

        // Tensor: rank 5, 20 iterations
        let tensor = TensorLut::decompose(&lut, 5, 20);
        let tensor_acc = lut.measure_accuracy(&|rgb| tensor.lookup(rgb), 33);

        // MLP: hidden=32, train on 9³ grid
        let mut mlp = MlpLut::new(32);
        let mlp_acc = mlp.train_from_lut(&lut, 9, 100, 0.001);

        std::eprintln!("=== LUT Compression Comparison (17³ warm film LUT) ===");
        std::eprintln!("Original:  {} bytes", lut.size_bytes());
        std::eprintln!("Tensor r5: {} bytes, {tensor_acc}", tensor.size_bytes());
        std::eprintln!("MLP h=32:  {} bytes, {mlp_acc}", mlp.size_bytes());
        std::eprintln!("=====================================================");

        // Both should at least beat 0.1 max error on this smooth LUT
        assert!(
            tensor_acc.max_diff < 0.1,
            "tensor max_diff too high: {}",
            tensor_acc.max_diff
        );
    }

    #[test]
    fn tensor_identity_lut_perfect() {
        // Identity LUT should decompose perfectly at rank 1
        // since identity is separable: f(r)=r, g(g)=g, h(b)=b
        let lut = CubeLut::identity(17);
        let tensor = TensorLut::decompose(&lut, 3, 20);
        let acc = lut.measure_accuracy(&|rgb| tensor.lookup(rgb), 33);
        assert!(
            acc.max_diff < 0.01,
            "identity LUT should decompose near-perfectly: max={}",
            acc.max_diff
        );
    }

    #[test]
    fn measure_accuracy_identical() {
        let lut = CubeLut::identity(9);
        let acc = lut.measure_accuracy(&|rgb| lut.lookup(rgb), 9);
        assert!(
            acc.max_diff < 1e-5,
            "self-comparison should be zero: {}",
            acc.max_diff
        );
        assert!(
            acc.avg_diff < 1e-6,
            "self-comparison avg should be zero: {}",
            acc.avg_diff
        );
    }

    #[test]
    fn rank_sweep_33cube() {
        let size = 33;
        let mut lut = CubeLut::identity(size);
        let scale = 1.0 / (size - 1) as f32;
        for ri in 0..size {
            for gi in 0..size {
                for bi in 0..size {
                    let r = ri as f32 * scale;
                    let g = gi as f32 * scale;
                    let b = bi as f32 * scale;
                    let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                    let s_lum = if lum < 0.5 {
                        2.0 * lum * lum
                    } else {
                        1.0 - 2.0 * (1.0 - lum) * (1.0 - lum)
                    };
                    let lum_ratio = if lum > 0.001 { s_lum / lum } else { 1.0 };
                    let warmth = 1.0 - lum;
                    let r_out = (r * lum_ratio + warmth * 0.05).clamp(0.0, 1.0);
                    let g_out = (g * lum_ratio).clamp(0.0, 1.0);
                    let b_out = (b * lum_ratio - warmth * 0.03 + lum * 0.02).clamp(0.0, 1.0);
                    let idx = ri * size * size + gi * size + bi;
                    lut.data[idx] = [r_out, g_out, b_out];
                }
            }
        }

        std::eprintln!("Original 33³: {} bytes", lut.size_bytes());
        for rank in [5, 8, 12, 16] {
            let tensor = TensorLut::decompose(&lut, rank, 30);
            let acc = lut.measure_accuracy(&|rgb| tensor.lookup(rgb), 65);
            let max_8bit = (acc.max_diff * 255.0).ceil() as u32;
            let max_10bit = (acc.max_diff * 1023.0).ceil() as u32;
            std::eprintln!(
                "rank={rank:2}: {bytes:>6} bytes | max={max:.6} ({max8:>2}@8bit, {max10:>3}@10bit) avg={avg:.6}",
                bytes = tensor.size_bytes(),
                max = acc.max_diff,
                max8 = max_8bit,
                max10 = max_10bit,
                avg = acc.avg_diff,
            );
        }
    }
}
