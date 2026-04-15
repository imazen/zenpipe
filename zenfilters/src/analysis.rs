//! Shared image analysis cache for auto filters.
//!
//! Multiple auto filters need the same statistics (histograms, percentiles,
//! means). Computing them once and caching saves redundant passes over the
//! pixel data. Each analysis costs ~2 passes over all planes (~3ms at 4K).

use crate::planes::OklabPlanes;

/// Cached image analysis results.
///
/// Computed from OklabPlanes by [`ImageAnalysis::compute`], stored in
/// [`FilterContext`](crate::FilterContext) for reuse across auto filters.
#[derive(Clone, Debug)]
pub struct ImageAnalysis {
    /// 1024-bin histogram of L values over [0, 1].
    pub histogram_l: [u32; 1024],
    /// Percentiles: p1, p5, p25, p50, p75, p95, p99.
    pub percentiles: [f32; 7],
    /// Arithmetic mean of L.
    pub mean_l: f32,
    /// Arithmetic mean of a (green-red axis). Indicates color cast.
    pub mean_a: f32,
    /// Arithmetic mean of b (blue-yellow axis). Indicates color cast.
    pub mean_b: f32,
    /// Standard deviation of L.
    pub std_l: f32,
    /// Standard deviation of a.
    pub std_a: f32,
    /// Standard deviation of b.
    pub std_b: f32,
    /// Geometric mean of L: exp(mean(ln(L))). Robust exposure estimator.
    pub geo_mean_l: f32,
    /// Dynamic range: p99 - p1.
    pub dynamic_range: f32,
    /// Contrast ratio: std_l / mean_l. Low = flat, high = contrasty.
    pub contrast_ratio: f32,
    /// Chroma energy: (std_a + std_b) * 0.5. Measures color variety.
    pub chroma_energy: f32,
    /// Number of pixels analyzed.
    pub pixel_count: usize,
}

/// Percentile indices into the `percentiles` array.
pub const P1: usize = 0;
pub const P5: usize = 1;
pub const P25: usize = 2;
pub const P50: usize = 3;
pub const P75: usize = 4;
pub const P95: usize = 5;
pub const P99: usize = 6;

const HIST_BINS: usize = 1024;
const PERCENTILE_TARGETS: [f64; 7] = [0.01, 0.05, 0.25, 0.50, 0.75, 0.95, 0.99];

impl ImageAnalysis {
    /// Compute full analysis from OklabPlanes.
    ///
    /// Two passes: pass 1 over L (histogram + mean + variance + geo_mean),
    /// pass 2 over a and b (mean + variance).
    pub fn compute(planes: &OklabPlanes) -> Self {
        let pc = planes.pixel_count();
        if pc == 0 {
            return Self::empty();
        }

        let n = pc as f64;
        let epsilon = 1e-6f64;

        // ── Pass 1: L plane ─────────────────────────────────────────
        let mut histogram_l = [0u32; HIST_BINS];
        let mut sum_l = 0.0f64;
        let mut log_sum = 0.0f64;

        for &v in &planes.l {
            let clamped = v.clamp(0.0, 1.0);
            let bin = (clamped * (HIST_BINS - 1) as f32) as usize;
            histogram_l[bin.min(HIST_BINS - 1)] += 1;
            sum_l += v as f64;
            log_sum += (v.max(epsilon as f32) as f64).ln();
        }

        let mean_l = (sum_l / n) as f32;
        let geo_mean_l = (log_sum / n).exp() as f32;

        // Variance of L (second pass on L)
        let mut var_l = 0.0f64;
        for &v in &planes.l {
            let d = v as f64 - mean_l as f64;
            var_l += d * d;
        }
        let std_l = (var_l / n).sqrt() as f32;

        // Percentiles from histogram
        let mut percentiles = [0.0f32; 7];
        for (pi, &target) in PERCENTILE_TARGETS.iter().enumerate() {
            let target_count = (n * target) as u64;
            let mut cumsum = 0u64;
            for (bin, &count) in histogram_l.iter().enumerate() {
                cumsum += count as u64;
                if cumsum >= target_count {
                    percentiles[pi] = bin as f32 / (HIST_BINS - 1) as f32;
                    break;
                }
            }
        }

        // ── Pass 2: a and b planes ──────────────────────────────────
        let mut sum_a = 0.0f64;
        let mut sum_b = 0.0f64;
        for (&a, &b) in planes.a.iter().zip(planes.b.iter()) {
            sum_a += a as f64;
            sum_b += b as f64;
        }
        let mean_a = (sum_a / n) as f32;
        let mean_b = (sum_b / n) as f32;

        let mut var_a = 0.0f64;
        let mut var_b = 0.0f64;
        for (&a, &b) in planes.a.iter().zip(planes.b.iter()) {
            let da = a as f64 - mean_a as f64;
            let db = b as f64 - mean_b as f64;
            var_a += da * da;
            var_b += db * db;
        }
        let std_a = (var_a / n).sqrt() as f32;
        let std_b = (var_b / n).sqrt() as f32;

        let dynamic_range = percentiles[P99] - percentiles[P1];
        let contrast_ratio = std_l / mean_l.max(0.01);
        let chroma_energy = (std_a + std_b) * 0.5;

        Self {
            histogram_l,
            percentiles,
            mean_l,
            mean_a,
            mean_b,
            std_l,
            std_a,
            std_b,
            geo_mean_l,
            dynamic_range,
            contrast_ratio,
            chroma_energy,
            pixel_count: pc,
        }
    }

    fn empty() -> Self {
        Self {
            histogram_l: [0; 1024],
            percentiles: [0.0; 7],
            mean_l: 0.0,
            mean_a: 0.0,
            mean_b: 0.0,
            std_l: 0.0,
            std_a: 0.0,
            std_b: 0.0,
            geo_mean_l: 0.0,
            dynamic_range: 0.0,
            contrast_ratio: 0.0,
            chroma_energy: 0.0,
            pixel_count: 0,
        }
    }

    /// p1 percentile.
    #[inline]
    pub fn p1(&self) -> f32 {
        self.percentiles[P1]
    }
    /// p5 percentile.
    #[inline]
    pub fn p5(&self) -> f32 {
        self.percentiles[P5]
    }
    /// p25 (first quartile).
    #[inline]
    pub fn p25(&self) -> f32 {
        self.percentiles[P25]
    }
    /// Median.
    #[inline]
    pub fn p50(&self) -> f32 {
        self.percentiles[P50]
    }
    /// p75 (third quartile).
    #[inline]
    pub fn p75(&self) -> f32 {
        self.percentiles[P75]
    }
    /// p95 percentile.
    #[inline]
    pub fn p95(&self) -> f32 {
        self.percentiles[P95]
    }
    /// p99 percentile.
    #[inline]
    pub fn p99(&self) -> f32 {
        self.percentiles[P99]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis_of_gradient() {
        let mut planes = OklabPlanes::new(100, 1);
        for i in 0..100 {
            planes.l[i] = i as f32 / 99.0;
        }
        let a = ImageAnalysis::compute(&planes);
        assert!((a.mean_l - 0.5).abs() < 0.01);
        assert!(a.p50() > 0.45 && a.p50() < 0.55);
        assert!(a.p1() < 0.02);
        assert!(a.p99() > 0.98);
        assert!((a.dynamic_range - 1.0).abs() < 0.05);
    }

    #[test]
    fn analysis_of_flat_image() {
        let mut planes = OklabPlanes::new(100, 1);
        for v in &mut planes.l {
            *v = 0.5;
        }
        let a = ImageAnalysis::compute(&planes);
        assert!((a.mean_l - 0.5).abs() < 0.001);
        assert!(a.std_l < 0.001);
        assert!(a.contrast_ratio < 0.01);
    }

    #[test]
    fn analysis_detects_color_cast() {
        let mut planes = OklabPlanes::new(100, 1);
        for v in &mut planes.l {
            *v = 0.5;
        }
        for v in &mut planes.a {
            *v = 0.1; // strong green-red cast
        }
        for v in &mut planes.b {
            *v = -0.05; // mild blue cast
        }
        let a = ImageAnalysis::compute(&planes);
        assert!((a.mean_a - 0.1).abs() < 0.001);
        assert!((a.mean_b - (-0.05)).abs() < 0.001);
    }
}
