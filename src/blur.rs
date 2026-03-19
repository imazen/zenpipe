/// Maximum supported kernel radius. Sigma 50 → radius 150 → 301 weights.
/// This covers any realistic blur sigma.
use crate::prelude::*;
const MAX_KERNEL_SIZE: usize = 512;

/// Minimum sigma for the stackblur fast path.
///
/// Below this threshold, SIMD FIR convolution is faster. Above it, stackblur
/// wins because it's O(1)/pixel with only 2 memory passes (vs FIR's O(radius)).
///
/// Benchmark-derived crossover (2026-03-16, x86_64 AVX2):
///   σ=4:  FIR 8.4ms vs stackblur 14.2ms (1080p), FIR 37.7ms vs stackblur 63.2ms (4K)
///   σ=16: box/FIR 34ms vs stackblur 14.2ms (1080p), box/FIR 152ms vs stackblur 63ms (4K)
///   σ=30: box/FIR 34ms vs stackblur 14.3ms (1080p), box/FIR 153ms vs stackblur 63ms (4K)
///
/// Stackblur is 2.4× faster than box blur at all sigma values.
/// Crossover with FIR: ~σ=7. Using σ=6 (conservative, FIR at σ=6 is ~14ms).
const STACKBLUR_SIGMA_THRESHOLD: f32 = 6.0;

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
///
/// For sigma >= threshold, dispatches to stackblur (O(1)/pixel, 2.4× faster than box blur).
/// For smaller sigma, uses direct FIR convolution (kernel is tiny).
pub fn gaussian_blur_plane_scalar(
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    kernel: &GaussianKernel,
    ctx: &mut FilterContext,
) {
    let sigma = kernel_sigma(kernel);
    if should_use_stackblur(sigma) {
        let radius = sigma_to_stackblur_radius(sigma);
        stackblur_plane(src, dst, width, height, radius, ctx);
        return;
    }

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

/// Transpose a w×h row-major f32 plane to h×w row-major, using 8×8 tiles.
fn transpose_plane(src: &[f32], dst: &mut [f32], w: usize, h: usize) {
    const TILE: usize = 8;

    // Full tiles
    let ty_end = h / TILE * TILE;
    let tx_end = w / TILE * TILE;

    for ty in (0..ty_end).step_by(TILE) {
        for tx in (0..tx_end).step_by(TILE) {
            // Transpose 8×8 block
            for dy in 0..TILE {
                for dx in 0..TILE {
                    dst[(tx + dx) * h + ty + dy] = src[(ty + dy) * w + tx + dx];
                }
            }
        }
        // Right edge (columns tx_end..w)
        for dy in 0..TILE {
            for x in tx_end..w {
                dst[x * h + ty + dy] = src[(ty + dy) * w + x];
            }
        }
    }
    // Bottom edge (rows ty_end..h)
    for y in ty_end..h {
        for x in 0..w {
            dst[x * h + y] = src[y * w + x];
        }
    }
}

/// Check if sigma is large enough to benefit from the stackblur fast path.
pub fn should_use_stackblur(sigma: f32) -> bool {
    sigma >= STACKBLUR_SIGMA_THRESHOLD
}

/// Get sigma from a GaussianKernel (approximate, from radius).
pub fn kernel_sigma(kernel: &GaussianKernel) -> f32 {
    kernel.radius as f32 / 3.0
}

// ─── Stackblur (O(1) per pixel, pyramid kernel) ────────────────────

/// Stackblur on a single f32 plane.
///
/// Mario Klingemann's stackblur uses a pyramid-shaped kernel (triangle weights)
/// maintained via running sums. O(1) per pixel regardless of radius.
/// Single pass per direction — fewer memory passes than 3-pass box blur.
///
/// Kernel weights for radius r: `[1, 2, 3, ..., r, r+1, r, ..., 3, 2, 1]`
/// Divisor: `(r+1)²`
///
/// Accuracy is between single box blur and 3-pass extended box blur.
/// The pyramid kernel has no zeros in its frequency response (unlike box blur).
pub fn stackblur_plane(
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    radius: u32,
    ctx: &mut FilterContext,
) {
    if radius == 0 {
        dst.copy_from_slice(src);
        return;
    }

    let w = width as usize;
    let h = height as usize;
    let r = radius as usize;
    let n = w * h;

    // Intermediate buffer for horizontal pass output
    let mut h_buf = ctx.take_f32(n);

    // Stack (circular buffer) for the sliding window
    let stack_size = 2 * r + 1;
    let mut stack = ctx.take_f32(stack_size);
    let inv_div = 1.0 / ((r as f32 + 1.0) * (r as f32 + 1.0));

    // --- Horizontal pass ---
    for y in 0..h {
        let row = &src[y * w..][..w];
        let out = &mut h_buf[y * w..][..w];
        stackblur_row(row, out, w, r, &mut stack, inv_div);
    }

    // --- Vertical pass: transpose → horizontal stackblur → transpose ---
    let mut transposed = ctx.take_f32(n);
    let mut transposed_out = ctx.take_f32(n);
    transpose_plane(&h_buf, &mut transposed, w, h);

    // Reuse stack — might need bigger if h > w, but stack_size = 2*r+1 is fixed
    for x in 0..w {
        let row = &transposed[x * h..][..h];
        let out = &mut transposed_out[x * h..][..h];
        stackblur_row(row, out, h, r, &mut stack, inv_div);
    }

    transpose_plane(&transposed_out, dst, h, w);

    ctx.return_f32(transposed_out);
    ctx.return_f32(transposed);
    ctx.return_f32(stack);
    ctx.return_f32(h_buf);
}

/// Single-row stackblur with pyramid kernel.
///
/// `stack` must have length >= `2 * radius + 1`.
///
/// The algorithm maintains a circular buffer of pixel values. As the window
/// slides right by one pixel:
///   1. sum -= sum_out          (remove leaving pixels' accumulated weight)
///   2. Remove oldest from sum_out, insert new pixel into sum_in
///   3. sum += sum_in           (add entering pixels' accumulated weight)
///   4. Transfer the midpoint pixel from sum_in to sum_out
///
/// This produces a pyramid kernel [1, 2, ..., r, r+1, r, ..., 2, 1] with
/// divisor (r+1)² — each step increments the weight of entering pixels by 1
/// and decrements the weight of leaving pixels by 1.
pub(crate) fn stackblur_row(
    input: &[f32],
    output: &mut [f32],
    len: usize,
    radius: usize,
    stack: &mut [f32],
    inv_div: f32,
) {
    let r = radius;
    let stack_size = 2 * r + 1;

    let mut sum = 0.0f32;
    let mut sum_in = 0.0f32;
    let mut sum_out = 0.0f32;

    // Initialize: fill the stack with edge-replicated values for position x=0.
    // Stack layout: positions [-r, -r+1, ..., 0, ..., r-1, r] relative to center.
    // stack[i] holds the pixel value at offset (i - r) from center.
    let first = input[0];
    for slot in &mut stack[..=r] {
        *slot = first;
    }
    for (i, slot) in stack[r + 1..stack_size].iter_mut().enumerate() {
        let offset = i + 1; // offset from center (positive)
        *slot = if offset < len {
            input[offset]
        } else {
            input[len - 1]
        };
    }

    // Compute initial weighted sum.
    // Weight of stack[i] = r + 1 - |i - r|
    for (i, &val) in stack[..stack_size].iter().enumerate() {
        let weight = (r + 1 - i.abs_diff(r)) as f32;
        sum += val * weight;
    }

    // sum_out = values in the "out" half (positions 0..=r)
    for &val in &stack[..=r] {
        sum_out += val;
    }
    // sum_in = values in the "in" half (positions r+1..stack_size)
    for &val in &stack[r + 1..stack_size] {
        sum_in += val;
    }

    // sp points to the slot that will be overwritten next (the oldest "out" slot)
    let mut sp = 0usize;

    for (x, out) in output[..len].iter_mut().enumerate() {
        *out = sum * inv_div;

        // 1. Remove outgoing contribution
        sum -= sum_out;

        // 2. Remove the oldest value from sum_out
        sum_out -= stack[sp];

        // 3. Insert new pixel at the vacated slot
        let new_x = x + r + 1;
        let new_px = if new_x < len {
            input[new_x]
        } else {
            input[len - 1]
        };
        stack[sp] = new_px;
        sum_in += new_px;

        // 4. Add incoming contribution
        sum += sum_in;

        // 5. Advance stack pointer (wrapping)
        sp += 1;
        if sp >= stack_size {
            sp = 0;
        }

        // 6. The pixel at (sp + r) is the new center — transfer from in to out.
        // After sp advances, sp points to the leftmost (oldest out).
        // The center is r positions ahead in the circular buffer.
        let center_idx = if sp + r >= stack_size {
            sp + r - stack_size
        } else {
            sp + r
        };
        let center_val = stack[center_idx];
        sum_out += center_val;
        sum_in -= center_val;
    }
}

/// Convert sigma to stackblur radius.
///
/// Stackblur's pyramid kernel has variance `r*(r+2)/6` for radius r.
/// For equivalence to Gaussian with variance σ², solve: r*(r+2)/6 = σ²
/// → r ≈ sqrt(6*σ² + 1) - 1
pub fn sigma_to_stackblur_radius(sigma: f32) -> u32 {
    let r = (6.0 * sigma * sigma + 1.0).sqrt() - 1.0;
    r.round().max(1.0) as u32
}

// ─── Deriche IIR Gaussian blur (O(1) per pixel, high accuracy) ──────

/// Coefficients for Young & van Vliet 3rd-order recursive Gaussian filter.
///
/// Approximates a Gaussian blur using causal (forward) and anticausal (backward)
/// 3rd-order IIR passes. The combined filter has better accuracy than the 3-pass
/// extended box blur (~1e-3 vs ~2e-2 max error).
///
/// The filter recurrence is:
///   y[n] = B*x[n] + d1*y[n-1] + d2*y[n-2] + d3*y[n-3]
///
/// Reference: Young & van Vliet, "Recursive implementation of the Gaussian
/// filter," Signal Processing 44(2), 1995, pp. 139-151.
#[derive(Clone, Debug)]
#[allow(dead_code)] // Used via blur_internals re-export under `experimental` feature
pub struct DericheCoefficients {
    /// Feedforward gain (numerator coefficient).
    pub b: f32,
    /// Feedback coefficients d[0..3]: d1, d2, d3.
    pub d: [f32; 3],
}

#[allow(dead_code)]
impl DericheCoefficients {
    /// Compute 3rd-order recursive Gaussian coefficients for the given sigma.
    ///
    /// # Panics
    /// Panics if sigma < 0.5 (filter becomes inaccurate for very small sigma).
    pub fn new(sigma: f32) -> Self {
        assert!(
            sigma >= 0.5,
            "Recursive Gaussian requires sigma >= 0.5, got {sigma}"
        );

        let s = sigma as f64;

        // q parameter determines pole radius scaling
        let q = if s >= 2.5 {
            0.98711 * s - 0.96330
        } else {
            3.97156 - 4.14554 * (1.0 - 0.26891 * s).sqrt()
        };

        // Polynomial coefficients from Young & van Vliet (1995), Table I
        let q2 = q * q;
        let q3 = q2 * q;
        let b0 = 1.57825 + 2.44413 * q + 1.42810 * q2 + 0.422205 * q3;
        let b1 = 2.44413 * q + 2.85619 * q2 + 1.26661 * q3;
        let b2 = -(1.42810 * q2 + 1.26661 * q3);
        let b3 = 0.422205 * q3;

        // Feedback coefficients (positive convention):
        //   y[n] = B*x[n] + d1*y[n-1] + d2*y[n-2] + d3*y[n-3]
        let inv_b0 = 1.0 / b0;
        let d1 = (b1 * inv_b0) as f32;
        let d2 = (b2 * inv_b0) as f32;
        let d3 = (b3 * inv_b0) as f32;

        // Feedforward gain for unit DC response of combined causal+anticausal:
        //   Causal DC:     B / (1 - d1 - d2 - d3)
        //   Anticausal DC: B / (1 - d1 - d2 - d3)
        //   Combined:      y = y_f + y_b - B*x  →  DC = 2B/(1-d1-d2-d3) - B
        //   Set DC = 1:    B = (1 - d1 - d2 - d3) / (2 - (1 - d1 - d2 - d3))
        //                    = (1 - d1 - d2 - d3) / (1 + d1 + d2 + d3)
        let sum_d = d1 + d2 + d3;
        let b_gain = (1.0 - sum_d) / (1.0 + sum_d);

        Self {
            b: b_gain,
            d: [d1, d2, d3],
        }
    }
}

/// Apply causal + anticausal IIR on a contiguous row.
///
/// `y_f` and `y_b` are scratch buffers of length >= `len`.
#[allow(dead_code, clippy::too_many_arguments)]
fn iir_row(
    input: &[f32],
    output: &mut [f32],
    len: usize,
    b: f32,
    d1: f32,
    d2: f32,
    d3: f32,
    y_f: &mut [f32],
    y_b: &mut [f32],
) {
    if len == 0 {
        return;
    }

    // Boundary initialization: assume constant extension at edges.
    // For constant input c: y_steady = B*c / (1 - d1 - d2 - d3)
    // But we want each pass alone to converge to c * B/(1-d1-d2-d3),
    // so we initialize the state to that steady-state value.
    let edge_l = input[0];
    let inv_denom = 1.0 / (1.0 - d1 - d2 - d3);
    let init_f = b * edge_l * inv_denom;

    // Causal (forward) pass
    y_f[0] = b * input[0] + d1 * init_f + d2 * init_f + d3 * init_f;
    if len > 1 {
        y_f[1] = b * input[1] + d1 * y_f[0] + d2 * init_f + d3 * init_f;
    }
    if len > 2 {
        y_f[2] = b * input[2] + d1 * y_f[1] + d2 * y_f[0] + d3 * init_f;
    }
    for n in 3..len {
        y_f[n] = b * input[n] + d1 * y_f[n - 1] + d2 * y_f[n - 2] + d3 * y_f[n - 3];
    }

    // Anticausal (backward) pass
    let edge_r = input[len - 1];
    let init_b = b * edge_r * inv_denom;

    y_b[len - 1] = b * input[len - 1] + d1 * init_b + d2 * init_b + d3 * init_b;
    if len > 1 {
        y_b[len - 2] = b * input[len - 2] + d1 * y_b[len - 1] + d2 * init_b + d3 * init_b;
    }
    if len > 2 {
        y_b[len - 3] = b * input[len - 3] + d1 * y_b[len - 2] + d2 * y_b[len - 1] + d3 * init_b;
    }
    for n in (0..len.saturating_sub(3)).rev() {
        y_b[n] = b * input[n] + d1 * y_b[n + 1] + d2 * y_b[n + 2] + d3 * y_b[n + 3];
    }

    // Combine: output = causal + anticausal - B * input
    // (subtract B*x to avoid double-counting the current sample)
    for n in 0..len {
        output[n] = y_f[n] + y_b[n] - b * input[n];
    }
}

/// Recursive IIR Gaussian blur on a single f32 plane.
///
/// Uses 3rd-order recursive filter (causal + anticausal) in both directions.
/// O(1) per pixel regardless of sigma. Better accuracy than box blur.
///
/// Vertical pass uses transpose → horizontal IIR → transpose for cache locality.
#[allow(dead_code)]
pub fn deriche_blur_plane(
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    coeffs: &DericheCoefficients,
    ctx: &mut FilterContext,
) {
    let w = width as usize;
    let h = height as usize;
    let n = w * h;

    let b = coeffs.b;
    let d1 = coeffs.d[0];
    let d2 = coeffs.d[1];
    let d3 = coeffs.d[2];

    // Intermediate buffer for horizontal pass output
    let mut h_buf = ctx.take_f32(n);

    // Scratch for per-row causal/anticausal
    let max_dim = w.max(h);
    let mut y_f = ctx.take_f32(max_dim);
    let mut y_b = ctx.take_f32(max_dim);

    // --- Horizontal pass: causal + anticausal per row ---
    for row_idx in 0..h {
        let row = &src[row_idx * w..(row_idx + 1) * w];
        let out = &mut h_buf[row_idx * w..(row_idx + 1) * w];
        iir_row(row, out, w, b, d1, d2, d3, &mut y_f, &mut y_b);
    }

    // --- Vertical pass: transpose → horizontal IIR → transpose ---
    let mut transposed = ctx.take_f32(n);
    let mut transposed_out = ctx.take_f32(n);
    transpose_plane(&h_buf, &mut transposed, w, h);

    // Apply horizontal IIR on transposed data (h elements per row, w rows)
    for row_idx in 0..w {
        let in_start = row_idx * h;
        let in_row = &transposed[in_start..in_start + h];
        let out_row = &mut transposed_out[in_start..in_start + h];
        iir_row(in_row, out_row, h, b, d1, d2, d3, &mut y_f, &mut y_b);
    }

    // Transpose back to row-major
    transpose_plane(&transposed_out, dst, h, w);

    ctx.return_f32(transposed_out);
    ctx.return_f32(transposed);
    ctx.return_f32(y_b);
    ctx.return_f32(y_f);
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

    #[test]
    fn fir_blur_gradient_accuracy() {
        // Non-trivial gradient image to verify tiled vertical pass produces
        // correct output. Compares dispatch path against a known-good naive
        // implementation that doesn't tile.
        let w = 200u32;
        let h = 150u32;
        let n = (w * h) as usize;
        let w_s = w as usize;
        let h_s = h as usize;

        let mut src = vec![0.0f32; n];
        for y in 0..h_s {
            for x in 0..w_s {
                src[y * w_s + x] = 0.1 + 0.8 * (x as f32 / w_s as f32);
            }
        }

        let kernel = GaussianKernel::new(4.0);
        let radius = kernel.radius;

        // Reference: naive (un-tiled) vertical pass
        let mut h_buf = vec![0.0f32; n];
        let mut padded = Vec::new();
        for y in 0..h_s {
            let row = &src[y * w_s..(y + 1) * w_s];
            pad_row(row, radius, &mut padded);
            for x in 0..w_s {
                let mut sum = 0.0f32;
                for (k, &weight) in kernel.weights().iter().enumerate() {
                    sum += padded[x + k] * weight;
                }
                h_buf[y * w_s + x] = sum;
            }
        }
        let mut ref_dst = vec![0.0f32; n];
        for y in 0..h_s {
            for x in 0..w_s {
                let mut sum = 0.0f32;
                for (k, &weight) in kernel.weights().iter().enumerate() {
                    let sy = (y + k).saturating_sub(radius).min(h_s - 1);
                    sum += h_buf[sy * w_s + x] * weight;
                }
                ref_dst[y * w_s + x] = sum;
            }
        }

        // Actual: dispatched path (uses tiled vertical)
        let mut actual_dst = vec![0.0f32; n];
        gaussian_blur_plane(
            &src,
            &mut actual_dst,
            w,
            h,
            &kernel,
            &mut FilterContext::new(),
        );

        let mut max_err = 0.0f32;
        for i in 0..n {
            let err = (ref_dst[i] - actual_dst[i]).abs();
            max_err = max_err.max(err);
        }
        assert!(
            max_err < 1e-5,
            "tiled FIR vs naive FIR max error = {max_err} (expected < 1e-5)"
        );
    }

    #[test]
    fn transpose_roundtrip() {
        let w = 37usize; // non-multiple of 8
        let h = 23usize;
        let mut src = vec![0.0f32; w * h];
        for (i, v) in src.iter_mut().enumerate() {
            *v = i as f32;
        }
        let mut transposed = vec![0.0f32; w * h];
        let mut back = vec![0.0f32; w * h];
        transpose_plane(&src, &mut transposed, w, h);
        transpose_plane(&transposed, &mut back, h, w);
        assert_eq!(src, back, "transpose roundtrip must be identity");
    }

    #[test]
    fn dispatch_uses_stackblur_for_large_sigma() {
        // Verify that gaussian_blur_plane with large sigma routes to stackblur
        // and produces reasonable results (constant image test).
        let w = 64u32;
        let h = 64u32;
        let src = vec![0.5f32; (w * h) as usize];
        let mut dst = vec![0.0f32; (w * h) as usize];
        let kernel = GaussianKernel::new(10.0);
        assert!(kernel.radius >= 5, "sigma=10 should have large radius");
        gaussian_blur_plane(&src, &mut dst, w, h, &kernel, &mut FilterContext::new());
        for &v in &dst {
            assert!(
                (v - 0.5).abs() < 0.01,
                "large-sigma constant plane: got {v}"
            );
        }
    }

    // ─── Deriche IIR tests ──────────────────────────────────────────

    // ─── Stackblur tests ─────────────────────────────────────────

    #[test]
    fn stackblur_constant_plane() {
        let w = 128u32;
        let h = 96u32;
        let src = vec![0.42f32; (w * h) as usize];
        let mut dst = vec![0.0f32; (w * h) as usize];
        stackblur_plane(&src, &mut dst, w, h, 10, &mut FilterContext::new());
        for (i, &v) in dst.iter().enumerate() {
            assert!(
                (v - 0.42).abs() < 1e-3,
                "stackblur constant plane pixel {i}: expected 0.42, got {v}"
            );
        }
    }

    #[test]
    fn stackblur_preserves_mean() {
        let w = 128u32;
        let h = 96u32;
        let n = (w * h) as usize;
        let mut src = vec![0.0f32; n];
        for (i, v) in src.iter_mut().enumerate() {
            *v = ((i as u32).wrapping_mul(2654435761) as f32 / u32::MAX as f32) * 0.8 + 0.1;
        }
        let src_mean: f32 = src.iter().sum::<f32>() / n as f32;

        let mut dst = vec![0.0f32; n];
        stackblur_plane(&src, &mut dst, w, h, 15, &mut FilterContext::new());
        let dst_mean: f32 = dst.iter().sum::<f32>() / n as f32;

        assert!(
            (src_mean - dst_mean).abs() < 0.02,
            "stackblur mean not preserved: src={src_mean}, dst={dst_mean}"
        );
    }

    #[test]
    fn stackblur_vs_fir_accuracy() {
        let w = 128u32;
        let h = 128u32;
        let n = (w * h) as usize;

        let mut src = vec![0.0f32; n];
        for y in 0..h as usize {
            for x in 0..w as usize {
                src[y * w as usize + x] = 0.1 + 0.8 * (x as f32 / w as f32) * (y as f32 / h as f32);
            }
        }

        let sigma = 5.0;
        let kernel = GaussianKernel::new(sigma);
        let sb_radius = sigma_to_stackblur_radius(sigma);

        let mut dst_fir = vec![0.0f32; n];
        gaussian_blur_plane_scalar(&src, &mut dst_fir, w, h, &kernel, &mut FilterContext::new());

        let mut dst_sb = vec![0.0f32; n];
        stackblur_plane(
            &src,
            &mut dst_sb,
            w,
            h,
            sb_radius,
            &mut FilterContext::new(),
        );

        let margin = (2.0 * sigma).ceil() as usize;
        let mut max_err = 0.0f32;
        let mut sum_sq_err = 0.0f64;
        let mut count = 0;
        for y in margin..h as usize - margin {
            for x in margin..w as usize - margin {
                let i = y * w as usize + x;
                let err = (dst_fir[i] - dst_sb[i]).abs();
                max_err = max_err.max(err);
                sum_sq_err += (err as f64) * (err as f64);
                count += 1;
            }
        }
        let rmse = (sum_sq_err / count as f64).sqrt();
        eprintln!(
            "stackblur vs FIR sigma={sigma} radius={sb_radius}: max_err={max_err:.6}, rmse={rmse:.6}"
        );
        assert!(
            max_err < 0.06,
            "stackblur vs FIR max error too large: {max_err}"
        );
        assert!(rmse < 0.02, "stackblur vs FIR RMSE too large: {rmse}");
    }

    #[test]
    fn sigma_to_stackblur_radius_sanity() {
        // Variance of pyramid kernel = r*(r+2)/6
        // For σ²: r = sqrt(6σ²+1) - 1
        // σ=4 → r≈9, σ=16 → r≈39, σ=30 → r≈73
        let r4 = sigma_to_stackblur_radius(4.0);
        let r16 = sigma_to_stackblur_radius(16.0);
        let r30 = sigma_to_stackblur_radius(30.0);
        assert!(r4 >= 8 && r4 <= 11, "sigma=4 radius={r4}");
        assert!(r16 >= 38 && r16 <= 41, "sigma=16 radius={r16}");
        assert!(r30 >= 72 && r30 <= 75, "sigma=30 radius={r30}");
    }

    // ─── Deriche IIR tests ──────────────────────────────────────────

    #[test]
    fn deriche_constant_plane() {
        let w = 128u32;
        let h = 96u32;
        let src = vec![0.42f32; (w * h) as usize];
        let mut dst = vec![0.0f32; (w * h) as usize];
        let coeffs = DericheCoefficients::new(8.0);
        deriche_blur_plane(&src, &mut dst, w, h, &coeffs, &mut FilterContext::new());
        for (i, &v) in dst.iter().enumerate() {
            assert!(
                (v - 0.42).abs() < 1e-3,
                "deriche constant plane pixel {i}: expected 0.42, got {v}"
            );
        }
    }

    #[test]
    fn deriche_preserves_mean() {
        let w = 128u32;
        let h = 96u32;
        let n = (w * h) as usize;
        let mut src = vec![0.0f32; n];
        for (i, v) in src.iter_mut().enumerate() {
            *v = ((i as u32).wrapping_mul(2654435761) as f32 / u32::MAX as f32) * 0.8 + 0.1;
        }
        let src_mean: f32 = src.iter().sum::<f32>() / n as f32;

        let mut dst = vec![0.0f32; n];
        let coeffs = DericheCoefficients::new(5.0);
        deriche_blur_plane(&src, &mut dst, w, h, &coeffs, &mut FilterContext::new());
        let dst_mean: f32 = dst.iter().sum::<f32>() / n as f32;

        assert!(
            (src_mean - dst_mean).abs() < 0.01,
            "deriche mean not preserved: src={src_mean}, dst={dst_mean}"
        );
    }

    #[test]
    fn deriche_vs_fir_accuracy() {
        // Deriche should match true Gaussian within ~1e-2 max error on a
        // gradient image (for sigma=5, away from boundaries).
        let w = 128u32;
        let h = 128u32;
        let n = (w * h) as usize;

        let mut src = vec![0.0f32; n];
        for y in 0..h as usize {
            for x in 0..w as usize {
                src[y * w as usize + x] = 0.1 + 0.8 * (x as f32 / w as f32) * (y as f32 / h as f32);
            }
        }

        let sigma = 5.0;
        let kernel = GaussianKernel::new(sigma);
        let coeffs = DericheCoefficients::new(sigma);

        // FIR reference
        let mut dst_fir = vec![0.0f32; n];
        gaussian_blur_plane_scalar(&src, &mut dst_fir, w, h, &kernel, &mut FilterContext::new());

        // Deriche
        let mut dst_deriche = vec![0.0f32; n];
        deriche_blur_plane(
            &src,
            &mut dst_deriche,
            w,
            h,
            &coeffs,
            &mut FilterContext::new(),
        );

        // Compare interior pixels (skip 2*sigma border where boundary effects differ)
        let margin = (2.0 * sigma).ceil() as usize;
        let mut max_err = 0.0f32;
        let mut sum_sq_err = 0.0f64;
        let mut count = 0;
        for y in margin..h as usize - margin {
            for x in margin..w as usize - margin {
                let i = y * w as usize + x;
                let err = (dst_fir[i] - dst_deriche[i]).abs();
                max_err = max_err.max(err);
                sum_sq_err += (err as f64) * (err as f64);
                count += 1;
            }
        }
        let rmse = (sum_sq_err / count as f64).sqrt();
        eprintln!(
            "deriche vs FIR sigma={sigma}: max_err={max_err:.6}, rmse={rmse:.6} (interior only)"
        );
        assert!(
            max_err < 0.02,
            "deriche vs FIR max error too large: {max_err}"
        );
        assert!(rmse < 0.005, "deriche vs FIR RMSE too large: {rmse}");
    }

    #[test]
    fn deriche_large_sigma_constant() {
        let w = 64u32;
        let h = 64u32;
        let src = vec![0.7f32; (w * h) as usize];
        let mut dst = vec![0.0f32; (w * h) as usize];
        let coeffs = DericheCoefficients::new(30.0);
        deriche_blur_plane(&src, &mut dst, w, h, &coeffs, &mut FilterContext::new());
        for &v in &dst {
            assert!(
                (v - 0.7).abs() < 1e-3,
                "deriche large sigma constant: expected 0.7, got {v}"
            );
        }
    }
}
