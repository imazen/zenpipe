//! zenpipe interactive editor — WASM worker backend.
//!
//! Wraps zenpipe Session for dual-view rendering:
//! - Overview: small resized version of the full image
//! - Detail: cropped region at higher resolution
//!
//! Both views share the same filter adjustments but have independent
//! Session caches — geometry prefix (decode + resize/crop) is cached
//! separately, filter suffix re-runs from cache on parameter changes.

pub mod decode;
mod editor;
pub mod encode;
mod schema;

pub use editor::{Editor, Region, RenderOutput};
pub use encode::{EncodedImage, encode};
pub use schema::export_filter_schema;

#[cfg(feature = "wasm")]
mod wasm_api;
