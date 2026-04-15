//! Image layout computation with constraint modes, orientation, and decoder negotiation.
//!
//! Pure geometry — no pixel operations, minimal allocations, `no_std` compatible.
//!
//! # Modules
//!
//! - [`constraint`] — Constraint modes (Fit, Within, FitCrop, etc.) and layout computation
//! - [`orientation`] — EXIF orientation, D4 dihedral group, coordinate transforms
//! - [`plan`] — Command pipeline, decoder negotiation, two-phase layout planning
//! - [`svg`] — SVG visualization of layout pipeline steps (requires `svg` feature)
//! - [`riapi`] — RIAPI query string parsing (`?w=800&h=600&mode=crop`) (requires `riapi` feature)

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

whereat::define_at_crate_info!();

mod float_math;

pub mod constraint;
pub mod dimension;
pub mod orientation;
pub mod plan;
#[cfg(feature = "riapi")]
pub mod riapi;
#[cfg(feature = "smart-crop")]
pub mod smart_crop;
#[cfg(feature = "svg")]
pub mod svg;
// #[cfg(feature = "zennode")]
// pub mod zennode_defs;

// Re-exports: core types from constraint module
pub use constraint::{
    CanvasColor, Constraint, ConstraintMode, Gravity, Layout, LayoutError, Rect, Size, SourceCrop,
};
pub use dimension::{
    DimensionEffect, ExpandEffect, PadEffect, ResolutionPolicy, RotateEffect, RotateMode,
    TrimEffect, WarpEffect, expanded_canvas_dims, expanded_canvas_inverse, inscribed_crop_dims,
    inscribed_crop_inverse, warp_output_dims,
};
pub use orientation::Orientation;
pub use plan::{
    Align, CodecLayout, Command, DecoderOffer, DecoderRequest, FlipAxis, IdealLayout, LayoutPlan,
    OutputLimits, Padding, Pipeline, PlaneLayout, Region, RegionCoord, ResolvedEffect, Rotation,
    Subsampling, compute_layout, compute_layout_sequential,
};
pub use whereat::{At, ResultAtExt};
