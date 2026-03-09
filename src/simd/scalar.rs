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

pub(super) fn power_contrast_plane_impl_scalar(
    _token: ScalarToken,
    plane: &mut [f32],
    exp: f32,
    scale: f32,
) {
    for v in plane.iter_mut() {
        if *v > 0.0 {
            *v = v.powf(exp) * scale;
        }
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

#[allow(clippy::too_many_arguments)]
pub(super) fn scatter_srgb_u8_to_oklab_impl_scalar(
    _token: ScalarToken,
    src: &[u8],
    l: &mut [f32],
    a: &mut [f32],
    b: &mut [f32],
    channels: u32,
    m1: &GamutMatrix,
) {
    let n = l.len();
    let ch = channels as usize;
    for i in 0..n {
        let base = i * ch;
        let r = linear_srgb::default::srgb_u8_to_linear(src[base]);
        let g = linear_srgb::default::srgb_u8_to_linear(src[base + 1]);
        let bv = linear_srgb::default::srgb_u8_to_linear(src[base + 2]);
        let [ol, oa, ob] = oklab::rgb_to_oklab(r, g, bv, m1);
        l[i] = ol;
        a[i] = oa;
        b[i] = ob;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn gather_oklab_to_srgb_u8_impl_scalar(
    _token: ScalarToken,
    l: &[f32],
    a: &[f32],
    b: &[f32],
    dst: &mut [u8],
    channels: u32,
    m1_inv: &GamutMatrix,
) {
    let n = l.len();
    let ch = channels as usize;
    for i in 0..n {
        let [r, g, bv] = oklab::oklab_to_rgb(l[i], a[i], b[i], m1_inv);
        let base = i * ch;
        dst[base] = linear_srgb::default::linear_to_srgb_u8(r);
        dst[base + 1] = linear_srgb::default::linear_to_srgb_u8(g);
        dst[base + 2] = linear_srgb::default::linear_to_srgb_u8(bv);
    }
}

pub(super) fn black_point_plane_impl_scalar(
    _token: ScalarToken,
    plane: &mut [f32],
    bp: f32,
    inv_range: f32,
) {
    for v in plane.iter_mut() {
        *v = ((*v - bp) * inv_range).max(0.0);
    }
}

pub(super) fn hue_rotate_impl_scalar(
    _token: ScalarToken,
    a: &mut [f32],
    b: &mut [f32],
    cos_r: f32,
    sin_r: f32,
) {
    for (a_val, b_val) in a.iter_mut().zip(b.iter_mut()) {
        let a_orig = *a_val;
        let b_orig = *b_val;
        *a_val = a_orig * cos_r - b_orig * sin_r;
        *b_val = a_orig * sin_r + b_orig * cos_r;
    }
}

pub(super) fn highlights_shadows_impl_scalar(
    _token: ScalarToken,
    plane: &mut [f32],
    shadows: f32,
    highlights: f32,
) {
    for v in plane.iter_mut() {
        let l = *v;
        let sm = (1.0 - l * 2.0).max(0.0);
        let mut l_new = l + sm * sm * shadows * 0.5;
        let hm = ((l_new - 0.5) * 2.0).clamp(0.0, 1.0);
        l_new -= hm * hm * highlights * 0.5;
        *v = l_new;
    }
}

pub(super) fn vibrance_impl_scalar(
    _token: ScalarToken,
    a: &mut [f32],
    b: &mut [f32],
    amount: f32,
    protection: f32,
) {
    const MAX_CHROMA: f32 = 0.4;
    for (a_val, b_val) in a.iter_mut().zip(b.iter_mut()) {
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
pub(super) fn fused_adjust_impl_scalar(
    _token: ScalarToken,
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
    // L pass
    for v in l.iter_mut() {
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
    // AB pass: exposure scales chroma to match L-channel exposure
    const MAX_CHROMA: f32 = 0.4;
    for (a_val, b_val) in a.iter_mut().zip(b.iter_mut()) {
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
