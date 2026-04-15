#![no_std]
#![forbid(unsafe_code)]
#![allow(
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::collapsible_if,
    clippy::assign_op_pattern,
    clippy::manual_range_contains,
    clippy::manual_memcpy,
    clippy::doc_lazy_continuation,
    clippy::excessive_precision,
    clippy::unnecessary_cast,
    clippy::duplicated_attributes,
    clippy::field_reassign_with_default,
    clippy::type_complexity
)]
#![cfg_attr(test, allow(unused_imports))]
//! Photo filter operations on planar Oklab f32 data with SIMD acceleration.
//!
//! zenfilters provides a pipeline for applying photo adjustments (exposure,
//! contrast, clarity, saturation, etc.) in Oklab color space using a planar
//! layout for maximum SIMD throughput.
//!
//! ## Pipeline
//!
//! ```text
//! Linear RGB f32 → scatter to planar Oklab → filters → gather to Linear RGB f32
//! ```
//!
//! Filters operate on separate L, a, b planes. Per-pixel L-only filters
//! (exposure, contrast) run at full SIMD width on contiguous memory.
//! Neighborhood filters (clarity, brilliance) use separable Gaussian blur
//! on the L plane for 188× speedup over naive interleaved approaches.
//!
//! ## Usage
//!
//! ```
//! use zenfilters::{Pipeline, PipelineConfig, FilterContext, OklabPlanes};
//! use zenfilters::filters::*;
//! use zenpixels::ColorPrimaries;
//!
//! let mut pipeline = Pipeline::new(PipelineConfig::default()).unwrap();
//!
//! let mut exposure = Exposure::default();
//! exposure.stops = 0.5;
//! pipeline.push(Box::new(exposure));
//!
//! let mut clarity = Clarity::default();
//! clarity.amount = 0.3;
//! pipeline.push(Box::new(clarity));
//!
//! let mut vibrance = Vibrance::default();
//! vibrance.amount = 0.4;
//! pipeline.push(Box::new(vibrance));
//!
//! // Create a reusable context to avoid per-call allocations
//! let mut ctx = FilterContext::new();
//!
//! // Apply to interleaved linear RGB f32 data
//! let (w, h) = (64, 64);
//! let src = vec![0.5f32; w * h * 3];
//! let mut dst = vec![0.0f32; w * h * 3];
//! pipeline.apply(&src, &mut dst, w as u32, h as u32, 3, &mut ctx).unwrap();
//! ```

extern crate alloc;

whereat::define_at_crate_info!();

/// LUT size for tone/color curves.
///
/// 1024 entries (10-bit precision) balances curve fidelity against L1 cache
/// pressure. Each LUT is 4KB; a realistic pipeline may have 6+ active LUTs
/// (ToneCurve + ParametricCurve + 3× ChannelCurves + Basecurve = 24KB),
/// which must coexist with working data in 32KB L1.
///
/// With linear interpolation, 1024 entries gives sub-0.001 max error on
/// any smooth curve — indistinguishable from 4096 at any bit depth.
///
/// Previous values: 256 (banding at >8bpc), 4096 (L1 cache contention).
pub(crate) const LUT_SIZE: usize = 1024;

/// Maximum LUT index (LUT_SIZE - 1), used for clamping.
pub(crate) const LUT_MAX: usize = LUT_SIZE - 1;

mod access;
pub mod analysis;
mod blur;
mod context;
pub(crate) mod fast_math;
mod filter;
pub mod filter_compat;
pub mod filters;
mod fused_params;
mod gamut_lut;
mod gamut_map;
pub mod masked;
pub mod param_schema;
mod pipeline;
mod planes;
pub(crate) mod prelude;
pub mod presets;
pub mod regional;
pub mod resize_pipeline;
mod scatter_gather;
mod simd;
pub mod slider;

#[cfg(feature = "experimental")]
pub mod document;
#[cfg(feature = "experimental")]
pub mod segment;

#[cfg(feature = "zennode")]
pub mod zennode_defs;

mod convenience;
#[cfg(feature = "srgb-filters")]
#[allow(clippy::manual_clamp)]
pub mod srgb_filters;

pub use access::ChannelAccess;
pub use blur::GaussianKernel;

/// Internal blur functions exposed for benchmarking. Not part of the public API.
#[cfg(feature = "experimental")]
#[doc(hidden)]
pub mod blur_internals {
    pub use crate::blur::{
        DericheCoefficients, GaussianKernel, deriche_blur_plane, gaussian_blur_plane,
        gaussian_blur_plane_scalar, kernel_sigma, sigma_to_stackblur_radius, stackblur_plane,
    };
}
pub use analysis::ImageAnalysis;
pub use context::FilterContext;
pub use convenience::{ConvenienceError, PipelineBufferExt, apply_to_buffer};
pub use filter::{Filter, PlaneSemantics, ResizePhase};
pub use fused_params::FusedAdjustParams;
pub use gamut_map::GamutMapping;
pub use pipeline::{Pipeline, PipelineConfig, PipelineError, WorkingSpace};
pub use planes::OklabPlanes;
pub use scatter_gather::{
    gather_from_oklab, gather_oklab_to_srgb_u8, scatter_srgb_u8_to_oklab, scatter_to_oklab,
};

/// Fused interleaved per-pixel adjust: RGB→Oklab→adjust→RGB in one SIMD pass.
#[cfg(feature = "experimental")]
pub fn fused_interleaved_adjust(
    src: &[f32],
    dst: &mut [f32],
    channels: u32,
    m1: &zenpixels_convert::gamut::GamutMatrix,
    m1_inv: &zenpixels_convert::gamut::GamutMatrix,
    inv_white: f32,
    reference_white: f32,
    params: &FusedAdjustParams,
) {
    simd::fused_interleaved_adjust(
        src,
        dst,
        channels,
        m1,
        m1_inv,
        inv_white,
        reference_white,
        params,
    );
}
