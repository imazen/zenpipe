use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Wavelet-based noise reduction for luminance and chroma.
///
/// Uses an à trous (with holes) wavelet decomposition on separate planes.
/// Each wavelet scale captures noise at a different frequency:
/// - Scale 0: finest noise (1–2px features)
/// - Scale 1: medium noise (2–4px)
/// - Scale 2: coarser noise (4–8px)
/// - Scale 3+: structural detail (preserved)
///
/// Thresholding is soft (shrinkage) to avoid artifacts. Chroma noise is
/// typically stronger than luminance noise, so chroma denoising uses a
/// higher effective threshold.
///
/// This approach is similar to darktable's "denoise (profiled)" module
/// and Lightroom's noise reduction.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct NoiseReduction {
    /// Luminance noise reduction strength. 0.0 = off, 1.0 = strong.
    /// Typical: 0.3–0.7.
    pub luminance: f32,
    /// Luminance detail preservation. Higher values keep more fine detail
    /// at the cost of less noise removal. Default: 0.5.
    pub detail: f32,
    /// Chroma noise reduction strength. 0.0 = off, 1.0 = strong.
    /// Typical: 0.5–1.0 (chroma noise is usually more objectionable).
    pub chroma: f32,
    /// Luminance contrast preservation. Higher values preserve more local
    /// contrast in denoised areas (at the cost of keeping some noise).
    /// Default: 0.5. Range: 0.0–1.0. Matches Lightroom's NR Contrast slider.
    pub luminance_contrast: f32,
    /// Chroma detail preservation. Higher values keep more fine color detail
    /// at the cost of less chroma noise removal. Default: 0.5. Range: 0.0–1.0.
    /// Matches Lightroom's Color Detail slider.
    pub chroma_detail: f32,
    /// Number of wavelet scales. More scales = smoother result.
    /// Default: 4. Range: 1–6.
    pub scales: u32,
}

impl Default for NoiseReduction {
    fn default() -> Self {
        Self {
            luminance: 0.0,
            chroma: 0.0,
            detail: 0.5,
            luminance_contrast: 0.5,
            chroma_detail: 0.5,
            scales: 4,
        }
    }
}

impl NoiseReduction {
    /// Create from perceptual slider values (0.0–1.0 each).
    ///
    /// `luminance_slider` and `chroma_slider` are sqrt-remapped so the first
    /// half of the slider covers the most useful denoising range (moderate NR).
    /// `detail` and `luminance_contrast` are already perceptually linear.
    pub fn from_slider(luminance_slider: f32, chroma_slider: f32) -> Self {
        Self {
            luminance: crate::slider::nr_strength_from_slider(luminance_slider.clamp(0.0, 1.0)),
            chroma: crate::slider::nr_strength_from_slider(chroma_slider.clamp(0.0, 1.0)),
            ..Default::default()
        }
    }

    fn is_identity(&self) -> bool {
        self.luminance.abs() < 1e-6 && self.chroma.abs() < 1e-6
    }
}

static NOISE_REDUCTION_SCHEMA: FilterSchema = FilterSchema {
    name: "noise_reduction",
    label: "Noise Reduction",
    description: "Wavelet-based luminance and chroma noise reduction",
    group: FilterGroup::Detail,
    params: &[
        ParamDesc {
            name: "luminance",
            label: "Luminance",
            description: "Luminance noise reduction strength",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.0,
                identity: 0.0,
                step: 0.05,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::SquareFromSlider,
        },
        ParamDesc {
            name: "chroma",
            label: "Color",
            description: "Chroma noise reduction strength",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.0,
                identity: 0.0,
                step: 0.05,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::SquareFromSlider,
        },
        ParamDesc {
            name: "detail",
            label: "Detail",
            description: "Luminance detail preservation (higher = keep more detail)",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.5,
                identity: 0.5,
                step: 0.05,
            },
            unit: "",
            section: "Advanced",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "luminance_contrast",
            label: "Contrast",
            description: "Luminance contrast preservation in denoised areas",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.5,
                identity: 0.5,
                step: 0.05,
            },
            unit: "",
            section: "Advanced",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "chroma_detail",
            label: "Color Detail",
            description: "Chroma detail preservation (higher = keep more color detail)",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.5,
                identity: 0.5,
                step: 0.05,
            },
            unit: "",
            section: "Advanced",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "scales",
            label: "Scales",
            description: "Number of wavelet scales (more = smoother)",
            kind: ParamKind::Int {
                min: 1,
                max: 6,
                default: 4,
            },
            unit: "",
            section: "Advanced",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for NoiseReduction {
    fn schema() -> &'static FilterSchema {
        &NOISE_REDUCTION_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "luminance" => Some(ParamValue::Float(self.luminance)),
            "chroma" => Some(ParamValue::Float(self.chroma)),
            "detail" => Some(ParamValue::Float(self.detail)),
            "luminance_contrast" => Some(ParamValue::Float(self.luminance_contrast)),
            "chroma_detail" => Some(ParamValue::Float(self.chroma_detail)),
            "scales" => Some(ParamValue::Int(self.scales as i32)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        match name {
            "luminance" | "chroma" | "detail" | "luminance_contrast" | "chroma_detail" => {
                let v = match value.as_f32() {
                    Some(v) => v,
                    None => return false,
                };
                match name {
                    "luminance" => self.luminance = v,
                    "chroma" => self.chroma = v,
                    "detail" => self.detail = v,
                    "luminance_contrast" => self.luminance_contrast = v,
                    "chroma_detail" => self.chroma_detail = v,
                    _ => unreachable!(),
                }
            }
            "scales" => {
                let v = match value.as_i32() {
                    Some(v) => v,
                    None => return false,
                };
                self.scales = v as u32;
            }
            _ => return false,
        }
        true
    }
}

/// B3 spline wavelet kernel coefficients [1/16, 4/16, 6/16, 4/16, 1/16].
const B3_KERNEL: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

/// À trous wavelet transform: one smoothing step at the given scale.
/// `scale` determines the spacing between kernel taps: spacing = 2^scale.
fn atrous_smooth(src: &[f32], dst: &mut [f32], w: usize, h: usize, scale: u32, tmp: &mut [f32]) {
    let step = 1usize << scale;
    let [w0, w1, w2, w3, w4] = B3_KERNEL;

    // Horizontal pass: src → tmp
    // Interior pixels (no boundary checks) for the middle of each row.
    let margin = 2 * step;
    for y in 0..h {
        let row = y * w;
        // Left boundary
        for x in 0..margin.min(w) {
            let sx = |k: usize| {
                (x as isize + (k as isize - 2) * step as isize).clamp(0, w as isize - 1) as usize
            };
            tmp[row + x] = w0 * src[row + sx(0)]
                + w1 * src[row + sx(1)]
                + w2 * src[row + sx(2)]
                + w3 * src[row + sx(3)]
                + w4 * src[row + sx(4)];
        }
        // Interior (no bounds checks)
        for x in margin..w.saturating_sub(margin) {
            tmp[row + x] = w0 * src[row + x - 2 * step]
                + w1 * src[row + x - step]
                + w2 * src[row + x]
                + w3 * src[row + x + step]
                + w4 * src[row + x + 2 * step];
        }
        // Right boundary
        for x in w.saturating_sub(margin)..w {
            let sx = |k: usize| {
                (x as isize + (k as isize - 2) * step as isize).clamp(0, w as isize - 1) as usize
            };
            tmp[row + x] = w0 * src[row + sx(0)]
                + w1 * src[row + sx(1)]
                + w2 * src[row + sx(2)]
                + w3 * src[row + sx(3)]
                + w4 * src[row + sx(4)];
        }
    }

    // Vertical pass: tmp → dst
    // Precompute clamped row offsets for boundary rows, then use unchecked for interior.
    for y in 0..h {
        let sy = |k: usize| {
            (y as isize + (k as isize - 2) * step as isize).clamp(0, h as isize - 1) as usize
        };
        let r0 = sy(0) * w;
        let r1 = sy(1) * w;
        let r2 = sy(2) * w;
        let r3 = sy(3) * w;
        let r4 = sy(4) * w;
        let out_row = y * w;
        for x in 0..w {
            dst[out_row + x] = w0 * tmp[r0 + x]
                + w1 * tmp[r1 + x]
                + w2 * tmp[r2 + x]
                + w3 * tmp[r3 + x]
                + w4 * tmp[r4 + x];
        }
    }
}

/// Soft thresholding (wavelet shrinkage).
/// Reduces coefficients toward zero by `threshold`, preserving sign.
#[inline]
fn soft_threshold(val: f32, threshold: f32) -> f32 {
    if val > threshold {
        val - threshold
    } else if val < -threshold {
        val + threshold
    } else {
        0.0
    }
}

/// Parameters for single-plane denoising.
struct DenoiseParams {
    strength: f32,
    detail_preserve: f32,
    contrast_preserve: f32,
    num_scales: u32,
}

/// Denoise a single plane using à trous wavelet shrinkage.
///
/// `contrast_preserve` controls how much local contrast (coarser wavelet scales)
/// is preserved. 0.0 = full denoising at all scales, 1.0 = only denoise the
/// finest scale. This matches Lightroom's NR Contrast slider behaviour.
fn denoise_plane(
    plane: &mut [f32],
    w: usize,
    h: usize,
    params: &DenoiseParams,
    ctx: &mut FilterContext,
) {
    if params.strength.abs() < 1e-6 {
        return;
    }

    let n = w * h;
    let mut smooth = ctx.take_f32(n);
    let mut tmp = ctx.take_f32(n);
    let mut current = ctx.take_f32(n);
    current.copy_from_slice(plane);

    // Accumulate the denoised result
    let mut result = ctx.take_f32(n);
    for v in &mut result[..n] {
        *v = 0.0;
    }

    let num_scales = params.num_scales.clamp(1, 6);

    for scale in 0..num_scales {
        atrous_smooth(&current, &mut smooth, w, h, scale, &mut tmp);

        // BayesShrink: scale-adaptive optimal threshold.
        //
        // σ_noise estimated from MAD of finest-scale wavelet coefficients.
        // σ_signal² = max(σ_total² - σ_noise², 0) at each scale.
        // threshold = σ_noise² / σ_signal (Bayes-optimal for Gaussian prior).
        //
        // Reference: Chang, Yu, Vetterli, "Adaptive wavelet thresholding for
        // image denoising and compression," IEEE TIP 2000.
        let threshold_scale = {
            // Noise sigma: estimated from detail coefficients at this scale
            let sigma_noise = estimate_noise_sigma_from_diff(&current, &smooth, n);

            // Signal variance: total variance minus noise variance
            let sigma_total_sq = variance_of_diff(&current, &smooth, n);
            let sigma_noise_sq = sigma_noise * sigma_noise;
            let sigma_signal_sq = (sigma_total_sq - sigma_noise_sq).max(0.0);

            let bayes_threshold = if sigma_signal_sq > 1e-10 {
                sigma_noise_sq / sigma_signal_sq.sqrt()
            } else {
                // Pure noise at this scale — threshold everything
                sigma_noise * 10.0
            };

            // User controls: strength scales the threshold inversely (more strength = less detail),
            // detail_preserve reduces it (more preservation = lower threshold).
            let decay = if scale == 0 {
                1.0
            } else {
                0.5f32.powi(scale as i32)
            };
            let contrast_factor = 1.0 - params.contrast_preserve * (1.0 - decay);
            let detail_factor = 1.0 - params.detail_preserve * if scale == 0 { 0.5 } else { 0.3 };

            bayes_threshold * params.strength * detail_factor * contrast_factor
        };

        // Soft-threshold the detail and add to result
        // SIMD: process 8 elements at a time
        crate::simd::wavelet_threshold_accumulate(
            &current[..n],
            &smooth[..n],
            &mut result[..n],
            threshold_scale,
        );

        // Next iteration works on the smooth approximation
        current[..n].copy_from_slice(&smooth[..n]);
    }

    // Add the final smooth (coarsest approximation) to the thresholded details
    crate::simd::add_clamped(&result[..n], &current[..n], &mut plane[..n]);

    ctx.return_f32(result);
    ctx.return_f32(current);
    ctx.return_f32(tmp);
    ctx.return_f32(smooth);
}

/// Estimate noise sigma from the MAD of detail coefficients (a - b).
/// Uses the MAD/0.6745 robust estimator (standard for wavelet denoising).
fn estimate_noise_sigma_from_diff(a: &[f32], b: &[f32], n: usize) -> f32 {
    let mean_abs = a[..n]
        .iter()
        .zip(b[..n].iter())
        .map(|(x, y)| (x - y).abs())
        .sum::<f32>()
        / n as f32;
    // MAD-based sigma estimate: MAD / 0.6745 for Gaussian noise
    mean_abs / 0.6745
}

/// Variance of detail coefficients (a - b).
fn variance_of_diff(a: &[f32], b: &[f32], n: usize) -> f32 {
    let mean = a[..n]
        .iter()
        .zip(b[..n].iter())
        .map(|(x, y)| x - y)
        .sum::<f32>()
        / n as f32;
    a[..n]
        .iter()
        .zip(b[..n].iter())
        .map(|(x, y)| {
            let d = (x - y) - mean;
            d * d
        })
        .sum::<f32>()
        / n as f32
}

impl Filter for NoiseReduction {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        // À trous wavelet: B3 kernel (5 taps) at step = 2^scale.
        // Max reach = 2 * 2^(scales-1) for the coarsest scale.
        let max_step = 1u32 << (self.scales.max(1) - 1);
        2 * max_step
    }

    fn tag(&self) -> crate::filter_compat::FilterTag {
        crate::filter_compat::FilterTag::NoiseReduction
    }
    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.is_identity() {
            return;
        }

        let w = planes.width as usize;
        let h = planes.height as usize;

        if self.luminance > 1e-6 {
            let params = DenoiseParams {
                strength: self.luminance,
                detail_preserve: self.detail,
                contrast_preserve: self.luminance_contrast,
                num_scales: self.scales,
            };
            denoise_plane(&mut planes.l, w, h, &params, ctx);
        }

        if self.chroma > 1e-6 {
            // Chroma noise is typically stronger — use higher effective strength
            let params = DenoiseParams {
                strength: self.chroma * 1.5,
                detail_preserve: self.chroma_detail * 0.4,
                contrast_preserve: 0.0, // no contrast preservation for chroma
                num_scales: self.scales,
            };
            denoise_plane(&mut planes.a, w, h, &params, ctx);
            denoise_plane(&mut planes.b, w, h, &params, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_strength_is_identity() {
        let mut planes = OklabPlanes::new(32, 32);
        for v in &mut planes.l {
            *v = 0.5;
        }
        let orig = planes.l.clone();
        NoiseReduction::default().apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, orig);
    }

    #[test]
    fn reduces_noise() {
        let mut planes = OklabPlanes::new(64, 64);
        // Add noise to a flat signal
        for (i, v) in planes.l.iter_mut().enumerate() {
            let noise = ((i as u32).wrapping_mul(2654435761) as f32 / u32::MAX as f32) * 0.1 - 0.05;
            *v = 0.5 + noise;
        }
        let before_std = std_dev(&planes.l);

        let mut nr = NoiseReduction::default();
        nr.luminance = 0.8;
        nr.apply(&mut planes, &mut FilterContext::new());

        let after_std = std_dev(&planes.l);
        assert!(
            after_std < before_std * 0.8,
            "noise should be reduced: {before_std} -> {after_std}"
        );
    }

    #[test]
    fn preserves_structure() {
        let mut planes = OklabPlanes::new(64, 64);
        // Create a gradient with noise
        for y in 0..64 {
            for x in 0..64 {
                let i = y * 64 + x;
                let base = y as f32 / 63.0;
                let noise =
                    ((i as u32).wrapping_mul(2654435761) as f32 / u32::MAX as f32) * 0.02 - 0.01;
                planes.l[i] = base + noise;
            }
        }

        let mut nr = NoiseReduction::default();
        nr.luminance = 0.5;
        nr.apply(&mut planes, &mut FilterContext::new());

        // The gradient should still be visible: top should be brighter than bottom
        let top_mean: f32 = planes.l[60 * 64..64 * 64].iter().sum::<f32>() / 256.0;
        let bottom_mean: f32 = planes.l[0..4 * 64].iter().sum::<f32>() / 256.0;
        assert!(
            top_mean > bottom_mean + 0.5,
            "gradient structure should be preserved: top={top_mean} bottom={bottom_mean}"
        );
    }

    #[test]
    fn chroma_denoising() {
        let mut planes = OklabPlanes::new(32, 32);
        for v in &mut planes.l {
            *v = 0.5;
        }
        // Add chroma noise
        for (i, v) in planes.a.iter_mut().enumerate() {
            let noise = ((i as u32).wrapping_mul(2654435761) as f32 / u32::MAX as f32) * 0.1 - 0.05;
            *v = noise;
        }
        let before_std = std_dev(&planes.a);

        let mut nr = NoiseReduction::default();
        nr.chroma = 0.8;
        nr.apply(&mut planes, &mut FilterContext::new());

        let after_std = std_dev(&planes.a);
        assert!(
            after_std < before_std * 0.8,
            "chroma noise should be reduced: {before_std} -> {after_std}"
        );
    }

    #[test]
    fn soft_threshold_works() {
        assert!((soft_threshold(0.5, 0.3) - 0.2).abs() < 1e-6);
        assert!((soft_threshold(-0.5, 0.3) - (-0.2)).abs() < 1e-6);
        assert_eq!(soft_threshold(0.1, 0.3), 0.0);
    }

    fn std_dev(data: &[f32]) -> f32 {
        let mean = data.iter().sum::<f32>() / data.len() as f32;
        let variance =
            data.iter().map(|&v| (v - mean) * (v - mean)).sum::<f32>() / data.len() as f32;
        variance.sqrt()
    }
}
