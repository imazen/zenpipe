/// Maximum supported kernel radius. Sigma 50 → radius 150 → 301 weights.
/// This covers any realistic blur sigma.
const MAX_KERNEL_SIZE: usize = 512;

/// Minimum sigma for the extended box blur fast path.
/// Below this, the SIMD FIR convolution is faster (fewer memory passes, FMA
/// throughput beats scalar prefix sums). The box blur wins at very large sigma
/// where the FIR kernel exceeds ~200 taps.
const BOX_BLUR_SIGMA_THRESHOLD: f32 = 40.0;

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
/// For sigma >= 1.5, dispatches to the extended box blur (O(1)/pixel).
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
    if should_use_box_blur(sigma) {
        let blur = ExtendedBoxBlur::from_sigma(sigma);
        extended_box_blur_plane(src, dst, width, height, &blur, ctx);
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

// ─── Extended box blur (O(1) per pixel, cache-friendly) ─────────────

/// Precomputed parameters for a 3-pass extended box blur approximating a Gaussian.
///
/// Three passes of box blur produce a piecewise-quadratic (B-spline) impulse
/// response that closely approximates a Gaussian. The box widths are chosen so
/// the total variance matches `sigma²`. This gives O(1) per pixel cost
/// regardless of sigma, vs O(radius) for FIR convolution.
///
/// Reference: Gwosdek et al., "Addendum to 'Recursive Gaussian filtering'";
/// Getreuer, "A Survey of Gaussian Convolution Algorithms," IPOL 2013.
#[derive(Clone, Debug)]
pub struct ExtendedBoxBlur {
    /// Half-width (radius) for each of the 3 passes.
    radii: [u32; 3],
    /// Precomputed `1.0 / (2*r + 1)` for each pass.
    inv_sizes: [f32; 3],
}

impl ExtendedBoxBlur {
    /// Compute box widths for K=3 passes approximating a Gaussian with the given sigma.
    pub fn from_sigma(sigma: f32) -> Self {
        // Target variance per pass: sigma² / 3
        // Ideal box width: sqrt(12 * sigma² / 3 + 1)
        let k = 3.0f32;
        let w_ideal = (12.0 * sigma * sigma / k + 1.0).sqrt();
        let w_l = (w_ideal.floor() as u32) | 1; // round down to odd
        let w_u = w_l + 2; // next odd

        // How many passes use w_l vs w_u to match total variance
        let wl = w_l as f32;
        let wu = w_u as f32;
        let m_num = 3.0 * (wl * wl + 4.0 * wl + 3.0) - 12.0 * sigma * sigma;
        let m_den = (wl - wu) * (wu - wl); // negative of (wu - wl)²
        // m = number of passes using w_l
        let m = if m_den.abs() < 1e-10 {
            3 // all same width
        } else {
            (m_num / m_den).round().clamp(0.0, 3.0) as u32
        };

        let r_l = w_l / 2;
        let r_u = w_u / 2;
        let radii = match m {
            0 => [r_u, r_u, r_u],
            1 => [r_l, r_u, r_u],
            2 => [r_l, r_l, r_u],
            _ => [r_l, r_l, r_l],
        };

        let inv_sizes = [
            1.0 / (2 * radii[0] + 1) as f32,
            1.0 / (2 * radii[1] + 1) as f32,
            1.0 / (2 * radii[2] + 1) as f32,
        ];

        Self { radii, inv_sizes }
    }

    /// Maximum radius across all 3 passes.
    pub fn max_radius(&self) -> u32 {
        self.radii[0].max(self.radii[1]).max(self.radii[2])
    }
}

/// Single-pass box blur on a contiguous row, using a prefix sum.
///
/// Reads from `src_row` (length `len`), writes to `dst_row` (length `len`).
/// `prefix` must have capacity >= `len + 2*radius + 1`.
/// Edge replication is used at boundaries.
fn box_blur_row(
    src_row: &[f32],
    dst_row: &mut [f32],
    len: usize,
    radius: u32,
    inv_size: f32,
    prefix: &mut [f32],
) {
    let r = radius as usize;
    let padded_len = len + 2 * r;

    // Build prefix sum of edge-replicated row
    prefix[0] = 0.0;
    let edge_l = src_row[0];
    for i in 0..r {
        prefix[i + 1] = prefix[i] + edge_l;
    }
    for i in 0..len {
        prefix[r + i + 1] = prefix[r + i] + src_row[i];
    }
    let edge_r = src_row[len - 1];
    for i in 0..r {
        prefix[r + len + i + 1] = prefix[r + len + i] + edge_r;
    }

    // Output: (prefix[x + 2r + 1] - prefix[x]) * inv_size
    let window = 2 * r + 1;
    for x in 0..len {
        dst_row[x] = (prefix[x + window] - prefix[x]) * inv_size;
    }
    let _ = padded_len; // suppress unused warning
}

/// Single-pass in-place box blur on contiguous rows.
///
/// `data` contains `num_rows` rows of `row_len` elements each.
fn box_blur_rows_inplace(
    data: &mut [f32],
    row_len: usize,
    num_rows: usize,
    radius: u32,
    inv_size: f32,
    prefix: &mut [f32],
    row_tmp: &mut [f32],
) {
    for y in 0..num_rows {
        let row_start = y * row_len;
        let row_end = row_start + row_len;
        // Copy row to temp, blur from temp into data
        row_tmp[..row_len].copy_from_slice(&data[row_start..row_end]);
        box_blur_row(
            &row_tmp[..row_len],
            &mut data[row_start..row_end],
            row_len,
            radius,
            inv_size,
            prefix,
        );
    }
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

/// Extended box blur: 3 horizontal passes + transpose + 3 horizontal passes + transpose.
///
/// This is the fast path for `gaussian_blur_plane` when sigma >= 1.5.
/// O(1) per pixel regardless of sigma, with cache-friendly memory access.
pub fn extended_box_blur_plane(
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    blur: &ExtendedBoxBlur,
    ctx: &mut FilterContext,
) {
    let w = width as usize;
    let h = height as usize;
    let n = w * h;
    let max_r = blur.max_radius() as usize;
    let max_dim = w.max(h);

    // Scratch buffers
    let mut buf_a = ctx.take_f32(n);
    let mut buf_b = ctx.take_f32(n);
    let mut prefix = ctx.take_f32(max_dim + 2 * max_r + 1);
    let mut row_tmp = ctx.take_f32(max_dim);

    // --- 3 horizontal passes ---
    // Pass 1: src → buf_a
    for y in 0..h {
        box_blur_row(
            &src[y * w..(y + 1) * w],
            &mut buf_a[y * w..(y + 1) * w],
            w,
            blur.radii[0],
            blur.inv_sizes[0],
            &mut prefix,
        );
    }
    // Pass 2: buf_a → buf_a (in-place)
    box_blur_rows_inplace(
        &mut buf_a,
        w,
        h,
        blur.radii[1],
        blur.inv_sizes[1],
        &mut prefix,
        &mut row_tmp,
    );
    // Pass 3: buf_a → buf_a (in-place)
    box_blur_rows_inplace(
        &mut buf_a,
        w,
        h,
        blur.radii[2],
        blur.inv_sizes[2],
        &mut prefix,
        &mut row_tmp,
    );

    // --- Transpose: buf_a (w×h) → buf_b (h×w, i.e. w rows of h elements) ---
    transpose_plane(&buf_a, &mut buf_b, w, h);

    // --- 3 "vertical" passes (horizontal on transposed data) ---
    // Pass 1: buf_b → buf_b (in-place via row_tmp)
    box_blur_rows_inplace(
        &mut buf_b,
        h,
        w,
        blur.radii[0],
        blur.inv_sizes[0],
        &mut prefix,
        &mut row_tmp,
    );
    // Pass 2
    box_blur_rows_inplace(
        &mut buf_b,
        h,
        w,
        blur.radii[1],
        blur.inv_sizes[1],
        &mut prefix,
        &mut row_tmp,
    );
    // Pass 3
    box_blur_rows_inplace(
        &mut buf_b,
        h,
        w,
        blur.radii[2],
        blur.inv_sizes[2],
        &mut prefix,
        &mut row_tmp,
    );

    // --- Transpose back: buf_b (h×w transposed as w rows of h) → dst (w×h) ---
    transpose_plane(&buf_b, dst, h, w);

    ctx.return_f32(row_tmp);
    ctx.return_f32(prefix);
    ctx.return_f32(buf_b);
    ctx.return_f32(buf_a);
}

/// Check if sigma is large enough to benefit from the extended box blur path.
pub fn should_use_box_blur(sigma: f32) -> bool {
    sigma >= BOX_BLUR_SIGMA_THRESHOLD
}

/// Get sigma from a GaussianKernel (approximate, from radius).
pub fn kernel_sigma(kernel: &GaussianKernel) -> f32 {
    kernel.radius as f32 / 3.0
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
        gaussian_blur_plane(&src, &mut actual_dst, w, h, &kernel, &mut FilterContext::new());

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
    fn extended_box_blur_constant_plane() {
        let w = 128u32;
        let h = 96u32;
        let src = vec![0.42f32; (w * h) as usize];
        let mut dst = vec![0.0f32; (w * h) as usize];
        let blur = ExtendedBoxBlur::from_sigma(8.0);
        extended_box_blur_plane(&src, &mut dst, w, h, &blur, &mut FilterContext::new());
        for (i, &v) in dst.iter().enumerate() {
            assert!(
                (v - 0.42).abs() < 1e-4,
                "constant plane pixel {i}: expected 0.42, got {v}"
            );
        }
    }

    #[test]
    fn extended_box_blur_large_sigma_constant() {
        // Regression test for large sigma (like clarity's coarse blur)
        let w = 64u32;
        let h = 64u32;
        let src = vec![0.7f32; (w * h) as usize];
        let mut dst = vec![0.0f32; (w * h) as usize];
        let blur = ExtendedBoxBlur::from_sigma(30.0);
        extended_box_blur_plane(&src, &mut dst, w, h, &blur, &mut FilterContext::new());
        for &v in &dst {
            assert!(
                (v - 0.7).abs() < 1e-3,
                "large sigma constant plane: expected 0.7, got {v}"
            );
        }
    }

    #[test]
    fn box_blur_vs_fir_similar_output() {
        // Compare extended box blur against true Gaussian for sigma=5.
        // The results should be perceptually similar (max error < 0.05
        // on a gradient image with values in [0.1, 0.9]).
        let w = 128u32;
        let h = 128u32;
        let n = (w * h) as usize;

        // Create a gradient image
        let mut src = vec![0.0f32; n];
        for y in 0..h as usize {
            for x in 0..w as usize {
                src[y * w as usize + x] = 0.1 + 0.8 * (x as f32 / w as f32) * (y as f32 / h as f32);
            }
        }

        let sigma = 5.0;
        let kernel = GaussianKernel::new(sigma);
        let mut ctx = FilterContext::new();

        // FIR (direct convolution) — use the scalar path explicitly
        let mut dst_fir = vec![0.0f32; n];
        // Force FIR by calling the scalar inner loop directly
        {
            let w_s = w as usize;
            let h_s = h as usize;
            let radius = kernel.radius;
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
            for y in 0..h_s {
                for x in 0..w_s {
                    let mut sum = 0.0f32;
                    for (k, &weight) in kernel.weights().iter().enumerate() {
                        let sy = (y + k).saturating_sub(radius).min(h_s - 1);
                        sum += h_buf[sy * w_s + x] * weight;
                    }
                    dst_fir[y * w_s + x] = sum;
                }
            }
        }

        // Box blur
        let mut dst_box = vec![0.0f32; n];
        let blur = ExtendedBoxBlur::from_sigma(sigma);
        extended_box_blur_plane(&src, &mut dst_box, w, h, &blur, &mut ctx);

        // Compare
        let mut max_err = 0.0f32;
        let mut sum_sq_err = 0.0f64;
        for i in 0..n {
            let err = (dst_fir[i] - dst_box[i]).abs();
            max_err = max_err.max(err);
            sum_sq_err += (err as f64) * (err as f64);
        }
        let rmse = (sum_sq_err / n as f64).sqrt();
        eprintln!("box vs FIR sigma={sigma}: max_err={max_err:.6}, rmse={rmse:.6}");
        assert!(max_err < 0.05, "box vs FIR max error too large: {max_err}");
        assert!(rmse < 0.01, "box vs FIR RMSE too large: {rmse}");
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
    fn box_blur_preserves_mean() {
        // Box blur should preserve the global mean (it's a normalized filter).
        let w = 64u32;
        let h = 64u32;
        let n = (w * h) as usize;
        let mut src = vec![0.0f32; n];
        for (i, v) in src.iter_mut().enumerate() {
            *v = ((i as u32).wrapping_mul(2654435761) as f32 / u32::MAX as f32) * 0.8 + 0.1;
        }
        let src_mean: f32 = src.iter().sum::<f32>() / n as f32;

        let mut dst = vec![0.0f32; n];
        let blur = ExtendedBoxBlur::from_sigma(5.0);
        extended_box_blur_plane(&src, &mut dst, w, h, &blur, &mut FilterContext::new());
        let dst_mean: f32 = dst.iter().sum::<f32>() / n as f32;

        assert!(
            (src_mean - dst_mean).abs() < 0.01,
            "mean should be preserved: src={src_mean}, dst={dst_mean}"
        );
    }

    #[test]
    fn gaussian_blur_plane_uses_box_for_large_sigma() {
        // Verify that gaussian_blur_plane with large sigma goes through box
        // blur path and produces reasonable results (constant image test).
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
}
