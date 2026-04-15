// TODO: ONNX-based auto-tuning of filter parameters.
//
// Architecture:
//   1. Extract lightweight features from OklabPlanes (histogram, percentiles, stats)
//   2. Run a small ONNX model via `tract-onnx` (pure Rust, no C++ deps)
//   3. Map model output (~15-20 floats) to filter parameter structs
//   4. Apply the tuned pipeline via existing SIMD filters
//
// The model runs once on features (not pixels), so inference is <1ms.
// The heavy lifting stays in the SIMD filter pipeline.
//
// Runtime: tract-onnx (pure Rust ONNX inference, supports no_std+alloc)
//   - Crate: https://crates.io/crates/tract-onnx
//   - Feature-gated: `zenfilters/auto-tune`
//
// Model input features (~160 floats):
//   - L histogram: 64 bins, normalized
//   - a histogram: 32 bins, normalized
//   - b histogram: 32 bins, normalized
//   - L percentiles: p1, p5, p25, p50, p75, p95, p99 (7 floats)
//   - Channel stats: mean_l, std_l, mean_a, std_a, mean_b, std_b (6 floats)
//   - Dynamic range: p99_l - p1_l (1 float)
//   - Color cast: mean_a, mean_b (2 floats, redundant but explicit)
//   - Thumbnail: 16x16 L channel (256 floats, optional spatial awareness)
//
// Model output (~18 floats):
//   - exposure: f32         (FusedAdjust)
//   - contrast: f32         (FusedAdjust)
//   - highlights: f32       (FusedAdjust)
//   - shadows: f32          (FusedAdjust)
//   - saturation: f32       (FusedAdjust)
//   - vibrance: f32         (FusedAdjust)
//   - temperature: f32      (FusedAdjust)
//   - tint: f32             (FusedAdjust)
//   - black_point: f32      (FusedAdjust)
//   - white_point: f32      (FusedAdjust)
//   - sigmoid_contrast: f32 (Sigmoid)
//   - sigmoid_skew: f32     (Sigmoid)
//   - clarity_amount: f32   (Clarity)
//   - sharpen_amount: f32   (AdaptiveSharpen)
//   - highlight_recovery: f32 (HighlightRecovery)
//   - shadow_lift: f32      (ShadowLift)
//   - local_tonemap: f32    (LocalToneMap compression)
//   - gamut_expand: f32     (GamutExpand)
//
// Training data sources:
//   - MIT-Adobe FiveK dataset (5000 images × 5 expert edits)
//   - Self-supervised: random params → quality scorer → optimize
//   - Before/after pairs from professional editing workflows
//
// Fallback: when no model is loaded, use rule-based parameter selection
// from histogram analysis (essentially combining the logic from
// AutoExposure + HighlightRecovery + ShadowLift into a single pass).

use crate::pipeline::{Pipeline, PipelineConfig};
use crate::planes::OklabPlanes;
use crate::prelude::*;

#[cfg(feature = "serde")]
use serde_big_array::BigArray;

use super::{
    AdaptiveSharpen, Clarity, FusedAdjust, GamutExpand, HighlightRecovery, LocalToneMap,
    ShadowLift, Sigmoid,
};

/// Lightweight image features for model inference.
///
/// Extracted from `OklabPlanes` in a single pass over the data.
/// Total size: ~160 floats (640 bytes). Cheap to compute.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ImageFeatures {
    /// L channel histogram, 64 bins, normalized to sum=1.
    #[cfg_attr(feature = "serde", serde(with = "BigArray"))]
    pub l_histogram: [f32; 64],
    /// a channel histogram, 32 bins over [-0.4, 0.4], normalized.
    pub a_histogram: [f32; 32],
    /// b channel histogram, 32 bins over [-0.4, 0.4], normalized.
    pub b_histogram: [f32; 32],
    /// L percentiles: p1, p5, p25, p50, p75, p95, p99.
    pub l_percentiles: [f32; 7],
    /// Channel statistics: [mean_l, std_l, mean_a, std_a, mean_b, std_b].
    pub channel_stats: [f32; 6],
    /// Dynamic range of L (p99 - p1).
    pub dynamic_range: f32,
}

impl ImageFeatures {
    /// Extract features from Oklab planes. Single pass for stats,
    /// second pass for histograms (cache-friendly).
    pub fn extract(planes: &OklabPlanes) -> Self {
        let n = planes.pixel_count();
        let inv_n = 1.0 / n.max(1) as f32;

        // Pass 1: mean
        let (sum_l, sum_a, sum_b) = planes
            .l
            .iter()
            .zip(planes.a.iter())
            .zip(planes.b.iter())
            .fold((0.0f64, 0.0f64, 0.0f64), |(sl, sa, sb), ((&l, &a), &b)| {
                (sl + l as f64, sa + a as f64, sb + b as f64)
            });
        let mean_l = (sum_l / n.max(1) as f64) as f32;
        let mean_a = (sum_a / n.max(1) as f64) as f32;
        let mean_b = (sum_b / n.max(1) as f64) as f32;

        // Pass 2: variance + histograms
        let mut var_l = 0.0f64;
        let mut var_a = 0.0f64;
        let mut var_b = 0.0f64;
        let mut l_hist = [0u32; 64];
        let mut a_hist = [0u32; 32];
        let mut b_hist = [0u32; 32];

        for ((&l, &a), &b) in planes.l.iter().zip(planes.a.iter()).zip(planes.b.iter()) {
            let dl = (l - mean_l) as f64;
            let da = (a - mean_a) as f64;
            let db = (b - mean_b) as f64;
            var_l += dl * dl;
            var_a += da * da;
            var_b += db * db;

            let l_bin = (l.clamp(0.0, 1.0) * 63.0) as usize;
            l_hist[l_bin.min(63)] += 1;

            let a_bin = ((a + 0.4) / 0.8 * 31.0) as usize;
            a_hist[a_bin.min(31)] += 1;

            let b_bin = ((b + 0.4) / 0.8 * 31.0) as usize;
            b_hist[b_bin.min(31)] += 1;
        }

        let std_l = (var_l / n.max(1) as f64).sqrt() as f32;
        let std_a = (var_a / n.max(1) as f64).sqrt() as f32;
        let std_b = (var_b / n.max(1) as f64).sqrt() as f32;

        // Normalize histograms
        let mut l_histogram = [0.0f32; 64];
        for (i, &c) in l_hist.iter().enumerate() {
            l_histogram[i] = c as f32 * inv_n;
        }
        let mut a_histogram = [0.0f32; 32];
        for (i, &c) in a_hist.iter().enumerate() {
            a_histogram[i] = c as f32 * inv_n;
        }
        let mut b_histogram = [0.0f32; 32];
        for (i, &c) in b_hist.iter().enumerate() {
            b_histogram[i] = c as f32 * inv_n;
        }

        // Percentiles from L histogram
        let percentile_targets = [0.01, 0.05, 0.25, 0.50, 0.75, 0.95, 0.99];
        let mut l_percentiles = [0.0f32; 7];
        for (pi, &target) in percentile_targets.iter().enumerate() {
            let mut cumsum = 0.0f32;
            for (bin, &freq) in l_histogram.iter().enumerate() {
                cumsum += freq;
                if cumsum >= target {
                    l_percentiles[pi] = bin as f32 / 63.0;
                    break;
                }
            }
        }

        let dynamic_range = l_percentiles[6] - l_percentiles[0]; // p99 - p1

        ImageFeatures {
            l_histogram,
            a_histogram,
            b_histogram,
            l_percentiles,
            channel_stats: [mean_l, std_l, mean_a, std_a, mean_b, std_b],
            dynamic_range,
        }
    }

    /// Pack features into a flat f32 slice for model input.
    /// Layout: [l_hist(64), a_hist(32), b_hist(32), percentiles(7), stats(6), dr(1)] = 142 floats.
    pub fn to_tensor(&self) -> alloc::vec::Vec<f32> {
        let mut v = alloc::vec::Vec::with_capacity(142);
        v.extend_from_slice(&self.l_histogram);
        v.extend_from_slice(&self.a_histogram);
        v.extend_from_slice(&self.b_histogram);
        v.extend_from_slice(&self.l_percentiles);
        v.extend_from_slice(&self.channel_stats);
        v.push(self.dynamic_range);
        v
    }

    /// Reconstruct `ImageFeatures` from a 142-float tensor (inverse of `to_tensor`).
    ///
    /// Panics if `tensor` is not exactly 142 elements.
    pub fn from_tensor(tensor: &[f32]) -> Self {
        assert_eq!(tensor.len(), 142, "expected 142-float tensor");
        let mut l_histogram = [0.0f32; 64];
        l_histogram.copy_from_slice(&tensor[..64]);
        let mut a_histogram = [0.0f32; 32];
        a_histogram.copy_from_slice(&tensor[64..96]);
        let mut b_histogram = [0.0f32; 32];
        b_histogram.copy_from_slice(&tensor[96..128]);
        let mut l_percentiles = [0.0f32; 7];
        l_percentiles.copy_from_slice(&tensor[128..135]);
        let mut channel_stats = [0.0f32; 6];
        channel_stats.copy_from_slice(&tensor[135..141]);
        Self {
            l_histogram,
            a_histogram,
            b_histogram,
            l_percentiles,
            channel_stats,
            dynamic_range: tensor[141],
        }
    }
}

/// Predicted filter parameters from a model or rule-based system.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TunedParams {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub saturation: f32,
    pub vibrance: f32,
    pub temperature: f32,
    pub tint: f32,
    pub black_point: f32,
    pub white_point: f32,
    pub sigmoid_contrast: f32,
    pub sigmoid_skew: f32,
    pub clarity: f32,
    pub sharpen: f32,
    pub highlight_recovery: f32,
    pub shadow_lift: f32,
    pub local_tonemap: f32,
    pub gamut_expand: f32,
}

impl Default for TunedParams {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            saturation: 1.0,
            vibrance: 0.0,
            temperature: 0.0,
            tint: 0.0,
            black_point: 0.0,
            white_point: 1.0,
            sigmoid_contrast: 1.0,
            sigmoid_skew: 0.5,
            clarity: 0.0,
            sharpen: 0.0,
            highlight_recovery: 0.0,
            shadow_lift: 0.0,
            local_tonemap: 0.0,
            gamut_expand: 0.0,
        }
    }
}

impl TunedParams {
    /// Build a filter pipeline for sRGB/JPEG input.
    ///
    /// Applies artistic adjustments: FusedAdjust, then optional Sigmoid,
    /// HighlightRecovery, ShadowLift, LocalToneMap, Clarity, AdaptiveSharpen,
    /// GamutExpand — each only added if its parameters differ from identity.
    pub fn build_pipeline(&self) -> Pipeline {
        let mut pipeline = Pipeline::new(PipelineConfig::default()).unwrap();
        self.push_artistic_filters(&mut pipeline);
        pipeline
    }

    /// Build a filter pipeline for linear (scene-referred) input.
    ///
    /// Prepends a base Sigmoid tone mapping step to convert scene→display
    /// before applying the same artistic adjustments as `build_pipeline()`.
    ///
    /// `base_contrast` and `base_skew` control the scene-to-display tone curve.
    /// Good defaults: contrast=1.4, skew=0.58 (tuned for darktable parity).
    #[allow(clippy::field_reassign_with_default)]
    pub fn build_pipeline_linear(&self, base_contrast: f32, base_skew: f32) -> Pipeline {
        let mut pipeline = Pipeline::new(PipelineConfig::default()).unwrap();

        // Base tone mapping: scene-referred → display-referred
        // chroma_compression=0.4 emulates RGB-space desaturation in highlights
        let mut base_sig = Sigmoid::default();
        base_sig.contrast = base_contrast;
        base_sig.skew = base_skew;
        base_sig.chroma_compression = 0.4;
        pipeline.push(Box::new(base_sig));

        self.push_artistic_filters(&mut pipeline);
        pipeline
    }

    /// Push artistic adjustment filters into an existing pipeline.
    #[allow(clippy::field_reassign_with_default)]
    fn push_artistic_filters(&self, pipeline: &mut Pipeline) {
        let mut fused = FusedAdjust::new();
        fused.exposure = self.exposure;
        fused.contrast = self.contrast;
        fused.highlights = self.highlights;
        fused.shadows = self.shadows;
        fused.saturation = self.saturation;
        fused.vibrance = self.vibrance;
        fused.temperature = self.temperature;
        fused.tint = self.tint;
        fused.black_point = self.black_point;
        fused.white_point = self.white_point;
        pipeline.push(Box::new(fused));

        if (self.sigmoid_contrast - 1.0).abs() > 0.01 || (self.sigmoid_skew - 0.5).abs() > 0.01 {
            let mut sig = Sigmoid::default();
            sig.contrast = self.sigmoid_contrast;
            sig.skew = self.sigmoid_skew;
            pipeline.push(Box::new(sig));
        }
        if self.highlight_recovery > 0.01 {
            let mut hr = HighlightRecovery::default();
            hr.strength = self.highlight_recovery;
            pipeline.push(Box::new(hr));
        }
        if self.shadow_lift > 0.01 {
            let mut sl = ShadowLift::default();
            sl.strength = self.shadow_lift;
            pipeline.push(Box::new(sl));
        }
        if self.local_tonemap > 0.01 {
            let mut ltm = LocalToneMap::default();
            ltm.compression = self.local_tonemap;
            pipeline.push(Box::new(ltm));
        }
        if self.clarity > 0.01 {
            let mut c = Clarity::default();
            c.amount = self.clarity;
            pipeline.push(Box::new(c));
        }
        if self.sharpen > 0.01 {
            let mut s = AdaptiveSharpen::default();
            s.amount = self.sharpen;
            pipeline.push(Box::new(s));
        }
        if self.gamut_expand > 0.01 {
            let mut ge = GamutExpand::default();
            ge.strength = self.gamut_expand;
            pipeline.push(Box::new(ge));
        }
    }

    /// Pack parameters into a fixed-size array.
    ///
    /// Order: exposure, contrast, highlights, shadows, saturation, vibrance,
    /// temperature, tint, black_point, white_point, sigmoid_contrast,
    /// sigmoid_skew, clarity, sharpen, highlight_recovery, shadow_lift,
    /// local_tonemap, gamut_expand.
    pub fn to_array(&self) -> [f32; LINEAR_MODEL_OUTPUTS] {
        [
            self.exposure,
            self.contrast,
            self.highlights,
            self.shadows,
            self.saturation,
            self.vibrance,
            self.temperature,
            self.tint,
            self.black_point,
            self.white_point,
            self.sigmoid_contrast,
            self.sigmoid_skew,
            self.clarity,
            self.sharpen,
            self.highlight_recovery,
            self.shadow_lift,
            self.local_tonemap,
            self.gamut_expand,
        ]
    }

    /// Reconstruct from a fixed-size array (inverse of `to_array`).
    pub fn from_array(a: &[f32; LINEAR_MODEL_OUTPUTS]) -> Self {
        Self {
            exposure: a[0],
            contrast: a[1],
            highlights: a[2],
            shadows: a[3],
            saturation: a[4],
            vibrance: a[5],
            temperature: a[6],
            tint: a[7],
            black_point: a[8],
            white_point: a[9],
            sigmoid_contrast: a[10],
            sigmoid_skew: a[11],
            clarity: a[12],
            sharpen: a[13],
            highlight_recovery: a[14],
            shadow_lift: a[15],
            local_tonemap: a[16],
            gamut_expand: a[17],
        }
    }
}

// ─── Rule-based tuner ───────────────────────────────────────────────

#[allow(dead_code)]
impl ImageFeatures {
    /// Shorthand accessors for named percentiles.
    fn p1(&self) -> f32 {
        self.l_percentiles[0]
    }
    fn p5(&self) -> f32 {
        self.l_percentiles[1]
    }
    fn p50(&self) -> f32 {
        self.l_percentiles[3]
    }
    fn p95(&self) -> f32 {
        self.l_percentiles[5]
    }
    fn p99(&self) -> f32 {
        self.l_percentiles[6]
    }
    fn mean_l(&self) -> f32 {
        self.channel_stats[0]
    }
    fn std_l(&self) -> f32 {
        self.channel_stats[1]
    }
    fn mean_a(&self) -> f32 {
        self.channel_stats[2]
    }
    fn std_a(&self) -> f32 {
        self.channel_stats[3]
    }
    fn mean_b(&self) -> f32 {
        self.channel_stats[4]
    }
    fn std_b(&self) -> f32 {
        self.channel_stats[5]
    }
}

/// Rule-based auto-tuner: pure heuristics, no learned weights.
///
/// Analyzes image features and produces coherent filter parameters.
/// This is what camera ISPs actually ship — well-tuned statistics,
/// not neural networks.
///
/// The rules are designed to be conservative: they correct obvious
/// problems without imposing a "look." Suitable as a default fallback
/// when no trained model is available.
pub fn rule_based_tune(features: &ImageFeatures) -> TunedParams {
    let mut params = TunedParams::default();

    // ── Derived stats ────────────────────────────────────────────
    let chroma_energy = (features.std_a() + features.std_b()) * 0.5;
    let contrast_ratio = features.std_l() / features.mean_l().max(0.01);
    let median = features.p50();

    // ── Adjustment need ──────────────────────────────────────────
    // Well-balanced images need less correction. Scale all "artistic"
    // adjustments by this factor so we don't over-process good input.
    // Exposure/highlight recovery/shadow lift are exempt — they fix
    // problems, not add style.
    let exposure_balance = 1.0 - (2.0 * (median - 0.45).abs()).min(1.0); // peak at 0.45
    let dr_balance = ((features.dynamic_range - 0.3) / 0.5).clamp(0.0, 1.0); // >0.8 = full
    let chroma_balance = (chroma_energy / 0.12).clamp(0.0, 1.0); // >0.12 = full
    // 0..1 where 1 = image is already well-balanced, 0 = needs work
    let balance = exposure_balance * dr_balance * chroma_balance;
    // Artistic scaling: 1.0 for bad images, 0.3 for perfectly balanced
    let art_scale = 1.0 - 0.7 * balance;

    // ── Exposure correction ─────────────────────────────────────
    // Only correct severely mis-exposed images. Most camera JPEGs are fine.
    if median < 0.2 {
        // Very dark: lift gently
        let factor = 0.35 / median.max(0.05);
        let stops = 3.0 * factor.log2();
        params.exposure = stops.clamp(0.0, 2.0) * 0.4;
    } else if median > 0.75 {
        // Very bright: darken gently
        let factor = 0.6 / median.max(0.01);
        let stops = 3.0 * factor.log2();
        params.exposure = stops.clamp(-2.0, 0.0) * 0.4;
    }

    // ── Highlights / Shadows (FusedAdjust) ────────────────────────
    // Subtle highlight compression and shadow opening.
    if features.p95() > 0.7 {
        params.highlights = (features.p95() - 0.6) * 0.3 * art_scale;
        params.highlights = params.highlights.clamp(0.0, 0.2);
    }
    if features.p5() < 0.3 {
        params.shadows = (0.3 - features.p5()) * 0.3 * art_scale;
        params.shadows = params.shadows.clamp(0.0, 0.2);
    }

    // ── Highlight recovery ──────────────────────────────────────
    // Dedicated recovery filter for severe clipping only.
    let highlight_headroom = features.p99() - features.p95();
    if features.p95() > 0.85 && highlight_headroom < 0.01 {
        params.highlight_recovery = 0.5;
    }

    // ── Shadow lift ─────────────────────────────────────────────
    // Dedicated lift filter for severely crushed shadows only.
    let shadow_headroom = features.p5() - features.p1();
    if features.p5() < 0.08 && shadow_headroom < 0.005 {
        params.shadow_lift = 0.4;
    }

    // ── Color cast correction ───────────────────────────────────
    // Only correct strong casts (>0.06 mean deviation).
    let cast_a = features.mean_a();
    let cast_b = features.mean_b();
    if cast_b.abs() > 0.06 {
        params.temperature = -cast_b * 0.8;
        params.temperature = params.temperature.clamp(-0.1, 0.1);
    }
    if cast_a.abs() > 0.06 {
        params.tint = -cast_a * 0.8;
        params.tint = params.tint.clamp(-0.1, 0.1);
    }

    // ── Saturation ─────────────────────────────────────────────
    // Conservative boost. Model mean=1.25, but applying that universally
    // hurts images where expert edit is subtle. Keep modest defaults.
    if chroma_energy < 0.08 {
        params.saturation = 1.0 + 0.18 * art_scale;
    } else if chroma_energy < 0.15 {
        params.saturation = 1.0 + 0.10 * art_scale;
    }

    // ── Vibrance ────────────────────────────────────────────────
    // Selective saturation boost for muted colors.
    params.vibrance = if chroma_energy < 0.08 {
        0.28 * art_scale
    } else {
        0.18 * art_scale
    };

    // ── Contrast ──────────────────────────────────────────────────
    if contrast_ratio < 0.15 {
        params.contrast = 0.12 * art_scale;
    } else if contrast_ratio < 0.25 {
        params.contrast = 0.06 * art_scale;
    }

    // ── Sigmoid ─────────────────────────────────────────────────
    // Mild S-curve for added "pop". Skip if dynamic range is already high.
    if features.dynamic_range > 0.3 && features.dynamic_range < 0.85 {
        params.sigmoid_contrast = 1.0 + 0.12 * art_scale;
    }

    // ── Black point ──────────────────────────────────────────────
    if features.p5() > 0.05 && features.p1() > 0.01 {
        params.black_point = 0.008 * art_scale;
    }

    // ── Clarity ─────────────────────────────────────────────────
    if contrast_ratio < 0.15 {
        params.clarity = 0.2 * art_scale;
    } else if contrast_ratio < 0.3 {
        params.clarity = 0.12 * art_scale;
    }

    // ── Adaptive sharpening ─────────────────────────────────────
    if features.dynamic_range < 0.5 {
        params.sharpen = 0.35 * art_scale;
    } else if features.dynamic_range < 0.8 {
        params.sharpen = 0.2 * art_scale;
    }

    // ── Gamut expand ────────────────────────────────────────────
    // Conservative expansion. Model mean=0.39 but that's per-cluster tuned.
    if chroma_energy < 0.06 {
        params.gamut_expand = 0.35 * art_scale;
    } else if chroma_energy < 0.12 {
        params.gamut_expand = 0.22 * art_scale;
    }

    params
}

// ─── Linear model ───────────────────────────────────────────────────

/// Number of input features for the linear model.
pub const LINEAR_MODEL_INPUTS: usize = 142;
/// Number of output parameters.
pub const LINEAR_MODEL_OUTPUTS: usize = 18;

/// Weights for a linear model: output = features * weights + bias.
///
/// `weights` is [INPUTS × OUTPUTS] row-major: weights[i * OUTPUTS + j]
/// maps input feature i to output parameter j.
///
/// These can be trained offline with least-squares regression on
/// (ImageFeatures, expert_params) pairs. Total: 2574 floats (~10KB).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LinearModel {
    /// Weight matrix, row-major [142 × 18].
    #[cfg_attr(feature = "serde", serde(with = "BigArray"))]
    pub weights: [f32; LINEAR_MODEL_INPUTS * LINEAR_MODEL_OUTPUTS],
    /// Bias vector [18].
    pub bias: [f32; LINEAR_MODEL_OUTPUTS],
}

impl LinearModel {
    /// Run inference: multiply feature vector by weight matrix, add bias.
    pub fn predict(&self, features: &ImageFeatures) -> TunedParams {
        let input = features.to_tensor();
        assert_eq!(input.len(), LINEAR_MODEL_INPUTS);

        let mut output = [0.0f32; LINEAR_MODEL_OUTPUTS];

        // Matrix multiply: output[j] = sum(input[i] * weights[i*O + j]) + bias[j]
        for (j, out) in output.iter_mut().enumerate() {
            let mut sum = self.bias[j];
            for (i, &inp) in input.iter().enumerate() {
                sum += inp * self.weights[i * LINEAR_MODEL_OUTPUTS + j];
            }
            *out = sum;
        }

        // Map output array to TunedParams with clamping
        TunedParams {
            exposure: output[0].clamp(-3.0, 3.0),
            contrast: output[1].clamp(-1.0, 1.0),
            highlights: output[2].clamp(-1.0, 1.0),
            shadows: output[3].clamp(-1.0, 1.0),
            saturation: output[4].clamp(0.0, 3.0),
            vibrance: output[5].clamp(-1.0, 1.0),
            temperature: output[6].clamp(-1.0, 1.0),
            tint: output[7].clamp(-1.0, 1.0),
            black_point: output[8].clamp(0.0, 0.2),
            white_point: output[9].clamp(0.5, 1.0),
            sigmoid_contrast: output[10].clamp(0.5, 3.0),
            sigmoid_skew: output[11].clamp(0.1, 0.9),
            clarity: output[12].clamp(0.0, 1.0),
            sharpen: output[13].clamp(0.0, 2.0),
            highlight_recovery: output[14].clamp(0.0, 1.0),
            shadow_lift: output[15].clamp(0.0, 1.0),
            local_tonemap: output[16].clamp(0.0, 1.0),
            gamut_expand: output[17].clamp(0.0, 1.0),
        }
    }
}

// ─── Cluster model ──────────────────────────────────────────────────

/// Number of clusters in the trained model.
pub const CLUSTER_COUNT: usize = 64;

/// Serde helper for `[[f32; N]; M]` where both dimensions exceed serde's
/// built-in array limit of 32 elements. Serializes as a flat sequence of
/// `N * M` floats and deserializes back into the nested array.
#[cfg(feature = "serde")]
mod serde_nested_big_array {
    use serde::de::{self, SeqAccess, Visitor};
    use serde::ser::SerializeSeq;
    use serde::{Deserializer, Serializer};

    use super::{CLUSTER_COUNT, LINEAR_MODEL_INPUTS};

    const N: usize = LINEAR_MODEL_INPUTS; // 142
    const M: usize = CLUSTER_COUNT; // 64

    pub fn serialize<S>(data: &[[f32; N]; M], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(N * M))?;
        for row in data {
            for &val in row {
                seq.serialize_element(&val)?;
            }
        }
        seq.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[[f32; N]; M], D::Error>
    where
        D: Deserializer<'de>,
    {
        struct NestedArrayVisitor;

        impl<'de> Visitor<'de> for NestedArrayVisitor {
            type Value = [[f32; N]; M];

            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "a sequence of {} floats", N * M)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut result = [[0.0f32; N]; M];
                for row in &mut result {
                    for val in row.iter_mut() {
                        *val = seq
                            .next_element()?
                            .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                    }
                }
                Ok(result)
            }
        }

        deserializer.deserialize_seq(NestedArrayVisitor)
    }
}

/// Cluster-based auto-tuner: nearest-centroid lookup.
///
/// Each cluster has a centroid (142 features) and optimized parameters (18 floats).
/// At inference, finds the nearest centroid and returns its parameters.
///
/// Trained on MIT-Adobe FiveK (4,958 images) using Nelder-Mead optimization
/// with zensim as the loss function. Model size: ~10 KB.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ClusterModel {
    /// Cluster centroids, each [142] floats.
    #[cfg_attr(feature = "serde", serde(with = "serde_nested_big_array"))]
    pub centroids: [[f32; LINEAR_MODEL_INPUTS]; CLUSTER_COUNT],
    /// Optimized parameters per cluster, each [18] floats.
    #[cfg_attr(feature = "serde", serde(with = "BigArray"))]
    pub params: [[f32; LINEAR_MODEL_OUTPUTS]; CLUSTER_COUNT],
}

impl ClusterModel {
    /// Find the nearest cluster and return its optimized parameters.
    pub fn predict(&self, features: &ImageFeatures) -> TunedParams {
        self.predict_blend(features, 1)
    }

    /// Predict using inverse-distance weighted blend of k nearest clusters.
    ///
    /// `k=1` is equivalent to nearest-neighbor. `k=3` blends the 3 closest
    /// clusters using 1/distance weighting, which smooths transitions.
    pub fn predict_blend(&self, features: &ImageFeatures, k: usize) -> TunedParams {
        let input = features.to_tensor();

        // Compute distances to all centroids
        let mut dists: alloc::vec::Vec<(usize, f32)> = self
            .centroids
            .iter()
            .enumerate()
            .map(|(i, centroid)| {
                let dist: f32 = input
                    .iter()
                    .zip(centroid.iter())
                    .map(|(a, b)| (a - b) * (a - b))
                    .sum();
                (i, dist)
            })
            .collect();

        dists.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let top_k = &dists[..k.min(dists.len())];

        // Weighted average of parameters
        let mut blended = [0.0f32; LINEAR_MODEL_OUTPUTS];
        let mut total_weight = 0.0f32;

        for &(idx, dist) in top_k {
            // 1/sqrt(dist) weighting — sqrt(dist) is actual Euclidean distance
            let w = 1.0 / (dist.sqrt() + 1e-6);
            total_weight += w;
            for (j, val) in self.params[idx].iter().enumerate() {
                blended[j] += val * w;
            }
        }

        let inv_w = 1.0 / total_weight;
        for v in &mut blended {
            *v *= inv_w;
        }

        TunedParams::from_array(&blended)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_extraction_runs() {
        let mut planes = OklabPlanes::new(64, 64);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = (i as f32 / (64.0 * 64.0)).sqrt();
        }
        for (i, v) in planes.a.iter_mut().enumerate() {
            *v = (i as f32 - 2048.0) / 20000.0;
        }
        for (i, v) in planes.b.iter_mut().enumerate() {
            *v = (i as f32 - 1500.0) / 20000.0;
        }

        let features = ImageFeatures::extract(&planes);

        // Histogram should sum to ~1.0
        let l_sum: f32 = features.l_histogram.iter().sum();
        assert!(
            (l_sum - 1.0).abs() < 0.01,
            "L histogram should sum to 1: {l_sum}"
        );

        // Percentiles should be monotonically increasing
        for i in 1..7 {
            assert!(
                features.l_percentiles[i] >= features.l_percentiles[i - 1],
                "percentiles should be monotonic: {:?}",
                features.l_percentiles
            );
        }

        // Dynamic range should be positive
        assert!(features.dynamic_range > 0.0);
    }

    #[test]
    fn tensor_packing() {
        let planes = OklabPlanes::new(16, 16);
        let features = ImageFeatures::extract(&planes);
        let tensor = features.to_tensor();
        assert_eq!(tensor.len(), 142, "tensor should be 142 floats");
    }

    #[test]
    fn default_params_are_identity() {
        let params = TunedParams::default();
        assert!((params.exposure).abs() < 1e-6);
        assert!((params.contrast).abs() < 1e-6);
        assert!((params.saturation - 1.0).abs() < 1e-6);
        assert!((params.sigmoid_contrast - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rule_based_brightens_dark_image() {
        let mut planes = OklabPlanes::new(64, 64);
        for v in &mut planes.l {
            *v = 0.15; // very dark
        }
        let features = ImageFeatures::extract(&planes);
        let params = rule_based_tune(&features);
        assert!(
            params.exposure > 0.3,
            "should boost exposure for dark image: {}",
            params.exposure
        );
    }

    #[test]
    fn rule_based_darkens_bright_image() {
        let mut planes = OklabPlanes::new(64, 64);
        for v in &mut planes.l {
            *v = 0.85; // very bright
        }
        let features = ImageFeatures::extract(&planes);
        let params = rule_based_tune(&features);
        assert!(
            params.exposure < -0.3,
            "should reduce exposure for bright image: {}",
            params.exposure
        );
    }

    #[test]
    fn rule_based_recovers_clipped_highlights() {
        let mut planes = OklabPlanes::new(100, 1);
        for i in 0..80 {
            planes.l[i] = 0.3 + (i as f32 / 80.0) * 0.5;
        }
        // 20% hard-clipped at 0.98
        for i in 80..100 {
            planes.l[i] = 0.98;
        }
        let features = ImageFeatures::extract(&planes);
        let params = rule_based_tune(&features);
        assert!(
            params.highlight_recovery > 0.1,
            "should recover clipped highlights: {}",
            params.highlight_recovery
        );
    }

    #[test]
    fn rule_based_lifts_crushed_shadows() {
        let mut planes = OklabPlanes::new(100, 1);
        // 30% crushed at 0.02
        for i in 0..30 {
            planes.l[i] = 0.02;
        }
        for i in 30..100 {
            planes.l[i] = 0.5;
        }
        let features = ImageFeatures::extract(&planes);
        let params = rule_based_tune(&features);
        assert!(
            params.shadow_lift > 0.1,
            "should lift crushed shadows: {}",
            params.shadow_lift
        );
    }

    #[test]
    fn rule_based_corrects_color_cast() {
        let mut planes = OklabPlanes::new(64, 64);
        for v in &mut planes.l {
            *v = 0.5;
        }
        // Strong warm cast (high b = warm)
        for v in &mut planes.b {
            *v = 0.08;
        }
        let features = ImageFeatures::extract(&planes);
        let params = rule_based_tune(&features);
        assert!(
            params.temperature < -0.05,
            "should correct warm cast: {}",
            params.temperature
        );
    }

    #[test]
    fn rule_based_leaves_good_image_alone() {
        let mut planes = OklabPlanes::new(100, 1);
        // Well-exposed, full range, no cast
        for i in 0..100 {
            planes.l[i] = 0.1 + (i as f32 / 100.0) * 0.8; // 0.1-0.9
        }
        let features = ImageFeatures::extract(&planes);
        let params = rule_based_tune(&features);
        // Exposure correction should be small
        assert!(
            params.exposure.abs() < 0.5,
            "good image should need little exposure correction: {}",
            params.exposure
        );
        // No highlight/shadow recovery needed
        assert!(
            params.highlight_recovery < 0.2,
            "good image shouldn't need highlight recovery: {}",
            params.highlight_recovery
        );
    }

    #[test]
    fn linear_model_identity_weights() {
        // Zero weights + identity bias should produce default-ish params
        let model = LinearModel {
            weights: [0.0; LINEAR_MODEL_INPUTS * LINEAR_MODEL_OUTPUTS],
            bias: [
                0.0, 0.0, 0.0, 0.0, // exposure, contrast, highlights, shadows
                1.0, 0.0, 0.0, 0.0, // saturation, vibrance, temperature, tint
                0.0, 1.0, // black_point, white_point
                1.0, 0.5, // sigmoid_contrast, sigmoid_skew
                0.0, 0.0, // clarity, sharpen
                0.0, 0.0, // highlight_recovery, shadow_lift
                0.0, 0.0, // local_tonemap, gamut_expand
            ],
        };
        let planes = OklabPlanes::new(16, 16);
        let features = ImageFeatures::extract(&planes);
        let params = model.predict(&features);
        assert!((params.saturation - 1.0).abs() < 1e-6);
        assert!((params.sigmoid_contrast - 1.0).abs() < 1e-6);
    }

    #[test]
    fn tensor_roundtrip() {
        let mut planes = OklabPlanes::new(64, 64);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = (i as f32 / (64.0 * 64.0)).sqrt();
        }
        for (i, v) in planes.a.iter_mut().enumerate() {
            *v = (i as f32 - 2048.0) / 20000.0;
        }
        for (i, v) in planes.b.iter_mut().enumerate() {
            *v = (i as f32 - 1500.0) / 20000.0;
        }
        let features = ImageFeatures::extract(&planes);
        let tensor = features.to_tensor();
        let restored = ImageFeatures::from_tensor(&tensor);
        assert_eq!(features.l_histogram, restored.l_histogram);
        assert_eq!(features.a_histogram, restored.a_histogram);
        assert_eq!(features.l_percentiles, restored.l_percentiles);
        assert_eq!(features.channel_stats, restored.channel_stats);
        assert!((features.dynamic_range - restored.dynamic_range).abs() < 1e-6);
    }

    #[test]
    fn params_array_roundtrip() {
        let params = TunedParams {
            exposure: 0.5,
            contrast: -0.2,
            highlights: 0.3,
            shadows: -0.4,
            saturation: 1.2,
            vibrance: 0.1,
            temperature: -0.05,
            tint: 0.03,
            black_point: 0.01,
            white_point: 0.95,
            sigmoid_contrast: 1.5,
            sigmoid_skew: 0.6,
            clarity: 0.15,
            sharpen: 0.3,
            highlight_recovery: 0.2,
            shadow_lift: 0.1,
            local_tonemap: 0.05,
            gamut_expand: 0.25,
        };
        let arr = params.to_array();
        let restored = TunedParams::from_array(&arr);
        assert!((params.exposure - restored.exposure).abs() < 1e-6);
        assert!((params.saturation - restored.saturation).abs() < 1e-6);
        assert!((params.sigmoid_skew - restored.sigmoid_skew).abs() < 1e-6);
        assert!((params.gamut_expand - restored.gamut_expand).abs() < 1e-6);
    }

    #[test]
    fn cluster_blend_k1_equals_nearest() {
        // With k=1, predict_blend should match nearest-neighbor
        let mut model = ClusterModel {
            centroids: [[0.0; LINEAR_MODEL_INPUTS]; CLUSTER_COUNT],
            params: [[0.0; LINEAR_MODEL_OUTPUTS]; CLUSTER_COUNT],
        };
        // Set up two distinct clusters
        model.centroids[0] = [0.1; LINEAR_MODEL_INPUTS];
        model.centroids[1] = [0.9; LINEAR_MODEL_INPUTS];
        model.params[0] = [0.5; LINEAR_MODEL_OUTPUTS];
        model.params[1] = [1.0; LINEAR_MODEL_OUTPUTS];

        let planes = OklabPlanes::new(16, 16);
        let features = ImageFeatures::extract(&planes);
        let p1 = model.predict(&features);
        let pb = model.predict_blend(&features, 1);
        assert!((p1.exposure - pb.exposure).abs() < 1e-6);
    }

    #[test]
    fn cluster_blend_k3_interpolates() {
        // With k=3, blend should produce a weighted average
        let mut model = ClusterModel {
            centroids: [[0.0; LINEAR_MODEL_INPUTS]; CLUSTER_COUNT],
            params: [[0.0; LINEAR_MODEL_OUTPUTS]; CLUSTER_COUNT],
        };
        // Three clusters equidistant from test point
        for i in 0..3 {
            model.centroids[i] = [0.5; LINEAR_MODEL_INPUTS];
            model.centroids[i][0] = (i as f32) * 0.1;
            model.params[i][0] = (i as f32) * 0.3; // exposure: 0, 0.3, 0.6
        }

        let mut planes = OklabPlanes::new(16, 16);
        for v in &mut planes.l {
            *v = 0.5;
        }
        let features = ImageFeatures::extract(&planes);
        let blend = model.predict_blend(&features, 3);
        // Blended exposure should be between 0 and 0.6
        assert!(blend.exposure >= 0.0 && blend.exposure <= 0.6);
    }
}
