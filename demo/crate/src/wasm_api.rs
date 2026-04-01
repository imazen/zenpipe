//! wasm-bindgen API for the demo editor.
//!
//! Exposes [`Editor`] to JavaScript as `WasmEditor` with methods that
//! accept/return JS-friendly types (Uint8Array, JSON strings, numbers).

use std::collections::BTreeMap;
use wasm_bindgen::prelude::*;

use crate::editor::{Editor, Region};

/// WASM-exposed image editor backed by zenpipe Session caching.
///
/// Created from raw RGBA8 pixel bytes. Holds two Session caches
/// (overview + detail) that persist across render calls.
#[wasm_bindgen]
pub struct WasmEditor {
    inner: Editor,
}

/// Result of a render operation: RGBA pixels + dimensions.
#[wasm_bindgen]
pub struct WasmRenderResult {
    data: Vec<u8>,
    width: u32,
    height: u32,
}

#[wasm_bindgen]
impl WasmRenderResult {
    /// RGBA8 pixel data as a Uint8Array (transferred, zero-copy).
    #[wasm_bindgen(getter)]
    pub fn data(&self) -> js_sys::Uint8Array {
        js_sys::Uint8Array::from(self.data.as_slice())
    }

    /// Width in pixels.
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Total byte length (width * height * 4).
    #[wasm_bindgen(getter)]
    pub fn byte_length(&self) -> u32 {
        self.data.len() as u32
    }
}

#[wasm_bindgen]
impl WasmEditor {
    /// Create an editor from RGBA8 pixel data.
    ///
    /// `rgba_data` must be exactly `width * height * 4` bytes.
    /// `overview_max` and `detail_max` control the output dimensions
    /// of the overview and detail views.
    #[wasm_bindgen(constructor)]
    pub fn new(
        rgba_data: &[u8],
        width: u32,
        height: u32,
        overview_max: u32,
        detail_max: u32,
    ) -> Result<WasmEditor, JsError> {
        let expected = (width as usize) * (height as usize) * 4;
        if rgba_data.len() != expected {
            return Err(JsError::new(&format!(
                "Expected {expected} bytes for {width}x{height} RGBA8, got {}",
                rgba_data.len()
            )));
        }
        Ok(WasmEditor {
            inner: Editor::from_rgba(rgba_data.to_vec(), width, height, overview_max, detail_max),
        })
    }

    /// Source image width.
    #[wasm_bindgen(getter)]
    pub fn source_width(&self) -> u32 {
        self.inner.source_width()
    }

    /// Source image height.
    #[wasm_bindgen(getter)]
    pub fn source_height(&self) -> u32 {
        self.inner.source_height()
    }

    /// Render the overview (small resized image).
    ///
    /// `adjustments_json` is a JSON object mapping adjustment keys to values,
    /// e.g., `{"exposure": 0.5, "contrast": 0.3}`.
    #[wasm_bindgen]
    pub fn render_overview(&mut self, adjustments_json: &str) -> Result<WasmRenderResult, JsError> {
        let adj = parse_adjustments(adjustments_json)?;
        let out = self
            .inner
            .render_overview(&adj)
            .map_err(|e| JsError::new(&e))?;
        Ok(WasmRenderResult {
            data: out.data,
            width: out.width,
            height: out.height,
        })
    }

    /// Render a detail region at higher resolution.
    ///
    /// Region coordinates are normalized (0..1) relative to the source image.
    #[wasm_bindgen]
    pub fn render_region(
        &mut self,
        adjustments_json: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    ) -> Result<WasmRenderResult, JsError> {
        let adj = parse_adjustments(adjustments_json)?;
        let region = Region { x, y, w, h };
        let out = self
            .inner
            .render_region(&adj, &region)
            .map_err(|e| JsError::new(&e))?;
        Ok(WasmRenderResult {
            data: out.data,
            width: out.width,
            height: out.height,
        })
    }

    /// Get the filter node schema as a JSON string.
    ///
    /// Returns JSON Schema with `$defs` for all zenfilters nodes,
    /// including slider metadata (min, max, step, default, identity,
    /// section, group, unit).
    #[wasm_bindgen]
    pub fn get_filter_schema() -> String {
        crate::export_filter_schema()
    }

    /// Number of entries in the overview Session cache.
    #[wasm_bindgen(getter)]
    pub fn overview_cache_len(&self) -> usize {
        self.inner.overview_session.cache_len()
    }

    /// Number of entries in the detail Session cache.
    #[wasm_bindgen(getter)]
    pub fn detail_cache_len(&self) -> usize {
        self.inner.detail_session.cache_len()
    }
}

fn parse_adjustments(json: &str) -> Result<BTreeMap<String, f64>, JsError> {
    if json.is_empty() || json == "{}" {
        return Ok(BTreeMap::new());
    }
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| JsError::new(&format!("Invalid JSON: {e}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| JsError::new("Adjustments must be a JSON object"))?;
    let mut map = BTreeMap::new();
    for (key, val) in obj {
        if let Some(n) = val.as_f64() {
            map.insert(key.clone(), n);
        }
    }
    Ok(map)
}
