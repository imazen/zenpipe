//! WASM SIMD128 dispatch targets for zenfilters.
//!
//! All implementations currently delegate to the scalar fallback for correctness.
//! Future work can add native WASM SIMD128 kernels using `magetypes::simd::f32x4`
//! for hot-path operations (scatter/gather, fused_adjust, etc.).

#![allow(clippy::too_many_arguments)]

use archmage::prelude::*;

use crate::blur::GaussianKernel;
use crate::context::FilterContext;
use zenpixels_convert::gamut::GamutMatrix;

#[archmage::arcane]
pub(super) fn scale_plane_impl_wasm128(_token: Wasm128Token, plane: &mut [f32], factor: f32) {
    super::scale_plane_impl_scalar(ScalarToken, plane, factor);
}

#[archmage::arcane]
pub(super) fn offset_plane_impl_wasm128(_token: Wasm128Token, plane: &mut [f32], offset: f32) {
    super::offset_plane_impl_scalar(ScalarToken, plane, offset);
}

#[archmage::arcane]
pub(super) fn power_contrast_plane_impl_wasm128(
    _token: Wasm128Token,
    plane: &mut [f32],
    exp: f32,
    scale: f32,
) {
    super::power_contrast_plane_impl_scalar(ScalarToken, plane, exp, scale);
}

#[archmage::arcane]
pub(super) fn sigmoid_tone_map_plane_impl_wasm128(
    _token: Wasm128Token,
    plane: &mut [f32],
    contrast: f32,
    bias_a: f32,
) {
    super::sigmoid_tone_map_plane_impl_scalar(ScalarToken, plane, contrast, bias_a);
}

#[archmage::arcane]
pub(super) fn unsharp_fuse_impl_wasm128(
    _token: Wasm128Token,
    src: &[f32],
    blurred: &[f32],
    dst: &mut [f32],
    amount: f32,
) {
    super::unsharp_fuse_impl_scalar(ScalarToken, src, blurred, dst, amount);
}

#[archmage::arcane]
pub(super) fn gaussian_blur_plane_impl_wasm128(
    _token: Wasm128Token,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    kernel: &GaussianKernel,
    ctx: &mut FilterContext,
) {
    super::gaussian_blur_plane_impl_scalar(ScalarToken, src, dst, width, height, kernel, ctx);
}

#[archmage::arcane]
pub(super) fn brilliance_apply_impl_wasm128(
    _token: Wasm128Token,
    src_l: &[f32],
    avg_l: &[f32],
    dst_l: &mut [f32],
    amount: f32,
    shadow_strength: f32,
    highlight_strength: f32,
) {
    super::brilliance_apply_impl_scalar(
        ScalarToken,
        src_l,
        avg_l,
        dst_l,
        amount,
        shadow_strength,
        highlight_strength,
    );
}

#[archmage::arcane]
pub(super) fn scatter_oklab_impl_wasm128(
    _token: Wasm128Token,
    src: &[f32],
    l: &mut [f32],
    a: &mut [f32],
    b: &mut [f32],
    channels: u32,
    m1: &GamutMatrix,
    inv_white: f32,
) {
    super::scatter_oklab_impl_scalar(ScalarToken, src, l, a, b, channels, m1, inv_white);
}

#[archmage::arcane]
pub(super) fn gather_oklab_impl_wasm128(
    _token: Wasm128Token,
    l: &[f32],
    a: &[f32],
    b: &[f32],
    dst: &mut [f32],
    channels: u32,
    m1_inv: &GamutMatrix,
    reference_white: f32,
) {
    super::gather_oklab_impl_scalar(
        ScalarToken,
        l,
        a,
        b,
        dst,
        channels,
        m1_inv,
        reference_white,
    );
}

#[archmage::arcane]
pub(super) fn scatter_srgb_u8_to_oklab_impl_wasm128(
    _token: Wasm128Token,
    src: &[u8],
    l: &mut [f32],
    a: &mut [f32],
    b: &mut [f32],
    channels: u32,
    m1: &GamutMatrix,
) {
    super::scatter_srgb_u8_to_oklab_impl_scalar(ScalarToken, src, l, a, b, channels, m1);
}

#[archmage::arcane]
pub(super) fn gather_oklab_to_srgb_u8_impl_wasm128(
    _token: Wasm128Token,
    l: &[f32],
    a: &[f32],
    b: &[f32],
    dst: &mut [u8],
    channels: u32,
    m1_inv: &GamutMatrix,
) {
    super::gather_oklab_to_srgb_u8_impl_scalar(ScalarToken, l, a, b, dst, channels, m1_inv);
}

#[archmage::arcane]
pub(super) fn black_point_plane_impl_wasm128(
    _token: Wasm128Token,
    plane: &mut [f32],
    bp: f32,
    inv_range: f32,
) {
    super::black_point_plane_impl_scalar(ScalarToken, plane, bp, inv_range);
}

#[archmage::arcane]
pub(super) fn hue_rotate_impl_wasm128(
    _token: Wasm128Token,
    a: &mut [f32],
    b: &mut [f32],
    cos_r: f32,
    sin_r: f32,
) {
    super::hue_rotate_impl_scalar(ScalarToken, a, b, cos_r, sin_r);
}

#[archmage::arcane]
pub(super) fn highlights_shadows_impl_wasm128(
    _token: Wasm128Token,
    plane: &mut [f32],
    shadows: f32,
    highlights: f32,
) {
    super::highlights_shadows_impl_scalar(ScalarToken, plane, shadows, highlights);
}

#[archmage::arcane]
pub(super) fn vibrance_impl_wasm128(
    _token: Wasm128Token,
    a: &mut [f32],
    b: &mut [f32],
    amount: f32,
    protection: f32,
) {
    super::vibrance_impl_scalar(ScalarToken, a, b, amount, protection);
}

#[archmage::arcane]
pub(super) fn subtract_planes_impl_wasm128(
    _token: Wasm128Token,
    a: &[f32],
    b: &[f32],
    dst: &mut [f32],
) {
    super::subtract_planes_impl_scalar(ScalarToken, a, b, dst);
}

#[archmage::arcane]
pub(super) fn square_plane_impl_wasm128(
    _token: Wasm128Token,
    src: &[f32],
    dst: &mut [f32],
) {
    super::square_plane_impl_scalar(ScalarToken, src, dst);
}

#[archmage::arcane]
pub(super) fn wavelet_threshold_accumulate_impl_wasm128(
    _token: Wasm128Token,
    current: &[f32],
    smooth: &[f32],
    result: &mut [f32],
    threshold: f32,
) {
    super::wavelet_threshold_accumulate_impl_scalar(
        ScalarToken,
        current,
        smooth,
        result,
        threshold,
    );
}

#[archmage::arcane]
pub(super) fn add_clamped_impl_wasm128(
    _token: Wasm128Token,
    a: &[f32],
    b: &[f32],
    dst: &mut [f32],
) {
    super::add_clamped_impl_scalar(ScalarToken, a, b, dst);
}

#[archmage::arcane]
pub(super) fn adaptive_sharpen_apply_impl_wasm128(
    _token: Wasm128Token,
    l: &[f32],
    detail: &[f32],
    energy: &[f32],
    dst: &mut [f32],
    amount: f32,
    noise_floor: f32,
    masking_threshold: f32,
) {
    super::adaptive_sharpen_apply_impl_scalar(
        ScalarToken,
        l,
        detail,
        energy,
        dst,
        amount,
        noise_floor,
        masking_threshold,
    );
}

#[archmage::arcane]
pub(super) fn fused_adjust_impl_wasm128(
    _token: Wasm128Token,
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
    super::fused_adjust_impl_scalar(
        ScalarToken,
        l,
        a,
        b,
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

#[allow(dead_code)]
#[archmage::arcane]
pub(super) fn fused_interleaved_adjust_impl_wasm128(
    _token: Wasm128Token,
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
    super::fused_interleaved_adjust_impl_scalar(
        ScalarToken,
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
