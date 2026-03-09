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

use crate::planes::OklabPlanes;

/// Lightweight image features for model inference.
///
/// Extracted from `OklabPlanes` in a single pass over the data.
/// Total size: ~160 floats (640 bytes). Cheap to compute.
pub struct ImageFeatures {
    /// L channel histogram, 64 bins, normalized to sum=1.
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
}

/// Predicted filter parameters from a model or rule-based system.
#[derive(Clone, Debug)]
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
}
