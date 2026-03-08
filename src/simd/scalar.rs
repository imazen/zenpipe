use archmage::prelude::*;

use crate::blur::{self, GaussianKernel};
use crate::context::FilterContext;
use zenpixels_convert::gamut::GamutMatrix;
use zenpixels_convert::oklab;

pub(super) fn scale_plane_impl_scalar(_token: ScalarToken, plane: &mut [f32], factor: f32) {
    for v in plane.iter_mut() {
        *v *= factor;
    }
}

pub(super) fn offset_plane_impl_scalar(_token: ScalarToken, plane: &mut [f32], offset: f32) {
    for v in plane.iter_mut() {
        *v += offset;
    }
}

pub(super) fn unsharp_fuse_impl_scalar(
    _token: ScalarToken,
    src: &[f32],
    blurred: &[f32],
    dst: &mut [f32],
    amount: f32,
) {
    for i in 0..src.len() {
        dst[i] = (src[i] + (src[i] - blurred[i]) * amount).max(0.0);
    }
}

pub(super) fn gaussian_blur_plane_impl_scalar(
    _token: ScalarToken,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    kernel: &GaussianKernel,
    ctx: &mut FilterContext,
) {
    blur::gaussian_blur_plane_scalar(src, dst, width, height, kernel, ctx);
}

pub(super) fn brilliance_apply_impl_scalar(
    _token: ScalarToken,
    src_l: &[f32],
    avg_l: &[f32],
    dst_l: &mut [f32],
    amount: f32,
    shadow_str: f32,
    highlight_str: f32,
) {
    for i in 0..src_l.len() {
        let l = src_l[i];
        let avg = avg_l[i].max(0.001);
        let ratio = l / avg;
        let c = if ratio < 1.0 {
            1.0 + (1.0 - ratio) * shadow_str * amount
        } else {
            1.0 - (ratio - 1.0).min(1.0) * highlight_str * amount
        };
        dst_l[i] = l * c;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn scatter_oklab_impl_scalar(
    _token: ScalarToken,
    src: &[f32],
    l: &mut [f32],
    a: &mut [f32],
    b: &mut [f32],
    channels: u32,
    m1: &GamutMatrix,
    inv_white: f32,
) {
    let n = l.len();
    let ch = channels as usize;
    for i in 0..n {
        let base = i * ch;
        let r = src[base] * inv_white;
        let g = src[base + 1] * inv_white;
        let bv = src[base + 2] * inv_white;
        let [ol, oa, ob] = oklab::rgb_to_oklab(r, g, bv, m1);
        l[i] = ol;
        a[i] = oa;
        b[i] = ob;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn gather_oklab_impl_scalar(
    _token: ScalarToken,
    l: &[f32],
    a: &[f32],
    b: &[f32],
    dst: &mut [f32],
    channels: u32,
    m1_inv: &GamutMatrix,
    reference_white: f32,
) {
    let n = l.len();
    let ch = channels as usize;
    for i in 0..n {
        let [r, g, bv] = oklab::oklab_to_rgb(l[i], a[i], b[i], m1_inv);
        let base = i * ch;
        dst[base] = (r * reference_white).max(0.0);
        dst[base + 1] = (g * reference_white).max(0.0);
        dst[base + 2] = (bv * reference_white).max(0.0);
    }
}
