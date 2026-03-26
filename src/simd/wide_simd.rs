//! Shared f32x8 SIMD implementations for NEON (AArch64) and WASM SIMD128.
//!
//! All public functions use `#[magetypes(neon, wasm128)]` to generate both
//! `_neon` and `_wasm128` variants from a single source. The generic `f32x8<T>`
//! type polyfills as 2xf32x4 on both platforms (same perf characteristics).
//!
//! Inner helper functions are also `#[magetypes]`-annotated and called via
//! their suffixed names (e.g., `foo_inner_neon` from `bar_neon`).
//! This works because `#[magetypes]` does NOT rename function calls in the body,
//! so we use explicit suffixed calls instead.

#![allow(clippy::too_many_arguments)]
#![allow(dead_code)]

use crate::prelude::*;
use archmage::prelude::*;

use crate::blur::GaussianKernel;
use crate::context::FilterContext;
use zenpixels_convert::gamut::GamutMatrix;
use zenpixels_convert::oklab::{self, LMS_CBRT_FROM_OKLAB, OKLAB_FROM_LMS_CBRT};

/// Import the generic f32x8 (has transcendentals on all platforms).
/// Inside each `#[magetypes]` function we define a local alias:
///   `type f32x8 = GenericF32x8<Token>;`
/// so that `Token` replacement makes it concrete.
use magetypes::simd::generic::f32x8 as GenericF32x8;

/// Batch sRGB u8 -> linear f32 via LUT (8 elements).
#[inline(always)]
fn srgb_u8_to_linear_x8(srgb: [u8; 8]) -> [f32; 8] {
    [
        linear_srgb::default::srgb_u8_to_linear(srgb[0]),
        linear_srgb::default::srgb_u8_to_linear(srgb[1]),
        linear_srgb::default::srgb_u8_to_linear(srgb[2]),
        linear_srgb::default::srgb_u8_to_linear(srgb[3]),
        linear_srgb::default::srgb_u8_to_linear(srgb[4]),
        linear_srgb::default::srgb_u8_to_linear(srgb[5]),
        linear_srgb::default::srgb_u8_to_linear(srgb[6]),
        linear_srgb::default::srgb_u8_to_linear(srgb[7]),
    ]
}

// In each `#[magetypes(neon, wasm128)]` function, we define a local alias:
//   #[allow(non_camel_case_types)]
//   type f32x8 = GenericF32x8<Token>;
// This must be written directly (not via macro_rules!) because #[magetypes]
// replaces `Token` before macro_rules! expansion.

// ============================================================================
// Simple per-element plane operations
// ============================================================================

#[magetypes(neon, wasm128)]
pub(super) fn scale_plane_simd(token: Token, plane: &mut [f32], factor: f32) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let factor_v = f32x8::splat(token, factor);
    let (chunks, tail) = f32x8::partition_slice_mut(token, plane);
    for chunk in chunks {
        let v = f32x8::load(token, chunk);
        (v * factor_v).store(chunk);
    }
    for v in tail {
        *v *= factor;
    }
}

#[magetypes(neon, wasm128)]
pub(super) fn offset_plane_simd(token: Token, plane: &mut [f32], offset: f32) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let offset_v = f32x8::splat(token, offset);
    let (chunks, tail) = f32x8::partition_slice_mut(token, plane);
    for chunk in chunks {
        let v = f32x8::load(token, chunk);
        (v + offset_v).store(chunk);
    }
    for v in tail {
        *v += offset;
    }
}

#[magetypes(neon, wasm128)]
pub(super) fn power_contrast_plane_simd(token: Token, plane: &mut [f32], exp: f32, scale: f32) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let scale_v = f32x8::splat(token, scale);
    let zero_v = f32x8::zero(token);

    let (chunks, tail) = f32x8::partition_slice_mut(token, plane);
    for chunk in chunks {
        let v = f32x8::load(token, &*chunk);
        let powered = v.max(zero_v).pow_lowp_unchecked(exp);
        (powered * scale_v).store(chunk);
    }
    for v in tail {
        if *v > 0.0 {
            *v = v.powf(exp) * scale;
        }
    }
}

#[magetypes(neon, wasm128)]
pub(super) fn sigmoid_tone_map_plane_simd(
    token: Token,
    plane: &mut [f32],
    contrast: f32,
    bias_a: f32,
) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let one_v = f32x8::splat(token, 1.0);
    let zero_v = f32x8::zero(token);
    let bias_a_v = f32x8::splat(token, bias_a);
    let has_bias = bias_a.abs() > 1e-6;

    let (chunks, tail) = f32x8::partition_slice_mut(token, plane);
    for chunk in chunks {
        let mut x = f32x8::load(token, &*chunk).max(zero_v).min(one_v);

        if has_bias {
            let denom = (one_v - x).mul_add(bias_a_v, one_v);
            x *= denom.recip();
            x = x.max(zero_v).min(one_v);
        }

        let x_safe = x.max(f32x8::splat(token, 1e-7));
        let ratio = (one_v - x_safe) * x_safe.recip();
        let powered = ratio.pow_lowp_unchecked(contrast);
        let result = (one_v + powered).recip();

        let is_zero = x.simd_le(zero_v);
        let is_one = x.simd_ge(one_v);
        let r = f32x8::blend(is_zero, zero_v, result);
        let r = f32x8::blend(is_one, one_v, r);
        r.store(chunk);
    }
    for v in tail {
        let mut x = v.clamp(0.0, 1.0);
        if has_bias {
            x = x / (bias_a * (1.0 - x) + 1.0);
        }
        *v = if x <= 0.0 {
            0.0
        } else if x >= 1.0 {
            1.0
        } else {
            let ratio = ((1.0 - x) / x).powf(contrast);
            1.0 / (1.0 + ratio)
        };
    }
}

#[magetypes(neon, wasm128)]
pub(super) fn unsharp_fuse_simd(
    token: Token,
    src: &[f32],
    blurred: &[f32],
    dst: &mut [f32],
    amount: f32,
) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let amount_v = f32x8::splat(token, amount);
    let zero_v = f32x8::zero(token);

    let (src_chunks, _) = f32x8::partition_slice(token, src);
    let (blur_chunks, _) = f32x8::partition_slice(token, blurred);
    let (dst_chunks, dst_tail) = f32x8::partition_slice_mut(token, dst);

    for ((sc, bc), dc) in src_chunks
        .iter()
        .zip(blur_chunks.iter())
        .zip(dst_chunks.iter_mut())
    {
        let orig = f32x8::load(token, sc);
        let blur = f32x8::load(token, bc);
        let hp = orig - blur;
        hp.mul_add(amount_v, orig).max(zero_v).store(dc);
    }

    let done = src_chunks.len() * 8;
    for (i, v) in dst_tail.iter_mut().enumerate() {
        let idx = done + i;
        *v = (src[idx] + (src[idx] - blurred[idx]) * amount).max(0.0);
    }
}

// ============================================================================
// Gaussian blur (FIR + stackblur hybrid)
// ============================================================================

// NOTE: The blur functions use `fn` (not `#[magetypes]`) because the dispatch
// function calls inner helpers. We generate the neon/wasm128 variants for the
// top-level dispatch, and the inner helpers are plain functions that accept
// a generic token via `impl F32x8Backend + Copy` bound.

use magetypes::simd::backends::{F32x8Backend, F32x8Convert};

#[magetypes(neon, wasm128)]
pub(super) fn gaussian_blur_plane_dispatch_simd(
    token: Token,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    kernel: &GaussianKernel,
    ctx: &mut FilterContext,
) {
    use crate::blur::{kernel_sigma, should_use_stackblur, sigma_to_stackblur_radius};

    let sigma = kernel_sigma(kernel);
    if should_use_stackblur(sigma) {
        let radius = sigma_to_stackblur_radius(sigma);
        stackblur_plane_generic(token, src, dst, width, height, radius, ctx);
        return;
    }
    gaussian_blur_fir_generic(token, src, dst, width, height, kernel, ctx);
}

#[inline]
fn gaussian_blur_fir_generic<T: F32x8Backend + F32x8Convert + Copy>(
    token: T,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    kernel: &GaussianKernel,
    ctx: &mut FilterContext,
) {
    type V<T> = GenericF32x8<T>;

    let w = width as usize;
    let h = height as usize;
    let radius = kernel.radius;

    let mut h_buf = ctx.take_f32(w * h);
    let mut padded = ctx.take_f32(w + 2 * radius);

    // Horizontal pass
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        padded.clear();
        let edge_l = row[0];
        let edge_r = row[w - 1];
        padded.extend(core::iter::repeat_n(edge_l, radius));
        padded.extend_from_slice(row);
        padded.extend(core::iter::repeat_n(edge_r, radius));

        let out_row = &mut h_buf[y * w..(y + 1) * w];
        let (out_chunks, out_tail) = V::<T>::partition_slice_mut(token, out_row);

        for (ci, out_chunk) in out_chunks.iter_mut().enumerate() {
            let x = ci * 8;
            let mut acc = V::<T>::zero(token);
            for (k, &weight) in kernel.weights().iter().enumerate() {
                let wv = V::<T>::splat(token, weight);
                let src_chunk: &[f32; 8] = padded[x + k..x + k + 8].try_into().unwrap();
                let vals = V::<T>::load(token, src_chunk);
                acc = vals.mul_add(wv, acc);
            }
            acc.store(out_chunk);
        }

        let x_start = out_chunks.len() * 8;
        for (xi, v) in out_tail.iter_mut().enumerate() {
            let x = x_start + xi;
            let mut sum = 0.0f32;
            for (k, &weight) in kernel.weights().iter().enumerate() {
                sum += padded[x + k] * weight;
            }
            *v = sum;
        }
    }

    // Vertical pass
    for y in 0..h {
        let out_row = &mut dst[y * w..(y + 1) * w];
        let (out_chunks, out_tail) = V::<T>::partition_slice_mut(token, out_row);

        for (ci, out_chunk) in out_chunks.iter_mut().enumerate() {
            let x = ci * 8;
            let mut acc = V::<T>::zero(token);
            for (k, &weight) in kernel.weights().iter().enumerate() {
                let sy = (y + k).saturating_sub(radius).min(h - 1);
                let wv = V::<T>::splat(token, weight);
                let src_chunk: &[f32; 8] = h_buf[sy * w + x..sy * w + x + 8].try_into().unwrap();
                acc = V::<T>::load(token, src_chunk).mul_add(wv, acc);
            }
            acc.store(out_chunk);
        }

        let x_start = out_chunks.len() * 8;
        for (xi, v) in out_tail.iter_mut().enumerate() {
            let x = x_start + xi;
            let mut sum = 0.0f32;
            for (k, &weight) in kernel.weights().iter().enumerate() {
                let sy = (y + k).saturating_sub(radius).min(h - 1);
                sum += h_buf[sy * w + x] * weight;
            }
            *v = sum;
        }
    }

    ctx.return_f32(padded);
    ctx.return_f32(h_buf);
}

#[inline]
fn stackblur_plane_generic<T: F32x8Backend + F32x8Convert + Copy>(
    token: T,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    radius: u32,
    ctx: &mut FilterContext,
) {
    use crate::blur::stackblur_row;
    type V<T> = GenericF32x8<T>;

    if radius == 0 {
        dst.copy_from_slice(src);
        return;
    }

    let w = width as usize;
    let h = height as usize;
    let r = radius as usize;
    let n = w * h;
    let stack_size = 2 * r + 1;
    let inv_div = 1.0 / ((r as f32 + 1.0) * (r as f32 + 1.0));
    let inv_div_v = V::<T>::splat(token, inv_div);

    let mut h_buf = ctx.take_f32(n);
    let mut stack_scratch = ctx.take_f32(stack_size);

    for y in 0..h {
        let row = &src[y * w..][..w];
        let out = &mut h_buf[y * w..][..w];
        stackblur_row(row, out, w, r, &mut stack_scratch, inv_div);
    }

    ctx.return_f32(stack_scratch);

    let num_tiles = w / 8;
    let mut stack_v: Vec<[f32; 8]> = vec![[0.0; 8]; stack_size];

    for tile in 0..num_tiles {
        let x = tile * 8;

        let mut sum = V::<T>::zero(token);
        let mut sum_in = V::<T>::zero(token);
        let mut sum_out = V::<T>::zero(token);

        let first: &[f32; 8] = h_buf[x..x + 8].try_into().unwrap();
        let first_v = V::<T>::load(token, first);
        for sv in stack_v.iter_mut().take(r + 1) {
            *sv = first_v.to_array();
        }

        for (i, sv) in stack_v.iter_mut().enumerate().take(stack_size).skip(r + 1) {
            let offset = i - r;
            let sy = offset.min(h - 1);
            let chunk: &[f32; 8] = h_buf[sy * w + x..sy * w + x + 8].try_into().unwrap();
            *sv = V::<T>::load(token, chunk).to_array();
        }

        for (i, sv) in stack_v.iter().enumerate().take(stack_size) {
            let dist = r.abs_diff(i);
            let weight = V::<T>::splat(token, (r + 1 - dist) as f32);
            let val = V::<T>::from_array(token, *sv);
            sum = val.mul_add(weight, sum);
        }

        for sv in stack_v.iter().take(r + 1) {
            sum_out += V::<T>::from_array(token, *sv);
        }
        for sv in stack_v.iter().take(stack_size).skip(r + 1) {
            sum_in += V::<T>::from_array(token, *sv);
        }

        let mut sp = 0usize;

        for y in 0..h {
            let out: &mut [f32; 8] = (&mut dst[y * w + x..y * w + x + 8]).try_into().unwrap();
            (sum * inv_div_v).store(out);

            sum -= sum_out;
            let old_val = V::<T>::from_array(token, stack_v[sp]);
            sum_out -= old_val;

            let new_y = (y + r + 1).min(h - 1);
            let new_chunk: &[f32; 8] = h_buf[new_y * w + x..new_y * w + x + 8].try_into().unwrap();
            let new_val = V::<T>::load(token, new_chunk);
            stack_v[sp] = new_val.to_array();
            sum_in += new_val;

            sum += sum_in;

            sp += 1;
            if sp >= stack_size {
                sp = 0;
            }

            let center_idx = if sp + r >= stack_size {
                sp + r - stack_size
            } else {
                sp + r
            };
            let center_val = V::<T>::from_array(token, stack_v[center_idx]);
            sum_out += center_val;
            sum_in -= center_val;
        }
    }

    let x_start = num_tiles * 8;
    if x_start < w {
        let mut stack_scalar = ctx.take_f32(stack_size);
        let mut col_buf = ctx.take_f32(h);
        let mut col_out = ctx.take_f32(h);

        for x in x_start..w {
            for y in 0..h {
                col_buf[y] = h_buf[y * w + x];
            }
            crate::blur::stackblur_row(&col_buf, &mut col_out, h, r, &mut stack_scalar, inv_div);
            for y in 0..h {
                dst[y * w + x] = col_out[y];
            }
        }

        ctx.return_f32(col_out);
        ctx.return_f32(col_buf);
        ctx.return_f32(stack_scalar);
    }

    ctx.return_f32(h_buf);
}

// ============================================================================
// Brilliance (local contrast correction)
// ============================================================================

#[magetypes(neon, wasm128)]
pub(super) fn brilliance_apply_simd(
    token: Token,
    src_l: &[f32],
    avg_l: &[f32],
    dst_l: &mut [f32],
    amount: f32,
    shadow_str: f32,
    highlight_str: f32,
) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let one = f32x8::splat(token, 1.0);
    let two = f32x8::splat(token, 2.0);
    let three = f32x8::splat(token, 3.0);
    let min_avg = f32x8::splat(token, 0.001);
    let zero = f32x8::zero(token);
    let sa = f32x8::splat(token, shadow_str * amount);
    let ha = f32x8::splat(token, highlight_str * amount);

    let (src_chunks, _) = f32x8::partition_slice(token, src_l);
    let (avg_chunks, _) = f32x8::partition_slice(token, avg_l);
    let (dst_chunks, dst_tail) = f32x8::partition_slice_mut(token, dst_l);

    for ((sc, ac), dc) in src_chunks
        .iter()
        .zip(avg_chunks.iter())
        .zip(dst_chunks.iter_mut())
    {
        let l = f32x8::load(token, sc);
        let avg = f32x8::load(token, ac).max(min_avg);
        let ratio = l * avg.recip();

        let st = (one - ratio).max(zero).min(one);
        let shadow_curve = st * st * (three - two * st);
        let shadow_corr = shadow_curve.mul_add(sa, one);

        let ht = (ratio - one).max(zero).min(one);
        let highlight_curve = ht * ht * (three - two * ht);
        let highlight_corr = one - highlight_curve * ha;

        let is_shadow = ratio.simd_lt(one);
        let correction = f32x8::blend(is_shadow, shadow_corr, highlight_corr);
        (l * correction).store(dc);
    }

    let done = src_chunks.len() * 8;
    for (i, v) in dst_tail.iter_mut().enumerate() {
        let idx = done + i;
        let l = src_l[idx];
        let avg = avg_l[idx].max(0.001);
        let ratio = l / avg;
        let c = if ratio < 1.0 {
            let t = (1.0 - ratio).clamp(0.0, 1.0);
            1.0 + t * t * (3.0 - 2.0 * t) * shadow_str * amount
        } else {
            let t = (ratio - 1.0).clamp(0.0, 1.0);
            1.0 - t * t * (3.0 - 2.0 * t) * highlight_str * amount
        };
        *v = l * c;
    }
}

// ============================================================================
// Scatter/Gather: interleaved linear RGB <-> planar Oklab
// ============================================================================

#[magetypes(neon, wasm128)]
pub(super) fn scatter_oklab_simd(
    token: Token,
    src: &[f32],
    l_out: &mut [f32],
    a_out: &mut [f32],
    b_out: &mut [f32],
    channels: u32,
    m1: &GamutMatrix,
    inv_white: f32,
) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let n = l_out.len();
    let ch = channels as usize;

    let m1_00 = f32x8::splat(token, m1[0][0]);
    let m1_01 = f32x8::splat(token, m1[0][1]);
    let m1_02 = f32x8::splat(token, m1[0][2]);
    let m1_10 = f32x8::splat(token, m1[1][0]);
    let m1_11 = f32x8::splat(token, m1[1][1]);
    let m1_12 = f32x8::splat(token, m1[1][2]);
    let m1_20 = f32x8::splat(token, m1[2][0]);
    let m1_21 = f32x8::splat(token, m1[2][1]);
    let m1_22 = f32x8::splat(token, m1[2][2]);

    let m2_00 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[0][0]);
    let m2_01 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[0][1]);
    let m2_02 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[0][2]);
    let m2_10 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[1][0]);
    let m2_11 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[1][1]);
    let m2_12 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[1][2]);
    let m2_20 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[2][0]);
    let m2_21 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[2][1]);
    let m2_22 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[2][2]);

    let inv_white_v = f32x8::splat(token, inv_white);

    let mut i = 0;
    while i + 8 <= n {
        let mut r_arr = [0.0f32; 8];
        let mut g_arr = [0.0f32; 8];
        let mut b_arr = [0.0f32; 8];
        for j in 0..8 {
            let base = (i + j) * ch;
            r_arr[j] = src[base];
            g_arr[j] = src[base + 1];
            b_arr[j] = src[base + 2];
        }

        let r = f32x8::from_array(token, r_arr) * inv_white_v;
        let g = f32x8::from_array(token, g_arr) * inv_white_v;
        let b = f32x8::from_array(token, b_arr) * inv_white_v;

        let lms_l = m1_00.mul_add(r, m1_01.mul_add(g, m1_02 * b));
        let lms_m = m1_10.mul_add(r, m1_11.mul_add(g, m1_12 * b));
        let lms_s = m1_20.mul_add(r, m1_21.mul_add(g, m1_22 * b));

        let l_ = lms_l.cbrt_lowp();
        let m_ = lms_m.cbrt_lowp();
        let s_ = lms_s.cbrt_lowp();

        let ok_l = m2_00.mul_add(l_, m2_01.mul_add(m_, m2_02 * s_));
        let ok_a = m2_10.mul_add(l_, m2_11.mul_add(m_, m2_12 * s_));
        let ok_b = m2_20.mul_add(l_, m2_21.mul_add(m_, m2_22 * s_));

        let l_chunk: &mut [f32; 8] = (&mut l_out[i..i + 8]).try_into().unwrap();
        let a_chunk: &mut [f32; 8] = (&mut a_out[i..i + 8]).try_into().unwrap();
        let b_chunk: &mut [f32; 8] = (&mut b_out[i..i + 8]).try_into().unwrap();
        ok_l.store(l_chunk);
        ok_a.store(a_chunk);
        ok_b.store(b_chunk);

        i += 8;
    }

    for idx in i..n {
        let base = idx * ch;
        let r = src[base] * inv_white;
        let g = src[base + 1] * inv_white;
        let bv = src[base + 2] * inv_white;
        let [ol, oa, ob] = oklab::rgb_to_oklab(r, g, bv, m1);
        l_out[idx] = ol;
        a_out[idx] = oa;
        b_out[idx] = ob;
    }
}

#[magetypes(neon, wasm128)]
pub(super) fn gather_oklab_simd(
    token: Token,
    l_in: &[f32],
    a_in: &[f32],
    b_in: &[f32],
    dst: &mut [f32],
    channels: u32,
    m1_inv: &GamutMatrix,
    reference_white: f32,
) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let n = l_in.len();
    let ch = channels as usize;

    let im2_00 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[0][0]);
    let im2_01 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[0][1]);
    let im2_02 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[0][2]);
    let im2_10 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[1][0]);
    let im2_11 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[1][1]);
    let im2_12 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[1][2]);
    let im2_20 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[2][0]);
    let im2_21 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[2][1]);
    let im2_22 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[2][2]);

    let im1_00 = f32x8::splat(token, m1_inv[0][0]);
    let im1_01 = f32x8::splat(token, m1_inv[0][1]);
    let im1_02 = f32x8::splat(token, m1_inv[0][2]);
    let im1_10 = f32x8::splat(token, m1_inv[1][0]);
    let im1_11 = f32x8::splat(token, m1_inv[1][1]);
    let im1_12 = f32x8::splat(token, m1_inv[1][2]);
    let im1_20 = f32x8::splat(token, m1_inv[2][0]);
    let im1_21 = f32x8::splat(token, m1_inv[2][1]);
    let im1_22 = f32x8::splat(token, m1_inv[2][2]);

    let white_v = f32x8::splat(token, reference_white);
    let zero_v = f32x8::zero(token);

    let mut i = 0;
    while i + 8 <= n {
        let l_chunk: &[f32; 8] = l_in[i..i + 8].try_into().unwrap();
        let a_chunk: &[f32; 8] = a_in[i..i + 8].try_into().unwrap();
        let b_chunk: &[f32; 8] = b_in[i..i + 8].try_into().unwrap();
        let lv = f32x8::load(token, l_chunk);
        let av = f32x8::load(token, a_chunk);
        let bv = f32x8::load(token, b_chunk);

        let l_ = im2_00.mul_add(lv, im2_01.mul_add(av, im2_02 * bv));
        let m_ = im2_10.mul_add(lv, im2_11.mul_add(av, im2_12 * bv));
        let s_ = im2_20.mul_add(lv, im2_21.mul_add(av, im2_22 * bv));

        let lms_l = l_ * l_ * l_;
        let lms_m = m_ * m_ * m_;
        let lms_s = s_ * s_ * s_;

        let r = im1_00.mul_add(lms_l, im1_01.mul_add(lms_m, im1_02 * lms_s));
        let g = im1_10.mul_add(lms_l, im1_11.mul_add(lms_m, im1_12 * lms_s));
        let b = im1_20.mul_add(lms_l, im1_21.mul_add(lms_m, im1_22 * lms_s));

        let r_out = (r * white_v).max(zero_v);
        let g_out = (g * white_v).max(zero_v);
        let b_out = (b * white_v).max(zero_v);

        let r_arr = r_out.to_array();
        let g_arr = g_out.to_array();
        let b_arr = b_out.to_array();
        for j in 0..8 {
            let base = (i + j) * ch;
            dst[base] = r_arr[j];
            dst[base + 1] = g_arr[j];
            dst[base + 2] = b_arr[j];
        }

        i += 8;
    }

    for idx in i..n {
        let [r, g, bv] = oklab::oklab_to_rgb(l_in[idx], a_in[idx], b_in[idx], m1_inv);
        let base = idx * ch;
        dst[base] = (r * reference_white).max(0.0);
        dst[base + 1] = (g * reference_white).max(0.0);
        dst[base + 2] = (bv * reference_white).max(0.0);
    }
}

// ============================================================================
// Fused sRGB u8 <-> Oklab
// Uses scalar LUT for u8 conversion (fast lookup), SIMD for Oklab math.
// ============================================================================

#[magetypes(neon, wasm128)]
pub(super) fn scatter_srgb_u8_to_oklab_simd(
    token: Token,
    src: &[u8],
    l_out: &mut [f32],
    a_out: &mut [f32],
    b_out: &mut [f32],
    channels: u32,
    m1: &GamutMatrix,
) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let n = l_out.len();
    let ch = channels as usize;

    let m1_00 = f32x8::splat(token, m1[0][0]);
    let m1_01 = f32x8::splat(token, m1[0][1]);
    let m1_02 = f32x8::splat(token, m1[0][2]);
    let m1_10 = f32x8::splat(token, m1[1][0]);
    let m1_11 = f32x8::splat(token, m1[1][1]);
    let m1_12 = f32x8::splat(token, m1[1][2]);
    let m1_20 = f32x8::splat(token, m1[2][0]);
    let m1_21 = f32x8::splat(token, m1[2][1]);
    let m1_22 = f32x8::splat(token, m1[2][2]);

    let m2_00 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[0][0]);
    let m2_01 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[0][1]);
    let m2_02 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[0][2]);
    let m2_10 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[1][0]);
    let m2_11 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[1][1]);
    let m2_12 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[1][2]);
    let m2_20 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[2][0]);
    let m2_21 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[2][1]);
    let m2_22 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[2][2]);

    let mut i = 0;
    while i + 8 <= n {
        let mut r_u8 = [0u8; 8];
        let mut g_u8 = [0u8; 8];
        let mut b_u8 = [0u8; 8];
        for j in 0..8 {
            let base = (i + j) * ch;
            r_u8[j] = src[base];
            g_u8[j] = src[base + 1];
            b_u8[j] = src[base + 2];
        }

        // LUT: sRGB u8 -> linear f32 (scalar, 8 lookups per channel)
        let r = f32x8::from_array(token, srgb_u8_to_linear_x8(r_u8));
        let g = f32x8::from_array(token, srgb_u8_to_linear_x8(g_u8));
        let b = f32x8::from_array(token, srgb_u8_to_linear_x8(b_u8));

        let lms_l = m1_00.mul_add(r, m1_01.mul_add(g, m1_02 * b));
        let lms_m = m1_10.mul_add(r, m1_11.mul_add(g, m1_12 * b));
        let lms_s = m1_20.mul_add(r, m1_21.mul_add(g, m1_22 * b));

        let l_ = lms_l.cbrt_lowp();
        let m_ = lms_m.cbrt_lowp();
        let s_ = lms_s.cbrt_lowp();

        let ok_l = m2_00.mul_add(l_, m2_01.mul_add(m_, m2_02 * s_));
        let ok_a = m2_10.mul_add(l_, m2_11.mul_add(m_, m2_12 * s_));
        let ok_b = m2_20.mul_add(l_, m2_21.mul_add(m_, m2_22 * s_));

        let l_chunk: &mut [f32; 8] = (&mut l_out[i..i + 8]).try_into().unwrap();
        let a_chunk: &mut [f32; 8] = (&mut a_out[i..i + 8]).try_into().unwrap();
        let b_chunk: &mut [f32; 8] = (&mut b_out[i..i + 8]).try_into().unwrap();
        ok_l.store(l_chunk);
        ok_a.store(a_chunk);
        ok_b.store(b_chunk);

        i += 8;
    }

    for idx in i..n {
        let base = idx * ch;
        let r = linear_srgb::default::srgb_u8_to_linear(src[base]);
        let g = linear_srgb::default::srgb_u8_to_linear(src[base + 1]);
        let bv = linear_srgb::default::srgb_u8_to_linear(src[base + 2]);
        let [ol, oa, ob] = oklab::rgb_to_oklab(r, g, bv, m1);
        l_out[idx] = ol;
        a_out[idx] = oa;
        b_out[idx] = ob;
    }
}

#[magetypes(neon, wasm128)]
pub(super) fn gather_oklab_to_srgb_u8_simd(
    token: Token,
    l_in: &[f32],
    a_in: &[f32],
    b_in: &[f32],
    dst: &mut [u8],
    channels: u32,
    m1_inv: &GamutMatrix,
) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let n = l_in.len();
    let ch = channels as usize;

    let im2_00 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[0][0]);
    let im2_01 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[0][1]);
    let im2_02 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[0][2]);
    let im2_10 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[1][0]);
    let im2_11 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[1][1]);
    let im2_12 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[1][2]);
    let im2_20 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[2][0]);
    let im2_21 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[2][1]);
    let im2_22 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[2][2]);

    let im1_00 = f32x8::splat(token, m1_inv[0][0]);
    let im1_01 = f32x8::splat(token, m1_inv[0][1]);
    let im1_02 = f32x8::splat(token, m1_inv[0][2]);
    let im1_10 = f32x8::splat(token, m1_inv[1][0]);
    let im1_11 = f32x8::splat(token, m1_inv[1][1]);
    let im1_12 = f32x8::splat(token, m1_inv[1][2]);
    let im1_20 = f32x8::splat(token, m1_inv[2][0]);
    let im1_21 = f32x8::splat(token, m1_inv[2][1]);
    let im1_22 = f32x8::splat(token, m1_inv[2][2]);

    let mut i = 0;
    while i + 8 <= n {
        let l_chunk: &[f32; 8] = l_in[i..i + 8].try_into().unwrap();
        let a_chunk: &[f32; 8] = a_in[i..i + 8].try_into().unwrap();
        let b_chunk: &[f32; 8] = b_in[i..i + 8].try_into().unwrap();
        let lv = f32x8::load(token, l_chunk);
        let av = f32x8::load(token, a_chunk);
        let bv = f32x8::load(token, b_chunk);

        let l_ = im2_00.mul_add(lv, im2_01.mul_add(av, im2_02 * bv));
        let m_ = im2_10.mul_add(lv, im2_11.mul_add(av, im2_12 * bv));
        let s_ = im2_20.mul_add(lv, im2_21.mul_add(av, im2_22 * bv));

        let lms_l = l_ * l_ * l_;
        let lms_m = m_ * m_ * m_;
        let lms_s = s_ * s_ * s_;

        let r = im1_00.mul_add(lms_l, im1_01.mul_add(lms_m, im1_02 * lms_s));
        let g = im1_10.mul_add(lms_l, im1_11.mul_add(lms_m, im1_12 * lms_s));
        let b = im1_20.mul_add(lms_l, im1_21.mul_add(lms_m, im1_22 * lms_s));

        let r_arr = r.to_array();
        let g_arr = g.to_array();
        let b_arr = b.to_array();
        for j in 0..8 {
            let base = (i + j) * ch;
            dst[base] = linear_srgb::default::linear_to_srgb_u8(r_arr[j]);
            dst[base + 1] = linear_srgb::default::linear_to_srgb_u8(g_arr[j]);
            dst[base + 2] = linear_srgb::default::linear_to_srgb_u8(b_arr[j]);
        }

        i += 8;
    }

    for idx in i..n {
        let [r, g, bv] = oklab::oklab_to_rgb(l_in[idx], a_in[idx], b_in[idx], m1_inv);
        let base = idx * ch;
        dst[base] = linear_srgb::default::linear_to_srgb_u8(r);
        dst[base + 1] = linear_srgb::default::linear_to_srgb_u8(g);
        dst[base + 2] = linear_srgb::default::linear_to_srgb_u8(bv);
    }
}

// ============================================================================
// Per-pixel filter operations
// ============================================================================

#[magetypes(neon, wasm128)]
pub(super) fn black_point_plane_simd(token: Token, plane: &mut [f32], bp: f32, inv_range: f32) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let bp_v = f32x8::splat(token, bp);
    let inv_range_v = f32x8::splat(token, inv_range);
    let zero_v = f32x8::zero(token);

    let (chunks, tail) = f32x8::partition_slice_mut(token, plane);
    for chunk in chunks {
        let v = f32x8::load(token, &*chunk);
        ((v - bp_v) * inv_range_v).max(zero_v).store(chunk);
    }
    for v in tail {
        *v = ((*v - bp) * inv_range).max(0.0);
    }
}

#[magetypes(neon, wasm128)]
pub(super) fn hue_rotate_simd(token: Token, a: &mut [f32], b: &mut [f32], cos_r: f32, sin_r: f32) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let cos_v = f32x8::splat(token, cos_r);
    let sin_v = f32x8::splat(token, sin_r);
    let neg_sin_v = f32x8::splat(token, -sin_r);

    let (a_chunks, a_tail) = f32x8::partition_slice_mut(token, a);
    let (b_chunks, b_tail) = f32x8::partition_slice_mut(token, b);

    for (ac, bc) in a_chunks.iter_mut().zip(b_chunks.iter_mut()) {
        let av = f32x8::load(token, &*ac);
        let bv = f32x8::load(token, &*bc);
        cos_v.mul_add(av, neg_sin_v * bv).store(ac);
        sin_v.mul_add(av, cos_v * bv).store(bc);
    }
    for (a_val, b_val) in a_tail.iter_mut().zip(b_tail.iter_mut()) {
        let a_orig = *a_val;
        let b_orig = *b_val;
        *a_val = a_orig * cos_r - b_orig * sin_r;
        *b_val = a_orig * sin_r + b_orig * cos_r;
    }
}

#[magetypes(neon, wasm128)]
pub(super) fn highlights_shadows_simd(
    token: Token,
    plane: &mut [f32],
    shadows: f32,
    highlights: f32,
) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let shadows_half = f32x8::splat(token, shadows * 0.5);
    let highlights_half = f32x8::splat(token, highlights * 0.5);
    let one_v = f32x8::splat(token, 1.0);
    let two_v = f32x8::splat(token, 2.0);
    let half_v = f32x8::splat(token, 0.5);
    let zero_v = f32x8::zero(token);

    let (chunks, tail) = f32x8::partition_slice_mut(token, plane);
    for chunk in chunks {
        let mut l = f32x8::load(token, &*chunk);
        let sm = (one_v - l * two_v).max(zero_v);
        l = (sm * sm).mul_add(shadows_half, l);
        let hm = ((l - half_v) * two_v).max(zero_v).min(one_v);
        l -= hm * hm * highlights_half;
        l.store(chunk);
    }
    for v in tail {
        let l = *v;
        let sm = (1.0 - l * 2.0).max(0.0);
        let mut l_new = l + sm * sm * shadows * 0.5;
        let hm = ((l_new - 0.5) * 2.0).clamp(0.0, 1.0);
        l_new -= hm * hm * highlights * 0.5;
        *v = l_new;
    }
}

#[magetypes(neon, wasm128)]
pub(super) fn vibrance_simd(
    token: Token,
    a: &mut [f32],
    b: &mut [f32],
    amount: f32,
    protection: f32,
) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    const MAX_CHROMA: f32 = 0.4;
    let amount_v = f32x8::splat(token, amount);
    let inv_max_chroma_v = f32x8::splat(token, 1.0 / MAX_CHROMA);
    let one_v = f32x8::splat(token, 1.0);

    let (a_chunks, a_tail) = f32x8::partition_slice_mut(token, a);
    let (b_chunks, b_tail) = f32x8::partition_slice_mut(token, b);

    for (ac, bc) in a_chunks.iter_mut().zip(b_chunks.iter_mut()) {
        let av = f32x8::load(token, &*ac);
        let bv = f32x8::load(token, &*bc);
        let chroma = (av * av + bv * bv).sqrt();
        let normalized = (chroma * inv_max_chroma_v).min(one_v);
        let pf = (one_v - normalized).pow_lowp_unchecked(protection);
        let scale = amount_v.mul_add(pf, one_v);
        (av * scale).store(ac);
        (bv * scale).store(bc);
    }
    for (a_val, b_val) in a_tail.iter_mut().zip(b_tail.iter_mut()) {
        let av = *a_val;
        let bv = *b_val;
        let chroma = (av * av + bv * bv).sqrt();
        let normalized = (chroma / MAX_CHROMA).min(1.0);
        let pf = (1.0 - normalized).powf(protection);
        let scale = 1.0 + amount * pf;
        *a_val = av * scale;
        *b_val = bv * scale;
    }
}

// ============================================================================
// Plane arithmetic
// ============================================================================

#[magetypes(neon, wasm128)]
pub(super) fn subtract_planes_simd(token: Token, a: &[f32], b: &[f32], dst: &mut [f32]) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let n = dst.len();
    let mut i = 0;
    while i + 8 <= n {
        let av: &[f32; 8] = a[i..i + 8].try_into().unwrap();
        let bv: &[f32; 8] = b[i..i + 8].try_into().unwrap();
        let out: &mut [f32; 8] = (&mut dst[i..i + 8]).try_into().unwrap();
        (f32x8::load(token, av) - f32x8::load(token, bv)).store(out);
        i += 8;
    }
    for idx in i..n {
        dst[idx] = a[idx] - b[idx];
    }
}

#[magetypes(neon, wasm128)]
pub(super) fn square_plane_simd(token: Token, src: &[f32], dst: &mut [f32]) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let n = dst.len();
    let mut i = 0;
    while i + 8 <= n {
        let sv: &[f32; 8] = src[i..i + 8].try_into().unwrap();
        let out: &mut [f32; 8] = (&mut dst[i..i + 8]).try_into().unwrap();
        let v = f32x8::load(token, sv);
        (v * v).store(out);
        i += 8;
    }
    for idx in i..n {
        dst[idx] = src[idx] * src[idx];
    }
}

// ============================================================================
// Wavelet + adaptive sharpen
// ============================================================================

#[magetypes(neon, wasm128)]
pub(super) fn wavelet_threshold_accumulate_simd(
    token: Token,
    current: &[f32],
    smooth: &[f32],
    result: &mut [f32],
    threshold: f32,
) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let thresh_v = f32x8::splat(token, threshold);
    let neg_thresh_v = f32x8::splat(token, -threshold);
    let zero_v = f32x8::zero(token);
    let n = result.len();
    let mut i = 0;
    while i + 8 <= n {
        let c: &[f32; 8] = current[i..i + 8].try_into().unwrap();
        let s: &[f32; 8] = smooth[i..i + 8].try_into().unwrap();
        let r: &mut [f32; 8] = (&mut result[i..i + 8]).try_into().unwrap();
        let detail = f32x8::load(token, c) - f32x8::load(token, s);
        let pos = (detail - thresh_v).max(zero_v);
        let neg = (detail + thresh_v).min(zero_v);
        let is_pos = detail.simd_gt(thresh_v);
        let is_neg = detail.simd_lt(neg_thresh_v);
        let thresholded = f32x8::blend(is_pos, pos, f32x8::blend(is_neg, neg, zero_v));
        (f32x8::load(token, &*r) + thresholded).store(r);
        i += 8;
    }
    for idx in i..n {
        let detail = current[idx] - smooth[idx];
        result[idx] += if detail > threshold {
            detail - threshold
        } else if detail < -threshold {
            detail + threshold
        } else {
            0.0
        };
    }
}

#[magetypes(neon, wasm128)]
pub(super) fn add_clamped_simd(token: Token, a: &[f32], b: &[f32], dst: &mut [f32]) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let zero_v = f32x8::zero(token);
    let n = dst.len();
    let mut i = 0;
    while i + 8 <= n {
        let av: &[f32; 8] = a[i..i + 8].try_into().unwrap();
        let bv: &[f32; 8] = b[i..i + 8].try_into().unwrap();
        let out: &mut [f32; 8] = (&mut dst[i..i + 8]).try_into().unwrap();
        (f32x8::load(token, av) + f32x8::load(token, bv))
            .max(zero_v)
            .store(out);
        i += 8;
    }
    for idx in i..n {
        dst[idx] = (a[idx] + b[idx]).max(0.0);
    }
}

#[magetypes(neon, wasm128)]
pub(super) fn adaptive_sharpen_apply_simd(
    token: Token,
    l: &[f32],
    detail: &[f32],
    energy: &[f32],
    dst: &mut [f32],
    amount: f32,
    noise_floor: f32,
    masking_threshold: f32,
) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let amount_v = f32x8::splat(token, amount);
    let nf_v = f32x8::splat(token, noise_floor);
    let mt_v = f32x8::splat(token, masking_threshold);
    let zero_v = f32x8::zero(token);
    let one_v = f32x8::splat(token, 1.0);
    let has_masking = masking_threshold > 1e-8;
    let n = dst.len();

    let mut i = 0;
    while i + 8 <= n {
        let lv: &[f32; 8] = l[i..i + 8].try_into().unwrap();
        let dv: &[f32; 8] = detail[i..i + 8].try_into().unwrap();
        let ev: &[f32; 8] = energy[i..i + 8].try_into().unwrap();
        let out: &mut [f32; 8] = (&mut dst[i..i + 8]).try_into().unwrap();

        let l_val = f32x8::load(token, lv);
        let d_val = f32x8::load(token, dv);
        let e_val = f32x8::load(token, ev).max(zero_v).sqrt();

        let gate = e_val * (e_val + nf_v).recip();
        let mask = if has_masking {
            e_val * (e_val + mt_v).recip()
        } else {
            one_v
        };
        (l_val + amount_v * d_val * gate * mask)
            .max(zero_v)
            .store(out);
        i += 8;
    }
    for idx in i..n {
        let e = energy[idx].max(0.0).sqrt();
        let gate = e / (e + noise_floor);
        let mask = if has_masking {
            e / (e + masking_threshold)
        } else {
            1.0
        };
        dst[idx] = (l[idx] + amount * detail[idx] * gate * mask).max(0.0);
    }
}

// ============================================================================
// Fused adjust (L pass + AB pass)
// ============================================================================

/// Inner L-channel adjustment pass (generic over backend token).
#[inline]
fn fused_adjust_l_generic<T: F32x8Backend + F32x8Convert + Copy>(
    token: T,
    l: &mut [f32],
    bp: f32,
    inv_range: f32,
    wp_exp: f32,
    contrast_exp: f32,
    contrast_scale: f32,
    shadows: f32,
    highlights: f32,
    dehaze_contrast: f32,
) {
    type V<T> = GenericF32x8<T>;

    let bp_v = V::<T>::splat(token, bp);
    let inv_range_v = V::<T>::splat(token, inv_range);
    let zero_v = V::<T>::zero(token);
    let wp_exp_v = V::<T>::splat(token, wp_exp);
    let cs_v = V::<T>::splat(token, contrast_scale);
    let shadows_half = V::<T>::splat(token, shadows * 0.5);
    let highlights_half = V::<T>::splat(token, highlights * 0.5);
    let one_v = V::<T>::splat(token, 1.0);
    let two_v = V::<T>::splat(token, 2.0);
    let half_v = V::<T>::splat(token, 0.5);
    let dc_v = V::<T>::splat(token, dehaze_contrast);
    let dc_offset = V::<T>::splat(token, 0.5 * (1.0 - dehaze_contrast));

    let (chunks, tail) = V::<T>::partition_slice_mut(token, l);
    for chunk in chunks {
        let mut v = V::<T>::load(token, &*chunk);
        v = ((v - bp_v) * inv_range_v).max(zero_v);
        v *= wp_exp_v;
        v = v.max(zero_v).pow_lowp_unchecked(contrast_exp) * cs_v;
        let sm = (one_v - v * two_v).max(zero_v);
        v = (sm * sm).mul_add(shadows_half, v);
        let hm = ((v - half_v) * two_v).max(zero_v).min(one_v);
        v -= hm * hm * highlights_half;
        v = v.mul_add(dc_v, dc_offset);
        v.store(chunk);
    }
    for v in tail {
        let mut lv = *v;
        lv = ((lv - bp) * inv_range).max(0.0);
        lv *= wp_exp;
        if lv > 0.0 {
            lv = lv.powf(contrast_exp) * contrast_scale;
        }
        let sm = (1.0 - lv * 2.0).max(0.0);
        lv += sm * sm * shadows * 0.5;
        let hm = ((lv - 0.5) * 2.0).clamp(0.0, 1.0);
        lv -= hm * hm * highlights * 0.5;
        lv = lv * dehaze_contrast + 0.5 * (1.0 - dehaze_contrast);
        *v = lv;
    }
}

/// Inner AB-channel adjustment pass (generic over backend token).
#[inline]
fn fused_adjust_ab_generic<T: F32x8Backend + F32x8Convert + Copy>(
    token: T,
    a: &mut [f32],
    b: &mut [f32],
    dehaze_chroma: f32,
    exposure_chroma: f32,
    temp_offset: f32,
    tint_offset: f32,
    sat: f32,
    vib_amount: f32,
    vib_protection: f32,
) {
    type V<T> = GenericF32x8<T>;
    const MAX_CHROMA: f32 = 0.4;

    let exp_v = V::<T>::splat(token, exposure_chroma);
    let dc_v = V::<T>::splat(token, dehaze_chroma);
    let temp_v = V::<T>::splat(token, temp_offset);
    let tint_v = V::<T>::splat(token, tint_offset);
    let sat_v = V::<T>::splat(token, sat);
    let vib_v = V::<T>::splat(token, vib_amount);
    let inv_mc = V::<T>::splat(token, 1.0 / MAX_CHROMA);
    let one_v = V::<T>::splat(token, 1.0);

    let (a_chunks, a_tail) = V::<T>::partition_slice_mut(token, a);
    let (b_chunks, b_tail) = V::<T>::partition_slice_mut(token, b);

    for (ac, bc) in a_chunks.iter_mut().zip(b_chunks.iter_mut()) {
        let mut av = V::<T>::load(token, &*ac);
        let mut bv = V::<T>::load(token, &*bc);
        av *= exp_v;
        bv *= exp_v;
        av *= dc_v;
        bv *= dc_v;
        bv += temp_v;
        av += tint_v;
        av *= sat_v;
        bv *= sat_v;
        let chroma = (av * av + bv * bv).sqrt();
        let normalized = (chroma * inv_mc).min(one_v);
        let pf = (one_v - normalized).pow_lowp_unchecked(vib_protection);
        let scale = vib_v.mul_add(pf, one_v);
        (av * scale).store(ac);
        (bv * scale).store(bc);
    }
    for (a_val, b_val) in a_tail.iter_mut().zip(b_tail.iter_mut()) {
        let mut av = *a_val * exposure_chroma;
        let mut bv = *b_val * exposure_chroma;
        av *= dehaze_chroma;
        bv *= dehaze_chroma;
        bv += temp_offset;
        av += tint_offset;
        av *= sat;
        bv *= sat;
        let chroma = (av * av + bv * bv).sqrt();
        let normalized = (chroma / MAX_CHROMA).min(1.0);
        let pf = (1.0 - normalized).powf(vib_protection);
        let scale = 1.0 + vib_amount * pf;
        *a_val = av * scale;
        *b_val = bv * scale;
    }
}

#[magetypes(neon, wasm128)]
pub(super) fn fused_adjust_simd(
    token: Token,
    l: &mut [f32],
    a: &mut [f32],
    b: &mut [f32],
    bp: f32,
    inv_range: f32,
    wp_exp: f32,
    contrast_exp: f32,
    contrast_scale: f32,
    shadows: f32,
    highlights: f32,
    dehaze_contrast: f32,
    dehaze_chroma: f32,
    exposure_chroma: f32,
    temp_offset: f32,
    tint_offset: f32,
    sat: f32,
    vib_amount: f32,
    vib_protection: f32,
) {
    fused_adjust_l_generic(
        token,
        l,
        bp,
        inv_range,
        wp_exp,
        contrast_exp,
        contrast_scale,
        shadows,
        highlights,
        dehaze_contrast,
    );
    fused_adjust_ab_generic(
        token,
        a,
        b,
        dehaze_chroma,
        exposure_chroma,
        temp_offset,
        tint_offset,
        sat,
        vib_amount,
        vib_protection,
    );
}

// ============================================================================
// Fused interleaved adjust (RGB -> Oklab -> adjust -> RGB in one pass)
// ============================================================================

#[magetypes(neon, wasm128)]
#[allow(dead_code)]
pub(super) fn fused_interleaved_adjust_simd(
    token: Token,
    src: &[f32],
    dst: &mut [f32],
    channels: u32,
    m1: &GamutMatrix,
    m1_inv: &GamutMatrix,
    inv_white: f32,
    reference_white: f32,
    bp: f32,
    inv_range: f32,
    wp_exp: f32,
    contrast_exp: f32,
    contrast_scale: f32,
    shadows: f32,
    highlights: f32,
    dehaze_contrast: f32,
    dehaze_chroma: f32,
    exposure_chroma: f32,
    temp_offset: f32,
    tint_offset: f32,
    sat: f32,
    vib_amount: f32,
    vib_protection: f32,
) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;
    let ch = channels as usize;
    let n = dst.len() / ch;

    let m1_00 = f32x8::splat(token, m1[0][0]);
    let m1_01 = f32x8::splat(token, m1[0][1]);
    let m1_02 = f32x8::splat(token, m1[0][2]);
    let m1_10 = f32x8::splat(token, m1[1][0]);
    let m1_11 = f32x8::splat(token, m1[1][1]);
    let m1_12 = f32x8::splat(token, m1[1][2]);
    let m1_20 = f32x8::splat(token, m1[2][0]);
    let m1_21 = f32x8::splat(token, m1[2][1]);
    let m1_22 = f32x8::splat(token, m1[2][2]);

    let m2_00 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[0][0]);
    let m2_01 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[0][1]);
    let m2_02 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[0][2]);
    let m2_10 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[1][0]);
    let m2_11 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[1][1]);
    let m2_12 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[1][2]);
    let m2_20 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[2][0]);
    let m2_21 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[2][1]);
    let m2_22 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[2][2]);

    let im2_00 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[0][0]);
    let im2_01 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[0][1]);
    let im2_02 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[0][2]);
    let im2_10 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[1][0]);
    let im2_11 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[1][1]);
    let im2_12 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[1][2]);
    let im2_20 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[2][0]);
    let im2_21 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[2][1]);
    let im2_22 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[2][2]);

    let im1_00 = f32x8::splat(token, m1_inv[0][0]);
    let im1_01 = f32x8::splat(token, m1_inv[0][1]);
    let im1_02 = f32x8::splat(token, m1_inv[0][2]);
    let im1_10 = f32x8::splat(token, m1_inv[1][0]);
    let im1_11 = f32x8::splat(token, m1_inv[1][1]);
    let im1_12 = f32x8::splat(token, m1_inv[1][2]);
    let im1_20 = f32x8::splat(token, m1_inv[2][0]);
    let im1_21 = f32x8::splat(token, m1_inv[2][1]);
    let im1_22 = f32x8::splat(token, m1_inv[2][2]);

    let inv_white_v = f32x8::splat(token, inv_white);
    let white_v = f32x8::splat(token, reference_white);
    let zero_v = f32x8::zero(token);

    let bp_v = f32x8::splat(token, bp);
    let inv_range_v = f32x8::splat(token, inv_range);
    let wp_exp_v = f32x8::splat(token, wp_exp);
    let cs_v = f32x8::splat(token, contrast_scale);
    let shadows_half = f32x8::splat(token, shadows * 0.5);
    let highlights_half = f32x8::splat(token, highlights * 0.5);
    let one_v = f32x8::splat(token, 1.0);
    let two_v = f32x8::splat(token, 2.0);
    let half_v = f32x8::splat(token, 0.5);
    let dc_v = f32x8::splat(token, dehaze_contrast);
    let dc_offset = f32x8::splat(token, 0.5 * (1.0 - dehaze_contrast));

    let exp_chroma_v = f32x8::splat(token, exposure_chroma);
    let dc_chroma_v = f32x8::splat(token, dehaze_chroma);
    let temp_v = f32x8::splat(token, temp_offset);
    let tint_v = f32x8::splat(token, tint_offset);
    let sat_v = f32x8::splat(token, sat);
    let vib_v = f32x8::splat(token, vib_amount);
    let inv_mc = f32x8::splat(token, 1.0 / 0.4);

    let mut i = 0;
    while i + 8 <= n {
        let mut r_arr = [0.0f32; 8];
        let mut g_arr = [0.0f32; 8];
        let mut b_arr = [0.0f32; 8];
        for j in 0..8 {
            let base = (i + j) * ch;
            r_arr[j] = src[base];
            g_arr[j] = src[base + 1];
            b_arr[j] = src[base + 2];
        }

        let r = f32x8::from_array(token, r_arr) * inv_white_v;
        let g = f32x8::from_array(token, g_arr) * inv_white_v;
        let b = f32x8::from_array(token, b_arr) * inv_white_v;

        let lms_l = m1_00.mul_add(r, m1_01.mul_add(g, m1_02 * b));
        let lms_m = m1_10.mul_add(r, m1_11.mul_add(g, m1_12 * b));
        let lms_s = m1_20.mul_add(r, m1_21.mul_add(g, m1_22 * b));

        let l_ = lms_l.cbrt_lowp();
        let m_ = lms_m.cbrt_lowp();
        let s_ = lms_s.cbrt_lowp();

        let mut ok_l = m2_00.mul_add(l_, m2_01.mul_add(m_, m2_02 * s_));
        let mut ok_a = m2_10.mul_add(l_, m2_11.mul_add(m_, m2_12 * s_));
        let mut ok_b = m2_20.mul_add(l_, m2_21.mul_add(m_, m2_22 * s_));

        ok_l = ((ok_l - bp_v) * inv_range_v).max(zero_v);
        ok_l *= wp_exp_v;
        ok_l = ok_l.max(zero_v).pow_lowp_unchecked(contrast_exp) * cs_v;
        let sm = (one_v - ok_l * two_v).max(zero_v);
        ok_l = (sm * sm).mul_add(shadows_half, ok_l);
        let hm = ((ok_l - half_v) * two_v).max(zero_v).min(one_v);
        ok_l -= hm * hm * highlights_half;
        ok_l = ok_l.mul_add(dc_v, dc_offset);

        ok_a *= exp_chroma_v;
        ok_b *= exp_chroma_v;
        ok_a *= dc_chroma_v;
        ok_b *= dc_chroma_v;
        ok_b += temp_v;
        ok_a += tint_v;
        ok_a *= sat_v;
        ok_b *= sat_v;
        let chroma = (ok_a * ok_a + ok_b * ok_b).sqrt();
        let normalized = (chroma * inv_mc).min(one_v);
        let pf = (one_v - normalized).pow_lowp_unchecked(vib_protection);
        let vib_scale = vib_v.mul_add(pf, one_v);
        ok_a *= vib_scale;
        ok_b *= vib_scale;

        let l2 = im2_00.mul_add(ok_l, im2_01.mul_add(ok_a, im2_02 * ok_b));
        let m2 = im2_10.mul_add(ok_l, im2_11.mul_add(ok_a, im2_12 * ok_b));
        let s2 = im2_20.mul_add(ok_l, im2_21.mul_add(ok_a, im2_22 * ok_b));

        let lms_l2 = l2 * l2 * l2;
        let lms_m2 = m2 * m2 * m2;
        let lms_s2 = s2 * s2 * s2;

        let r_out =
            (im1_00.mul_add(lms_l2, im1_01.mul_add(lms_m2, im1_02 * lms_s2)) * white_v).max(zero_v);
        let g_out =
            (im1_10.mul_add(lms_l2, im1_11.mul_add(lms_m2, im1_12 * lms_s2)) * white_v).max(zero_v);
        let b_out =
            (im1_20.mul_add(lms_l2, im1_21.mul_add(lms_m2, im1_22 * lms_s2)) * white_v).max(zero_v);

        let r_a = r_out.to_array();
        let g_a = g_out.to_array();
        let b_a = b_out.to_array();
        for j in 0..8 {
            let base = (i + j) * ch;
            dst[base] = r_a[j];
            dst[base + 1] = g_a[j];
            dst[base + 2] = b_a[j];
        }

        i += 8;
    }

    for idx in i..n {
        let base = idx * ch;
        let r = src[base] * inv_white;
        let g = src[base + 1] * inv_white;
        let bv = src[base + 2] * inv_white;
        let [mut ok_l, mut ok_a, mut ok_b] = oklab::rgb_to_oklab(r, g, bv, m1);

        ok_l = ((ok_l - bp) * inv_range).max(0.0);
        ok_l *= wp_exp;
        if ok_l > 0.0 {
            ok_l = ok_l.powf(contrast_exp) * contrast_scale;
        }
        let sm = (1.0 - ok_l * 2.0).max(0.0);
        ok_l += sm * sm * shadows * 0.5;
        let hm = ((ok_l - 0.5) * 2.0).clamp(0.0, 1.0);
        ok_l -= hm * hm * highlights * 0.5;
        ok_l = ok_l * dehaze_contrast + 0.5 * (1.0 - dehaze_contrast);

        ok_a *= exposure_chroma * dehaze_chroma;
        ok_b *= exposure_chroma * dehaze_chroma;
        ok_b += temp_offset;
        ok_a += tint_offset;
        ok_a *= sat;
        ok_b *= sat;
        let chroma = (ok_a * ok_a + ok_b * ok_b).sqrt();
        let pf = (1.0 - (chroma / 0.4).min(1.0)).powf(vib_protection);
        let vib_scale = 1.0 + vib_amount * pf;
        ok_a *= vib_scale;
        ok_b *= vib_scale;

        let [ro, go, bo] = oklab::oklab_to_rgb(ok_l, ok_a, ok_b, m1_inv);
        dst[base] = (ro * reference_white).max(0.0);
        dst[base + 1] = (go * reference_white).max(0.0);
        dst[base + 2] = (bo * reference_white).max(0.0);
    }
}
