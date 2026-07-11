//! Imageflow v2 compatibility layer.
//!
//! Translates v2 [`Node`](imageflow_types::Node), [`Framewise`](imageflow_types::Framewise),
//! [`EncoderPreset`](imageflow_types::EncoderPreset), and [`Build001`](imageflow_types::Build001)
//! into zen pipeline operations and executes them.
//!
//! This module is gated behind the `imageflow-compat` feature. It depends on
//! `imageflow_types` and `imageflow_riapi` crates.
//!
//! # Entry points
//!
//! - [`execute_framewise`] — execute a v2 Framewise pipeline
//! - [`zen_get_image_info`] — probe without decoding
//! - [`CapturedBitmap`] — pixel data captured by CaptureBitmapKey nodes

pub mod captured;
mod cms;
mod color;
pub mod converter;
pub mod execute;
pub mod nodes;
pub mod preset_map;
pub mod riapi;
pub mod translate;
pub mod watermark;

pub use captured::CapturedBitmap;
pub use execute::{ExecuteResult, ZenEncodeResult, ZenError, execute_framewise};
pub use riapi::RiapiEngine;

/// Color-management mode for the imageflow-compat execution path.
///
/// imageflow briefly carried this as `imageflow_types::CmsMode` on
/// `JobOptions` (added upstream in 542276d2, moved in b9d53807, removed
/// again — `JobOptions` is an empty reserved struct on current imageflow
/// main, and the rev this workspace pins never had it). The compat path
/// owns the policy now; the default reproduces imageflow v2 behavior
/// (convert everything to sRGB at decode).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompatCmsMode {
    /// imageflow v2 behavior: convert to sRGB during decode (byte parity).
    #[default]
    Imageflow2Compat,
    /// Strict scene-referred color management.
    SceneReferred,
}
