/// Maximum supported kernel radius. Sigma 50 → radius 150 → 301 weights.
/// This covers any realistic blur sigma.
const MAX_KERNEL_SIZE: usize = 512;

/// Precomputed separable Gaussian kernel.
///
/// The kernel is symmetric with `2 * radius + 1` pre-normalized weights
/// that sum to 1.0. Used by clarity, brilliance, and sharpen filters.
///
/// Weights are stored inline (no heap allocation) to avoid per-apply allocs.
#[derive(Clone, Debug)]
pub struct GaussianKernel {
    /// Pre-normalized weights. Only `[0..len]` are valid.
    weights_buf: [f32; MAX_KERNEL_SIZE],
    /// Number of valid weights (`2 * radius + 1`).
    len: usize,
    /// Kernel radius in pixels.
    pub radius: usize,
}

impl GaussianKernel {
    /// Create a Gaussian kernel for the given sigma.
    ///
    /// Radius is `ceil(3 * sigma)`. Weights are pre-normalized to sum to 1.0.
    ///
    /// # Panics
    /// Panics if the kernel size exceeds the maximum (sigma > ~85).
    pub fn new(sigma: f32) -> Self {
        let radius = (sigma * 3.0).ceil() as usize;
        let len = radius * 2 + 1;
        assert!(
            len <= MAX_KERNEL_SIZE,
            "kernel too large: sigma={sigma}, size={len} > {MAX_KERNEL_SIZE}"
        );
        let sigma2 = 2.0 * sigma * sigma;
        let mut weights_buf = [0.0f32; MAX_KERNEL_SIZE];
        let mut sum = 0.0f32;
        for (i, wt) in weights_buf.iter_mut().enumerate().take(len) {
            let x = i as f32 - radius as f32;
            let w = (-x * x / sigma2).exp();
            *wt = w;
            sum += w;
        }
        let inv_sum = 1.0 / sum;
        for w in &mut weights_buf[..len] {
            *w *= inv_sum;
        }
        Self {
            weights_buf,
            len,
            radius,
        }
    }

    /// The kernel weights slice.
    #[inline]
    pub fn weights(&self) -> &[f32] {
        &self.weights_buf[..self.len]
    }
}

/// Pad a row with edge replication for the blur boundary.
fn pad_row(src: &[f32], radius: usize, padded: &mut Vec<f32>) {
    padded.clear();
    let edge_l = src[0];
    let edge_r = src[src.len() - 1];
    padded.extend(core::iter::repeat_n(edge_l, radius));
    padded.extend_from_slice(src);
    padded.extend(core::iter::repeat_n(edge_r, radius));
}

use crate::context::FilterContext;

/// Separable Gaussian blur on a single f32 plane.
///
/// Performs horizontal then vertical pass. The result is written to `dst`.
/// `src` and `dst` must both be `width * height` elements.
/// `ctx` provides pooled scratch buffers for the intermediate horizontal pass.
pub fn gaussian_blur_plane(
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    kernel: &GaussianKernel,
    ctx: &mut FilterContext,
) {
    crate::simd::gaussian_blur_plane_dispatch(src, dst, width, height, kernel, ctx);
}

/// Scalar implementation of separable Gaussian blur.
pub(crate) fn gaussian_blur_plane_scalar(
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    kernel: &GaussianKernel,
    ctx: &mut FilterContext,
) {
    let w = width as usize;
    let h = height as usize;
    let radius = kernel.radius;

    // Temp buffer for horizontal pass output
    let mut h_buf = ctx.take_f32(w * h);
    let mut padded = ctx.take_f32(w + 2 * radius);

    // Horizontal pass
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        pad_row(row, radius, &mut padded);
        let out_row = &mut h_buf[y * w..(y + 1) * w];
        for x in 0..w {
            let mut sum = 0.0f32;
            for (k, &weight) in kernel.weights().iter().enumerate() {
                sum += padded[x + k] * weight;
            }
            out_row[x] = sum;
        }
    }

    // Vertical pass
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0.0f32;
            for (k, &weight) in kernel.weights().iter().enumerate() {
                let sy = (y + k).saturating_sub(radius).min(h - 1);
                sum += h_buf[sy * w + x] * weight;
            }
            dst[y * w + x] = sum;
        }
    }

    ctx.return_f32(padded);
    ctx.return_f32(h_buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_plane_stays_constant() {
        let w = 64u32;
        let h = 64u32;
        let src = vec![0.5f32; (w * h) as usize];
        let mut dst = vec![0.0f32; (w * h) as usize];
        let kernel = GaussianKernel::new(3.0);
        gaussian_blur_plane(&src, &mut dst, w, h, &kernel, &mut FilterContext::new());
        for &v in &dst {
            assert!(
                (v - 0.5).abs() < 0.01,
                "constant plane should stay constant, got {v}"
            );
        }
    }

    #[test]
    fn kernel_weights_sum_to_one() {
        let kernel = GaussianKernel::new(5.0);
        let sum: f32 = kernel.weights().iter().sum();
        assert!((sum - 1.0).abs() < 1e-5, "weights sum = {sum}");
    }

    #[test]
    fn zero_sigma_is_identity() {
        // sigma=0.01 with radius=1, should be near-identity
        let kernel = GaussianKernel::new(0.01);
        assert_eq!(kernel.radius, 1);
        // Center weight should dominate
        assert!(kernel.weights()[1] > 0.99);
    }
}
