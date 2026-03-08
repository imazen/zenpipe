#![allow(clippy::too_many_arguments)]

use archmage::prelude::*;
use magetypes::simd::f32x8;

use crate::blur::GaussianKernel;
use crate::context::FilterContext;
use zenpixels_convert::gamut::GamutMatrix;
use zenpixels_convert::oklab::{self, LMS_CBRT_FROM_OKLAB, OKLAB_FROM_LMS_CBRT};

#[arcane]
pub(super) fn scale_plane_impl_v3(token: X64V3Token, plane: &mut [f32], factor: f32) {
    scale_plane_simd(token, plane, factor);
}

#[rite]
fn scale_plane_simd(token: X64V3Token, plane: &mut [f32], factor: f32) {
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
pub(super) fn offset_plane_impl_v3(token: X64V3Token, plane: &mut [f32], offset: f32) {
    offset_plane_simd(token, plane, offset);
}

#[rite]
fn offset_plane_simd(token: X64V3Token, plane: &mut [f32], offset: f32) {
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
pub(super) fn unsharp_fuse_impl_v3(
    token: X64V3Token,
    src: &[f32],
    blurred: &[f32],
    dst: &mut [f32],
    amount: f32,
) {
    unsharp_fuse_simd(token, src, blurred, dst, amount);
}

#[rite]
fn unsharp_fuse_simd(
    token: X64V3Token,
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
pub(super) fn gaussian_blur_plane_impl_v3(
    token: X64V3Token,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    kernel: &GaussianKernel,
    ctx: &mut FilterContext,
) {
    gaussian_blur_plane_simd(token, src, dst, width, height, kernel, ctx);
}

#[rite]
fn gaussian_blur_plane_simd(
    token: X64V3Token,
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

    // Vertical pass
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
pub(super) fn brilliance_apply_impl_v3(
    token: X64V3Token,
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
    token: X64V3Token,
    src_l: &[f32],
    avg_l: &[f32],
    dst_l: &mut [f32],
    amount: f32,
    shadow_str: f32,
    highlight_str: f32,
) {
    let one = f32x8::splat(token, 1.0);
    let min_avg = f32x8::splat(token, 0.001);
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

        let shadow_corr = (one - ratio).mul_add(sa, one);
        let highlight_corr = one - (ratio - one).min(one) * ha;
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
            1.0 + (1.0 - ratio) * shadow_str * amount
        } else {
            1.0 - (ratio - 1.0).min(1.0) * highlight_str * amount
        };
        *v = l * c;
    }
}

#[allow(clippy::too_many_arguments)]
#[arcane]
pub(super) fn scatter_oklab_impl_v3(
    token: X64V3Token,
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
    token: X64V3Token,
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

        // Cube root (SIMD, ~3 ULP precision)
        let l_ = lms_l.cbrt_midp();
        let m_ = lms_m.cbrt_midp();
        let s_ = lms_s.cbrt_midp();

        // M2: LMS^(1/3) → Oklab (FMA chains)
        let ok_l = m2_00.mul_add(l_, m2_01.mul_add(m_, m2_02 * s_));
        let ok_a = m2_10.mul_add(l_, m2_11.mul_add(m_, m2_12 * s_));
        let ok_b = m2_20.mul_add(l_, m2_21.mul_add(m_, m2_22 * s_));

        // Store to planes
        let l_arr = ok_l.to_array();
        let a_arr = ok_a.to_array();
        let b_arr = ok_b.to_array();
        l_out[i..i + 8].copy_from_slice(&l_arr);
        a_out[i..i + 8].copy_from_slice(&a_arr);
        b_out[i..i + 8].copy_from_slice(&b_arr);

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
pub(super) fn gather_oklab_impl_v3(
    token: X64V3Token,
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
    token: X64V3Token,
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
