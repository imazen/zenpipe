//! SIMD-accelerated warp/rotation for planar f32 images.
//!
//! Two approaches benchmarked:
//!
//! **Approach A — Planar SIMD (8-wide Robidoux):**
//! Process 8 consecutive output pixels in parallel per plane.
//! Coordinate transform is incremental (add m0/m3 per x step for affine).
//! Robidoux kernel weights are evaluated in SIMD via Horner's method on f32x8.
//! Gather is scalar (no safe AVX2 gather wrapper), but accumulation is SIMD.
//!
//! **Approach B — Separable row-gather:**
//! For each output row, precompute integer Y positions for all 4 kernel rows,
//! then for each kernel row, do a full-width SIMD horizontal convolution pass.
//! This is more cache-friendly because each source row is read contiguously.

use crate::prelude::*;

use archmage::prelude::*;

use crate::filters::warp::{WarpBackground, WarpInterpolation};

/// Generic f32x8 — polyfills as 2×f32x4 on NEON/WASM, native on AVX2.
use magetypes::simd::generic::f32x8 as GenericF32x8;

// ─── Robidoux kernel constants ──────────────────────────────────────
//
// Mitchell-Netravali with B=0.37821575509399867, C=0.31089212245300067
//
// For |t| < 1: f(t) = a3*|t|^3 + a2*|t|^2 + a0
//   a3 = (12 - 9B - 6C) / 6
//   a2 = (-18 + 12B + 6C) / 6
//   a0 = (6 - 2B) / 6
//
// For 1 <= |t| < 2: f(t) = b3*|t|^3 + b2*|t|^2 + b1*|t| + b0
//   b3 = (-B - 6C) / 6
//   b2 = (6B + 30C) / 6
//   b1 = (-12B - 48C) / 6
//   b0 = (8B + 24C) / 6

const B: f64 = 0.37821575509399867;
const C: f64 = 0.31089212245300067;

pub(super) const A3: f32 = ((12.0 - 9.0 * B - 6.0 * C) / 6.0) as f32;
pub(super) const A2: f32 = ((-18.0 + 12.0 * B + 6.0 * C) / 6.0) as f32;
pub(super) const A0: f32 = ((6.0 - 2.0 * B) / 6.0) as f32;

pub(super) const B3: f32 = ((-B - 6.0 * C) / 6.0) as f32;
pub(super) const B2: f32 = ((6.0 * B + 30.0 * C) / 6.0) as f32;
pub(super) const B1: f32 = ((-12.0 * B - 48.0 * C) / 6.0) as f32;
pub(super) const B0: f32 = ((8.0 * B + 24.0 * C) / 6.0) as f32;

/// Scalar Robidoux for reference / tail handling.
#[inline]
fn robidoux_scalar(t: f32) -> f32 {
    let t = t.abs();
    if t < 1.0 {
        ((A3 * t + A2) * t) * t + A0
    } else if t < 2.0 {
        (((B3 * t + B2) * t) + B1) * t + B0
    } else {
        0.0
    }
}

// ─── Approach A: Planar SIMD warp (8-wide) ──────────────────────────

/// Warp a single f32 plane using SIMD-accelerated affine transform + Robidoux.
///
/// Processes 8 output pixels at a time. The coordinate transform exploits
/// the linearity of affine mapping: for consecutive x positions, sx increases
/// by m0 and sy increases by m3.
///
/// The kernel evaluation uses Horner's method on f32x8, evaluating all 8
/// pixels' kernel weights simultaneously. Gather is scalar (load individual
/// f32 values from the source plane), but the weighted accumulation is SIMD.
#[allow(clippy::too_many_arguments)]
pub fn warp_plane_simd_planar(
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    background: WarpBackground,
    _interp: WarpInterpolation,
) {
    // Dispatch through archmage
    archmage::incant!(
        warp_plane_simd_planar_dispatch(src, dst, width, height, m, background),
        [v3, neon, wasm128, scalar]
    );
}

#[archmage::arcane]
fn warp_plane_simd_planar_dispatch_v3(
    token: archmage::X64V3Token,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    background: WarpBackground,
) {
    warp_plane_v3(token, src, dst, width, height, m, background);
}

// NEON/WASM128: fall through to scalar for single-plane (alpha only).
// The fused 3-plane path handles the hot path on all architectures.
fn warp_plane_simd_planar_dispatch_neon(
    _token: archmage::NeonToken,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    background: WarpBackground,
) {
    warp_plane_scalar_inner(src, dst, width, height, m, background);
}

fn warp_plane_simd_planar_dispatch_wasm128(
    _token: archmage::Wasm128Token,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    background: WarpBackground,
) {
    warp_plane_scalar_inner(src, dst, width, height, m, background);
}

fn warp_plane_simd_planar_dispatch_scalar(
    _token: archmage::ScalarToken,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    background: WarpBackground,
) {
    warp_plane_scalar_inner(src, dst, width, height, m, background);
}

/// Scalar fallback — identical algorithm, no SIMD.
fn warp_plane_scalar_inner(
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    background: WarpBackground,
) {
    let w = width as usize;
    let h = height as usize;
    let wf = width as f32;
    let hf = height as f32;
    let bg_val = match background {
        WarpBackground::Clamp => None,
        WarpBackground::Color { l, .. } => Some(l), // caller passes per-plane
    };

    for dy in 0..h {
        let dyf = dy as f32;
        let mut sx = m[0] * 0.0 + m[1] * dyf + m[2];
        let mut sy = m[3] * 0.0 + m[4] * dyf + m[5];

        for dx in 0..w {
            let out_idx = dy * w + dx;

            // Out-of-bounds check
            if let Some(bg) = bg_val {
                if sx < -0.5 || sx >= wf - 0.5 || sy < -0.5 || sy >= hf - 0.5 {
                    dst[out_idx] = bg;
                    sx += m[0];
                    sy += m[3];
                    continue;
                }
            }

            let sx_c = sx.clamp(0.0, wf - 1.0);
            let sy_c = sy.clamp(0.0, hf - 1.0);

            dst[out_idx] = sample_robidoux_scalar(src, w, width, height, sx_c, sy_c);

            sx += m[0];
            sy += m[3];
        }
    }
}

/// Scalar separable Robidoux sample on a single plane.
#[inline]
fn sample_robidoux_scalar(plane: &[f32], stride: usize, w: u32, h: u32, x: f32, y: f32) -> f32 {
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let fx = x - ix as f32;
    let fy = y - iy as f32;

    // Precompute 1D weights (4-tap)
    let mut wx = [0.0f32; 4];
    let mut wy = [0.0f32; 4];
    let mut wx_sum = 0.0f32;
    let mut wy_sum = 0.0f32;

    for i in 0..4 {
        let offset = i as i32 - 1;
        let wt_x = robidoux_scalar(offset as f32 - fx);
        let wt_y = robidoux_scalar(offset as f32 - fy);
        wx[i] = wt_x;
        wy[i] = wt_y;
        wx_sum += wt_x;
        wy_sum += wt_y;
    }

    // Normalize
    let inv_wx = if wx_sum.abs() > 1e-10 {
        1.0 / wx_sum
    } else {
        1.0
    };
    let inv_wy = if wy_sum.abs() > 1e-10 {
        1.0 / wy_sum
    } else {
        1.0
    };
    for wt in &mut wx {
        *wt *= inv_wx;
    }
    for wt in &mut wy {
        *wt *= inv_wy;
    }

    // 2D separable convolution
    let mut sum = 0.0f32;
    for j in 0..4 {
        let sy = (iy + j as i32 - 1).clamp(0, h as i32 - 1) as usize;
        let mut row_sum = 0.0f32;
        for i in 0..4 {
            let sx = (ix + i as i32 - 1).clamp(0, w as i32 - 1) as usize;
            row_sum += plane[sy * stride + sx] * wx[i];
        }
        sum += row_sum * wy[j];
    }
    sum
}

/// AVX2 inner loop: 8-wide Robidoux warp on a single f32 plane.
#[archmage::rite]
fn warp_plane_v3(
    token: archmage::X64V3Token,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    background: WarpBackground,
) {
    use magetypes::simd::f32x8;

    let w = width as usize;
    let h = height as usize;
    let wf = width as f32;
    let hf = height as f32;

    let bg_val = match background {
        WarpBackground::Clamp => 0.0f32, // placeholder — clamp mode doesn't use this
        WarpBackground::Color { l, .. } => l,
    };
    let is_color_bg = matches!(background, WarpBackground::Color { .. });

    // Coordinate increments for 8 consecutive x positions
    let m0 = m[0]; // d(sx)/dx
    let m3 = m[3]; // d(sy)/dx

    // f32x8 with offsets [0,1,2,3,4,5,6,7] * m0
    let dx_offsets = f32x8::from_array(
        token,
        [
            0.0 * m0,
            1.0 * m0,
            2.0 * m0,
            3.0 * m0,
            4.0 * m0,
            5.0 * m0,
            6.0 * m0,
            7.0 * m0,
        ],
    );
    let dy_offsets = f32x8::from_array(
        token,
        [
            0.0 * m3,
            1.0 * m3,
            2.0 * m3,
            3.0 * m3,
            4.0 * m3,
            5.0 * m3,
            6.0 * m3,
            7.0 * m3,
        ],
    );

    let step_sx = f32x8::splat(token, 8.0 * m0); // advance 8 pixels
    let step_sy = f32x8::splat(token, 8.0 * m3);

    // Clamp bounds
    let zero_v = f32x8::zero(token);
    let wf_minus1 = f32x8::splat(token, wf - 1.0);
    let hf_minus1 = f32x8::splat(token, hf - 1.0);

    // OOB bounds for color fill mode
    let neg_half = f32x8::splat(token, -0.5);
    let wf_half = f32x8::splat(token, wf - 0.5);
    let hf_half = f32x8::splat(token, hf - 0.5);

    let one_v = f32x8::splat(token, 1.0);

    // Robidoux kernel constants for SIMD Horner evaluation
    let a3_v = f32x8::splat(token, A3);
    let a2_v = f32x8::splat(token, A2);
    let a0_v = f32x8::splat(token, A0);
    let b3_v = f32x8::splat(token, B3);
    let b2_v = f32x8::splat(token, B2);
    let b1_v = f32x8::splat(token, B1);
    let b0_v = f32x8::splat(token, B0);
    let two_v = f32x8::splat(token, 2.0);
    let bg_v = f32x8::splat(token, bg_val);
    let _eps_v = f32x8::splat(token, 1e-10);

    for dy in 0..h {
        let dyf = dy as f32;
        // Base source coords for x=0 in this row
        let base_sx = m[0] * 0.0 + m[1] * dyf + m[2];
        let base_sy = m[3] * 0.0 + m[4] * dyf + m[5];

        let mut sx_v = f32x8::splat(token, base_sx) + dx_offsets;
        let mut sy_v = f32x8::splat(token, base_sy) + dy_offsets;

        let row_start = dy * w;
        let mut dx = 0usize;

        // Process 8 pixels at a time
        while dx + 8 <= w {
            let out_base = row_start + dx;

            if is_color_bg {
                // Check if ALL 8 pixels are out of bounds → fill with bg
                let oob_left = sx_v.simd_lt(neg_half);
                let oob_right = sx_v.simd_ge(wf_half);
                let oob_top = sy_v.simd_lt(neg_half);
                let oob_bottom = sy_v.simd_ge(hf_half);

                // Any pixel out of bounds in any direction
                // Combine masks using bitwise OR on the f32x8 masks
                // f32x8 comparison returns all-1s/all-0s per lane
                let oob_x = f32x8::blend(oob_left, one_v, f32x8::blend(oob_right, one_v, zero_v));
                let oob_y = f32x8::blend(oob_top, one_v, f32x8::blend(oob_bottom, one_v, zero_v));
                let any_oob = (oob_x + oob_y).simd_gt(zero_v);

                // If any pixel is OOB, handle per-pixel (fall through to scalar tail for mixed)
                // For simplicity, if ALL are OOB, fill the block
                let all_oob_sum = f32x8::blend(any_oob, one_v, zero_v).reduce_add();
                if all_oob_sum >= 8.0 {
                    let chunk: &mut [f32; 8] =
                        (&mut dst[out_base..out_base + 8]).try_into().unwrap();
                    bg_v.store(chunk);
                    sx_v = sx_v + step_sx;
                    sy_v = sy_v + step_sy;
                    dx += 8;
                    continue;
                }
            }

            // Clamp source coordinates
            let sx_c = sx_v.clamp(zero_v, wf_minus1);
            let sy_c = sy_v.clamp(zero_v, hf_minus1);

            // Floor to get integer positions
            let sx_floor = sx_c.floor();
            let sy_floor = sy_c.floor();

            // Fractional parts
            let fx = sx_c - sx_floor;
            let fy = sy_c - sy_floor;

            // Integer positions (as i32 via f32→i32 truncation, after floor)
            let ix_arr = sx_floor.to_i32x8().to_array();
            let iy_arr = sy_floor.to_i32x8().to_array();

            // Compute 4 horizontal kernel weights and 4 vertical kernel weights
            // entirely in SIMD registers — no store/reload through arrays.
            //
            // Robidoux kernel via Horner's method on f32x8:
            //   |t| < 1: ((a3*t + a2)*t)*t + a0  (note: a1 = 0 for Mitchell-Netravali)
            //   1 <= |t| < 2: ((b3*t + b2)*t + b1)*t + b0

            // Tap offsets: -1, 0, +1, +2 relative to floor
            let neg1_v = f32x8::splat(token, -1.0);
            let pos2_v = f32x8::splat(token, 2.0);

            // Horizontal weights (4 taps)
            let tx0 = (neg1_v - fx).abs();
            let tx1 = fx; // (0 - fx).abs() = fx for 0 <= fx < 1
            let tx2 = (one_v - fx).abs();
            let tx3 = (pos2_v - fx).abs();

            // Evaluate kernel for all 4 taps (inline Horner, no branch — use blend)
            #[inline(always)]
            fn robidoux_f32x8(
                t: magetypes::simd::f32x8,
                a3: magetypes::simd::f32x8,
                a2: magetypes::simd::f32x8,
                a0: magetypes::simd::f32x8,
                b3: magetypes::simd::f32x8,
                b2: magetypes::simd::f32x8,
                b1: magetypes::simd::f32x8,
                b0: magetypes::simd::f32x8,
                one: magetypes::simd::f32x8,
                two: magetypes::simd::f32x8,
                zero: magetypes::simd::f32x8,
            ) -> magetypes::simd::f32x8 {
                // Inner: ((a3*t + a2)*t)*t + a0
                let inner = t.mul_add(a3, a2).mul_add(t, zero).mul_add(t, a0);
                // Outer: ((b3*t + b2)*t + b1)*t + b0
                let outer = t.mul_add(b3, b2).mul_add(t, b1).mul_add(t, b0);
                let is_inner = t.simd_lt(one);
                let is_valid = t.simd_lt(two);
                magetypes::simd::f32x8::blend(
                    is_inner,
                    inner,
                    magetypes::simd::f32x8::blend(is_valid, outer, zero),
                )
            }

            let wx0 = robidoux_f32x8(
                tx0, a3_v, a2_v, a0_v, b3_v, b2_v, b1_v, b0_v, one_v, two_v, zero_v,
            );
            let wx1 = robidoux_f32x8(
                tx1, a3_v, a2_v, a0_v, b3_v, b2_v, b1_v, b0_v, one_v, two_v, zero_v,
            );
            let wx2 = robidoux_f32x8(
                tx2, a3_v, a2_v, a0_v, b3_v, b2_v, b1_v, b0_v, one_v, two_v, zero_v,
            );
            let wx3 = robidoux_f32x8(
                tx3, a3_v, a2_v, a0_v, b3_v, b2_v, b1_v, b0_v, one_v, two_v, zero_v,
            );

            // Vertical weights
            let ty0 = (neg1_v - fy).abs();
            let ty1 = fy;
            let ty2 = (one_v - fy).abs();
            let ty3 = (pos2_v - fy).abs();

            let wy0 = robidoux_f32x8(
                ty0, a3_v, a2_v, a0_v, b3_v, b2_v, b1_v, b0_v, one_v, two_v, zero_v,
            );
            let wy1 = robidoux_f32x8(
                ty1, a3_v, a2_v, a0_v, b3_v, b2_v, b1_v, b0_v, one_v, two_v, zero_v,
            );
            let wy2 = robidoux_f32x8(
                ty2, a3_v, a2_v, a0_v, b3_v, b2_v, b1_v, b0_v, one_v, two_v, zero_v,
            );
            let wy3 = robidoux_f32x8(
                ty3, a3_v, a2_v, a0_v, b3_v, b2_v, b1_v, b0_v, one_v, two_v, zero_v,
            );

            // Normalize weights in SIMD
            let wx_sum = (wx0 + wx1) + (wx2 + wx3);
            let wy_sum = (wy0 + wy1) + (wy2 + wy3);
            let inv_wx = wx_sum.recip();
            let inv_wy = wy_sum.recip();
            let wx0 = wx0 * inv_wx;
            let wx1 = wx1 * inv_wx;
            let wx2 = wx2 * inv_wx;
            let wx3 = wx3 * inv_wx;
            let wy0 = wy0 * inv_wy;
            let wy1 = wy1 * inv_wy;
            let wy2 = wy2 * inv_wy;
            let wy3 = wy3 * inv_wy;

            // Gather source pixels and accumulate.
            // For each of the 4 vertical kernel rows, gather 4 horizontal
            // samples per pixel (scalar), then do SIMD horizontal dot product
            // and vertical accumulation.
            let w_i32 = width as i32;
            let h_i32 = height as i32;

            // Helper: gather 8 source values and do horizontal dot product
            #[inline(always)]
            fn gather_row_dot(
                src: &[f32],
                w: usize,
                w_i32: i32,
                h_i32: i32,
                ix: &[i32; 8],
                iy_row: &[i32; 8],
                wx0: magetypes::simd::f32x8,
                wx1: magetypes::simd::f32x8,
                wx2: magetypes::simd::f32x8,
                wx3: magetypes::simd::f32x8,
                token: archmage::X64V3Token,
            ) -> magetypes::simd::f32x8 {
                // Gather 4 columns for 8 pixels each
                let mut g0 = [0.0f32; 8];
                let mut g1 = [0.0f32; 8];
                let mut g2 = [0.0f32; 8];
                let mut g3 = [0.0f32; 8];
                for p in 0..8 {
                    let gy = iy_row[p].clamp(0, h_i32 - 1) as usize;
                    let base = gy * w;
                    let x0 = (ix[p] - 1).clamp(0, w_i32 - 1) as usize;
                    let x1 = ix[p].clamp(0, w_i32 - 1) as usize;
                    let x2 = (ix[p] + 1).clamp(0, w_i32 - 1) as usize;
                    let x3 = (ix[p] + 2).clamp(0, w_i32 - 1) as usize;
                    g0[p] = src[base + x0];
                    g1[p] = src[base + x1];
                    g2[p] = src[base + x2];
                    g3[p] = src[base + x3];
                }
                let s0 = magetypes::simd::f32x8::from_array(token, g0);
                let s1 = magetypes::simd::f32x8::from_array(token, g1);
                let s2 = magetypes::simd::f32x8::from_array(token, g2);
                let s3 = magetypes::simd::f32x8::from_array(token, g3);
                // Horizontal dot product: s0*wx0 + s1*wx1 + s2*wx2 + s3*wx3
                s0.mul_add(wx0, s1.mul_add(wx1, s2.mul_add(wx2, s3 * wx3)))
            }

            // Compute iy for each of the 4 rows
            let mut iy_m1 = [0i32; 8];
            let mut iy_0 = [0i32; 8];
            let mut iy_p1 = [0i32; 8];
            let mut iy_p2 = [0i32; 8];
            for p in 0..8 {
                iy_m1[p] = iy_arr[p] - 1;
                iy_0[p] = iy_arr[p];
                iy_p1[p] = iy_arr[p] + 1;
                iy_p2[p] = iy_arr[p] + 2;
            }

            let row0 = gather_row_dot(
                src, w, w_i32, h_i32, &ix_arr, &iy_m1, wx0, wx1, wx2, wx3, token,
            );
            let row1 = gather_row_dot(
                src, w, w_i32, h_i32, &ix_arr, &iy_0, wx0, wx1, wx2, wx3, token,
            );
            let row2 = gather_row_dot(
                src, w, w_i32, h_i32, &ix_arr, &iy_p1, wx0, wx1, wx2, wx3, token,
            );
            let row3 = gather_row_dot(
                src, w, w_i32, h_i32, &ix_arr, &iy_p2, wx0, wx1, wx2, wx3, token,
            );

            // Vertical accumulation
            let mut result = row0.mul_add(wy0, row1.mul_add(wy1, row2.mul_add(wy2, row3 * wy3)));

            // Handle OOB pixels for color fill mode
            if is_color_bg {
                let oob_left = sx_v.simd_lt(neg_half);
                let oob_right = sx_v.simd_ge(wf_half);
                let oob_top = sy_v.simd_lt(neg_half);
                let oob_bottom = sy_v.simd_ge(hf_half);
                // Combine: any direction OOB
                let oob = f32x8::blend(
                    oob_left,
                    one_v,
                    f32x8::blend(
                        oob_right,
                        one_v,
                        f32x8::blend(oob_top, one_v, f32x8::blend(oob_bottom, one_v, zero_v)),
                    ),
                );
                let is_oob = oob.simd_gt(zero_v);
                result = f32x8::blend(is_oob, bg_v, result);
            }

            let chunk: &mut [f32; 8] = (&mut dst[out_base..out_base + 8]).try_into().unwrap();
            result.store(chunk);

            sx_v = sx_v + step_sx;
            sy_v = sy_v + step_sy;
            dx += 8;
        }

        // Scalar tail for remaining < 8 pixels
        while dx < w {
            let out_idx = row_start + dx;
            let sx = m[0] * dx as f32 + m[1] * dyf + m[2];
            let sy = m[3] * dx as f32 + m[4] * dyf + m[5];

            if is_color_bg && (sx < -0.5 || sx >= wf - 0.5 || sy < -0.5 || sy >= hf - 0.5) {
                dst[out_idx] = bg_val;
            } else {
                let sx_c = sx.clamp(0.0, wf - 1.0);
                let sy_c = sy.clamp(0.0, hf - 1.0);
                dst[out_idx] = sample_robidoux_scalar(src, w, width, height, sx_c, sy_c);
            }
            dx += 1;
        }
    }
}

// ─── Approach B: Separable row-gather warp ──────────────────────────
//
// Instead of gathering 16 source values per output pixel, this approach
// precomputes integer Y positions for the 4 kernel rows across the whole
// output row, then for each kernel row, reads the source row once and
// does the horizontal convolution in SIMD.
//
// This trades random access (gather) for sequential access (sweep) and
// is more cache-friendly for large images.

/// Warp a single f32 plane using the row-gather approach.
///
/// For each output row:
/// 1. Compute all source coordinates for the row
/// 2. Compute and normalize all kernel weights
/// 3. For each of the 4 vertical kernel taps:
///    a. For each of the 4 horizontal taps:
///       - Gather source values and multiply by horizontal weight
///    b. Accumulate with vertical weight
#[allow(clippy::too_many_arguments)]
pub fn warp_plane_simd_rowgather(
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    background: WarpBackground,
    _interp: WarpInterpolation,
) {
    archmage::incant!(
        warp_plane_rowgather_dispatch(src, dst, width, height, m, background),
        [v3, neon, wasm128, scalar]
    );
}

#[archmage::arcane]
fn warp_plane_rowgather_dispatch_v3(
    token: archmage::X64V3Token,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    background: WarpBackground,
) {
    warp_plane_rowgather_v3(token, src, dst, width, height, m, background);
}

fn warp_plane_rowgather_dispatch_neon(
    _token: archmage::NeonToken,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    background: WarpBackground,
) {
    warp_plane_scalar_inner(src, dst, width, height, m, background);
}

fn warp_plane_rowgather_dispatch_wasm128(
    _token: archmage::Wasm128Token,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    background: WarpBackground,
) {
    warp_plane_scalar_inner(src, dst, width, height, m, background);
}

fn warp_plane_rowgather_dispatch_scalar(
    _token: archmage::ScalarToken,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    background: WarpBackground,
) {
    warp_plane_scalar_inner(src, dst, width, height, m, background);
}

/// Row-gather approach: precompute coordinates, sweep source rows.
#[archmage::rite]
fn warp_plane_rowgather_v3(
    token: archmage::X64V3Token,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    background: WarpBackground,
) {
    use magetypes::simd::f32x8;

    let w = width as usize;
    let h = height as usize;
    let wf = width as f32;
    let hf = height as f32;

    let bg_val = match background {
        WarpBackground::Clamp => 0.0f32,
        WarpBackground::Color { l, .. } => l,
    };
    let is_color_bg = matches!(background, WarpBackground::Color { .. });

    let m0 = m[0];
    let m3 = m[3];

    // Pre-allocate workspace for one row of coordinates and weights
    // Aligned to 8 for SIMD
    let w_aligned = (w + 7) & !7;
    let mut ix_buf = vec![0i32; w_aligned];
    let mut iy_buf = vec![0i32; w_aligned];
    let mut fx_buf = vec![0.0f32; w_aligned];
    let mut fy_buf = vec![0.0f32; w_aligned];
    let mut wx_buf = vec![[0.0f32; 4]; w_aligned]; // [pixel][tap]
    let mut wy_buf = vec![[0.0f32; 4]; w_aligned];
    let mut oob_buf = vec![false; w_aligned];

    let zero_v = f32x8::zero(token);
    let one_v = f32x8::splat(token, 1.0);
    let two_v = f32x8::splat(token, 2.0);
    let a3_v = f32x8::splat(token, A3);
    let a2_v = f32x8::splat(token, A2);
    let a0_v = f32x8::splat(token, A0);
    let b3_v = f32x8::splat(token, B3);
    let b2_v = f32x8::splat(token, B2);
    let b1_v = f32x8::splat(token, B1);
    let b0_v = f32x8::splat(token, B0);
    let wf_minus1 = f32x8::splat(token, wf - 1.0);
    let hf_minus1 = f32x8::splat(token, hf - 1.0);

    // Offsets for incremental coordinate computation
    let dx_offsets = f32x8::from_array(
        token,
        [
            0.0 * m0,
            1.0 * m0,
            2.0 * m0,
            3.0 * m0,
            4.0 * m0,
            5.0 * m0,
            6.0 * m0,
            7.0 * m0,
        ],
    );
    let dy_offsets = f32x8::from_array(
        token,
        [
            0.0 * m3,
            1.0 * m3,
            2.0 * m3,
            3.0 * m3,
            4.0 * m3,
            5.0 * m3,
            6.0 * m3,
            7.0 * m3,
        ],
    );
    let step8_sx = f32x8::splat(token, 8.0 * m0);
    let step8_sy = f32x8::splat(token, 8.0 * m3);

    let neg_half = f32x8::splat(token, -0.5);
    let wf_half = f32x8::splat(token, wf - 0.5);
    let hf_half = f32x8::splat(token, hf - 0.5);

    for dy in 0..h {
        let dyf = dy as f32;
        let base_sx = m[1] * dyf + m[2];
        let base_sy = m[4] * dyf + m[5];

        // Step 1: Compute coordinates for the entire row in SIMD
        let mut sx_v = f32x8::splat(token, base_sx) + dx_offsets;
        let mut sy_v = f32x8::splat(token, base_sy) + dy_offsets;

        let mut dx = 0usize;
        while dx + 8 <= w_aligned {
            let batch_end = (dx + 8).min(w);
            let count = batch_end.saturating_sub(dx);

            if is_color_bg {
                // Check OOB
                let oob_x_lo = sx_v.simd_lt(neg_half);
                let oob_x_hi = sx_v.simd_ge(wf_half);
                let oob_y_lo = sy_v.simd_lt(neg_half);
                let oob_y_hi = sy_v.simd_ge(hf_half);
                let oob_any = f32x8::blend(
                    oob_x_lo,
                    one_v,
                    f32x8::blend(
                        oob_x_hi,
                        one_v,
                        f32x8::blend(oob_y_lo, one_v, f32x8::blend(oob_y_hi, one_v, zero_v)),
                    ),
                );
                let oob_arr = oob_any.to_array();
                for p in 0..count {
                    oob_buf[dx + p] = oob_arr[p] > 0.0;
                }
            }

            // Clamp and compute floor/frac
            let sx_c = sx_v.clamp(zero_v, wf_minus1);
            let sy_c = sy_v.clamp(zero_v, hf_minus1);
            let sx_fl = sx_c.floor();
            let sy_fl = sy_c.floor();
            let fx_v = sx_c - sx_fl;
            let fy_v = sy_c - sy_fl;

            let ix = sx_fl.to_i32x8().to_array();
            let iy = sy_fl.to_i32x8().to_array();
            let fx_a = fx_v.to_array();
            let fy_a = fy_v.to_array();

            for p in 0..count {
                ix_buf[dx + p] = ix[p];
                iy_buf[dx + p] = iy[p];
                fx_buf[dx + p] = fx_a[p];
                fy_buf[dx + p] = fy_a[p];
            }

            // Compute kernel weights in SIMD
            for tap in 0..4u32 {
                let offset_f = f32x8::splat(token, (tap as i32 - 1) as f32);
                let tx = (offset_f - fx_v).abs();
                let ty = (offset_f - fy_v).abs();

                let inner_x = tx.mul_add(a3_v, a2_v).mul_add(tx, zero_v).mul_add(tx, a0_v);
                let outer_x = tx.mul_add(b3_v, b2_v).mul_add(tx, b1_v).mul_add(tx, b0_v);
                let is_inner_x = tx.simd_lt(one_v);
                let is_valid_x = tx.simd_lt(two_v);
                let kx = f32x8::blend(
                    is_inner_x,
                    inner_x,
                    f32x8::blend(is_valid_x, outer_x, zero_v),
                );

                let inner_y = ty.mul_add(a3_v, a2_v).mul_add(ty, zero_v).mul_add(ty, a0_v);
                let outer_y = ty.mul_add(b3_v, b2_v).mul_add(ty, b1_v).mul_add(ty, b0_v);
                let is_inner_y = ty.simd_lt(one_v);
                let is_valid_y = ty.simd_lt(two_v);
                let ky = f32x8::blend(
                    is_inner_y,
                    inner_y,
                    f32x8::blend(is_valid_y, outer_y, zero_v),
                );

                let kx_a = kx.to_array();
                let ky_a = ky.to_array();
                for p in 0..count {
                    wx_buf[dx + p][tap as usize] = kx_a[p];
                    wy_buf[dx + p][tap as usize] = ky_a[p];
                }
            }

            sx_v = sx_v + step8_sx;
            sy_v = sy_v + step8_sy;
            dx += 8;
        }

        // Normalize weights
        for px in 0..w {
            let mut sx_sum = 0.0f32;
            let mut sy_sum = 0.0f32;
            for tap in 0..4 {
                sx_sum += wx_buf[px][tap];
                sy_sum += wy_buf[px][tap];
            }
            let inv_x = if sx_sum.abs() > 1e-10 {
                1.0 / sx_sum
            } else {
                1.0
            };
            let inv_y = if sy_sum.abs() > 1e-10 {
                1.0 / sy_sum
            } else {
                1.0
            };
            for tap in 0..4 {
                wx_buf[px][tap] *= inv_x;
                wy_buf[px][tap] *= inv_y;
            }
        }

        // Step 2: Gather and accumulate per row
        let row_start = dy * w;
        let w_i32 = width as i32;
        let h_i32 = height as i32;

        // Process 8 output pixels at a time
        dx = 0;
        while dx + 8 <= w {
            // Compute accumulated result for 8 pixels
            let mut accum = f32x8::zero(token);

            for j in 0..4 {
                let wy_j_arr: [f32; 8] = core::array::from_fn(|p| wy_buf[dx + p][j]);
                let wy_j = f32x8::from_array(token, wy_j_arr);

                let mut row_sum = f32x8::zero(token);
                for i in 0..4 {
                    let wx_i_arr: [f32; 8] = core::array::from_fn(|p| wx_buf[dx + p][i]);
                    let wx_i = f32x8::from_array(token, wx_i_arr);

                    // Scalar gather
                    let gathered: [f32; 8] = core::array::from_fn(|p| {
                        let gx = (ix_buf[dx + p] + i as i32 - 1).clamp(0, w_i32 - 1) as usize;
                        let gy = (iy_buf[dx + p] + j as i32 - 1).clamp(0, h_i32 - 1) as usize;
                        src[gy * w + gx]
                    });

                    let src_v = f32x8::from_array(token, gathered);
                    row_sum = src_v.mul_add(wx_i, row_sum);
                }

                accum = row_sum.mul_add(wy_j, accum);
            }

            // Handle OOB for color fill
            if is_color_bg {
                let bg_v = f32x8::splat(token, bg_val);
                let oob_mask: [f32; 8] = core::array::from_fn(|p| {
                    if oob_buf[dx + p] {
                        f32::from_bits(0xFFFF_FFFF)
                    } else {
                        0.0
                    }
                });
                let mask = f32x8::from_array(token, oob_mask);
                let is_oob = mask.simd_ne(zero_v);
                accum = f32x8::blend(is_oob, bg_v, accum);
            }

            let chunk: &mut [f32; 8] = (&mut dst[row_start + dx..row_start + dx + 8])
                .try_into()
                .unwrap();
            accum.store(chunk);
            dx += 8;
        }

        // Scalar tail
        while dx < w {
            let out_idx = row_start + dx;
            if is_color_bg && oob_buf[dx] {
                dst[out_idx] = bg_val;
            } else {
                let mut sum = 0.0f32;
                for j in 0..4 {
                    let mut row_sum = 0.0f32;
                    for i in 0..4 {
                        let gx = (ix_buf[dx] + i as i32 - 1).clamp(0, w_i32 - 1) as usize;
                        let gy = (iy_buf[dx] + j as i32 - 1).clamp(0, h_i32 - 1) as usize;
                        row_sum += src[gy * w + gx] * wx_buf[dx][i as usize];
                    }
                    sum += row_sum * wy_buf[dx][j as usize];
                }
                dst[out_idx] = sum;
            }
            dx += 1;
        }
    }
}

// ─── Full multi-plane warp API ──────────────────────────────────────

/// Full SIMD warp of all Oklab planes using Approach A (planar SIMD).
///
/// Call this instead of the scalar `Warp::apply()` for SIMD-accelerated rotation.
/// Pass the per-plane background value for Color mode.
#[allow(clippy::too_many_arguments)]
pub fn warp_all_planes_simd(
    src_l: &[f32],
    src_a: &[f32],
    src_b: &[f32],
    src_alpha: Option<&[f32]>,
    dst_l: &mut [f32],
    dst_a: &mut [f32],
    dst_b: &mut [f32],
    dst_alpha: Option<&mut [f32]>,
    width: u32,
    height: u32,
    m: &[f32; 9],
    background: WarpBackground,
    interp: WarpInterpolation,
) {
    // Per-plane background values
    let (bg_l, bg_a, bg_b) = match background {
        WarpBackground::Clamp => (0.0, 0.0, 0.0),
        WarpBackground::Color { l, a, b, .. } => (l, a, b),
    };

    let mk_bg = |val: f32| -> WarpBackground {
        match background {
            WarpBackground::Clamp => WarpBackground::Clamp,
            WarpBackground::Color { .. } => WarpBackground::Color {
                l: val,
                a: 0.0,
                b: 0.0,
                alpha: 0.0,
            },
        }
    };

    warp_plane_simd_planar(src_l, dst_l, width, height, m, mk_bg(bg_l), interp);
    warp_plane_simd_planar(src_a, dst_a, width, height, m, mk_bg(bg_a), interp);
    warp_plane_simd_planar(src_b, dst_b, width, height, m, mk_bg(bg_b), interp);

    if let (Some(sa), Some(da)) = (src_alpha, dst_alpha) {
        let alpha_bg = match background {
            WarpBackground::Clamp => WarpBackground::Clamp,
            WarpBackground::Color { alpha, .. } => WarpBackground::Color {
                l: alpha,
                a: 0.0,
                b: 0.0,
                alpha: 0.0,
            },
        };
        warp_plane_simd_planar(sa, da, width, height, m, alpha_bg, interp);
    }
}

/// Full SIMD warp using Approach B (row-gather).
#[allow(clippy::too_many_arguments)]
pub fn warp_all_planes_rowgather(
    src_l: &[f32],
    src_a: &[f32],
    src_b: &[f32],
    src_alpha: Option<&[f32]>,
    dst_l: &mut [f32],
    dst_a: &mut [f32],
    dst_b: &mut [f32],
    dst_alpha: Option<&mut [f32]>,
    width: u32,
    height: u32,
    m: &[f32; 9],
    background: WarpBackground,
    interp: WarpInterpolation,
) {
    let (bg_l, bg_a, bg_b) = match background {
        WarpBackground::Clamp => (0.0, 0.0, 0.0),
        WarpBackground::Color { l, a, b, .. } => (l, a, b),
    };

    let mk_bg = |val: f32| -> WarpBackground {
        match background {
            WarpBackground::Clamp => WarpBackground::Clamp,
            WarpBackground::Color { .. } => WarpBackground::Color {
                l: val,
                a: 0.0,
                b: 0.0,
                alpha: 0.0,
            },
        }
    };

    warp_plane_simd_rowgather(src_l, dst_l, width, height, m, mk_bg(bg_l), interp);
    warp_plane_simd_rowgather(src_a, dst_a, width, height, m, mk_bg(bg_a), interp);
    warp_plane_simd_rowgather(src_b, dst_b, width, height, m, mk_bg(bg_b), interp);

    if let (Some(sa), Some(da)) = (src_alpha, dst_alpha) {
        let alpha_bg = match background {
            WarpBackground::Clamp => WarpBackground::Clamp,
            WarpBackground::Color { alpha, .. } => WarpBackground::Color {
                l: alpha,
                a: 0.0,
                b: 0.0,
                alpha: 0.0,
            },
        };
        warp_plane_simd_rowgather(sa, da, width, height, m, alpha_bg, interp);
    }
}

// ─── Approach C: Fused 3-plane warp ─────────────────────────────────
//
// Process all 3 planes in a single pass. Coordinate transform and kernel
// weight computation happen once, then the same addresses gather from
// L, a, and b planes. This amortizes the most expensive part (address
// computation + clamp + index) across all 3 planes.

/// Fused 3-plane warp: computes source addresses once, gathers from all planes.
///
/// Expected to be ~2x faster than separate per-plane warps for 3-plane
/// workloads because the address computation dominates the cost.
#[allow(clippy::too_many_arguments)]
pub fn warp_3plane_fused(
    src_l: &[f32],
    src_a: &[f32],
    src_b: &[f32],
    dst_l: &mut [f32],
    dst_a: &mut [f32],
    dst_b: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    bg_l: f32,
    bg_a: f32,
    bg_b: f32,
    is_color_bg: bool,
) {
    let w = width as usize;
    let h = height as usize;
    let wf = width as f32;
    let hf = height as f32;
    let w_i32 = width as i32;
    let h_i32 = height as i32;
    let m0 = m[0];
    let m3 = m[3];

    for dy in 0..h {
        let dyf = dy as f32;
        let mut sx = m[1] * dyf + m[2];
        let mut sy = m[4] * dyf + m[5];
        let row_start = dy * w;

        for dx in 0..w {
            let out_idx = row_start + dx;

            // OOB check
            if is_color_bg && (sx < -0.5 || sx >= wf - 0.5 || sy < -0.5 || sy >= hf - 0.5) {
                dst_l[out_idx] = bg_l;
                dst_a[out_idx] = bg_a;
                dst_b[out_idx] = bg_b;
                sx += m0;
                sy += m3;
                continue;
            }

            let sx_c = sx.clamp(0.0, wf - 1.0);
            let sy_c = sy.clamp(0.0, hf - 1.0);

            let ix = sx_c.floor() as i32;
            let iy = sy_c.floor() as i32;
            let fx = sx_c - ix as f32;
            let fy = sy_c - iy as f32;

            // Kernel weights (computed once)
            let mut wx = [0.0f32; 4];
            let mut wy = [0.0f32; 4];
            let mut wx_sum = 0.0f32;
            let mut wy_sum = 0.0f32;
            for i in 0..4 {
                let o = i as i32 - 1;
                let wt_x = robidoux_scalar(o as f32 - fx);
                let wt_y = robidoux_scalar(o as f32 - fy);
                wx[i] = wt_x;
                wy[i] = wt_y;
                wx_sum += wt_x;
                wy_sum += wt_y;
            }
            let inv_wx = if wx_sum.abs() > 1e-10 {
                1.0 / wx_sum
            } else {
                1.0
            };
            let inv_wy = if wy_sum.abs() > 1e-10 {
                1.0 / wy_sum
            } else {
                1.0
            };
            for i in 0..4 {
                wx[i] *= inv_wx;
                wy[i] *= inv_wy;
            }

            // Precompute row indices (shared across planes)
            let gy: [usize; 4] =
                core::array::from_fn(|j| (iy + j as i32 - 1).clamp(0, h_i32 - 1) as usize);
            let gx: [usize; 4] =
                core::array::from_fn(|i| (ix + i as i32 - 1).clamp(0, w_i32 - 1) as usize);

            // Precompute linear indices (shared across all 3 planes)
            // 4 rows x 4 cols = 16 indices
            let idx: [[usize; 4]; 4] = core::array::from_fn(|j| {
                let base = gy[j] * w;
                core::array::from_fn(|i| base + gx[i])
            });

            // Gather and accumulate all 3 planes using the same indices
            let mut sum_l = 0.0f32;
            let mut sum_a = 0.0f32;
            let mut sum_b = 0.0f32;
            for j in 0..4 {
                let mut rl = 0.0f32;
                let mut ra = 0.0f32;
                let mut rb = 0.0f32;
                for i in 0..4 {
                    let idx_ji = idx[j][i];
                    rl += src_l[idx_ji] * wx[i];
                    ra += src_a[idx_ji] * wx[i];
                    rb += src_b[idx_ji] * wx[i];
                }
                sum_l += rl * wy[j];
                sum_a += ra * wy[j];
                sum_b += rb * wy[j];
            }

            dst_l[out_idx] = sum_l;
            dst_a[out_idx] = sum_a;
            dst_b[out_idx] = sum_b;

            sx += m0;
            sy += m3;
        }
    }
}

// ─── Approach D: Fused 3-plane SIMD warp ────────────────────────────
//
// Combines the SIMD kernel evaluation from Approach A with the address
// amortization from Approach C. For each batch of 8 output pixels:
// 1. Compute source coordinates + kernel weights in SIMD (f32x8)
// 2. Extract addresses once
// 3. Gather from all 3 planes at the same addresses
// 4. Accumulate all 3 planes with the same weights

/// Fused 3-plane SIMD warp: SIMD kernel + shared gather addresses.
#[allow(clippy::too_many_arguments)]
pub fn warp_3plane_fused_simd(
    src_l: &[f32],
    src_a: &[f32],
    src_b: &[f32],
    dst_l: &mut [f32],
    dst_a: &mut [f32],
    dst_b: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    bg_l: f32,
    bg_a: f32,
    bg_b: f32,
    is_color_bg: bool,
) {
    archmage::incant!(
        warp_3plane_fused_simd_dispatch(
            src_l,
            src_a,
            src_b,
            dst_l,
            dst_a,
            dst_b,
            width,
            height,
            m,
            bg_l,
            bg_a,
            bg_b,
            is_color_bg
        ),
        [v3, neon, wasm128, scalar]
    );
}

#[archmage::arcane]
#[allow(clippy::too_many_arguments)]
fn warp_3plane_fused_simd_dispatch_v3(
    token: archmage::X64V3Token,
    src_l: &[f32],
    src_a: &[f32],
    src_b: &[f32],
    dst_l: &mut [f32],
    dst_a: &mut [f32],
    dst_b: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    bg_l: f32,
    bg_a: f32,
    bg_b: f32,
    is_color_bg: bool,
) {
    warp_3plane_fused_inner_v3(
        token,
        src_l,
        src_a,
        src_b,
        dst_l,
        dst_a,
        dst_b,
        width,
        height,
        m,
        bg_l,
        bg_a,
        bg_b,
        is_color_bg,
    );
}

#[archmage::arcane]
#[allow(clippy::too_many_arguments)]
fn warp_3plane_fused_simd_dispatch_neon(
    token: archmage::NeonToken,
    src_l: &[f32],
    src_a: &[f32],
    src_b: &[f32],
    dst_l: &mut [f32],
    dst_a: &mut [f32],
    dst_b: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    bg_l: f32,
    bg_a: f32,
    bg_b: f32,
    is_color_bg: bool,
) {
    warp_3plane_fused_inner_neon(
        token,
        src_l,
        src_a,
        src_b,
        dst_l,
        dst_a,
        dst_b,
        width,
        height,
        m,
        bg_l,
        bg_a,
        bg_b,
        is_color_bg,
    );
}

#[archmage::arcane]
#[allow(clippy::too_many_arguments)]
fn warp_3plane_fused_simd_dispatch_wasm128(
    token: archmage::Wasm128Token,
    src_l: &[f32],
    src_a: &[f32],
    src_b: &[f32],
    dst_l: &mut [f32],
    dst_a: &mut [f32],
    dst_b: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    bg_l: f32,
    bg_a: f32,
    bg_b: f32,
    is_color_bg: bool,
) {
    warp_3plane_fused_inner_wasm128(
        token,
        src_l,
        src_a,
        src_b,
        dst_l,
        dst_a,
        dst_b,
        width,
        height,
        m,
        bg_l,
        bg_a,
        bg_b,
        is_color_bg,
    );
}

#[allow(clippy::too_many_arguments)]
fn warp_3plane_fused_simd_dispatch_scalar(
    _token: archmage::ScalarToken,
    src_l: &[f32],
    src_a: &[f32],
    src_b: &[f32],
    dst_l: &mut [f32],
    dst_a: &mut [f32],
    dst_b: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    bg_l: f32,
    bg_a: f32,
    bg_b: f32,
    is_color_bg: bool,
) {
    warp_3plane_fused(
        src_l,
        src_a,
        src_b,
        dst_l,
        dst_a,
        dst_b,
        width,
        height,
        m,
        bg_l,
        bg_a,
        bg_b,
        is_color_bg,
    );
}

/// Fused 3-plane warp inner loop — multi-arch via GenericF32x8.
///
/// `#[magetypes(neon, wasm128)]` generates `_neon` and `_wasm128` variants.
/// The v3 (AVX2) dispatch calls this too since GenericF32x8<X64V3Token> = native f32x8.
#[magetypes(v3, neon, wasm128)]
#[allow(clippy::too_many_arguments)]
fn warp_3plane_fused_inner(
    token: Token,
    src_l: &[f32],
    src_a: &[f32],
    src_b: &[f32],
    dst_l: &mut [f32],
    dst_a: &mut [f32],
    dst_b: &mut [f32],
    width: u32,
    height: u32,
    m: &[f32; 9],
    bg_l: f32,
    bg_a: f32,
    bg_b: f32,
    is_color_bg: bool,
) {
    #[allow(non_camel_case_types)]
    type f32x8 = GenericF32x8<Token>;

    let w = width as usize;
    let h = height as usize;
    let wf = width as f32;
    let hf = height as f32;
    let w_i32 = width as i32;
    let h_i32 = height as i32;
    let m0 = m[0];
    let m3 = m[3];

    let zero_v = f32x8::zero(token);
    let one_v = f32x8::splat(token, 1.0);
    let two_v = f32x8::splat(token, 2.0);
    let neg1_v = f32x8::splat(token, -1.0);
    let pos2_v = f32x8::splat(token, 2.0);
    let wf_minus1 = f32x8::splat(token, wf - 1.0);
    let hf_minus1 = f32x8::splat(token, hf - 1.0);
    let a3_v = f32x8::splat(token, A3);
    let a2_v = f32x8::splat(token, A2);
    let a0_v = f32x8::splat(token, A0);
    let b3_v = f32x8::splat(token, B3);
    let b2_v = f32x8::splat(token, B2);
    let b1_v = f32x8::splat(token, B1);
    let b0_v = f32x8::splat(token, B0);
    let neg_half = f32x8::splat(token, -0.5);
    let wf_half = f32x8::splat(token, wf - 0.5);
    let hf_half = f32x8::splat(token, hf - 0.5);
    let bg_l_v = f32x8::splat(token, bg_l);
    let bg_a_v = f32x8::splat(token, bg_a);
    let bg_b_v = f32x8::splat(token, bg_b);

    let dx_offsets = f32x8::from_array(
        token,
        [
            0.0 * m0,
            1.0 * m0,
            2.0 * m0,
            3.0 * m0,
            4.0 * m0,
            5.0 * m0,
            6.0 * m0,
            7.0 * m0,
        ],
    );
    let dy_offsets = f32x8::from_array(
        token,
        [
            0.0 * m3,
            1.0 * m3,
            2.0 * m3,
            3.0 * m3,
            4.0 * m3,
            5.0 * m3,
            6.0 * m3,
            7.0 * m3,
        ],
    );
    let step8_sx = f32x8::splat(token, 8.0 * m0);
    let step8_sy = f32x8::splat(token, 8.0 * m3);

    // Robidoux kernel in SIMD — inline macro to avoid inner fn type issues
    // with #[magetypes] token replacement.
    macro_rules! robidoux_v {
        ($t:expr) => {{
            let t = $t;
            let inner = t.mul_add(a3_v, a2_v).mul_add(t, zero_v).mul_add(t, a0_v);
            let outer = t.mul_add(b3_v, b2_v).mul_add(t, b1_v).mul_add(t, b0_v);
            let is_inner = t.simd_lt(one_v);
            let is_valid = t.simd_lt(two_v);
            f32x8::blend(is_inner, inner, f32x8::blend(is_valid, outer, zero_v))
        }};
    }

    for dy in 0..h {
        let dyf = dy as f32;
        let base_sx = m[1] * dyf + m[2];
        let base_sy = m[4] * dyf + m[5];
        let mut sx_v = f32x8::splat(token, base_sx) + dx_offsets;
        let mut sy_v = f32x8::splat(token, base_sy) + dy_offsets;
        let row_start = dy * w;
        let mut dx = 0usize;

        while dx + 8 <= w {
            let out_base = row_start + dx;

            // OOB fast fill
            if is_color_bg {
                let oob_any = f32x8::blend(
                    sx_v.simd_lt(neg_half),
                    one_v,
                    f32x8::blend(
                        sx_v.simd_ge(wf_half),
                        one_v,
                        f32x8::blend(
                            sy_v.simd_lt(neg_half),
                            one_v,
                            f32x8::blend(sy_v.simd_ge(hf_half), one_v, zero_v),
                        ),
                    ),
                );
                if oob_any.reduce_add() >= 8.0 {
                    let cl: &mut [f32; 8] =
                        (&mut dst_l[out_base..out_base + 8]).try_into().unwrap();
                    let ca: &mut [f32; 8] =
                        (&mut dst_a[out_base..out_base + 8]).try_into().unwrap();
                    let cb: &mut [f32; 8] =
                        (&mut dst_b[out_base..out_base + 8]).try_into().unwrap();
                    bg_l_v.store(cl);
                    bg_a_v.store(ca);
                    bg_b_v.store(cb);
                    sx_v = sx_v + step8_sx;
                    sy_v = sy_v + step8_sy;
                    dx += 8;
                    continue;
                }
            }

            // Clamp + floor + frac
            let sx_c = sx_v.clamp(zero_v, wf_minus1);
            let sy_c = sy_v.clamp(zero_v, hf_minus1);
            let sx_fl = sx_c.floor();
            let sy_fl = sy_c.floor();
            let fx = sx_c - sx_fl;
            let fy = sy_c - sy_fl;
            let ix_arr = sx_fl.to_i32x8().to_array();
            let iy_arr = sy_fl.to_i32x8().to_array();

            // Kernel weights (SIMD)
            let wx0 = robidoux_v!((neg1_v - fx).abs());
            let wx1 = robidoux_v!(fx);
            let wx2 = robidoux_v!((one_v - fx).abs());
            let wx3 = robidoux_v!((pos2_v - fx).abs());

            let wy0 = robidoux_v!((neg1_v - fy).abs());
            let wy1 = robidoux_v!(fy);
            let wy2 = robidoux_v!((one_v - fy).abs());
            let wy3 = robidoux_v!((pos2_v - fy).abs());

            // Normalize
            let inv_wx = ((wx0 + wx1) + (wx2 + wx3)).recip();
            let inv_wy = ((wy0 + wy1) + (wy2 + wy3)).recip();
            let wx0 = wx0 * inv_wx;
            let wx1 = wx1 * inv_wx;
            let wx2 = wx2 * inv_wx;
            let wx3 = wx3 * inv_wx;
            let wy0 = wy0 * inv_wy;
            let wy1 = wy1 * inv_wy;
            let wy2 = wy2 * inv_wy;
            let wy3 = wy3 * inv_wy;

            // Extract weight arrays for per-pixel use
            let wx = [
                wx0.to_array(),
                wx1.to_array(),
                wx2.to_array(),
                wx3.to_array(),
            ];
            let wy = [
                wy0.to_array(),
                wy1.to_array(),
                wy2.to_array(),
                wy3.to_array(),
            ];

            // Process all 8 pixels, all 3 planes
            let mut out_l = [0.0f32; 8];
            let mut out_a = [0.0f32; 8];
            let mut out_b = [0.0f32; 8];

            for p in 0..8 {
                let ix = ix_arr[p];
                let iy = iy_arr[p];

                let mut sl = 0.0f32;
                let mut sa = 0.0f32;
                let mut sb = 0.0f32;

                for j in 0..4 {
                    let gy = (iy + j as i32 - 1).clamp(0, h_i32 - 1) as usize;
                    let base = gy * w;
                    let mut rl = 0.0f32;
                    let mut ra = 0.0f32;
                    let mut rb = 0.0f32;
                    for i in 0..4 {
                        let gx = (ix + i as i32 - 1).clamp(0, w_i32 - 1) as usize;
                        let idx = base + gx;
                        let wxi = wx[i][p];
                        rl += src_l[idx] * wxi;
                        ra += src_a[idx] * wxi;
                        rb += src_b[idx] * wxi;
                    }
                    let wyj = wy[j][p];
                    sl += rl * wyj;
                    sa += ra * wyj;
                    sb += rb * wyj;
                }

                out_l[p] = sl;
                out_a[p] = sa;
                out_b[p] = sb;
            }

            // Store results
            let mut res_l = f32x8::from_array(token, out_l);
            let mut res_a = f32x8::from_array(token, out_a);
            let mut res_b = f32x8::from_array(token, out_b);

            // OOB masking
            if is_color_bg {
                let oob = f32x8::blend(
                    sx_v.simd_lt(neg_half),
                    one_v,
                    f32x8::blend(
                        sx_v.simd_ge(wf_half),
                        one_v,
                        f32x8::blend(
                            sy_v.simd_lt(neg_half),
                            one_v,
                            f32x8::blend(sy_v.simd_ge(hf_half), one_v, zero_v),
                        ),
                    ),
                );
                let is_oob = oob.simd_gt(zero_v);
                res_l = f32x8::blend(is_oob, bg_l_v, res_l);
                res_a = f32x8::blend(is_oob, bg_a_v, res_a);
                res_b = f32x8::blend(is_oob, bg_b_v, res_b);
            }

            let cl: &mut [f32; 8] = (&mut dst_l[out_base..out_base + 8]).try_into().unwrap();
            let ca: &mut [f32; 8] = (&mut dst_a[out_base..out_base + 8]).try_into().unwrap();
            let cb: &mut [f32; 8] = (&mut dst_b[out_base..out_base + 8]).try_into().unwrap();
            res_l.store(cl);
            res_a.store(ca);
            res_b.store(cb);

            sx_v = sx_v + step8_sx;
            sy_v = sy_v + step8_sy;
            dx += 8;
        }

        // Scalar tail
        while dx < w {
            let out_idx = row_start + dx;
            let sx = m0 * dx as f32 + m[1] * dyf + m[2];
            let sy = m3 * dx as f32 + m[4] * dyf + m[5];

            if is_color_bg && (sx < -0.5 || sx >= wf - 0.5 || sy < -0.5 || sy >= hf - 0.5) {
                dst_l[out_idx] = bg_l;
                dst_a[out_idx] = bg_a;
                dst_b[out_idx] = bg_b;
            } else {
                let sx_c = sx.clamp(0.0, wf - 1.0);
                let sy_c = sy.clamp(0.0, hf - 1.0);
                let ix = sx_c.floor() as i32;
                let iy = sy_c.floor() as i32;
                let f_x = sx_c - ix as f32;
                let f_y = sy_c - iy as f32;

                let mut wxs = [0.0f32; 4];
                let mut wys = [0.0f32; 4];
                let mut wxs_sum = 0.0f32;
                let mut wys_sum = 0.0f32;
                for i in 0..4 {
                    let o = i as i32 - 1;
                    let wt_x = robidoux_scalar(o as f32 - f_x);
                    let wt_y = robidoux_scalar(o as f32 - f_y);
                    wxs[i] = wt_x;
                    wys[i] = wt_y;
                    wxs_sum += wt_x;
                    wys_sum += wt_y;
                }
                let inv_x = if wxs_sum.abs() > 1e-10 {
                    1.0 / wxs_sum
                } else {
                    1.0
                };
                let inv_y = if wys_sum.abs() > 1e-10 {
                    1.0 / wys_sum
                } else {
                    1.0
                };
                for i in 0..4 {
                    wxs[i] *= inv_x;
                    wys[i] *= inv_y;
                }

                let mut sl = 0.0f32;
                let mut sa = 0.0f32;
                let mut sb = 0.0f32;
                for j in 0..4 {
                    let gy = (iy + j as i32 - 1).clamp(0, h_i32 - 1) as usize;
                    let base = gy * w;
                    let mut rl = 0.0f32;
                    let mut ra = 0.0f32;
                    let mut rb = 0.0f32;
                    for i in 0..4 {
                        let gx = (ix + i as i32 - 1).clamp(0, w_i32 - 1) as usize;
                        let idx = base + gx;
                        rl += src_l[idx] * wxs[i];
                        ra += src_a[idx] * wxs[i];
                        rb += src_b[idx] * wxs[i];
                    }
                    sl += rl * wys[j];
                    sa += ra * wys[j];
                    sb += rb * wys[j];
                }
                dst_l[out_idx] = sl;
                dst_a[out_idx] = sa;
                dst_b[out_idx] = sb;
            }
            dx += 1;
        }
    }
}

// ─── High-level entry point for Warp::apply ───────────────────────

/// Warp all planes (L/a/b + optional alpha) using the fused SIMD path.
///
/// Uses `warp_3plane_fused_simd` for L/a/b (shared gather addresses,
/// 222 Mpix/s at 1080p), then a separate `warp_plane_simd_planar` pass
/// for alpha if present.
#[allow(clippy::too_many_arguments)]
pub fn warp_planes_fused(
    src_l: &[f32],
    src_a: &[f32],
    src_b: &[f32],
    src_alpha: Option<&[f32]>,
    dst_l: &mut [f32],
    dst_a: &mut [f32],
    dst_b: &mut [f32],
    dst_alpha: Option<&mut [f32]>,
    width: u32,
    height: u32,
    m: &[f32; 9],
    background: WarpBackground,
) {
    let (bg_l, bg_a, bg_b, bg_alpha) = match background {
        WarpBackground::Clamp => (0.0, 0.0, 0.0, 1.0),
        WarpBackground::Color { l, a, b, alpha } => (l, a, b, alpha),
    };
    let is_color_bg = matches!(background, WarpBackground::Color { .. });

    // Fused 3-plane SIMD for L/a/b
    warp_3plane_fused_simd(
        src_l,
        src_a,
        src_b,
        dst_l,
        dst_a,
        dst_b,
        width,
        height,
        m,
        bg_l,
        bg_a,
        bg_b,
        is_color_bg,
    );

    // Separate pass for alpha (still SIMD, just not fused)
    if let (Some(sa), Some(da)) = (src_alpha, dst_alpha) {
        let alpha_bg = if is_color_bg {
            WarpBackground::Color {
                l: bg_alpha,
                a: 0.0,
                b: 0.0,
                alpha: 0.0,
            }
        } else {
            WarpBackground::Clamp
        };
        warp_plane_simd_planar(
            sa,
            da,
            width,
            height,
            m,
            alpha_bg,
            WarpInterpolation::Robidoux,
        );
    }
}

// ─── Dead end: interleaved RGBA u8 warp ────────────────────────────
//
// We prototyped and benchmarked interleaved RGBA u8 warp (scalar and
// SIMD) as an alternative to the planar f32 Oklab path. The hypothesis
// was that skipping Oklab conversion would be faster for standalone
// rotation. Results at 1080p, 5° Robidoux:
//
//   Fused 3-plane f32 SIMD:  28ms  222 Mpix/s  (planar, Oklab)
//   RGBA u8 SIMD:            64ms   32 Mpix/s  (interleaved, sRGB)
//   RGBA u8 scalar:          99ms   21 Mpix/s  (interleaved, sRGB)
//
// The interleaved u8 path is 2.3× slower than planar f32 SIMD even
// WITHOUT counting Oklab conversion overhead. Root cause: 4-byte-
// strided u8 gathers are catastrophically cache-hostile compared to
// contiguous f32 plane access. Each 4×4 kernel tap reads 4 scattered
// bytes from an interleaved buffer vs 1 sequential f32 from a plane.
//
// The Oklab conversion cost (~5ms) is dwarfed by the gather penalty
// (~36ms). Planar f32 SIMD wins on every metric.
//
// This code was removed. See git history (commits ddc26a9..502b014)
// for the full implementation if revisiting.

// ─── Integration: wiring SIMD into Filter::apply ──────────────────
//
// TODO: Replace the scalar gather loop in Warp::apply with
// warp_3plane_fused_simd (+ alpha as 4th channel in the fused loop).
// The fused path already shares gather indices across L/a/b — adding
// alpha is one more accumulator at near-zero cost.

// (interleaved RGBA u8 code removed — see dead-end note above)

#[cfg(test)]
mod tests {
    use super::*;

    fn make_gradient_plane(w: u32, h: u32) -> Vec<f32> {
        let mut plane = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                plane.push((x as f32 + y as f32 * 0.1) / w as f32);
            }
        }
        plane
    }

    /// Verify that the SIMD planar warp matches the scalar reference for a 5deg rotation.
    #[test]
    fn simd_planar_matches_scalar() {
        let w = 64u32;
        let h = 64u32;
        let n = (w * h) as usize;
        let src = make_gradient_plane(w, h);

        // Rotation matrix for 5 degrees
        let angle_deg = 5.0f32;
        let angle_rad = angle_deg * core::f32::consts::PI / 180.0;
        let cos_a = angle_rad.cos();
        let sin_a = angle_rad.sin();
        let cx = (w as f32 - 1.0) * 0.5;
        let cy = (h as f32 - 1.0) * 0.5;
        let m = [
            cos_a,
            sin_a,
            cx - cx * cos_a - cy * sin_a,
            -sin_a,
            cos_a,
            cy + cx * sin_a - cy * cos_a,
            0.0,
            0.0,
            1.0,
        ];

        let mut dst_scalar = vec![0.0f32; n];
        let mut dst_simd = vec![0.0f32; n];

        warp_plane_scalar_inner(&src, &mut dst_scalar, w, h, &m, WarpBackground::Clamp);
        warp_plane_simd_planar(
            &src,
            &mut dst_simd,
            w,
            h,
            &m,
            WarpBackground::Clamp,
            WarpInterpolation::Robidoux,
        );

        let mut max_diff = 0.0f32;
        for i in 0..n {
            let diff = (dst_scalar[i] - dst_simd[i]).abs();
            max_diff = max_diff.max(diff);
        }

        // Allow small floating-point differences from mul_add vs separate mul+add
        assert!(
            max_diff < 1e-4,
            "SIMD planar vs scalar max diff: {max_diff} (should be < 1e-4)"
        );
    }

    /// Verify that the row-gather approach matches the scalar reference.
    #[test]
    fn rowgather_matches_scalar() {
        let w = 64u32;
        let h = 64u32;
        let n = (w * h) as usize;
        let src = make_gradient_plane(w, h);

        let angle_deg = 5.0f32;
        let angle_rad = angle_deg * core::f32::consts::PI / 180.0;
        let cos_a = angle_rad.cos();
        let sin_a = angle_rad.sin();
        let cx = (w as f32 - 1.0) * 0.5;
        let cy = (h as f32 - 1.0) * 0.5;
        let m = [
            cos_a,
            sin_a,
            cx - cx * cos_a - cy * sin_a,
            -sin_a,
            cos_a,
            cy + cx * sin_a - cy * cos_a,
            0.0,
            0.0,
            1.0,
        ];

        let mut dst_scalar = vec![0.0f32; n];
        let mut dst_rowgather = vec![0.0f32; n];

        warp_plane_scalar_inner(&src, &mut dst_scalar, w, h, &m, WarpBackground::Clamp);
        warp_plane_simd_rowgather(
            &src,
            &mut dst_rowgather,
            w,
            h,
            &m,
            WarpBackground::Clamp,
            WarpInterpolation::Robidoux,
        );

        let mut max_diff = 0.0f32;
        for i in 0..n {
            let diff = (dst_scalar[i] - dst_rowgather[i]).abs();
            max_diff = max_diff.max(diff);
        }

        assert!(
            max_diff < 1e-4,
            "Row-gather vs scalar max diff: {max_diff} (should be < 1e-4)"
        );
    }

    /// Test with color background mode.
    #[test]
    fn simd_color_background() {
        let w = 32u32;
        let h = 32u32;
        let n = (w * h) as usize;
        let src = make_gradient_plane(w, h);

        // Large rotation to get OOB pixels
        let angle_deg = 30.0f32;
        let angle_rad = angle_deg * core::f32::consts::PI / 180.0;
        let cos_a = angle_rad.cos();
        let sin_a = angle_rad.sin();
        let cx = (w as f32 - 1.0) * 0.5;
        let cy = (h as f32 - 1.0) * 0.5;
        let m = [
            cos_a,
            sin_a,
            cx - cx * cos_a - cy * sin_a,
            -sin_a,
            cos_a,
            cy + cx * sin_a - cy * cos_a,
            0.0,
            0.0,
            1.0,
        ];

        let bg = WarpBackground::Color {
            l: 0.5,
            a: 0.0,
            b: 0.0,
            alpha: 1.0,
        };

        let mut dst_scalar = vec![0.0f32; n];
        let mut dst_simd = vec![0.0f32; n];

        warp_plane_scalar_inner(&src, &mut dst_scalar, w, h, &m, bg);
        warp_plane_simd_planar(
            &src,
            &mut dst_simd,
            w,
            h,
            &m,
            bg,
            WarpInterpolation::Robidoux,
        );

        let mut max_diff = 0.0f32;
        for i in 0..n {
            let diff = (dst_scalar[i] - dst_simd[i]).abs();
            max_diff = max_diff.max(diff);
        }

        assert!(
            max_diff < 1e-4,
            "SIMD color bg vs scalar max diff: {max_diff} (should be < 1e-4)"
        );
    }
}
