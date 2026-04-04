//! wasm-bindgen API for the demo editor.
//!
//! Thin wrapper over `zeneditor::EditorState` that exposes methods to
//! JavaScript with JS-friendly types (Uint8Array, JSON strings, numbers).

use wasm_bindgen::prelude::*;
use zeneditor::EditorState;

/// WASM-exposed image editor backed by zeneditor.
#[wasm_bindgen]
pub struct WasmEditor {
    inner: EditorState,
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
    #[wasm_bindgen(getter)]
    pub fn data(&self) -> js_sys::Uint8Array {
        js_sys::Uint8Array::from(self.data.as_slice())
    }

    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.height
    }

    #[wasm_bindgen(getter)]
    pub fn byte_length(&self) -> u32 {
        self.data.len() as u32
    }
}

#[wasm_bindgen]
impl WasmEncodeResult {
    #[wasm_bindgen(getter)]
    pub fn data(&self) -> js_sys::Uint8Array {
        js_sys::Uint8Array::from(self.data.as_slice())
    }

    #[wasm_bindgen(getter)]
    pub fn format(&self) -> String {
        self.format.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn mime(&self) -> String {
        self.mime.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn size(&self) -> u32 {
        self.data.len() as u32
    }

    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.height
    }
}

/// Result of a native decode upgrade: metadata summary for the UI.
#[wasm_bindgen]
pub struct WasmUpgradeResult {
    format: String,
    width: u32,
    height: u32,
    has_icc: bool,
    has_exif: bool,
    has_xmp: bool,
    has_gain_map: bool,
}

#[wasm_bindgen]
impl WasmUpgradeResult {
    #[wasm_bindgen(getter)]
    pub fn format(&self) -> String {
        self.format.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.width
    }
    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.height
    }
    #[wasm_bindgen(getter)]
    pub fn has_icc(&self) -> bool {
        self.has_icc
    }
    #[wasm_bindgen(getter)]
    pub fn has_exif(&self) -> bool {
        self.has_exif
    }
    #[wasm_bindgen(getter)]
    pub fn has_xmp(&self) -> bool {
        self.has_xmp
    }
    #[wasm_bindgen(getter)]
    pub fn has_gain_map(&self) -> bool {
        self.has_gain_map
    }
}

#[wasm_bindgen]
impl WasmEditor {
    /// Create an editor from RGBA8 pixel data.
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
        let mut inner = EditorState::new(overview_max, detail_max);
        inner.init_from_rgba(rgba_data.to_vec(), width, height);
        Ok(WasmEditor {
            inner,
            preset_thumbnails: Vec::new(),
        })
    }

    /// Upgrade the editor's source by decoding original image bytes natively.
    #[wasm_bindgen]
    pub fn upgrade_from_bytes(&mut self, bytes: &[u8]) -> Result<WasmUpgradeResult, JsError> {
        let decoded =
            crate::decode::decode_native(bytes).map_err(|e| JsError::new(&e))?;

        let has_icc = decoded.metadata.icc_profile.is_some();
        let has_exif = decoded.metadata.exif.is_some();
        let has_xmp = decoded.metadata.xmp.is_some();
        let has_gain_map = decoded.has_gain_map;
        let format_str = decoded.format.extension().to_string();
        let width = decoded.width;
        let height = decoded.height;

        self.inner.upgrade_source(
            decoded.data,
            decoded.width,
            decoded.height,
            decoded.metadata,
            decoded.format,
        );

        Ok(WasmUpgradeResult {
            format: format_str,
            width,
            height,
            has_icc,
            has_exif,
            has_xmp,
            has_gain_map,
        })
    }

    /// Whether the editor has metadata from native decode.
    #[wasm_bindgen(getter)]
    pub fn has_metadata(&self) -> bool {
        self.inner.metadata().is_some()
    }

    /// Source format detected by native decode.
    #[wasm_bindgen(getter)]
    pub fn source_format(&self) -> Option<String> {
        self.inner
            .source_format()
            .map(|f| f.extension().to_string())
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
    #[wasm_bindgen]
    pub fn render_overview(
        &mut self,
        adjustments_json: &str,
        film_preset: Option<String>,
    ) -> Result<WasmRenderResult, JsError> {
        apply_adjustments_json(&mut self.inner, adjustments_json, film_preset.as_deref())?;
        let out = self
            .inner
            .render_overview()
            .map_err(|e| JsError::new(&e))?;
        Ok(WasmRenderResult {
            data: out.data,
            width: out.width,
            height: out.height,
        })
    }

    /// Render a detail region at higher resolution.
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
        apply_adjustments_json(&mut self.inner, adjustments_json, film_preset.as_deref())?;
        self.inner.region.set(x, y, w, h);
        let out = self
            .inner
            .render_detail()
            .map_err(|e| JsError::new(&e))?;
        Ok(WasmRenderResult {
            data: out.data,
            width: out.width,
            height: out.height,
        })
    }

    /// Render all film preset thumbnails as a batch.
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
    #[wasm_bindgen]
    pub fn get_preset_thumbnail_data(&self, index: usize) -> Option<js_sys::Uint8Array> {
        self.preset_thumbnails
            .get(index)
            .map(|(_, data)| js_sys::Uint8Array::from(data.as_slice()))
    }

    /// List all available film preset IDs and names as JSON.
    #[wasm_bindgen]
    pub fn list_presets() -> String {
        let presets = EditorState::list_presets();
        let entries: Vec<serde_json::Value> = presets
            .into_iter()
            .map(|(id, name)| serde_json::json!({"id": id, "name": name}))
            .collect();
        serde_json::to_string(&entries).unwrap_or_else(|_| "[]".into())
    }

    /// Encode at overview size for inline preview in the export modal.
    #[wasm_bindgen]
    pub fn encode_preview(
        &mut self,
        adjustments_json: &str,
        format: &str,
        options_json: &str,
        film_preset: Option<String>,
    ) -> Result<WasmEncodeResult, JsError> {
        apply_adjustments_json(&mut self.inner, adjustments_json, film_preset.as_deref())?;
        let options = parse_options(options_json)?;

        let result = self
            .inner
            .encode_at_overview_size(format, &options)
            .map_err(|e| JsError::new(&e))?;

        Ok(WasmEncodeResult {
            data: result.data,
            format: format.to_string(),
            mime: result.mime.to_string(),
            width: result.width,
            height: result.height,
        })
    }

    /// Encode at full resolution for export/download.
    #[wasm_bindgen]
    pub fn encode_full(
        &mut self,
        adjustments_json: &str,
        width: u32,
        height: u32,
        format: &str,
        options_json: &str,
        film_preset: Option<String>,
    ) -> Result<WasmEncodeResult, JsError> {
        apply_adjustments_json(&mut self.inner, adjustments_json, film_preset.as_deref())?;
        let options = parse_options(options_json)?;

        let max_dim = if width > 0 { width.max(height) } else { 0 };
        let result = self
            .inner
            .encode_at_size(max_dim, format, &options)
            .map_err(|e| JsError::new(&e))?;

        Ok(WasmEncodeResult {
            data: result.data,
            format: format.to_string(),
            mime: result.mime.to_string(),
            width: result.width,
            height: result.height,
        })
    }

    /// Get the filter node schema as a JSON string.
    #[wasm_bindgen]
    pub fn get_filter_schema() -> String {
        crate::export_filter_schema()
    }

    /// Number of entries in the overview Session cache.
    #[wasm_bindgen(getter)]
    pub fn overview_cache_len(&self) -> usize {
        self.inner.overview_cache_len()
    }

    /// Number of entries in the detail Session cache.
    #[wasm_bindgen(getter)]
    pub fn detail_cache_len(&self) -> usize {
        self.inner.detail_cache_len()
    }
}

/// Apply a JSON adjustments string and optional film preset to the editor state.
///
/// Parses the JSON and updates the EditorState's AdjustmentModel to match.
/// This bridges the existing JS protocol (JSON strings) to zeneditor's typed API.
fn apply_adjustments_json(
    state: &mut EditorState,
    json: &str,
    film_preset: Option<&str>,
) -> Result<(), JsError> {
    // Parse the nested JSON format: {"zenfilters.exposure": {"stops": 1.5}, ...}
    let adj: std::collections::BTreeMap<String, serde_json::Value> = if json.is_empty() || json == "{}" {
        std::collections::BTreeMap::new()
    } else {
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| JsError::new(&format!("Invalid JSON: {e}")))?;
        let obj = value
            .as_object()
            .ok_or_else(|| JsError::new("Adjustments must be a JSON object"))?;
        obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    };

    // Flatten into the EditorState's AdjustmentModel.
    // For each node, set each param's adjust_key.
    for node in &state.schema.nodes {
        for p in &node.params {
            if let Some(node_params) = adj.get(&node.id) {
                if let Some(val) = node_params.get(&p.param_name) {
                    match p.kind {
                        zeneditor::model::schema::ParamKind::Number => {
                            if let Some(n) = val.as_f64() {
                                state.adjustments.set(&p.adjust_key, n);
                            }
                        }
                        zeneditor::model::schema::ParamKind::Boolean => {
                            if let Some(b) = val.as_bool() {
                                state.adjustments.set_bool(&p.adjust_key, b);
                            }
                        }
                        zeneditor::model::schema::ParamKind::ArrayElement => {
                            // Array elements: the JSON has the full array on the param_name
                            // but the adjust_key has the index
                            if let (Some(arr), Some(idx)) = (val.as_array(), p.array_index) {
                                if let Some(elem) = arr.get(idx).and_then(|v| v.as_f64()) {
                                    state.adjustments.set(&p.adjust_key, elem);
                                }
                            }
                        }
                    }
                } else {
                    // Param not in JSON — reset to identity
                    state.adjustments.set(&p.adjust_key, p.identity);
                }
            } else {
                // Node not in JSON — reset param to identity
                state.adjustments.set(&p.adjust_key, p.identity);
            }
        }
    }

    // Film preset
    state.adjustments.film_preset = film_preset.map(|s| s.to_string());
    // Extract intensity from adjustments if present
    if let Some(intensity) = adj
        .get("film_look_strength")
        .and_then(|v| v.as_f64())
    {
        state.adjustments.film_preset_intensity = intensity as f32;
    }

    Ok(())
}

fn parse_options(json: &str) -> Result<serde_json::Value, JsError> {
    if json.is_empty() || json == "{}" {
        Ok(serde_json::Value::Object(serde_json::Map::new()))
    } else {
        serde_json::from_str(json)
            .map_err(|e| JsError::new(&format!("Invalid options JSON: {e}")))
    }
}

/// Try to decode image bytes using WASM codecs.
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
    crate::decode::try_decode_check(bytes)
}
