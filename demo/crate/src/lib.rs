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
mod schema;

pub use editor::{Editor, EncodeResult, Region, RenderOutput};
pub use schema::export_filter_schema;

// Re-export init_thread_pool for multithreaded WASM.
// JS calls `await wasm.initThreadPool(navigator.hardwareConcurrency)`
// before any rayon parallel operations.
#[cfg(feature = "parallel")]
pub use wasm_bindgen_rayon::init_thread_pool;

#[cfg(feature = "wasm")]
mod wasm_api;
