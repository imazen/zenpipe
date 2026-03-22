#![allow(clippy::too_many_arguments)]

use crate::prelude::*;
use archmage::prelude::*;
use magetypes::simd::f32x8;

use crate::blur::GaussianKernel;
use crate::context::FilterContext;
use linear_srgb::tokens::x8::{linear_to_srgb_u8_neon, srgb_u8_to_linear_neon};
use zenpixels_convert::gamut::GamutMatrix;
use zenpixels_convert::oklab::{self, LMS_CBRT_FROM_OKLAB, OKLAB_FROM_LMS_CBRT};

#[arcane]
pub(super) fn scale_plane_impl_neon(token: NeonToken, plane: &mut [f32], factor: f32) {
    scale_plane_simd(token, plane, factor);
}

#[rite]
fn scale_plane_simd(token: NeonToken, plane: &mut [f32], factor: f32) {
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

#[arcane]
pub(super) fn offset_plane_impl_neon(token: NeonToken, plane: &mut [f32], offset: f32) {
    offset_plane_simd(token, plane, offset);
}

#[rite]
fn offset_plane_simd(token: NeonToken, plane: &mut [f32], offset: f32) {
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

#[arcane]
pub(super) fn power_contrast_plane_impl_neon(
    token: NeonToken,
    plane: &mut [f32],
    exp: f32,
    scale: f32,
) {
    power_contrast_plane_simd(token, plane, exp, scale);
}

#[rite]
fn power_contrast_plane_simd(token: NeonToken, plane: &mut [f32], exp: f32, scale: f32) {
    let scale_v = f32x8::splat(token, scale);
    let zero_v = f32x8::zero(token);

    let (chunks, tail) = f32x8::partition_slice_mut(token, plane);
    for chunk in chunks {
        let v = f32x8::load(token, &*chunk);
        // pow_midp handles v=0 correctly (returns 0)
        let powered = v.max(zero_v).pow_lowp_unchecked(exp);
        (powered * scale_v).store(chunk);
    }
    for v in tail {
        if *v > 0.0 {
            *v = v.powf(exp) * scale;
        }
    }
}

#[arcane]
pub(super) fn sigmoid_tone_map_plane_impl_neon(
    token: NeonToken,
    plane: &mut [f32],
    contrast: f32,
    bias_a: f32,
) {
    sigmoid_tone_map_plane_simd(token, plane, contrast, bias_a);
}

#[rite]
fn sigmoid_tone_map_plane_simd(token: NeonToken, plane: &mut [f32], contrast: f32, bias_a: f32) {
    let one_v = f32x8::splat(token, 1.0);
    let zero_v = f32x8::zero(token);
    let bias_a_v = f32x8::splat(token, bias_a);
    let has_bias = bias_a.abs() > 1e-6;

    let (chunks, tail) = f32x8::partition_slice_mut(token, plane);
    for chunk in chunks {
        let mut x = f32x8::load(token, &*chunk).max(zero_v).min(one_v);

        // Schlick bias: x / (bias_a * (1 - x) + 1)
        if has_bias {
            let denom = (one_v - x).mul_add(bias_a_v, one_v);
            x *= denom.recip();
            x = x.max(zero_v).min(one_v);
        }

        // Sigmoid: 1 / (1 + ((1-x)/x)^c)
        // Compute ratio = (1-x)/x, handling x near 0 by clamping
        let x_safe = x.max(f32x8::splat(token, 1e-7));
        let ratio = (one_v - x_safe) * x_safe.recip();
        let powered = ratio.pow_lowp_unchecked(contrast);
        let result = (one_v + powered).recip();

        // Blend: use 0.0 where x <= 0, 1.0 where x >= 1
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

#[arcane]
pub(super) fn unsharp_fuse_impl_neon(
    token: NeonToken,
    src: &[f32],
    blurred: &[f32],
    dst: &mut [f32],
    amount: f32,
) {
    unsharp_fuse_simd(token, src, blurred, dst, amount);
}

#[rite]
fn unsharp_fuse_simd(
    token: NeonToken,
    src: &[f32],
    blurred: &[f32],
    dst: &mut [f32],
    amount: f32,
) {
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

#[arcane]
pub(super) fn gaussian_blur_plane_impl_neon(
    _token: NeonToken,
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
        stackblur_plane_simd(_token, src, dst, width, height, radius, ctx);
        return;
    }
    gaussian_blur_plane_simd(_token, src, dst, width, height, kernel, ctx);
}

#[rite]
fn gaussian_blur_plane_simd(
    token: NeonToken,
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

    let mut h_buf = ctx.take_f32(w * h);
    let mut padded = ctx.take_f32(w + 2 * radius);

    // Horizontal pass
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        // Pad row with edge replication
        padded.clear();
        let edge_l = row[0];
        let edge_r = row[w - 1];
        padded.extend(core::iter::repeat_n(edge_l, radius));
        padded.extend_from_slice(row);
        padded.extend(core::iter::repeat_n(edge_r, radius));

        let out_row = &mut h_buf[y * w..(y + 1) * w];
        let (out_chunks, out_tail) = f32x8::partition_slice_mut(token, out_row);

        for (ci, out_chunk) in out_chunks.iter_mut().enumerate() {
            let x = ci * 8;
            let mut acc = f32x8::zero(token);
            for (k, &weight) in kernel.weights().iter().enumerate() {
                let wv = f32x8::splat(token, weight);
                let src_chunk: &[f32; 8] = padded[x + k..x + k + 8].try_into().unwrap();
                let vals = f32x8::load(token, src_chunk);
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

    // Vertical pass — row-major for sequential writes.
    // Column-tiling was benchmarked twice and regressed both times
    // (strided writes are worse than strided reads for FIR where
    // O(kernel_size) reads per pixel dominate the access pattern).
    for y in 0..h {
        let out_row = &mut dst[y * w..(y + 1) * w];
        let (out_chunks, out_tail) = f32x8::partition_slice_mut(token, out_row);

        for (ci, out_chunk) in out_chunks.iter_mut().enumerate() {
            let x = ci * 8;
            let mut acc = f32x8::zero(token);
            for (k, &weight) in kernel.weights().iter().enumerate() {
                let sy = (y + k).saturating_sub(radius).min(h - 1);
                let wv = f32x8::splat(token, weight);
                let src_chunk: &[f32; 8] = h_buf[sy * w + x..sy * w + x + 8].try_into().unwrap();
                acc = f32x8::load(token, src_chunk).mul_add(wv, acc);
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

#[arcane]
pub(super) fn brilliance_apply_impl_neon(
    token: NeonToken,
    src_l: &[f32],
    avg_l: &[f32],
    dst_l: &mut [f32],
    amount: f32,
    shadow_str: f32,
    highlight_str: f32,
) {
    brilliance_apply_simd(
        token,
        src_l,
        avg_l,
        dst_l,
        amount,
        shadow_str,
        highlight_str,
    );
}

#[rite]
fn brilliance_apply_simd(
    token: NeonToken,
    src_l: &[f32],
    avg_l: &[f32],
    dst_l: &mut [f32],
    amount: f32,
    shadow_str: f32,
    highlight_str: f32,
) {
    // S-curve adaptation: smoothstep-weighted correction factors.
    // Instead of linear correction (which clips hard at ratio boundaries),
    // use quadratic smoothstep: t² * (3 - 2t) for smooth S-curve response.
    // This matches the perceptual response of commercial "Brilliance" sliders.
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

        // Shadow: t = clamp(1 - ratio, 0, 1), smoothstep = t² * (3 - 2t)
        let st = (one - ratio).max(zero).min(one);
        let shadow_curve = st * st * (three - two * st);
        let shadow_corr = shadow_curve.mul_add(sa, one);

        // Highlight: t = clamp(ratio - 1, 0, 1), smoothstep
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

#[allow(clippy::too_many_arguments)]
#[arcane]
pub(super) fn scatter_oklab_impl_neon(
    token: NeonToken,
    src: &[f32],
    l: &mut [f32],
    a: &mut [f32],
    b: &mut [f32],
    channels: u32,
    m1: &GamutMatrix,
    inv_white: f32,
) {
    scatter_oklab_simd(token, src, l, a, b, channels, m1, inv_white);
}

#[rite]
#[allow(clippy::too_many_arguments)]
fn scatter_oklab_simd(
    token: NeonToken,
    src: &[f32],
    l_out: &mut [f32],
    a_out: &mut [f32],
    b_out: &mut [f32],
    channels: u32,
    m1: &GamutMatrix,
    inv_white: f32,
) {
    let n = l_out.len();
    let ch = channels as usize;

    // M1 coefficients (RGB → LMS, gamut-dependent)
    let m1_00 = f32x8::splat(token, m1[0][0]);
    let m1_01 = f32x8::splat(token, m1[0][1]);
    let m1_02 = f32x8::splat(token, m1[0][2]);
    let m1_10 = f32x8::splat(token, m1[1][0]);
    let m1_11 = f32x8::splat(token, m1[1][1]);
    let m1_12 = f32x8::splat(token, m1[1][2]);
    let m1_20 = f32x8::splat(token, m1[2][0]);
    let m1_21 = f32x8::splat(token, m1[2][1]);
    let m1_22 = f32x8::splat(token, m1[2][2]);

    // M2 coefficients (LMS^(1/3) → Oklab, universal)
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
        // Deinterleave 8 pixels from interleaved src
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

        // M1: linear RGB → LMS (FMA chains)
        let lms_l = m1_00.mul_add(r, m1_01.mul_add(g, m1_02 * b));
        let lms_m = m1_10.mul_add(r, m1_11.mul_add(g, m1_12 * b));
        let lms_s = m1_20.mul_add(r, m1_21.mul_add(g, m1_22 * b));

        // Cube root (SIMD lowp — 1 Halley iteration, 1.8× faster than midp)
        let l_ = lms_l.cbrt_lowp();
        let m_ = lms_m.cbrt_lowp();
        let s_ = lms_s.cbrt_lowp();

        // M2: LMS^(1/3) → Oklab (FMA chains)
        let ok_l = m2_00.mul_add(l_, m2_01.mul_add(m_, m2_02 * s_));
        let ok_a = m2_10.mul_add(l_, m2_11.mul_add(m_, m2_12 * s_));
        let ok_b = m2_20.mul_add(l_, m2_21.mul_add(m_, m2_22 * s_));

        // Store to planes (direct SIMD store to contiguous memory)
        let l_chunk: &mut [f32; 8] = (&mut l_out[i..i + 8]).try_into().unwrap();
        let a_chunk: &mut [f32; 8] = (&mut a_out[i..i + 8]).try_into().unwrap();
        let b_chunk: &mut [f32; 8] = (&mut b_out[i..i + 8]).try_into().unwrap();
        ok_l.store(l_chunk);
        ok_a.store(a_chunk);
        ok_b.store(b_chunk);

        i += 8;
    }

    // Scalar tail
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

#[allow(clippy::too_many_arguments)]
#[arcane]
pub(super) fn gather_oklab_impl_neon(
    token: NeonToken,
    l: &[f32],
    a: &[f32],
    b: &[f32],
    dst: &mut [f32],
    channels: u32,
    m1_inv: &GamutMatrix,
    reference_white: f32,
) {
    gather_oklab_simd(token, l, a, b, dst, channels, m1_inv, reference_white);
}

#[rite]
#[allow(clippy::too_many_arguments)]
fn gather_oklab_simd(
    token: NeonToken,
    l_in: &[f32],
    a_in: &[f32],
    b_in: &[f32],
    dst: &mut [f32],
    channels: u32,
    m1_inv: &GamutMatrix,
    reference_white: f32,
) {
    let n = l_in.len();
    let ch = channels as usize;

    // Inverse M2 coefficients (Oklab → LMS^(1/3), universal)
    let im2_00 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[0][0]);
    let im2_01 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[0][1]);
    let im2_02 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[0][2]);
    let im2_10 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[1][0]);
    let im2_11 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[1][1]);
    let im2_12 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[1][2]);
    let im2_20 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[2][0]);
    let im2_21 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[2][1]);
    let im2_22 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[2][2]);

    // Inverse M1 coefficients (LMS → RGB, gamut-dependent)
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
        // Load 8 values from each plane
        let l_chunk: &[f32; 8] = l_in[i..i + 8].try_into().unwrap();
        let a_chunk: &[f32; 8] = a_in[i..i + 8].try_into().unwrap();
        let b_chunk: &[f32; 8] = b_in[i..i + 8].try_into().unwrap();
        let lv = f32x8::load(token, l_chunk);
        let av = f32x8::load(token, a_chunk);
        let bv = f32x8::load(token, b_chunk);

        // Inverse M2: Oklab → LMS^(1/3) (FMA chains)
        let l_ = im2_00.mul_add(lv, im2_01.mul_add(av, im2_02 * bv));
        let m_ = im2_10.mul_add(lv, im2_11.mul_add(av, im2_12 * bv));
        let s_ = im2_20.mul_add(lv, im2_21.mul_add(av, im2_22 * bv));

        // Cube: LMS^(1/3) → LMS
        let lms_l = l_ * l_ * l_;
        let lms_m = m_ * m_ * m_;
        let lms_s = s_ * s_ * s_;

        // Inverse M1: LMS → linear RGB (FMA chains)
        let r = im1_00.mul_add(lms_l, im1_01.mul_add(lms_m, im1_02 * lms_s));
        let g = im1_10.mul_add(lms_l, im1_11.mul_add(lms_m, im1_12 * lms_s));
        let b = im1_20.mul_add(lms_l, im1_21.mul_add(lms_m, im1_22 * lms_s));

        // Scale by reference white and clamp to [0, ∞)
        let r_out = (r * white_v).max(zero_v);
        let g_out = (g * white_v).max(zero_v);
        let b_out = (b * white_v).max(zero_v);

        // Reinterleave and store
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

    // Scalar tail
    for idx in i..n {
        let [r, g, bv] = oklab::oklab_to_rgb(l_in[idx], a_in[idx], b_in[idx], m1_inv);
        let base = idx * ch;
        dst[base] = (r * reference_white).max(0.0);
        dst[base + 1] = (g * reference_white).max(0.0);
        dst[base + 2] = (bv * reference_white).max(0.0);
    }
}

// ============================================================================
// Fused sRGB u8 ↔ Oklab (eliminates intermediate linear f32 buffer)
// ============================================================================

#[allow(clippy::too_many_arguments)]
#[arcane]
pub(super) fn scatter_srgb_u8_to_oklab_impl_neon(
    token: NeonToken,
    src: &[u8],
    l: &mut [f32],
    a: &mut [f32],
    b: &mut [f32],
    channels: u32,
    m1: &GamutMatrix,
) {
    scatter_srgb_u8_to_oklab_rite(token, src, l, a, b, channels, m1);
}

#[rite]
#[allow(clippy::too_many_arguments)]
fn scatter_srgb_u8_to_oklab_rite(
    token: NeonToken,
    src: &[u8],
    l_out: &mut [f32],
    a_out: &mut [f32],
    b_out: &mut [f32],
    channels: u32,
    m1: &GamutMatrix,
) {
    let n = l_out.len();
    let ch = channels as usize;

    // M1 coefficients (RGB → LMS, gamut-dependent)
    let m1_00 = f32x8::splat(token, m1[0][0]);
    let m1_01 = f32x8::splat(token, m1[0][1]);
    let m1_02 = f32x8::splat(token, m1[0][2]);
    let m1_10 = f32x8::splat(token, m1[1][0]);
    let m1_11 = f32x8::splat(token, m1[1][1]);
    let m1_12 = f32x8::splat(token, m1[1][2]);
    let m1_20 = f32x8::splat(token, m1[2][0]);
    let m1_21 = f32x8::splat(token, m1[2][1]);
    let m1_22 = f32x8::splat(token, m1[2][2]);

    // M2 coefficients (LMS^(1/3) → Oklab, universal)
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
        // Deinterleave 8 pixels from interleaved u8 src
        let mut r_u8 = [0u8; 8];
        let mut g_u8 = [0u8; 8];
        let mut b_u8 = [0u8; 8];
        for j in 0..8 {
            let base = (i + j) * ch;
            r_u8[j] = src[base];
            g_u8[j] = src[base + 1];
            b_u8[j] = src[base + 2];
        }

        // LUT: sRGB u8 → linear f32 (no math, just 8 lookups per channel)
        let r = f32x8::from_array(token, srgb_u8_to_linear_neon(token, r_u8));
        let g = f32x8::from_array(token, srgb_u8_to_linear_neon(token, g_u8));
        let b = f32x8::from_array(token, srgb_u8_to_linear_neon(token, b_u8));

        // M1: linear RGB → LMS (FMA chains)
        let lms_l = m1_00.mul_add(r, m1_01.mul_add(g, m1_02 * b));
        let lms_m = m1_10.mul_add(r, m1_11.mul_add(g, m1_12 * b));
        let lms_s = m1_20.mul_add(r, m1_21.mul_add(g, m1_22 * b));

        // Cube root (SIMD lowp — 1 Halley iteration, 1.8× faster than midp)
        let l_ = lms_l.cbrt_lowp();
        let m_ = lms_m.cbrt_lowp();
        let s_ = lms_s.cbrt_lowp();

        // M2: LMS^(1/3) → Oklab (FMA chains)
        let ok_l = m2_00.mul_add(l_, m2_01.mul_add(m_, m2_02 * s_));
        let ok_a = m2_10.mul_add(l_, m2_11.mul_add(m_, m2_12 * s_));
        let ok_b = m2_20.mul_add(l_, m2_21.mul_add(m_, m2_22 * s_));

        // Store to planes (direct SIMD store to contiguous memory)
        let l_chunk: &mut [f32; 8] = (&mut l_out[i..i + 8]).try_into().unwrap();
        let a_chunk: &mut [f32; 8] = (&mut a_out[i..i + 8]).try_into().unwrap();
        let b_chunk: &mut [f32; 8] = (&mut b_out[i..i + 8]).try_into().unwrap();
        ok_l.store(l_chunk);
        ok_a.store(a_chunk);
        ok_b.store(b_chunk);

        i += 8;
    }

    // Scalar tail
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

#[allow(clippy::too_many_arguments)]
#[arcane]
pub(super) fn gather_oklab_to_srgb_u8_impl_neon(
    token: NeonToken,
    l: &[f32],
    a: &[f32],
    b: &[f32],
    dst: &mut [u8],
    channels: u32,
    m1_inv: &GamutMatrix,
) {
    gather_oklab_to_srgb_u8_rite(token, l, a, b, dst, channels, m1_inv);
}

#[rite]
#[allow(clippy::too_many_arguments)]
fn gather_oklab_to_srgb_u8_rite(
    token: NeonToken,
    l_in: &[f32],
    a_in: &[f32],
    b_in: &[f32],
    dst: &mut [u8],
    channels: u32,
    m1_inv: &GamutMatrix,
) {
    let n = l_in.len();
    let ch = channels as usize;

    // Inverse M2 coefficients (Oklab → LMS^(1/3), universal)
    let im2_00 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[0][0]);
    let im2_01 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[0][1]);
    let im2_02 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[0][2]);
    let im2_10 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[1][0]);
    let im2_11 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[1][1]);
    let im2_12 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[1][2]);
    let im2_20 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[2][0]);
    let im2_21 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[2][1]);
    let im2_22 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[2][2]);

    // Inverse M1 coefficients (LMS → RGB, gamut-dependent)
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
        // Load 8 values from each plane
        let l_chunk: &[f32; 8] = l_in[i..i + 8].try_into().unwrap();
        let a_chunk: &[f32; 8] = a_in[i..i + 8].try_into().unwrap();
        let b_chunk: &[f32; 8] = b_in[i..i + 8].try_into().unwrap();
        let lv = f32x8::load(token, l_chunk);
        let av = f32x8::load(token, a_chunk);
        let bv = f32x8::load(token, b_chunk);

        // Inverse M2: Oklab → LMS^(1/3) (FMA chains)
        let l_ = im2_00.mul_add(lv, im2_01.mul_add(av, im2_02 * bv));
        let m_ = im2_10.mul_add(lv, im2_11.mul_add(av, im2_12 * bv));
        let s_ = im2_20.mul_add(lv, im2_21.mul_add(av, im2_22 * bv));

        // Cube: LMS^(1/3) → LMS
        let lms_l = l_ * l_ * l_;
        let lms_m = m_ * m_ * m_;
        let lms_s = s_ * s_ * s_;

        // Inverse M1: LMS → linear RGB (FMA chains)
        let r = im1_00.mul_add(lms_l, im1_01.mul_add(lms_m, im1_02 * lms_s));
        let g = im1_10.mul_add(lms_l, im1_11.mul_add(lms_m, im1_12 * lms_s));
        let b = im1_20.mul_add(lms_l, im1_21.mul_add(lms_m, im1_22 * lms_s));

        // LUT: linear f32 → sRGB u8 (includes clamp to [0, 1])
        let r_u8 = linear_to_srgb_u8_neon(token, r.to_array());
        let g_u8 = linear_to_srgb_u8_neon(token, g.to_array());
        let b_u8 = linear_to_srgb_u8_neon(token, b.to_array());

        // Reinterleave and store
        for j in 0..8 {
            let base = (i + j) * ch;
            dst[base] = r_u8[j];
            dst[base + 1] = g_u8[j];
            dst[base + 2] = b_u8[j];
        }

        i += 8;
    }

    // Scalar tail
    for idx in i..n {
        let [r, g, bv] = oklab::oklab_to_rgb(l_in[idx], a_in[idx], b_in[idx], m1_inv);
        let base = idx * ch;
        dst[base] = linear_srgb::default::linear_to_srgb_u8(r);
        dst[base + 1] = linear_srgb::default::linear_to_srgb_u8(g);
        dst[base + 2] = linear_srgb::default::linear_to_srgb_u8(bv);
    }
}

// ============================================================================
// Per-pixel filter SIMD: replaces scalar for-loops in filters
// ============================================================================

#[arcane]
pub(super) fn black_point_plane_impl_neon(
    token: NeonToken,
    plane: &mut [f32],
    bp: f32,
    inv_range: f32,
) {
    black_point_plane_rite(token, plane, bp, inv_range);
}

#[rite]
fn black_point_plane_rite(token: NeonToken, plane: &mut [f32], bp: f32, inv_range: f32) {
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

#[arcane]
pub(super) fn hue_rotate_impl_neon(
    token: NeonToken,
    a: &mut [f32],
    b: &mut [f32],
    cos_r: f32,
    sin_r: f32,
) {
    hue_rotate_rite(token, a, b, cos_r, sin_r);
}

#[rite]
fn hue_rotate_rite(token: NeonToken, a: &mut [f32], b: &mut [f32], cos_r: f32, sin_r: f32) {
    let cos_v = f32x8::splat(token, cos_r);
    let sin_v = f32x8::splat(token, sin_r);
    let neg_sin_v = f32x8::splat(token, -sin_r);

    let (a_chunks, a_tail) = f32x8::partition_slice_mut(token, a);
    let (b_chunks, b_tail) = f32x8::partition_slice_mut(token, b);

    for (ac, bc) in a_chunks.iter_mut().zip(b_chunks.iter_mut()) {
        let av = f32x8::load(token, &*ac);
        let bv = f32x8::load(token, &*bc);
        // a' = a*cos - b*sin, b' = a*sin + b*cos
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

#[arcane]
pub(super) fn highlights_shadows_impl_neon(
    token: NeonToken,
    plane: &mut [f32],
    shadows: f32,
    highlights: f32,
) {
    highlights_shadows_rite(token, plane, shadows, highlights);
}

#[rite]
fn highlights_shadows_rite(token: NeonToken, plane: &mut [f32], shadows: f32, highlights: f32) {
    let shadows_half = f32x8::splat(token, shadows * 0.5);
    let highlights_half = f32x8::splat(token, highlights * 0.5);
    let one_v = f32x8::splat(token, 1.0);
    let two_v = f32x8::splat(token, 2.0);
    let half_v = f32x8::splat(token, 0.5);
    let zero_v = f32x8::zero(token);

    let (chunks, tail) = f32x8::partition_slice_mut(token, plane);
    for chunk in chunks {
        let mut l = f32x8::load(token, &*chunk);
        // Shadows: mask = max(1 - l*2, 0), l += mask² * shadows * 0.5
        let sm = (one_v - l * two_v).max(zero_v);
        l = (sm * sm).mul_add(shadows_half, l);
        // Highlights: mask = clamp((l-0.5)*2, 0, 1), l -= mask² * highlights * 0.5
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

#[arcane]
pub(super) fn vibrance_impl_neon(
    token: NeonToken,
    a: &mut [f32],
    b: &mut [f32],
    amount: f32,
    protection: f32,
) {
    vibrance_rite(token, a, b, amount, protection);
}

#[rite]
fn vibrance_rite(token: NeonToken, a: &mut [f32], b: &mut [f32], amount: f32, protection: f32) {
    const MAX_CHROMA: f32 = 0.4;
    let amount_v = f32x8::splat(token, amount);
    let inv_max_chroma_v = f32x8::splat(token, 1.0 / MAX_CHROMA);
    let one_v = f32x8::splat(token, 1.0);

    let (a_chunks, a_tail) = f32x8::partition_slice_mut(token, a);
    let (b_chunks, b_tail) = f32x8::partition_slice_mut(token, b);

    for (ac, bc) in a_chunks.iter_mut().zip(b_chunks.iter_mut()) {
        let av = f32x8::load(token, &*ac);
        let bv = f32x8::load(token, &*bc);
        // chroma = sqrt(a² + b²)
        let chroma = (av * av + bv * bv).sqrt();
        // normalized = min(chroma / MAX_CHROMA, 1.0)
        let normalized = (chroma * inv_max_chroma_v).min(one_v);
        // protection_factor = (1 - normalized)^protection
        let pf = (one_v - normalized).pow_lowp_unchecked(protection);
        // scale = 1 + amount * pf
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

#[allow(clippy::too_many_arguments)]
#[arcane]
pub(super) fn fused_adjust_impl_neon(
    token: NeonToken,
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
    fused_adjust_l_rite(
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
    fused_adjust_ab_rite(
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

#[rite]
#[allow(clippy::too_many_arguments)]
fn fused_adjust_l_rite(
    token: NeonToken,
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
    let bp_v = f32x8::splat(token, bp);
    let inv_range_v = f32x8::splat(token, inv_range);
    let zero_v = f32x8::zero(token);
    let wp_exp_v = f32x8::splat(token, wp_exp);
    let cs_v = f32x8::splat(token, contrast_scale);
    let shadows_half = f32x8::splat(token, shadows * 0.5);
    let highlights_half = f32x8::splat(token, highlights * 0.5);
    let one_v = f32x8::splat(token, 1.0);
    let two_v = f32x8::splat(token, 2.0);
    let half_v = f32x8::splat(token, 0.5);
    let dc_v = f32x8::splat(token, dehaze_contrast);
    let dc_offset = f32x8::splat(token, 0.5 * (1.0 - dehaze_contrast));

    let (chunks, tail) = f32x8::partition_slice_mut(token, l);
    for chunk in chunks {
        let mut v = f32x8::load(token, &*chunk);
        // 1. Black point: (v - bp) * inv_range, clamped to 0
        v = ((v - bp_v) * inv_range_v).max(zero_v);
        // 2+3. White point * exposure (combined multiply)
        v *= wp_exp_v;
        // 4. Contrast: power curve v^exp * scale, pivot at middle grey
        v = v.max(zero_v).pow_lowp_unchecked(contrast_exp) * cs_v;
        // 5. Shadows: mask = max(1 - v*2, 0), v += mask² * shadows*0.5
        let sm = (one_v - v * two_v).max(zero_v);
        v = (sm * sm).mul_add(shadows_half, v);
        // 6. Highlights: mask = clamp((v-0.5)*2, 0, 1), v -= mask² * highlights*0.5
        let hm = ((v - half_v) * two_v).max(zero_v).min(one_v);
        v -= hm * hm * highlights_half;
        // 7. Dehaze L: v * dc + 0.5 * (1 - dc)
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

#[rite]
#[allow(clippy::too_many_arguments)]
fn fused_adjust_ab_rite(
    token: NeonToken,
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
    const MAX_CHROMA: f32 = 0.4;
    let exp_v = f32x8::splat(token, exposure_chroma);
    let dc_v = f32x8::splat(token, dehaze_chroma);
    let temp_v = f32x8::splat(token, temp_offset);
    let tint_v = f32x8::splat(token, tint_offset);
    let sat_v = f32x8::splat(token, sat);
    let vib_v = f32x8::splat(token, vib_amount);
    let inv_mc = f32x8::splat(token, 1.0 / MAX_CHROMA);
    let one_v = f32x8::splat(token, 1.0);

    let (a_chunks, a_tail) = f32x8::partition_slice_mut(token, a);
    let (b_chunks, b_tail) = f32x8::partition_slice_mut(token, b);

    for (ac, bc) in a_chunks.iter_mut().zip(b_chunks.iter_mut()) {
        let mut av = f32x8::load(token, &*ac);
        let mut bv = f32x8::load(token, &*bc);
        // Exposure chroma scaling (matches L-pass exposure)
        av *= exp_v;
        bv *= exp_v;
        // Dehaze chroma
        av *= dc_v;
        bv *= dc_v;
        // Temperature (b) + tint (a)
        bv += temp_v;
        av += tint_v;
        // Saturation
        av *= sat_v;
        bv *= sat_v;
        // Vibrance
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

// ============================================================================
// Plane arithmetic SIMD helpers
// ============================================================================

#[arcane]
pub(super) fn subtract_planes_impl_neon(token: NeonToken, a: &[f32], b: &[f32], dst: &mut [f32]) {
    subtract_planes_rite(token, a, b, dst);
}

#[rite]
fn subtract_planes_rite(token: NeonToken, a: &[f32], b: &[f32], dst: &mut [f32]) {
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

#[arcane]
pub(super) fn square_plane_impl_neon(token: NeonToken, src: &[f32], dst: &mut [f32]) {
    square_plane_rite(token, src, dst);
}

#[rite]
fn square_plane_rite(token: NeonToken, src: &[f32], dst: &mut [f32]) {
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
// Wavelet + adaptive sharpen SIMD helpers
// ============================================================================

#[arcane]
pub(super) fn wavelet_threshold_accumulate_impl_neon(
    token: NeonToken,
    current: &[f32],
    smooth: &[f32],
    result: &mut [f32],
    threshold: f32,
) {
    wavelet_threshold_accumulate_rite(token, current, smooth, result, threshold);
}

#[rite]
fn wavelet_threshold_accumulate_rite(
    token: NeonToken,
    current: &[f32],
    smooth: &[f32],
    result: &mut [f32],
    threshold: f32,
) {
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
        // Soft threshold: clamp(detail - thresh, 0) for positive, clamp(detail + thresh, -inf, 0) for negative
        let pos = (detail - thresh_v).max(zero_v);
        let neg = (detail + thresh_v).min(zero_v);
        // Combine: if detail > thresh → pos, if detail < -thresh → neg, else 0
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

#[arcane]
pub(super) fn add_clamped_impl_neon(token: NeonToken, a: &[f32], b: &[f32], dst: &mut [f32]) {
    add_clamped_rite(token, a, b, dst);
}

#[rite]
fn add_clamped_rite(token: NeonToken, a: &[f32], b: &[f32], dst: &mut [f32]) {
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

#[arcane]
#[allow(clippy::too_many_arguments)]
pub(super) fn adaptive_sharpen_apply_impl_neon(
    token: NeonToken,
    l: &[f32],
    detail: &[f32],
    energy: &[f32],
    dst: &mut [f32],
    amount: f32,
    noise_floor: f32,
    masking_threshold: f32,
) {
    adaptive_sharpen_apply_rite(
        token,
        l,
        detail,
        energy,
        dst,
        amount,
        noise_floor,
        masking_threshold,
    );
}

#[rite]
#[allow(clippy::too_many_arguments)]
fn adaptive_sharpen_apply_rite(
    token: NeonToken,
    l: &[f32],
    detail: &[f32],
    energy: &[f32],
    dst: &mut [f32],
    amount: f32,
    noise_floor: f32,
    masking_threshold: f32,
) {
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

        // gate = e / (e + noise_floor)
        let gate = e_val * (e_val + nf_v).recip();
        // mask = e / (e + masking_threshold) or 1.0
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
// Fused interleaved per-pixel: RGB→Oklab→adjust→RGB in one streaming pass
// ============================================================================

/// Fused per-pixel adjustment on interleaved linear RGB f32 data.
///
/// Performs RGB→Oklab conversion, all per-pixel L and AB adjustments, and
/// Oklab→RGB conversion in a single streaming pass — no planar buffers,
/// no scatter/gather. All data stays in SIMD registers between conversions.
///
/// This is faster than scatter→adjust→gather for per-pixel-only pipelines
/// because it touches memory only twice (read src, write dst) instead of
/// ~6 times (scatter 3 planes + adjust L + adjust AB + gather 3 planes).
#[allow(clippy::too_many_arguments)]
#[arcane]
pub(super) fn fused_interleaved_adjust_impl_neon(
    token: NeonToken,
    src: &[f32],
    dst: &mut [f32],
    channels: u32,
    m1: &GamutMatrix,
    m1_inv: &GamutMatrix,
    inv_white: f32,
    reference_white: f32,
    // L adjustments
    bp: f32,
    inv_range: f32,
    wp_exp: f32,
    contrast_exp: f32,
    contrast_scale: f32,
    shadows: f32,
    highlights: f32,
    dehaze_contrast: f32,
    // AB adjustments
    dehaze_chroma: f32,
    exposure_chroma: f32,
    temp_offset: f32,
    tint_offset: f32,
    sat: f32,
    vib_amount: f32,
    vib_protection: f32,
) {
    fused_interleaved_adjust_rite(
        token,
        src,
        dst,
        channels,
        m1,
        m1_inv,
        inv_white,
        reference_white,
        bp,
        inv_range,
        wp_exp,
        contrast_exp,
        contrast_scale,
        shadows,
        highlights,
        dehaze_contrast,
        dehaze_chroma,
        exposure_chroma,
        temp_offset,
        tint_offset,
        sat,
        vib_amount,
        vib_protection,
    );
}

#[rite]
#[allow(clippy::too_many_arguments)]
fn fused_interleaved_adjust_rite(
    token: NeonToken,
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
    let ch = channels as usize;
    let n = dst.len() / ch;

    // Forward M1: RGB → LMS
    let m1_00 = f32x8::splat(token, m1[0][0]);
    let m1_01 = f32x8::splat(token, m1[0][1]);
    let m1_02 = f32x8::splat(token, m1[0][2]);
    let m1_10 = f32x8::splat(token, m1[1][0]);
    let m1_11 = f32x8::splat(token, m1[1][1]);
    let m1_12 = f32x8::splat(token, m1[1][2]);
    let m1_20 = f32x8::splat(token, m1[2][0]);
    let m1_21 = f32x8::splat(token, m1[2][1]);
    let m1_22 = f32x8::splat(token, m1[2][2]);

    // Forward M2: LMS^(1/3) → Oklab
    let m2_00 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[0][0]);
    let m2_01 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[0][1]);
    let m2_02 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[0][2]);
    let m2_10 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[1][0]);
    let m2_11 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[1][1]);
    let m2_12 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[1][2]);
    let m2_20 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[2][0]);
    let m2_21 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[2][1]);
    let m2_22 = f32x8::splat(token, OKLAB_FROM_LMS_CBRT[2][2]);

    // Inverse M2: Oklab → LMS^(1/3)
    let im2_00 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[0][0]);
    let im2_01 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[0][1]);
    let im2_02 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[0][2]);
    let im2_10 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[1][0]);
    let im2_11 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[1][1]);
    let im2_12 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[1][2]);
    let im2_20 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[2][0]);
    let im2_21 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[2][1]);
    let im2_22 = f32x8::splat(token, LMS_CBRT_FROM_OKLAB[2][2]);

    // Inverse M1: LMS → RGB
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

    // L adjustment constants
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

    // AB adjustment constants
    let exp_chroma_v = f32x8::splat(token, exposure_chroma);
    let dc_chroma_v = f32x8::splat(token, dehaze_chroma);
    let temp_v = f32x8::splat(token, temp_offset);
    let tint_v = f32x8::splat(token, tint_offset);
    let sat_v = f32x8::splat(token, sat);
    let vib_v = f32x8::splat(token, vib_amount);
    let inv_mc = f32x8::splat(token, 1.0 / 0.4);

    let mut i = 0;
    while i + 8 <= n {
        // ── Deinterleave 8 pixels ──
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

        // ── RGB → Oklab (stays in registers) ──
        let lms_l = m1_00.mul_add(r, m1_01.mul_add(g, m1_02 * b));
        let lms_m = m1_10.mul_add(r, m1_11.mul_add(g, m1_12 * b));
        let lms_s = m1_20.mul_add(r, m1_21.mul_add(g, m1_22 * b));

        let l_ = lms_l.cbrt_lowp();
        let m_ = lms_m.cbrt_lowp();
        let s_ = lms_s.cbrt_lowp();

        let mut ok_l = m2_00.mul_add(l_, m2_01.mul_add(m_, m2_02 * s_));
        let mut ok_a = m2_10.mul_add(l_, m2_11.mul_add(m_, m2_12 * s_));
        let mut ok_b = m2_20.mul_add(l_, m2_21.mul_add(m_, m2_22 * s_));

        // ── L adjustments (all in-register) ──
        ok_l = ((ok_l - bp_v) * inv_range_v).max(zero_v);
        ok_l *= wp_exp_v;
        ok_l = ok_l.max(zero_v).pow_lowp_unchecked(contrast_exp) * cs_v;
        let sm = (one_v - ok_l * two_v).max(zero_v);
        ok_l = (sm * sm).mul_add(shadows_half, ok_l);
        let hm = ((ok_l - half_v) * two_v).max(zero_v).min(one_v);
        ok_l -= hm * hm * highlights_half;
        ok_l = ok_l.mul_add(dc_v, dc_offset);

        // ── AB adjustments (all in-register) ──
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

        // ── Oklab → RGB (stays in registers) ──
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

        // ── Reinterleave and store ──
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

    // Scalar tail
    for idx in i..n {
        let base = idx * ch;
        let r = src[base] * inv_white;
        let g = src[base + 1] * inv_white;
        let bv = src[base + 2] * inv_white;
        let [mut ok_l, mut ok_a, mut ok_b] = oklab::rgb_to_oklab(r, g, bv, m1);

        // L adjust
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

        // AB adjust
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

// ============================================================================
// SIMD Stackblur — 2 memory passes, no transpose
// ============================================================================

/// Stackblur on a single f32 plane with SIMD vertical pass.
///
/// Horizontal pass: scalar, one row at a time (cache-optimal sequential access).
/// Vertical pass: f32x8, processes 8 adjacent columns simultaneously (no transpose).
///
/// Total: 2 memory passes over the data vs 4 in the scalar transpose-based version.
#[rite]
fn stackblur_plane_simd(
    token: NeonToken,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    radius: u32,
    ctx: &mut FilterContext,
) {
    use crate::blur::stackblur_row;

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
    let inv_div_v = f32x8::splat(token, inv_div);

    // --- Pass 1: Horizontal (scalar, row-by-row) → h_buf ---
    let mut h_buf = ctx.take_f32(n);
    let mut stack_scratch = ctx.take_f32(stack_size);

    for y in 0..h {
        let row = &src[y * w..][..w];
        let out = &mut h_buf[y * w..][..w];
        stackblur_row(row, out, w, r, &mut stack_scratch, inv_div);
    }

    ctx.return_f32(stack_scratch);

    // --- Pass 2: Vertical (SIMD, 8 columns at a time) → dst ---
    // Each tile of 8 columns is processed independently with f32x8.
    // No transpose needed — loads from h_buf[y*w + x..x+8] are contiguous.
    let num_tiles = w / 8;

    // Stack for SIMD vertical: stack_size elements, each f32x8
    let mut stack_v: Vec<[f32; 8]> = vec![[0.0; 8]; stack_size];

    for tile in 0..num_tiles {
        let x = tile * 8;

        // Initialize stack and running sums for this tile
        let mut sum = f32x8::zero(token);
        let mut sum_in = f32x8::zero(token);
        let mut sum_out = f32x8::zero(token);

        // Fill left side + center of stack (edge-replicated from row 0)
        let first: &[f32; 8] = h_buf[x..x + 8].try_into().unwrap();
        let first_v = f32x8::load(token, first);
        for sv in stack_v.iter_mut().take(r + 1) {
            *sv = first_v.to_array();
        }

        // Fill right side of stack
        for (i, sv) in stack_v.iter_mut().enumerate().take(stack_size).skip(r + 1) {
            let offset = i - r; // row offset from center (positive)
            let sy = offset.min(h - 1);
            let chunk: &[f32; 8] = h_buf[sy * w + x..sy * w + x + 8].try_into().unwrap();
            *sv = f32x8::load(token, chunk).to_array();
        }

        // Compute initial weighted sum
        for (i, sv) in stack_v.iter().enumerate().take(stack_size) {
            let dist = r.abs_diff(i);
            let weight = f32x8::splat(token, (r + 1 - dist) as f32);
            let val = f32x8::from_array(token, *sv);
            sum = val.mul_add(weight, sum);
        }

        // Initial sum_out (positions 0..=r) and sum_in (positions r+1..stack_size)
        for sv in stack_v.iter().take(r + 1) {
            sum_out += f32x8::from_array(token, *sv);
        }
        for sv in stack_v.iter().take(stack_size).skip(r + 1) {
            sum_in += f32x8::from_array(token, *sv);
        }

        let mut sp = 0usize;

        // Main vertical scan
        for y in 0..h {
            // Output
            let out: &mut [f32; 8] = (&mut dst[y * w + x..y * w + x + 8]).try_into().unwrap();
            (sum * inv_div_v).store(out);

            // Update running sums
            sum -= sum_out;
            let old_val = f32x8::from_array(token, stack_v[sp]);
            sum_out -= old_val;

            // Load new pixel from row y + r + 1 (clamped)
            let new_y = (y + r + 1).min(h - 1);
            let new_chunk: &[f32; 8] = h_buf[new_y * w + x..new_y * w + x + 8].try_into().unwrap();
            let new_val = f32x8::load(token, new_chunk);
            stack_v[sp] = new_val.to_array();
            sum_in += new_val;

            sum += sum_in;

            sp += 1;
            if sp >= stack_size {
                sp = 0;
            }

            // Transfer center from in to out
            let center_idx = if sp + r >= stack_size {
                sp + r - stack_size
            } else {
                sp + r
            };
            let center_val = f32x8::from_array(token, stack_v[center_idx]);
            sum_out += center_val;
            sum_in -= center_val;
        }
    }

    // Scalar tail: remaining columns (w % 8)
    let x_start = num_tiles * 8;
    if x_start < w {
        let mut stack_scalar = ctx.take_f32(stack_size);
        let mut col_buf = ctx.take_f32(h);
        let mut col_out = ctx.take_f32(h);

        for x in x_start..w {
            // Gather column from h_buf
            for y in 0..h {
                col_buf[y] = h_buf[y * w + x];
            }
            crate::blur::stackblur_row(&col_buf, &mut col_out, h, r, &mut stack_scalar, inv_div);
            // Scatter column to dst
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
