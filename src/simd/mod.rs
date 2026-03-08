use crate::blur::GaussianKernel;
use crate::context::FilterContext;

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
