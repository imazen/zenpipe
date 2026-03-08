use archmage::prelude::*;

use crate::blur::{self, GaussianKernel};

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
) {
    blur::gaussian_blur_plane_scalar(src, dst, width, height, kernel);
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
