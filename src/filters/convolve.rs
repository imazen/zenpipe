use crate::access::ChannelAccess;
use crate::blur::GaussianKernel;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::prelude::*;

/// Convolution kernel — separable (fast 2-pass) or general (NxM).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ConvolutionKernel {
    /// Separable kernel: outer product of `h_coeffs ⊗ v_coeffs`.
    /// Applied as horizontal pass then vertical pass → O(w+h) per pixel.
    Separable {
        /// Horizontal coefficients (length = 2*radius + 1).
        h_coeffs: Vec<f32>,
        /// Vertical coefficients (length = 2*radius + 1).
        v_coeffs: Vec<f32>,
    },
    /// General NxM kernel applied directly → O(N*M) per pixel.
    /// Coefficients in row-major order.
    Matrix {
        /// Kernel coefficients, row-major, `width * height` elements.
        coeffs: Vec<f32>,
        /// Kernel width (odd for centered kernel).
        width: usize,
        /// Kernel height (odd for centered kernel).
        height: usize,
    },
}

impl ConvolutionKernel {
    /// Symmetric separable Gaussian kernel.
    pub fn gaussian(sigma: f32) -> Self {
        let gk = GaussianKernel::new(sigma);
        let weights = gk.weights().to_vec();
        Self::Separable {
            h_coeffs: weights.clone(),
            v_coeffs: weights,
        }
    }

    /// Symmetric separable box blur kernel.
    pub fn box_blur(radius: usize) -> Self {
        let size = 2 * radius + 1;
        let w = 1.0 / size as f32;
        let coeffs = vec![w; size];
        Self::Separable {
            h_coeffs: coeffs.clone(),
            v_coeffs: coeffs,
        }
    }

    /// 3×3 Emboss kernel (directional light from top-left).
    ///
    /// ```text
    /// [-2 -1  0]
    /// [-1  0  1]
    /// [ 0  1  2]
    /// ```
    ///
    /// Zero-sum kernel — output is 0 for flat regions.
    /// Requires `bias = 0.5` to center output around mid-gray.
    pub fn emboss() -> Self {
        Self::Matrix {
            coeffs: vec![-2.0, -1.0, 0.0, -1.0, 0.0, 1.0, 0.0, 1.0, 2.0],
            width: 3,
            height: 3,
        }
    }

    /// Emboss with configurable light angle (in degrees, 0 = right, 90 = down).
    ///
    /// Interpolates a 3×3 emboss kernel for arbitrary directions.
    pub fn emboss_angle(angle_deg: f32) -> Self {
        let a = angle_deg.to_radians();
        let (sin_a, cos_a) = (a.sin(), a.cos());

        // Build directional emboss from gradient components
        // Horizontal gradient kernel scaled by cos(angle):
        //   [-1  0  1]
        //   [-2  0  2] * cos(a)
        //   [-1  0  1]
        // Vertical gradient kernel scaled by sin(angle):
        //   [-1 -2 -1]
        //   [ 0  0  0] * sin(a)
        //   [ 1  2  1]
        let gx = [-1.0, 0.0, 1.0, -2.0, 0.0, 2.0, -1.0, 0.0, 1.0];
        let gy = [-1.0, -2.0, -1.0, 0.0, 0.0, 0.0, 1.0, 2.0, 1.0];

        let mut coeffs = vec![0.0f32; 9];
        for i in 0..9 {
            coeffs[i] = gx[i] * cos_a + gy[i] * sin_a;
        }

        Self::Matrix {
            coeffs,
            width: 3,
            height: 3,
        }
    }

    /// 3×3 isotropic ridge detection kernel.
    ///
    /// ```text
    /// [-1 -1 -1]
    /// [-1  8 -1]
    /// [-1 -1 -1]
    /// ```
    pub fn ridge_detect() -> Self {
        Self::Matrix {
            coeffs: vec![-1.0, -1.0, -1.0, -1.0, 8.0, -1.0, -1.0, -1.0, -1.0],
            width: 3,
            height: 3,
        }
    }

    /// 3×3 Sharpen kernel (unsharp mask in kernel form).
    ///
    /// ```text
    /// [ 0 -1  0]
    /// [-1  5 -1]
    /// [ 0 -1  0]
    /// ```
    pub fn sharpen_3x3() -> Self {
        Self::Matrix {
            coeffs: vec![0.0, -1.0, 0.0, -1.0, 5.0, -1.0, 0.0, -1.0, 0.0],
            width: 3,
            height: 3,
        }
    }

    /// Custom separable kernel from horizontal and vertical coefficient vectors.
    pub fn custom_separable(h_coeffs: Vec<f32>, v_coeffs: Vec<f32>) -> Self {
        Self::Separable { h_coeffs, v_coeffs }
    }

    /// Custom matrix kernel from row-major coefficients.
    pub fn custom_matrix(coeffs: Vec<f32>, width: usize, height: usize) -> Self {
        assert_eq!(
            coeffs.len(),
            width * height,
            "kernel coefficients length must equal width * height"
        );
        Self::Matrix {
            coeffs,
            width,
            height,
        }
    }

    /// Half-width of the kernel in each direction.
    pub fn radius(&self) -> usize {
        match self {
            Self::Separable { h_coeffs, v_coeffs } => {
                let hr = h_coeffs.len() / 2;
                let vr = v_coeffs.len() / 2;
                hr.max(vr)
            }
            Self::Matrix { width, height, .. } => {
                let wr = width / 2;
                let hr = height / 2;
                wr.max(hr)
            }
        }
    }
}

/// Which channels the convolution targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ConvolveTarget {
    /// L channel only (edge detection, detail, emboss).
    #[default]
    LOnly,
    /// L + chroma (a, b).
    All,
}

/// Generic convolution filter supporting separable and matrix kernels.
///
/// Applies a user-defined convolution kernel to the image planes.
/// For separable kernels (Gaussian, box), uses efficient two-pass
/// horizontal+vertical processing. For arbitrary matrix kernels (emboss,
/// ridge detect), uses direct 2D convolution.
///
/// # Factory kernels
///
/// `ConvolutionKernel` provides factory methods for common operations:
/// - `gaussian(sigma)` — separable Gaussian blur
/// - `box_blur(radius)` — separable uniform blur
/// - `sharpen_3x3()` — 3×3 unsharp mask
/// - `emboss()` / `emboss_angle(deg)` — directional light emboss
/// - `ridge_detect()` — isotropic edge enhancement
/// - `custom_separable(h, v)` / `custom_matrix(coeffs, w, h)` — user-defined
///
/// # Bias
///
/// Some kernels (emboss, Sobel) produce values centered around 0.
/// Set `bias` to 0.5 to remap these to the [0, 1] range for display.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Convolve {
    /// The convolution kernel.
    pub kernel: ConvolutionKernel,
    /// Normalize kernel weights to sum to 1.0 before applying.
    /// Default: false (kernels are used as-is).
    pub normalize: bool,
    /// Which channels to convolve.
    pub target: ConvolveTarget,
    /// Constant added to each output pixel after convolution.
    /// Use 0.5 for emboss-style kernels to center around mid-gray.
    pub bias: f32,
}

impl Default for Convolve {
    fn default() -> Self {
        Self {
            kernel: ConvolutionKernel::sharpen_3x3(),
            normalize: false,
            target: ConvolveTarget::LOnly,
            bias: 0.0,
        }
    }
}

impl Convolve {
    /// Create with a specific kernel.
    pub fn new(kernel: ConvolutionKernel) -> Self {
        Self {
            kernel,
            ..Default::default()
        }
    }

    /// Set the target channels.
    pub fn with_target(mut self, target: ConvolveTarget) -> Self {
        self.target = target;
        self
    }

    /// Set normalization.
    pub fn with_normalize(mut self, normalize: bool) -> Self {
        self.normalize = normalize;
        self
    }

    /// Set bias offset.
    pub fn with_bias(mut self, bias: f32) -> Self {
        self.bias = bias;
        self
    }
}

impl Filter for Convolve {
    fn channel_access(&self) -> ChannelAccess {
        match self.target {
            ConvolveTarget::LOnly => ChannelAccess::L_ONLY,
            ConvolveTarget::All => ChannelAccess::ALL,
        }
    }

    fn plane_semantics(&self) -> crate::filter::PlaneSemantics {
        crate::filter::PlaneSemantics::Any
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        self.kernel.radius() as u32
    }

    fn tag(&self) -> crate::filter_compat::FilterTag {
        crate::filter_compat::FilterTag::Other
    }

    fn resize_phase(&self) -> crate::filter::ResizePhase {
        crate::filter::ResizePhase::Either
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        let w = planes.width as usize;
        let h = planes.height as usize;

        match &self.kernel {
            ConvolutionKernel::Separable { h_coeffs, v_coeffs } => {
                self.apply_separable(planes, ctx, w, h, h_coeffs, v_coeffs);
            }
            ConvolutionKernel::Matrix {
                coeffs,
                width: kw,
                height: kh,
            } => {
                self.apply_matrix(planes, ctx, w, h, coeffs, *kw, *kh);
            }
        }
    }
}

impl Convolve {
    fn apply_separable(
        &self,
        planes: &mut OklabPlanes,
        ctx: &mut FilterContext,
        w: usize,
        h: usize,
        h_coeffs: &[f32],
        v_coeffs: &[f32],
    ) {
        let (h_coeffs, v_coeffs) = if self.normalize {
            (normalize_coeffs(h_coeffs), normalize_coeffs(v_coeffs))
        } else {
            (h_coeffs.to_vec(), v_coeffs.to_vec())
        };

        let convolve_plane = |src: &mut Vec<f32>, ctx: &mut FilterContext| {
            let n = w * h;
            let hr = h_coeffs.len() / 2;
            let vr = v_coeffs.len() / 2;

            // Horizontal pass
            let mut h_buf = ctx.take_f32(n);
            let mut padded = ctx.take_f32(w + 2 * hr);
            for y in 0..h {
                let row = &src[y * w..(y + 1) * w];
                pad_row(row, hr, &mut padded);
                let out_row = &mut h_buf[y * w..(y + 1) * w];
                for x in 0..w {
                    let mut sum = 0.0f32;
                    for (k, &weight) in h_coeffs.iter().enumerate() {
                        sum += padded[x + k] * weight;
                    }
                    out_row[x] = sum;
                }
            }
            ctx.return_f32(padded);

            // Vertical pass
            let mut dst = ctx.take_f32(n);
            for y in 0..h {
                for x in 0..w {
                    let mut sum = 0.0f32;
                    for (k, &weight) in v_coeffs.iter().enumerate() {
                        let sy = (y + k).saturating_sub(vr).min(h - 1);
                        sum += h_buf[sy * w + x] * weight;
                    }
                    dst[y * w + x] = (sum + self.bias).clamp(0.0, 1.0);
                }
            }
            ctx.return_f32(h_buf);

            let old = core::mem::replace(src, dst);
            ctx.return_f32(old);
        };

        convolve_plane(&mut planes.l, ctx);
        if self.target == ConvolveTarget::All {
            convolve_plane(&mut planes.a, ctx);
            convolve_plane(&mut planes.b, ctx);
            if let Some(alpha) = &mut planes.alpha {
                convolve_plane(alpha, ctx);
            }
        }
    }

    fn apply_matrix(
        &self,
        planes: &mut OklabPlanes,
        ctx: &mut FilterContext,
        w: usize,
        h: usize,
        coeffs: &[f32],
        kw: usize,
        kh: usize,
    ) {
        let coeffs = if self.normalize {
            normalize_coeffs(coeffs)
        } else {
            coeffs.to_vec()
        };

        let bias = self.bias;

        let convolve_plane = |src: &mut Vec<f32>, ctx: &mut FilterContext| {
            let n = w * h;
            let rx = kw / 2;
            let ry = kh / 2;

            let mut dst = ctx.take_f32(n);

            for y in 0..h {
                for x in 0..w {
                    let mut sum = 0.0f32;
                    for ky in 0..kh {
                        for kx in 0..kw {
                            let sy =
                                (y as isize + ky as isize - ry as isize).clamp(0, h as isize - 1)
                                    as usize;
                            let sx =
                                (x as isize + kx as isize - rx as isize).clamp(0, w as isize - 1)
                                    as usize;
                            sum += src[sy * w + sx] * coeffs[ky * kw + kx];
                        }
                    }
                    dst[y * w + x] = (sum + bias).clamp(0.0, 1.0);
                }
            }

            let old = core::mem::replace(src, dst);
            ctx.return_f32(old);
        };

        convolve_plane(&mut planes.l, ctx);
        if self.target == ConvolveTarget::All {
            convolve_plane(&mut planes.a, ctx);
            convolve_plane(&mut planes.b, ctx);
            if let Some(alpha) = &mut planes.alpha {
                convolve_plane(alpha, ctx);
            }
        }
    }
}

/// Pad a row with edge replication.
fn pad_row(src: &[f32], radius: usize, padded: &mut Vec<f32>) {
    padded.clear();
    let edge_l = src[0];
    let edge_r = src[src.len() - 1];
    padded.extend(core::iter::repeat_n(edge_l, radius));
    padded.extend_from_slice(src);
    padded.extend(core::iter::repeat_n(edge_r, radius));
}

/// Normalize coefficients so they sum to 1.0.
fn normalize_coeffs(coeffs: &[f32]) -> Vec<f32> {
    let sum: f32 = coeffs.iter().sum();
    if sum.abs() < 1e-10 {
        return coeffs.to_vec();
    }
    let inv = 1.0 / sum;
    coeffs.iter().map(|&c| c * inv).collect()
}

// ─── Param schema ──────────────────────────────────────────────────

static CONVOLVE_SCHEMA: FilterSchema = FilterSchema {
    name: "convolve",
    label: "Convolve",
    description: "Generic convolution with separable or matrix kernels",
    group: FilterGroup::Detail,
    params: &[
        ParamDesc {
            name: "normalize",
            label: "Normalize",
            description: "Normalize kernel weights to sum to 1.0",
            kind: ParamKind::Bool { default: false },
            unit: "",
            section: "Main",
            slider: SliderMapping::NotSlider,
        },
        ParamDesc {
            name: "bias",
            label: "Bias",
            description: "Constant added after convolution (0.5 for emboss)",
            kind: ParamKind::Float {
                min: -1.0,
                max: 1.0,
                default: 0.0,
                identity: 0.0,
                step: 0.05,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for Convolve {
    fn schema() -> &'static FilterSchema {
        &CONVOLVE_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "normalize" => Some(ParamValue::Bool(self.normalize)),
            "bias" => Some(ParamValue::Float(self.bias)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        match name {
            "normalize" => {
                if let ParamValue::Bool(v) = value {
                    self.normalize = v;
                    true
                } else {
                    false
                }
            }
            "bias" => {
                if let Some(v) = value.as_f32() {
                    self.bias = v;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::FilterContext;

    #[test]
    fn identity_kernel() {
        // 1×1 kernel with weight 1.0 should be identity
        let conv = Convolve::new(ConvolutionKernel::custom_matrix(vec![1.0], 1, 1));
        let mut planes = OklabPlanes::new(16, 16);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = (i as f32 / 256.0).clamp(0.0, 1.0);
        }
        let original = planes.l.clone();
        conv.apply(&mut planes, &mut FilterContext::new());
        for (i, (&got, &expected)) in planes.l.iter().zip(original.iter()).enumerate() {
            assert!(
                (got - expected).abs() < 1e-5,
                "pixel {i}: expected {expected}, got {got}"
            );
        }
    }

    #[test]
    fn constant_plane_with_normalized_kernel() {
        let conv = Convolve::new(ConvolutionKernel::box_blur(2)).with_normalize(true);
        let mut planes = OklabPlanes::new(32, 32);
        planes.l.fill(0.5);
        conv.apply(&mut planes, &mut FilterContext::new());
        for &v in &planes.l {
            assert!(
                (v - 0.5).abs() < 0.01,
                "constant plane should stay ~0.5, got {v}"
            );
        }
    }

    #[test]
    fn box_blur_smooths_step() {
        let conv = Convolve::new(ConvolutionKernel::box_blur(1));
        let mut planes = OklabPlanes::new(32, 32);
        for y in 0..32u32 {
            for x in 0..32u32 {
                let i = planes.index(x, y);
                planes.l[i] = if x < 16 { 0.2 } else { 0.8 };
            }
        }
        conv.apply(&mut planes, &mut FilterContext::new());
        // Edge should be smoothed
        let edge = planes.l[planes.index(16, 16)];
        assert!(
            edge > 0.3 && edge < 0.7,
            "edge should be smoothed, got {edge}"
        );
    }

    #[test]
    fn emboss_detects_structure() {
        let conv = Convolve::new(ConvolutionKernel::emboss()).with_bias(0.5);
        let mut planes = OklabPlanes::new(32, 32);
        for y in 0..32u32 {
            for x in 0..32u32 {
                let i = planes.index(x, y);
                planes.l[i] = if x < 16 { 0.3 } else { 0.7 };
            }
        }
        conv.apply(&mut planes, &mut FilterContext::new());
        // Interior flat areas should be near 0.5 (bias), edges should differ
        let interior = planes.l[planes.index(8, 16)];
        let edge = planes.l[planes.index(15, 16)];
        assert!(
            (interior - 0.5).abs() < 0.1,
            "flat interior should be ~0.5, got {interior}"
        );
        assert!(
            (edge - 0.5).abs() > 0.05,
            "edge should differ from 0.5, got {edge}"
        );
    }

    #[test]
    fn separable_gaussian_matches_gaussian_kernel() {
        let sigma = 2.0;
        let conv = Convolve::new(ConvolutionKernel::gaussian(sigma));
        let mut planes = OklabPlanes::new(32, 32);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = ((i % 32) as f32 / 31.0).clamp(0.0, 1.0);
        }
        let before = planes.l.clone();
        conv.apply(&mut planes, &mut FilterContext::new());
        // Should be different from input (it's blurred)
        let mut diff = 0.0f32;
        for (&a, &b) in before.iter().zip(planes.l.iter()) {
            diff += (a - b).abs();
        }
        assert!(diff > 1.0, "Gaussian should blur the gradient, diff={diff}");
    }

    #[test]
    fn ridge_detect_highlights_edges() {
        let conv = Convolve::new(ConvolutionKernel::ridge_detect());
        let mut planes = OklabPlanes::new(32, 32);
        // Step edge at x=16
        for y in 0..32u32 {
            for x in 0..32u32 {
                let i = planes.index(x, y);
                planes.l[i] = if x < 16 { 0.2 } else { 0.8 };
            }
        }
        conv.apply(&mut planes, &mut FilterContext::new());
        let interior = planes.l[planes.index(8, 16)];
        // x=16 is the bright side of the edge — ridge detect gives positive response
        // (center brighter than some neighbors)
        let edge = planes.l[planes.index(16, 16)];
        assert!(
            edge > interior,
            "ridge detect should highlight edge: edge={edge}, interior={interior}"
        );
    }

    #[test]
    fn all_channels_mode_convolves_chroma() {
        let conv =
            Convolve::new(ConvolutionKernel::box_blur(1)).with_target(ConvolveTarget::All);
        let mut planes = OklabPlanes::new(16, 16);
        // Sharp chroma step
        for y in 0..16u32 {
            for x in 0..16u32 {
                let i = planes.index(x, y);
                planes.a[i] = if x < 8 { -0.1 } else { 0.1 };
            }
        }
        conv.apply(&mut planes, &mut FilterContext::new());
        // Edge should be blurred
        let edge = planes.a[planes.index(8, 8)];
        assert!(
            edge.abs() < 0.08,
            "chroma edge should be smoothed, got {edge}"
        );
    }
}
