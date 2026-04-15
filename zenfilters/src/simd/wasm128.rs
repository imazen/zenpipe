//! WASM SIMD128 dispatch targets for zenfilters.
//!
//! Real f32x8 SIMD via the shared `wide_simd` module (polyfilled as 2xf32x4).

#![allow(clippy::too_many_arguments)]

use archmage::prelude::*;

use crate::blur::GaussianKernel;
use crate::context::FilterContext;
use zenpixels_convert::gamut::GamutMatrix;

#[arcane]
pub(super) fn scale_plane_impl_wasm128(token: Wasm128Token, plane: &mut [f32], factor: f32) {
    super::wide_simd::scale_plane_simd_wasm128(token, plane, factor);
}

#[arcane]
pub(super) fn offset_plane_impl_wasm128(token: Wasm128Token, plane: &mut [f32], offset: f32) {
    super::wide_simd::offset_plane_simd_wasm128(token, plane, offset);
}

#[arcane]
pub(super) fn power_contrast_plane_impl_wasm128(
    token: Wasm128Token,
    plane: &mut [f32],
    exp: f32,
    scale: f32,
) {
    super::wide_simd::power_contrast_plane_simd_wasm128(token, plane, exp, scale);
}

#[arcane]
pub(super) fn sigmoid_tone_map_plane_impl_wasm128(
    token: Wasm128Token,
    plane: &mut [f32],
    contrast: f32,
    bias_a: f32,
) {
    super::wide_simd::sigmoid_tone_map_plane_simd_wasm128(token, plane, contrast, bias_a);
}

#[arcane]
pub(super) fn unsharp_fuse_impl_wasm128(
    token: Wasm128Token,
    src: &[f32],
    blurred: &[f32],
    dst: &mut [f32],
    amount: f32,
) {
    super::wide_simd::unsharp_fuse_simd_wasm128(token, src, blurred, dst, amount);
}

#[arcane]
pub(super) fn gaussian_blur_plane_impl_wasm128(
    token: Wasm128Token,
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    kernel: &GaussianKernel,
    ctx: &mut FilterContext,
) {
    super::wide_simd::gaussian_blur_plane_dispatch_simd_wasm128(
        token, src, dst, width, height, kernel, ctx,
    );
}

#[arcane]
pub(super) fn brilliance_apply_impl_wasm128(
    token: Wasm128Token,
    src_l: &[f32],
    avg_l: &[f32],
    dst_l: &mut [f32],
    amount: f32,
    shadow_strength: f32,
    highlight_strength: f32,
) {
    super::wide_simd::brilliance_apply_simd_wasm128(
        token,
        src_l,
        avg_l,
        dst_l,
        amount,
        shadow_strength,
        highlight_strength,
    );
}

#[arcane]
pub(super) fn scatter_oklab_impl_wasm128(
    token: Wasm128Token,
    src: &[f32],
    l: &mut [f32],
    a: &mut [f32],
    b: &mut [f32],
    channels: u32,
    m1: &GamutMatrix,
    inv_white: f32,
) {
    super::wide_simd::scatter_oklab_simd_wasm128(token, src, l, a, b, channels, m1, inv_white);
}

#[arcane]
pub(super) fn gather_oklab_impl_wasm128(
    token: Wasm128Token,
    l: &[f32],
    a: &[f32],
    b: &[f32],
    dst: &mut [f32],
    channels: u32,
    m1_inv: &GamutMatrix,
    reference_white: f32,
) {
    super::wide_simd::gather_oklab_simd_wasm128(
        token,
        l,
        a,
        b,
        dst,
        channels,
        m1_inv,
        reference_white,
    );
}

#[arcane]
pub(super) fn scatter_srgb_u8_to_oklab_impl_wasm128(
    token: Wasm128Token,
    src: &[u8],
    l: &mut [f32],
    a: &mut [f32],
    b: &mut [f32],
    channels: u32,
    m1: &GamutMatrix,
) {
    super::wide_simd::scatter_srgb_u8_to_oklab_simd_wasm128(token, src, l, a, b, channels, m1);
}

#[arcane]
pub(super) fn gather_oklab_to_srgb_u8_impl_wasm128(
    token: Wasm128Token,
    l: &[f32],
    a: &[f32],
    b: &[f32],
    dst: &mut [u8],
    channels: u32,
    m1_inv: &GamutMatrix,
) {
    super::wide_simd::gather_oklab_to_srgb_u8_simd_wasm128(token, l, a, b, dst, channels, m1_inv);
}

#[arcane]
pub(super) fn black_point_plane_impl_wasm128(
    token: Wasm128Token,
    plane: &mut [f32],
    bp: f32,
    inv_range: f32,
) {
    super::wide_simd::black_point_plane_simd_wasm128(token, plane, bp, inv_range);
}

#[arcane]
pub(super) fn hue_rotate_impl_wasm128(
    token: Wasm128Token,
    a: &mut [f32],
    b: &mut [f32],
    cos_r: f32,
    sin_r: f32,
) {
    super::wide_simd::hue_rotate_simd_wasm128(token, a, b, cos_r, sin_r);
}

#[arcane]
pub(super) fn highlights_shadows_impl_wasm128(
    token: Wasm128Token,
    plane: &mut [f32],
    shadows: f32,
    highlights: f32,
) {
    super::wide_simd::highlights_shadows_simd_wasm128(token, plane, shadows, highlights);
}

#[arcane]
pub(super) fn vibrance_impl_wasm128(
    token: Wasm128Token,
    a: &mut [f32],
    b: &mut [f32],
    amount: f32,
    protection: f32,
) {
    super::wide_simd::vibrance_simd_wasm128(token, a, b, amount, protection);
}

#[arcane]
pub(super) fn subtract_planes_impl_wasm128(
    token: Wasm128Token,
    a: &[f32],
    b: &[f32],
    dst: &mut [f32],
) {
    super::wide_simd::subtract_planes_simd_wasm128(token, a, b, dst);
}

#[arcane]
pub(super) fn square_plane_impl_wasm128(token: Wasm128Token, src: &[f32], dst: &mut [f32]) {
    super::wide_simd::square_plane_simd_wasm128(token, src, dst);
}

#[arcane]
pub(super) fn wavelet_threshold_accumulate_impl_wasm128(
    token: Wasm128Token,
    current: &[f32],
    smooth: &[f32],
    result: &mut [f32],
    threshold: f32,
) {
    super::wide_simd::wavelet_threshold_accumulate_simd_wasm128(
        token, current, smooth, result, threshold,
    );
}

#[arcane]
pub(super) fn add_clamped_impl_wasm128(token: Wasm128Token, a: &[f32], b: &[f32], dst: &mut [f32]) {
    super::wide_simd::add_clamped_simd_wasm128(token, a, b, dst);
}

#[arcane]
pub(super) fn adaptive_sharpen_apply_impl_wasm128(
    token: Wasm128Token,
    l: &[f32],
    detail: &[f32],
    energy: &[f32],
    dst: &mut [f32],
    amount: f32,
    noise_floor: f32,
    masking_threshold: f32,
) {
    super::wide_simd::adaptive_sharpen_apply_simd_wasm128(
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

#[arcane]
pub(super) fn fused_adjust_impl_wasm128(
    token: Wasm128Token,
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
    super::wide_simd::fused_adjust_simd_wasm128(
        token,
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
#[arcane]
pub(super) fn fused_interleaved_adjust_impl_wasm128(
    token: Wasm128Token,
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
    super::wide_simd::fused_interleaved_adjust_simd_wasm128(
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
