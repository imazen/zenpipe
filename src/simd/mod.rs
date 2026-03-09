use crate::blur::GaussianKernel;
use crate::context::FilterContext;
use zenpixels_convert::gamut::GamutMatrix;

mod scalar;
use scalar::*;

#[cfg(target_arch = "x86_64")]
mod x86;
#[cfg(target_arch = "x86_64")]
use x86::*;

/// Dispatch: scale every element of a plane by a constant factor.
pub(crate) fn scale_plane(plane: &mut [f32], factor: f32) {
    archmage::incant!(scale_plane_impl(plane, factor), [v3]);
}

/// Dispatch: add a constant to every element of a plane.
pub(crate) fn offset_plane(plane: &mut [f32], offset: f32) {
    archmage::incant!(offset_plane_impl(plane, offset), [v3]);
}

/// Dispatch: power-curve contrast on a plane: `v = v^exp * scale` (v > 0).
pub(crate) fn power_contrast_plane(plane: &mut [f32], exp: f32, scale: f32) {
    archmage::incant!(power_contrast_plane_impl(plane, exp, scale), [v3]);
}

/// Dispatch: sigmoid tone map on L plane.
///
/// Applies Schlick bias (if bias_a != 0) then generalized sigmoid `x^c / (x^c + (1-x)^c)`.
pub(crate) fn sigmoid_tone_map_plane(plane: &mut [f32], contrast: f32, bias_a: f32) {
    archmage::incant!(sigmoid_tone_map_plane_impl(plane, contrast, bias_a), [v3]);
}

/// Dispatch: unsharp mask fuse: dst[i] = (src[i] + (src[i] - blurred[i]) * amount).max(0)
pub(crate) fn unsharp_fuse(src: &[f32], blurred: &[f32], dst: &mut [f32], amount: f32) {
    archmage::incant!(unsharp_fuse_impl(src, blurred, dst, amount), [v3]);
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
        [v3]
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
        [v3]
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
        [v3]
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
        [v3]
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
        [v3]
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
        [v3]
    );
}

/// Dispatch: black point remap on a single plane.
pub(crate) fn black_point_plane(plane: &mut [f32], bp: f32, inv_range: f32) {
    archmage::incant!(black_point_plane_impl(plane, bp, inv_range), [v3]);
}

/// Dispatch: 2D hue rotation on a/b planes.
pub(crate) fn hue_rotate(a: &mut [f32], b: &mut [f32], cos_r: f32, sin_r: f32) {
    archmage::incant!(hue_rotate_impl(a, b, cos_r, sin_r), [v3]);
}

/// Dispatch: highlights and shadows recovery on L plane.
pub(crate) fn highlights_shadows(plane: &mut [f32], shadows: f32, highlights: f32) {
    archmage::incant!(highlights_shadows_impl(plane, shadows, highlights), [v3]);
}

/// Dispatch: vibrance (smart saturation) on a/b planes.
pub(crate) fn vibrance(a: &mut [f32], b: &mut [f32], amount: f32, protection: f32) {
    archmage::incant!(vibrance_impl(a, b, amount, protection), [v3]);
}

/// Dispatch: fused per-pixel adjustment (L pass + AB pass).
#[allow(clippy::too_many_arguments)]
pub(crate) fn fused_adjust(
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
    archmage::incant!(
        fused_adjust_impl(
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
            vib_protection
        ),
        [v3]
    );
}
