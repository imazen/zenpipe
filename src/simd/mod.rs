use crate::blur::GaussianKernel;
use crate::context::FilterContext;
use zenpixels_convert::gamut::GamutMatrix;

mod scalar;
use scalar::*;

#[cfg(target_arch = "x86_64")]
mod x86;
#[cfg(target_arch = "x86_64")]
use x86::*;

#[cfg(target_arch = "aarch64")]
mod neon;
#[cfg(target_arch = "aarch64")]
use neon::*;

#[cfg(target_arch = "wasm32")]
mod wasm128;
#[cfg(target_arch = "wasm32")]
use wasm128::*;

/// Dispatch: scale every element of a plane by a constant factor.
pub(crate) fn scale_plane(plane: &mut [f32], factor: f32) {
    archmage::incant!(scale_plane_impl(plane, factor), [v3, neon, wasm128, scalar]);
}

/// Dispatch: add a constant to every element of a plane.
pub(crate) fn offset_plane(plane: &mut [f32], offset: f32) {
    archmage::incant!(offset_plane_impl(plane, offset), [v3, neon, wasm128, scalar]);
}

/// Dispatch: power-curve contrast on a plane: `v = v^exp * scale` (v > 0).
pub(crate) fn power_contrast_plane(plane: &mut [f32], exp: f32, scale: f32) {
    archmage::incant!(power_contrast_plane_impl(plane, exp, scale), [v3, neon, wasm128, scalar]);
}

/// Dispatch: sigmoid tone map on L plane.
///
/// Applies Schlick bias (if bias_a != 0) then generalized sigmoid `x^c / (x^c + (1-x)^c)`.
pub(crate) fn sigmoid_tone_map_plane(plane: &mut [f32], contrast: f32, bias_a: f32) {
    archmage::incant!(
        sigmoid_tone_map_plane_impl(plane, contrast, bias_a),
        [v3, neon, wasm128, scalar]
    );
}

/// Dispatch: unsharp mask fuse: dst[i] = (src[i] + (src[i] - blurred[i]) * amount).max(0)
pub(crate) fn unsharp_fuse(src: &[f32], blurred: &[f32], dst: &mut [f32], amount: f32) {
    archmage::incant!(unsharp_fuse_impl(src, blurred, dst, amount), [v3, neon, wasm128, scalar]);
}

/// Dispatch: separable Gaussian blur on a single f32 plane.
pub(crate) fn gaussian_blur_plane_dispatch(
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    kernel: &GaussianKernel,
    ctx: &mut FilterContext,
) {
    archmage::incant!(
        gaussian_blur_plane_impl(src, dst, width, height, kernel, ctx),
        [v3, neon, wasm128, scalar]
    );
}

/// Dispatch: brilliance apply: adaptive local contrast correction.
pub(crate) fn brilliance_apply(
    src_l: &[f32],
    avg_l: &[f32],
    dst_l: &mut [f32],
    amount: f32,
    shadow_strength: f32,
    highlight_strength: f32,
) {
    archmage::incant!(
        brilliance_apply_impl(
            src_l,
            avg_l,
            dst_l,
            amount,
            shadow_strength,
            highlight_strength
        ),
        [v3, neon, wasm128, scalar]
    );
}

/// Dispatch: scatter interleaved linear RGB f32 to planar Oklab.
///
/// Alpha is handled separately by the caller (it's just a copy).
pub(crate) fn scatter_oklab(
    src: &[f32],
    l: &mut [f32],
    a: &mut [f32],
    b: &mut [f32],
    channels: u32,
    m1: &GamutMatrix,
    inv_white: f32,
) {
    archmage::incant!(
        scatter_oklab_impl(src, l, a, b, channels, m1, inv_white),
        [v3, neon, wasm128, scalar]
    );
}

/// Dispatch: gather planar Oklab to interleaved linear RGB f32.
///
/// Alpha is handled separately by the caller.
pub(crate) fn gather_oklab(
    l: &[f32],
    a: &[f32],
    b: &[f32],
    dst: &mut [f32],
    channels: u32,
    m1_inv: &GamutMatrix,
    reference_white: f32,
) {
    archmage::incant!(
        gather_oklab_impl(l, a, b, dst, channels, m1_inv, reference_white),
        [v3, neon, wasm128, scalar]
    );
}

/// Dispatch: scatter interleaved sRGB u8 to planar Oklab (fused path).
///
/// Fuses sRGB→linear LUT with RGB→Oklab conversion in one SIMD pass,
/// eliminating the intermediate linear f32 buffer.
/// Alpha is handled separately by the caller.
pub(crate) fn scatter_srgb_u8_to_oklab(
    src: &[u8],
    l: &mut [f32],
    a: &mut [f32],
    b: &mut [f32],
    channels: u32,
    m1: &GamutMatrix,
) {
    archmage::incant!(
        scatter_srgb_u8_to_oklab_impl(src, l, a, b, channels, m1),
        [v3, neon, wasm128, scalar]
    );
}

/// Dispatch: gather planar Oklab to interleaved sRGB u8 (fused path).
///
/// Fuses Oklab→RGB conversion with linear→sRGB LUT in one SIMD pass,
/// eliminating the intermediate linear f32 buffer.
/// Alpha is handled separately by the caller.
pub(crate) fn gather_oklab_to_srgb_u8(
    l: &[f32],
    a: &[f32],
    b: &[f32],
    dst: &mut [u8],
    channels: u32,
    m1_inv: &GamutMatrix,
) {
    archmage::incant!(
        gather_oklab_to_srgb_u8_impl(l, a, b, dst, channels, m1_inv),
        [v3, neon, wasm128, scalar]
    );
}

/// Dispatch: black point remap on a single plane.
pub(crate) fn black_point_plane(plane: &mut [f32], bp: f32, inv_range: f32) {
    archmage::incant!(black_point_plane_impl(plane, bp, inv_range), [v3, neon, wasm128, scalar]);
}

/// Dispatch: 2D hue rotation on a/b planes.
pub(crate) fn hue_rotate(a: &mut [f32], b: &mut [f32], cos_r: f32, sin_r: f32) {
    archmage::incant!(hue_rotate_impl(a, b, cos_r, sin_r), [v3, neon, wasm128, scalar]);
}

/// Dispatch: highlights and shadows recovery on L plane.
pub(crate) fn highlights_shadows(plane: &mut [f32], shadows: f32, highlights: f32) {
    archmage::incant!(
        highlights_shadows_impl(plane, shadows, highlights),
        [v3, neon, wasm128, scalar]
    );
}

/// Dispatch: vibrance (smart saturation) on a/b planes.
pub(crate) fn vibrance(a: &mut [f32], b: &mut [f32], amount: f32, protection: f32) {
    archmage::incant!(vibrance_impl(a, b, amount, protection), [v3, neon, wasm128, scalar]);
}

/// Dispatch: subtract two planes. dst[i] = a[i] - b[i]
pub(crate) fn subtract_planes(a: &[f32], b: &[f32], dst: &mut [f32]) {
    archmage::incant!(subtract_planes_impl(a, b, dst), [v3, neon, wasm128, scalar]);
}

/// Dispatch: square a plane. dst[i] = src[i] * src[i]
pub(crate) fn square_plane(src: &[f32], dst: &mut [f32]) {
    archmage::incant!(square_plane_impl(src, dst), [v3, neon, wasm128, scalar]);
}

/// Dispatch: wavelet soft-threshold and accumulate.
/// result[i] += soft_threshold(current[i] - smooth[i], threshold)
pub(crate) fn wavelet_threshold_accumulate(
    current: &[f32],
    smooth: &[f32],
    result: &mut [f32],
    threshold: f32,
) {
    archmage::incant!(
        wavelet_threshold_accumulate_impl(current, smooth, result, threshold),
        [v3, neon, wasm128, scalar]
    );
}

/// Dispatch: add two planes with clamping to zero.
/// dst[i] = (a[i] + b[i]).max(0.0)
pub(crate) fn add_clamped(a: &[f32], b: &[f32], dst: &mut [f32]) {
    archmage::incant!(add_clamped_impl(a, b, dst), [v3, neon, wasm128, scalar]);
}

/// Dispatch: adaptive sharpen per-pixel (detail extraction + energy gating).
pub(crate) fn adaptive_sharpen_apply(
    l: &[f32],
    detail: &[f32],
    energy: &[f32],
    dst: &mut [f32],
    amount: f32,
    noise_floor: f32,
    masking_threshold: f32,
) {
    archmage::incant!(
        adaptive_sharpen_apply_impl(
            l,
            detail,
            energy,
            dst,
            amount,
            noise_floor,
            masking_threshold
        ),
        [v3, neon, wasm128, scalar]
    );
}

/// Dispatch: fused per-pixel adjustment (L pass + AB pass).
pub(crate) fn fused_adjust(
    l: &mut [f32],
    a: &mut [f32],
    b: &mut [f32],
    p: &crate::fused_params::FusedAdjustParams,
) {
    archmage::incant!(
        fused_adjust_impl(
            l,
            a,
            b,
            p.bp,
            p.inv_range,
            p.wp_exp,
            p.contrast_exp,
            p.contrast_scale,
            p.shadows,
            p.highlights,
            p.dehaze_contrast,
            p.dehaze_chroma,
            p.exposure_chroma,
            p.temp_offset,
            p.tint_offset,
            p.sat,
            p.vib_amount,
            p.vib_protection
        ),
        [v3, neon, wasm128, scalar]
    );
}

/// Dispatch: fused interleaved per-pixel adjustment (RGB→Oklab→adjust→RGB in one pass).
#[allow(dead_code, clippy::too_many_arguments)]
pub(crate) fn fused_interleaved_adjust(
    src: &[f32],
    dst: &mut [f32],
    channels: u32,
    m1: &GamutMatrix,
    m1_inv: &GamutMatrix,
    inv_white: f32,
    reference_white: f32,
    p: &crate::fused_params::FusedAdjustParams,
) {
    archmage::incant!(
        fused_interleaved_adjust_impl(
            src,
            dst,
            channels,
            m1,
            m1_inv,
            inv_white,
            reference_white,
            p.bp,
            p.inv_range,
            p.wp_exp,
            p.contrast_exp,
            p.contrast_scale,
            p.shadows,
            p.highlights,
            p.dehaze_contrast,
            p.dehaze_chroma,
            p.exposure_chroma,
            p.temp_offset,
            p.tint_offset,
            p.sat,
            p.vib_amount,
            p.vib_protection
        ),
        [v3, neon, wasm128, scalar]
    );
}
