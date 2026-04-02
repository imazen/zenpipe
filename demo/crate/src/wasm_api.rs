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
    /// Cached preset thumbnail pixel data from the last render_preset_thumbnails call.
    preset_thumbnails: Vec<(String, Vec<u8>)>,
}

/// Result of a render operation: RGBA pixels + dimensions.
#[wasm_bindgen]
pub struct WasmRenderResult {
    data: Vec<u8>,
    width: u32,
    height: u32,
}

/// Result of an encode operation: encoded bytes + format metadata.
#[wasm_bindgen]
pub struct WasmEncodeResult {
    data: Vec<u8>,
    format: String,
    mime: String,
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
impl WasmEncodeResult {
    /// Encoded bytes as a Uint8Array (transferred, zero-copy).
    #[wasm_bindgen(getter)]
    pub fn data(&self) -> js_sys::Uint8Array {
        js_sys::Uint8Array::from(self.data.as_slice())
    }

    /// Output format name (e.g., "jpeg", "webp", "png", "gif").
    #[wasm_bindgen(getter)]
    pub fn format(&self) -> String {
        self.format.clone()
    }

    /// MIME type of the encoded output.
    #[wasm_bindgen(getter)]
    pub fn mime(&self) -> String {
        self.mime.clone()
    }

    /// Encoded byte count.
    #[wasm_bindgen(getter)]
    pub fn size(&self) -> u32 {
        self.data.len() as u32
    }

    /// Output width in pixels.
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Output height in pixels.
    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.height
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
            preset_thumbnails: Vec::new(),
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
    /// `film_preset` is an optional film look preset ID (e.g., "portra", "velvia").
    #[wasm_bindgen]
    pub fn render_overview(
        &mut self,
        adjustments_json: &str,
        film_preset: Option<String>,
    ) -> Result<WasmRenderResult, JsError> {
        let adj = parse_adjustments(adjustments_json)?;
        let out = self
            .inner
            .render_overview_with_preset(&adj, film_preset.as_deref())
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
        film_preset: Option<String>,
    ) -> Result<WasmRenderResult, JsError> {
        let adj = parse_adjustments(adjustments_json)?;
        let region = Region { x, y, w, h };
        let out = self
            .inner
            .render_region_with_preset(&adj, &region, film_preset.as_deref())
            .map_err(|e| JsError::new(&e))?;
        Ok(WasmRenderResult {
            data: out.data,
            width: out.width,
            height: out.height,
        })
    }

    /// Render all film preset thumbnails as a batch.
    ///
    /// Returns a JSON string: `[{"id": "portra", "name": "Portra", "width": 48, "height": 36}, ...]`
    /// The RGBA pixel data for each is returned separately via `get_preset_thumbnail_data`.
    #[wasm_bindgen]
    pub fn render_preset_thumbnails(&mut self, thumb_size: u32) -> Result<String, JsError> {
        let results = self.inner.render_preset_thumbnails(thumb_size);
        let mut entries = Vec::new();
        self.preset_thumbnails.clear();

        for (id, name, result) in results {
            match result {
                Ok(out) => {
                    entries.push(serde_json::json!({
                        "id": id,
                        "name": name,
                        "width": out.width,
                        "height": out.height,
                    }));
                    self.preset_thumbnails.push((id, out.data));
                }
                Err(e) => {
                    entries.push(serde_json::json!({
                        "id": id,
                        "name": name,
                        "error": e,
                    }));
                }
            }
        }

        Ok(serde_json::to_string(&entries).unwrap_or_else(|_| "[]".into()))
    }

    /// Get the RGBA pixel data for a specific preset thumbnail by index.
    ///
    /// Call after `render_preset_thumbnails`. Index matches the JSON array order.
    #[wasm_bindgen]
    pub fn get_preset_thumbnail_data(&self, index: usize) -> Option<js_sys::Uint8Array> {
        self.preset_thumbnails
            .get(index)
            .map(|(_, data)| js_sys::Uint8Array::from(data.as_slice()))
    }

    /// List all available film preset IDs and names as JSON.
    ///
    /// Returns `[{"id": "portra", "name": "Portra"}, ...]`
    #[wasm_bindgen]
    pub fn list_presets() -> String {
        let presets = crate::editor::Editor::list_presets();
        let entries: Vec<serde_json::Value> = presets
            .into_iter()
            .map(|(id, name)| serde_json::json!({"id": id, "name": name}))
            .collect();
        serde_json::to_string(&entries).unwrap_or_else(|_| "[]".into())
    }

    /// Render and encode an image to a specific format.
    ///
    /// Renders at the given `width`/`height` (using the larger dimension as
    /// the constrain limit), applies filters and optional film preset, then
    /// encodes to the requested format (jpeg, webp, png, gif).
    ///
    /// `options_json` is a JSON object with format-specific settings
    /// (e.g., `{"quality": 85}` for JPEG/WebP, `{"effort": 5}` for PNG).
    ///
    /// Returns encoded bytes with format metadata. For unsupported formats
    /// (avif, jxl), returns an error — use browser-native encoding instead.
    #[wasm_bindgen]
    pub fn encode_image(
        &mut self,
        adjustments_json: &str,
        width: u32,
        height: u32,
        format: &str,
        options_json: &str,
        film_preset: Option<String>,
    ) -> Result<WasmEncodeResult, JsError> {
        let adj = parse_adjustments(adjustments_json)?;

        // Render at requested dimensions (0 = source size).
        let max_dim = if width > 0 { width.max(height) } else { 0 };
        let out = self
            .inner
            .render_at_size(&adj, max_dim, film_preset.as_deref())
            .map_err(|e| JsError::new(&e))?;

        // Parse encode options.
        let options: serde_json::Value = if options_json.is_empty() || options_json == "{}" {
            serde_json::Value::Object(serde_json::Map::new())
        } else {
            serde_json::from_str(options_json)
                .map_err(|e| JsError::new(&format!("Invalid options JSON: {e}")))?
        };

        // Encode the rendered RGBA pixels.
        let encoded = crate::encode::encode(&out.data, out.width, out.height, format, &options)
            .map_err(|e| JsError::new(&e))?;

        Ok(WasmEncodeResult {
            data: encoded.data,
            format: encoded.format.to_string(),
            mime: encoded.mime.to_string(),
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

fn parse_adjustments(json: &str) -> Result<BTreeMap<String, serde_json::Value>, JsError> {
    if json.is_empty() || json == "{}" {
        return Ok(BTreeMap::new());
    }
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| JsError::new(&format!("Invalid JSON: {e}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| JsError::new("Adjustments must be a JSON object"))?;
    Ok(obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

/// Try to decode image bytes using WASM codecs (JXL, AVIF).
///
/// Returns null if the format is not recognized or supported.
/// For JPEG/PNG/WebP/GIF, the browser's native decoder is faster.
#[wasm_bindgen]
pub fn wasm_decode_image(bytes: &[u8]) -> Option<WasmRenderResult> {
    let decoded = crate::decode::try_decode(bytes)?;
    Some(WasmRenderResult {
        data: decoded.data,
        width: decoded.width,
        height: decoded.height,
    })
}

/// Check if WASM can decode a format that the browser might not support.
#[wasm_bindgen]
pub fn wasm_can_decode(bytes: &[u8]) -> bool {
    if bytes.len() < 12 {
        return false;
    }
    // JXL
    if bytes[0] == 0xFF && bytes[1] == 0x0A {
        return true;
    }
    if bytes[..12]
        == [
            0, 0, 0, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
        ]
    {
        return true;
    }
    // AVIF
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        if brand == b"avif" || brand == b"avis" || brand == b"mif1" {
            return true;
        }
    }
    false
}
