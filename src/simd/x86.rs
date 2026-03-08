use archmage::prelude::*;
use magetypes::simd::f32x8;

use crate::blur::GaussianKernel;

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
) {
    gaussian_blur_plane_simd(token, src, dst, width, height, kernel);
}

#[rite]
fn gaussian_blur_plane_simd(
    token: X64V3Token,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    kernel: &GaussianKernel,
) {
    let w = width as usize;
    let h = height as usize;
    let radius = kernel.radius;

    let mut h_buf = vec![0.0f32; w * h];
    let mut padded = Vec::with_capacity(w + 2 * radius);

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
            for (k, &weight) in kernel.weights.iter().enumerate() {
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
            for (k, &weight) in kernel.weights.iter().enumerate() {
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
            for (k, &weight) in kernel.weights.iter().enumerate() {
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
            for (k, &weight) in kernel.weights.iter().enumerate() {
                let sy = (y + k).saturating_sub(radius).min(h - 1);
                sum += h_buf[sy * w + x] * weight;
            }
            *v = sum;
        }
    }
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
