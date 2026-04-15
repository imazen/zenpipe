use crate::blur::{GaussianKernel, gaussian_blur_plane};
use crate::context::FilterContext;

/// O(1) guided filter for edge-preserving smoothing.
///
/// The guided filter uses a guidance image (typically the input itself) to
/// produce locally-linear output that preserves edges from the guide.
///
/// For self-guided filtering (guide = input):
///   output[i] = a[i] * I[i] + b[i]
///
/// where a, b are locally estimated from:
///   a = cov(I, p) / (var(I) + eps)
///   b = mean(p) - a * mean(I)
///
/// The filter is equivalent to a bilateral filter but:
/// - O(1) per pixel (uses box/Gaussian blurs, not per-pixel search)
/// - Separable (no 2D kernel needed)
/// - No exp() per neighbor (no range kernel)
/// - Gradient-preserving (not just edge-preserving)
///
/// Reference: He, Sun, Tang, "Guided Image Filtering," TPAMI 2013.
/// eps controls smoothing strength: larger eps = more smoothing.
#[allow(clippy::too_many_arguments)]
pub fn guided_filter_plane(
    input: &[f32],
    guide: &[f32],
    output: &mut [f32],
    width: u32,
    height: u32,
    sigma: f32,
    eps: f32,
    ctx: &mut FilterContext,
) {
    let n = (width as usize) * (height as usize);
    let kernel = GaussianKernel::new(sigma);

    // Step 1: Compute means
    // mean_I = blur(I), mean_p = blur(p)
    let mut mean_i = ctx.take_f32(n);
    let mut mean_p = ctx.take_f32(n);
    gaussian_blur_plane(guide, &mut mean_i, width, height, &kernel, ctx);
    gaussian_blur_plane(input, &mut mean_p, width, height, &kernel, ctx);

    // Step 2: Compute correlations
    // corr_ip = blur(I * p), corr_ii = blur(I * I)
    let mut ip = ctx.take_f32(n);
    let mut ii = ctx.take_f32(n);
    for idx in 0..n {
        ip[idx] = guide[idx] * input[idx];
        ii[idx] = guide[idx] * guide[idx];
    }

    let mut mean_ip = ctx.take_f32(n);
    let mut mean_ii = ctx.take_f32(n);
    gaussian_blur_plane(&ip, &mut mean_ip, width, height, &kernel, ctx);
    gaussian_blur_plane(&ii, &mut mean_ii, width, height, &kernel, ctx);
    ctx.return_f32(ii);
    ctx.return_f32(ip);

    // Step 3: Compute a, b coefficients
    // cov_ip = mean_ip - mean_i * mean_p
    // var_i  = mean_ii - mean_i * mean_i
    // a = cov_ip / (var_i + eps)
    // b = mean_p - a * mean_i
    let mut a = ctx.take_f32(n);
    let mut b = ctx.take_f32(n);
    for idx in 0..n {
        let cov = mean_ip[idx] - mean_i[idx] * mean_p[idx];
        let var = mean_ii[idx] - mean_i[idx] * mean_i[idx];
        a[idx] = cov / (var + eps);
        b[idx] = mean_p[idx] - a[idx] * mean_i[idx];
    }
    ctx.return_f32(mean_ii);
    ctx.return_f32(mean_ip);
    ctx.return_f32(mean_p);

    // Step 4: Average a, b over the window
    let mut mean_a = ctx.take_f32(n);
    let mut mean_b = ctx.take_f32(n);
    gaussian_blur_plane(&a, &mut mean_a, width, height, &kernel, ctx);
    gaussian_blur_plane(&b, &mut mean_b, width, height, &kernel, ctx);
    ctx.return_f32(b);
    ctx.return_f32(a);

    // Step 5: Output = mean_a * I + mean_b
    for idx in 0..n {
        output[idx] = mean_a[idx] * guide[idx] + mean_b[idx];
    }

    ctx.return_f32(mean_b);
    ctx.return_f32(mean_a);
    ctx.return_f32(mean_i);
}
