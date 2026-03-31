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
