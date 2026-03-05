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

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod constraint;
pub mod orientation;
pub mod plan;
#[cfg(feature = "riapi")]
pub mod riapi;
#[cfg(feature = "alloc")]
pub mod smart_crop;
#[cfg(feature = "svg")]
pub mod svg;

// Re-exports: core types from constraint module
pub use constraint::{
    CanvasColor, Constraint, ConstraintMode, Gravity, Layout, LayoutError, Rect, Size, SourceCrop,
};
pub use orientation::Orientation;
pub use plan::{
    Align, CodecLayout, Command, DecoderOffer, DecoderRequest, FlipAxis, IdealLayout, LayoutPlan,
    OutputLimits, Padding, Pipeline, PlaneLayout, Region, RegionCoord, Rotation, Subsampling,
    compute_layout, compute_layout_sequential,
};
