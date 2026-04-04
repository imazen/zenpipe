//! zenpipe WASM demo — thin platform layer over zeneditor.
//!
//! This crate provides:
//! - WASM API (`WasmEditor`) — wasm-bindgen bindings for the web worker
//! - Image decoding — browser fallback and native zencodecs decode
//!
//! All editor logic (adjustments, rendering, encoding, undo/redo, schema)
//! lives in the `zeneditor` crate.

pub mod decode;

// Re-export zeneditor types for convenience
pub use zeneditor::{EditorState, RenderOutput};

/// Export the filter node schema as JSON (delegates to zeneditor).
pub fn export_filter_schema() -> String {
    let schema = zeneditor::SchemaModel::from_registry();
    schema.schema_json().to_string()
}

// Re-export init_thread_pool for multithreaded WASM.
#[cfg(feature = "parallel")]
pub use wasm_bindgen_rayon::init_thread_pool;

#[cfg(feature = "wasm")]
mod wasm_api;
