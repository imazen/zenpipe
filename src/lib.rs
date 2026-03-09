#![forbid(unsafe_code)]
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
//! let mut pipeline = Pipeline::new(PipelineConfig {
//!     primaries: ColorPrimaries::Bt709,
//!     reference_white: 1.0,
//! }).unwrap();
//! pipeline.push(Box::new(Exposure { stops: 0.5 }));
//! pipeline.push(Box::new(Clarity { sigma: 10.0, amount: 0.3 }));
//! pipeline.push(Box::new(Vibrance { amount: 0.4, protection: 2.0 }));
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

mod access;
mod blur;
mod context;
mod filter;
pub mod filters;
mod gamut_map;
mod pipeline;
mod planes;
mod scatter_gather;
mod simd;

#[cfg(feature = "buffer")]
mod convenience;
#[cfg(feature = "srgb-filters")]
#[allow(clippy::manual_clamp)]
pub mod srgb_filters;

pub use access::ChannelAccess;
pub use blur::GaussianKernel;
pub use context::FilterContext;
#[cfg(feature = "buffer")]
pub use convenience::{ConvenienceError, PipelineBufferExt, apply_to_buffer};
pub use filter::Filter;
pub use gamut_map::GamutMapping;
pub use pipeline::{Pipeline, PipelineConfig, PipelineError};
pub use planes::OklabPlanes;
pub use scatter_gather::{
    gather_from_oklab, gather_oklab_to_srgb_u8, scatter_srgb_u8_to_oklab, scatter_to_oklab,
};
