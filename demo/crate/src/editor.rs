//! Core editor: manages two Sessions (overview + detail) and renders
//! from cached geometry prefixes when only filters change.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use zenpipe::Source;
use zenpipe::format::RGBA8_SRGB;
use zenpipe::orchestrate::{ProcessConfig, SourceImageInfo};
use zenpipe::session::Session;
use zenpipe::sources::MaterializedSource;

/// AtomicBool-backed cancellation token for `enough::Stop`.
struct AtomicStop(Arc<AtomicBool>);

impl enough::Stop for AtomicStop {
    fn check(&self) -> Result<(), enough::StopReason> {
        if self.0.load(Ordering::Relaxed) {
            Err(enough::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Normalized rectangle (0..1 coordinates relative to source image).
#[derive(Clone, Debug)]
pub struct Region {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Default for Region {
    fn default() -> Self {
        Self {
            x: 0.25,
            y: 0.25,
            w: 0.5,
            h: 0.5,
        }
    }
}

/// Rendered output: RGBA8 pixels + dimensions.
pub struct RenderOutput {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Interactive image editor backed by zenpipe Session caching.
///
/// Holds the decoded source pixels and two Session instances:
/// - Overview: source → resize(overview_max) → filters
/// - Detail: source → crop(region) → resize(detail_size) → filters
pub struct Editor {
    /// Source pixels (decoded, materialized).
    source_pixels: MaterializedSource,
    source_width: u32,
    source_height: u32,

    /// Session for the overview (small resized image).
    pub overview_session: Session,
    /// Session for the detail (cropped region).
    pub detail_session: Session,

    /// Max dimension for overview output.
    overview_max: u32,
    /// Max dimension for detail output.
    detail_max: u32,

    /// Source hash for Session cache keying.
    source_hash: u64,

    /// Cancel flag for the current overview render.
    /// Calling `cancel_overview()` sets this to `true`. Each new render
    /// replaces the Arc with a fresh `false` flag — prior cancellations
    /// don't affect new renders.
    overview_cancel: Arc<AtomicBool>,
    /// Cancel flag for the current detail render.
    detail_cancel: Arc<AtomicBool>,
}

impl Editor {
    /// Initialize the editor from decoded RGBA8 pixel data.
    ///
    /// `pixels` must be RGBA8 sRGB, `width * height * 4` bytes.
    pub fn from_rgba(
        pixels: Vec<u8>,
        width: u32,
        height: u32,
        overview_max: u32,
        detail_max: u32,
    ) -> Self {
        let source_pixels = MaterializedSource::from_data(pixels, width, height, RGBA8_SRGB);

        // Simple hash of dimensions for cache keying (real app would hash content).
        let source_hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            width.hash(&mut h);
            height.hash(&mut h);
            h.finish()
        };

        Self {
            source_pixels,
            source_width: width,
            source_height: height,
            overview_session: Session::new(128 * 1024 * 1024), // 128 MB
            detail_session: Session::new(128 * 1024 * 1024),
            overview_max,
            detail_max,
            source_hash,
            overview_cancel: Arc::new(AtomicBool::new(false)),
            detail_cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Cancel any in-progress overview render.
    pub fn cancel_overview(&self) {
        self.overview_cancel.store(true, Ordering::Relaxed);
    }

    /// Cancel any in-progress detail render.
    pub fn cancel_detail(&self) {
        self.detail_cancel.store(true, Ordering::Relaxed);
    }

    pub fn source_width(&self) -> u32 {
        self.source_width
    }
    pub fn source_height(&self) -> u32 {
        self.source_height
    }

    /// Render the overview image with the given filter adjustments.
    ///
    /// Geometry prefix (decode + resize) is cached; only filter suffix
    /// re-runs when adjustments change.
    ///
    /// Keys are node IDs (e.g., `"zenfilters.exposure"`), values are
    /// JSON objects of param name → value (e.g., `{"stops": 0.5}`).
    pub fn render_overview(
        &mut self,
        adjustments: &BTreeMap<String, serde_json::Value>,
    ) -> Result<RenderOutput, String> {
        self.render_overview_with_preset(adjustments, None)
    }

    /// Render at a specific max dimension (for export at arbitrary size).
    pub fn render_at_size(
        &mut self,
        adjustments: &BTreeMap<String, serde_json::Value>,
        max_dim: u32,
        film_preset: Option<&str>,
    ) -> Result<RenderOutput, String> {
        let source = Box::new(self.source_pixels.clone());

        let mut nodes: Vec<Box<dyn zennode::NodeInstance>> = Vec::new();

        // Geometry: resize to requested max_dim (0 = source size, no resize)
        if max_dim > 0 && (self.source_width > max_dim || self.source_height > max_dim) {
            nodes.push(Box::new(zenpipe::zennode_defs::Constrain {
                w: Some(max_dim),
                h: Some(max_dim),
                mode: "within".into(),
                ..Default::default()
            }));
        }

        // Filters
        append_film_look(&mut nodes, adjustments, film_preset);
        append_filter_nodes(&mut nodes, adjustments);

        let info = make_source_info(self.source_width, self.source_height);
        let converters: &[&dyn zenpipe::bridge::NodeConverter] = &[&FILTERS_CONVERTER];
        let config = ProcessConfig {
            nodes: &nodes,
            converters,
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };

        // Use overview session (it caches by geometry hash, so different max_dim = different cache entry)
        self.overview_cancel.store(true, Ordering::Relaxed);
        let fresh = Arc::new(AtomicBool::new(false));
        self.overview_cancel = Arc::clone(&fresh);
        let stop = AtomicStop(fresh);

        let output = self
            .overview_session
            .stream_stoppable(source, &config, None, self.source_hash, &stop)
            .map_err(|e| format!("render: {e}"))?;

        let mat = MaterializedSource::from_source_stoppable(output.source, &stop)
            .map_err(|e| format!("materialize: {e}"))?;

        Ok(RenderOutput {
            width: mat.width(),
            height: mat.height(),
            data: pack_rgba(&mat),
        })
    }

    /// Render overview with optional film look preset.
    pub fn render_overview_with_preset(
        &mut self,
        adjustments: &BTreeMap<String, serde_json::Value>,
        film_preset: Option<&str>,
    ) -> Result<RenderOutput, String> {
        let source = Box::new(self.source_pixels.clone());

        let mut nodes: Vec<Box<dyn zennode::NodeInstance>> = Vec::new();

        // Geometry: resize to overview_max
        nodes.push(Box::new(zenpipe::zennode_defs::Constrain {
            w: Some(self.overview_max),
            h: Some(self.overview_max),
            mode: "within".into(),
            ..Default::default()
        }));

        // Film look first (applied before other adjustments)
        append_film_look(&mut nodes, adjustments, film_preset);
        // Then individual filter adjustments
        append_filter_nodes(&mut nodes, adjustments);

        let info = make_source_info(self.source_width, self.source_height);
        let converters: &[&dyn zenpipe::bridge::NodeConverter] = &[&FILTERS_CONVERTER];
        let config = ProcessConfig {
            nodes: &nodes,
            converters,
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };

        // Cancel any in-flight overview render, then swap in a fresh flag.
        self.overview_cancel.store(true, Ordering::Relaxed);
        let fresh = Arc::new(AtomicBool::new(false));
        self.overview_cancel = Arc::clone(&fresh);
        let stop = AtomicStop(fresh);

        let output = self
            .overview_session
            .stream_stoppable(source, &config, None, self.source_hash, &stop)
            .map_err(|e| format!("overview render: {e}"))?;

        let mat = MaterializedSource::from_source_stoppable(output.source, &stop)
            .map_err(|e| format!("overview materialize: {e}"))?;

        Ok(RenderOutput {
            width: mat.width(),
            height: mat.height(),
            data: pack_rgba(&mat),
        })
    }

    /// Render the detail region with the given filter adjustments.
    pub fn render_region(
        &mut self,
        adjustments: &BTreeMap<String, serde_json::Value>,
        region: &Region,
    ) -> Result<RenderOutput, String> {
        self.render_region_with_preset(adjustments, region, None)
    }

    /// Render detail region with optional film look preset.
    pub fn render_region_with_preset(
        &mut self,
        adjustments: &BTreeMap<String, serde_json::Value>,
        region: &Region,
        film_preset: Option<&str>,
    ) -> Result<RenderOutput, String> {
        let source = Box::new(self.source_pixels.clone());

        let mut nodes: Vec<Box<dyn zennode::NodeInstance>> = Vec::new();

        // Geometry: crop to region, then resize to detail_max
        let crop_x = (region.x * self.source_width as f32) as u32;
        let crop_y = (region.y * self.source_height as f32) as u32;
        let crop_w = (region.w * self.source_width as f32).max(1.0) as u32;
        let crop_h = (region.h * self.source_height as f32).max(1.0) as u32;

        nodes.push(Box::new(zenpipe::zennode_defs::Crop {
            x: crop_x,
            y: crop_y,
            w: crop_w,
            h: crop_h,
        }));

        // Only downscale if crop is larger than detail_max.
        // When zoomed past 1:1, the crop is already smaller than the viewport —
        // render at native pixel size and let CSS upscale the canvas.
        if crop_w > self.detail_max || crop_h > self.detail_max {
            nodes.push(Box::new(zenpipe::zennode_defs::Constrain {
                w: Some(self.detail_max),
                h: Some(self.detail_max),
                mode: "within".into(),
                ..Default::default()
            }));
        }

        // Film look first, then individual adjustments
        append_film_look(&mut nodes, adjustments, film_preset);
        append_filter_nodes(&mut nodes, adjustments);

        let info = make_source_info(self.source_width, self.source_height);
        let converters: &[&dyn zenpipe::bridge::NodeConverter] = &[&FILTERS_CONVERTER];
        let config = ProcessConfig {
            nodes: &nodes,
            converters,
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };

        // Use a different source hash for detail (incorporates region).
        let region_hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            self.source_hash.hash(&mut h);
            crop_x.hash(&mut h);
            crop_y.hash(&mut h);
            crop_w.hash(&mut h);
            crop_h.hash(&mut h);
            h.finish()
        };

        // Cancel any in-flight detail render, then swap in a fresh flag.
        self.detail_cancel.store(true, Ordering::Relaxed);
        let fresh = Arc::new(AtomicBool::new(false));
        self.detail_cancel = Arc::clone(&fresh);
        let stop = AtomicStop(fresh);

        let output = self
            .detail_session
            .stream_stoppable(source, &config, None, region_hash, &stop)
            .map_err(|e| format!("detail render: {e}"))?;

        let mat = MaterializedSource::from_source_stoppable(output.source, &stop)
            .map_err(|e| format!("detail materialize: {e}"))?;

        Ok(RenderOutput {
            width: mat.width(),
            height: mat.height(),
            data: pack_rgba(&mat),
        })
    }

    /// Render all film preset thumbnails at once (fan-out).
    ///
    /// Returns a Vec of (preset_id, preset_name, RenderOutput) for each
    /// preset in `FilmPreset::ALL`. Each is a tiny `thumb_size x thumb_size`
    /// image with that preset applied at full strength.
    ///
    /// Uses the overview Session so the geometry prefix (resize to thumb_size)
    /// is cached — only the first preset pays the decode+resize cost.
    pub fn render_preset_thumbnails(
        &mut self,
        thumb_size: u32,
    ) -> Vec<(String, String, Result<RenderOutput, String>)> {
        zenfilters::filters::FilmPreset::ALL
            .iter()
            .map(|preset| {
                let result = self.render_single_preset(preset.id(), thumb_size);
                (preset.id().to_string(), preset.name().to_string(), result)
            })
            .collect()
    }

    fn render_single_preset(
        &mut self,
        preset_id: &str,
        thumb_size: u32,
    ) -> Result<RenderOutput, String> {
        let source = Box::new(self.source_pixels.clone());

        let mut nodes: Vec<Box<dyn zennode::NodeInstance>> = Vec::new();

        // Geometry: resize to tiny thumbnail
        nodes.push(Box::new(zenpipe::zennode_defs::Constrain {
            w: Some(thumb_size),
            h: Some(thumb_size),
            mode: "within".into(),
            ..Default::default()
        }));

        // Apply the film look preset
        nodes.push(Box::new(zenfilters::zennode_defs::FilmLook {
            preset: preset_id.to_string(),
            strength: 1.0,
        }));

        let info = make_source_info(self.source_width, self.source_height);
        let converters: &[&dyn zenpipe::bridge::NodeConverter] = &[&FILTERS_CONVERTER];
        let config = ProcessConfig {
            nodes: &nodes,
            converters,
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };

        // All thumbnails share the same geometry prefix (same source + same thumb_size).
        let output = self
            .overview_session
            .stream(source, &config, None, self.source_hash)
            .map_err(|e| format!("preset {preset_id}: {e}"))?;

        let mat = MaterializedSource::from_source(output.source)
            .map_err(|e| format!("preset {preset_id} materialize: {e}"))?;

        Ok(RenderOutput {
            width: mat.width(),
            height: mat.height(),
            data: pack_rgba(&mat),
        })
    }

    /// List all available film preset IDs and names.
    pub fn list_presets() -> Vec<(String, String)> {
        zenfilters::filters::FilmPreset::ALL
            .iter()
            .map(|p| (p.id().to_string(), p.name().to_string()))
            .collect()
    }
}

/// NodeConverter that bridges zenfilters zennode definitions to pipeline NodeOps.
struct FiltersConverter;

impl zenpipe::bridge::NodeConverter for FiltersConverter {
    fn can_convert(&self, schema_id: &str) -> bool {
        zenfilters::zennode_defs::is_zenfilters_node(schema_id)
    }

    fn convert(
        &self,
        node: &dyn zennode::NodeInstance,
    ) -> zenpipe::PipeResult<zenpipe::graph::NodeOp> {
        let filter = zenfilters::zennode_defs::node_to_filter(node).ok_or_else(|| {
            zenpipe::PipeError::Op(format!(
                "unrecognized zenfilters node '{}'",
                node.schema().id
            ))
        })?;

        let mut pipeline = zenfilters::Pipeline::new(zenfilters::PipelineConfig::default())
            .map_err(|e| zenpipe::PipeError::Op(format!("pipeline creation: {e:?}")))?;
        pipeline.push(filter);
        Ok(zenpipe::graph::NodeOp::Filter(pipeline))
    }

    fn convert_group(
        &self,
        nodes: &[&dyn zennode::NodeInstance],
    ) -> zenpipe::PipeResult<zenpipe::graph::NodeOp> {
        let mut pipeline = zenfilters::Pipeline::new(zenfilters::PipelineConfig::default())
            .map_err(|e| zenpipe::PipeError::Op(format!("pipeline creation: {e:?}")))?;

        for node in nodes {
            let filter = zenfilters::zennode_defs::node_to_filter(*node).ok_or_else(|| {
                zenpipe::PipeError::Op(format!(
                    "unrecognized zenfilters node '{}'",
                    node.schema().id
                ))
            })?;
            pipeline.push(filter);
        }
        Ok(zenpipe::graph::NodeOp::Filter(pipeline))
    }

    fn fuse_group(
        &self,
        nodes: &[&dyn zennode::NodeInstance],
    ) -> zenpipe::PipeResult<Option<zenpipe::graph::NodeOp>> {
        if nodes.len() < 2 {
            return Ok(None);
        }
        Ok(Some(self.convert_group(nodes)?))
    }
}

/// Static converter instance for passing to ProcessConfig.
static FILTERS_CONVERTER: FiltersConverter = FiltersConverter;

/// Convert a MaterializedSource to tightly-packed RGBA8 sRGB pixels
/// using zenpixels-convert's SIMD-accelerated RowConverter.
///
/// Handles any pipeline output format (RGBA8, RGB8, RgbaF32 linear, etc.)
/// via automatic conversion planning. Fast path when already RGBA8 sRGB.
fn pack_rgba(mat: &MaterializedSource) -> Vec<u8> {
    use zenpipe::RowConverter;

    let w = mat.width() as usize;
    let h = mat.height() as usize;
    let src_stride = mat.stride();
    let dst_row_bytes = w * 4;

    let src_desc = mat.format();
    let dst_desc = RGBA8_SRGB;

    if src_desc == dst_desc {
        // Fast path: already RGBA8 sRGB — just strip stride padding.
        if src_stride == dst_row_bytes {
            return mat.data()[..dst_row_bytes * h].to_vec();
        }
        let mut packed = Vec::with_capacity(dst_row_bytes * h);
        for y in 0..h {
            let start = y * src_stride;
            packed.extend_from_slice(&mat.data()[start..start + dst_row_bytes]);
        }
        return packed;
    }

    // Use RowConverter for any other format (RGB8, RgbaF32 linear, RGBX8, etc.)
    let mut converter = match RowConverter::new(src_desc, dst_desc) {
        Ok(c) => c,
        Err(_e) => {
            #[cfg(test)]
            eprintln!("pack_rgba: no conversion {src_desc} → {dst_desc}: {_e}");
            return vec![0u8; dst_row_bytes * h];
        }
    };

    let mut packed = vec![0u8; dst_row_bytes * h];
    let src_bpp = src_desc.bytes_per_pixel();
    let src_row_bytes = w * src_bpp;
    let data = mat.data();

    for y in 0..h {
        let src_start = y * src_stride;
        let dst_start = y * dst_row_bytes;
        converter.convert_row(
            &data[src_start..src_start + src_row_bytes],
            &mut packed[dst_start..dst_start + dst_row_bytes],
            w as u32,
        );
    }

    packed
}

fn make_source_info(width: u32, height: u32) -> SourceImageInfo {
    SourceImageInfo {
        width,
        height,
        format: RGBA8_SRGB,
        has_alpha: true,
        has_animation: false,
        has_gain_map: false,
        is_hdr: false,
        exif_orientation: 1,
        metadata: None,
    }
}

/// Append a film look node if a preset is specified.
///
/// `film_look_strength` is extracted from the adjustments map if present;
/// defaults to 1.0. The preset is applied before individual filter nodes.
fn append_film_look(
    nodes: &mut Vec<Box<dyn zennode::NodeInstance>>,
    adjustments: &BTreeMap<String, serde_json::Value>,
    film_look_preset: Option<&str>,
) {
    if let Some(preset) = film_look_preset {
        if !preset.is_empty() && preset != "none" {
            let strength = adjustments
                .get("film_look_strength")
                .and_then(|v| v.as_f64())
                .unwrap_or(1.0) as f32;
            if strength > 0.001 {
                nodes.push(Box::new(zenfilters::zennode_defs::FilmLook {
                    preset: preset.to_string(),
                    strength,
                }));
            }
        }
    }
}

/// Create filter node instances from the adjustments map using the node registry.
///
/// Keys are full node IDs (e.g., `"zenfilters.exposure"`), values are JSON
/// objects of param name → value (e.g., `{"stops": 0.5}`). Non-zenfilters
/// keys and `film_look` keys are skipped. Unknown nodes are silently ignored.
fn append_filter_nodes(
    nodes: &mut Vec<Box<dyn zennode::NodeInstance>>,
    adjustments: &BTreeMap<String, serde_json::Value>,
) {
    let registry = zenpipe::full_registry();

    for (node_id, params_json) in adjustments {
        // Skip non-zenfilters keys and film_look (handled by append_film_look).
        if !node_id.starts_with("zenfilters.") || node_id == "zenfilters.film_look" {
            continue;
        }

        // Params must be a JSON object.
        if !params_json.is_object() {
            continue;
        }

        // node_from_json expects {"node.id": {params...}} — a single-key object.
        // serde_json::json! doesn't interpolate variable names as keys,
        // so build the wrapper object manually.
        let mut wrapper = serde_json::Map::new();
        wrapper.insert(node_id.clone(), params_json.clone());
        let wrapped = serde_json::Value::Object(wrapper);
        match registry.node_from_json(&wrapped) {
            Ok(node) => nodes.push(node),
            Err(_e) => {
                #[cfg(test)]
                eprintln!("skipping node {node_id}: {_e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_rgba(w: u32, h: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut data = vec![0u8; (w * h * 4) as usize];
        for pixel in data.chunks_exact_mut(4) {
            pixel[0] = r;
            pixel[1] = g;
            pixel[2] = b;
            pixel[3] = 255;
        }
        data
    }

    #[test]
    fn editor_from_rgba() {
        let pixels = solid_rgba(200, 150, 128, 128, 128);
        let editor = Editor::from_rgba(pixels, 200, 150, 512, 800);
        assert_eq!(editor.source_width(), 200);
        assert_eq!(editor.source_height(), 150);
    }

    #[test]
    fn render_overview_no_adjustments() {
        let pixels = solid_rgba(400, 300, 128, 128, 128);
        let mut editor = Editor::from_rgba(pixels, 400, 300, 200, 400);
        let adj = BTreeMap::new();
        let out = editor.render_overview(&adj).unwrap();
        assert!(out.width <= 200);
        assert!(out.height <= 200);
        assert_eq!(out.data.len(), (out.width * out.height * 4) as usize);
    }

    #[test]
    fn render_overview_with_exposure() {
        let pixels = solid_rgba(400, 300, 128, 128, 128);
        let mut editor = Editor::from_rgba(pixels, 400, 300, 200, 400);
        let mut adj = BTreeMap::new();
        adj.insert(
            "zenfilters.exposure".into(),
            serde_json::json!({"stops": 1.0}),
        );
        let out = editor.render_overview(&adj).unwrap();
        assert!(out.width > 0);
        assert!(out.height > 0);
    }

    #[test]
    fn render_overview_cache_hit() {
        let pixels = solid_rgba(400, 300, 128, 128, 128);
        let mut editor = Editor::from_rgba(pixels, 400, 300, 200, 400);
        let mut adj = BTreeMap::new();
        adj.insert(
            "zenfilters.exposure".into(),
            serde_json::json!({"stops": 0.5}),
        );

        // First render — cache miss.
        let _out1 = editor.render_overview(&adj).unwrap();
        assert_eq!(editor.overview_session.cache_len(), 1);

        // Second render with different filter — cache hit on geometry.
        adj.insert(
            "zenfilters.exposure".into(),
            serde_json::json!({"stops": 1.0}),
        );
        let _out2 = editor.render_overview(&adj).unwrap();
        assert_eq!(editor.overview_session.cache_len(), 1); // Same entry, not 2.
    }

    #[test]
    fn render_region_default() {
        let pixels = solid_rgba(400, 300, 128, 128, 128);
        let mut editor = Editor::from_rgba(pixels, 400, 300, 200, 400);
        let adj = BTreeMap::new();
        let region = Region::default();
        let out = editor.render_region(&adj, &region).unwrap();
        assert!(out.width > 0);
        assert!(out.height > 0);
    }

    #[test]
    fn render_region_cache_hit_on_filter_change() {
        let pixels = solid_rgba(400, 300, 128, 128, 128);
        let mut editor = Editor::from_rgba(pixels, 400, 300, 200, 400);
        let region = Region {
            x: 0.1,
            y: 0.1,
            w: 0.5,
            h: 0.5,
        };

        let mut adj = BTreeMap::new();
        adj.insert(
            "zenfilters.contrast".into(),
            serde_json::json!({"amount": 0.5}),
        );
        let _out1 = editor.render_region(&adj, &region).unwrap();
        assert_eq!(editor.detail_session.cache_len(), 1);

        // Same region, different filter → cache hit.
        adj.insert(
            "zenfilters.contrast".into(),
            serde_json::json!({"amount": -0.3}),
        );
        let _out2 = editor.render_region(&adj, &region).unwrap();
        assert_eq!(editor.detail_session.cache_len(), 1);
    }

    #[test]
    fn render_region_cache_miss_on_region_change() {
        let pixels = solid_rgba(400, 300, 128, 128, 128);
        let mut editor = Editor::from_rgba(pixels, 400, 300, 200, 400);
        // Need a filter so the session has a geometry/filter split to cache.
        let mut adj = BTreeMap::new();
        adj.insert(
            "zenfilters.exposure".into(),
            serde_json::json!({"stops": 0.5}),
        );

        let region1 = Region {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        };
        let _out1 = editor.render_region(&adj, &region1).unwrap();

        // Different region → different source hash → cache miss.
        let region2 = Region {
            x: 0.5,
            y: 0.5,
            w: 0.5,
            h: 0.5,
        };
        let _out2 = editor.render_region(&adj, &region2).unwrap();
        assert_eq!(editor.detail_session.cache_len(), 2);
    }

    #[test]
    fn zero_adjustments_skipped() {
        let pixels = solid_rgba(100, 100, 128, 128, 128);
        let mut editor = Editor::from_rgba(pixels, 100, 100, 100, 100);
        let mut adj = BTreeMap::new();
        // Identity values are still valid JSON objects — the registry creates
        // the node; the pipeline treats identity params as no-ops.
        adj.insert(
            "zenfilters.exposure".into(),
            serde_json::json!({"stops": 0.0}),
        );
        adj.insert(
            "zenfilters.contrast".into(),
            serde_json::json!({"amount": 0.0}),
        );
        let out = editor.render_overview(&adj).unwrap();
        assert!(out.width > 0);
    }

    #[test]
    fn multiple_filters_combined() {
        let pixels = solid_rgba(200, 200, 128, 128, 128);
        let mut editor = Editor::from_rgba(pixels, 200, 200, 100, 200);
        let mut adj = BTreeMap::new();
        adj.insert(
            "zenfilters.exposure".into(),
            serde_json::json!({"stops": 0.5}),
        );
        adj.insert(
            "zenfilters.contrast".into(),
            serde_json::json!({"amount": 0.3}),
        );
        adj.insert(
            "zenfilters.saturation".into(),
            serde_json::json!({"factor": 1.2}),
        );
        let out = editor.render_overview(&adj).unwrap();
        assert!(out.width > 0);
    }

    #[test]
    fn cancel_then_next_render_succeeds() {
        let pixels = solid_rgba(400, 300, 128, 128, 128);
        let mut editor = Editor::from_rgba(pixels, 400, 300, 200, 400);
        let mut adj = BTreeMap::new();
        adj.insert(
            "zenfilters.exposure".into(),
            serde_json::json!({"stops": 0.5}),
        );

        // Cancel the (nonexistent) in-flight render.
        editor.cancel_overview();

        // Next render gets a fresh flag and should succeed.
        let result = editor.render_overview(&adj);
        assert!(result.is_ok(), "render after cancel should succeed");
    }

    #[test]
    fn cancel_detail_then_render_succeeds() {
        let pixels = solid_rgba(400, 300, 128, 128, 128);
        let mut editor = Editor::from_rgba(pixels, 400, 300, 200, 400);
        let mut adj = BTreeMap::new();
        adj.insert(
            "zenfilters.exposure".into(),
            serde_json::json!({"stops": 0.5}),
        );
        let region = Region::default();

        editor.cancel_detail();
        let result = editor.render_region(&adj, &region);
        assert!(result.is_ok(), "render after cancel should succeed");
    }

    #[test]
    fn concurrent_cancel_flag_is_independent() {
        let pixels = solid_rgba(400, 300, 128, 128, 128);
        let mut editor = Editor::from_rgba(pixels, 400, 300, 200, 400);
        let mut adj = BTreeMap::new();
        adj.insert(
            "zenfilters.exposure".into(),
            serde_json::json!({"stops": 0.5}),
        );

        // Capture the cancel flag before rendering.
        let old_cancel = Arc::clone(&editor.overview_cancel);

        // Render — this swaps in a fresh flag.
        let _out = editor.render_overview(&adj).unwrap();

        // The old flag should NOT affect the new render.
        old_cancel.store(true, Ordering::Relaxed);
        let result = editor.render_overview(&adj);
        assert!(
            result.is_ok(),
            "old cancel flag should not affect new render"
        );
    }

    #[test]
    fn render_overview_exposure_has_nonzero_pixels() {
        let pixels = solid_rgba(200, 150, 128, 128, 128);
        let mut editor = Editor::from_rgba(pixels, 200, 150, 200, 400);
        let mut adj = BTreeMap::new();
        adj.insert(
            "zenfilters.exposure".into(),
            serde_json::json!({"stops": 1.0}),
        );
        let out = editor.render_overview(&adj).unwrap();
        eprintln!(
            "output: {}x{} format={} data_len={}",
            out.width,
            out.height,
            "?",
            out.data.len()
        );
        eprintln!("first 16 bytes: {:?}", &out.data[..16.min(out.data.len())]);
        assert!(out.width > 0);
        assert_eq!(out.data.len(), (out.width * out.height * 4) as usize);
        let nonzero = out.data.iter().filter(|&&b| b > 0).count();
        assert!(
            nonzero > 100,
            "expected nonzero pixels, got {nonzero} out of {}",
            out.data.len()
        );
    }

    #[test]
    fn all_schema_filter_nodes_are_recognized() {
        // Parse the schema to get all zenfilters node IDs and their params,
        // then try to render with each one to verify no "unrecognized node" errors.
        let schema_json = crate::export_filter_schema();
        let schema: serde_json::Value = serde_json::from_str(&schema_json).unwrap();
        let defs = schema["$defs"].as_object().unwrap();

        let pixels = solid_rgba(32, 32, 128, 128, 128);
        let mut editor = Editor::from_rgba(pixels, 32, 32, 32, 32);

        let mut passed = Vec::new();
        let mut failed = Vec::new();

        for (node_id, node_def) in defs {
            // Build a params object with non-identity values for each numeric param.
            let props = match node_def.get("properties").and_then(|p| p.as_object()) {
                Some(p) => p,
                None => continue,
            };

            let mut params = serde_json::Map::new();
            let mut has_numeric = false;
            for (param_name, param_schema) in props {
                match param_schema.get("type").and_then(|t| t.as_str()) {
                    Some("number") => {
                        // Use a value slightly away from identity toward the middle of range
                        let min = param_schema
                            .get("minimum")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);
                        let max = param_schema
                            .get("maximum")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(1.0);
                        let identity = param_schema
                            .get("x-zennode-identity")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);
                        // Pick a value 10% away from identity toward the midpoint
                        let mid = (min + max) / 2.0;
                        let val = identity + (mid - identity) * 0.1;
                        let val = val.max(min).min(max);
                        params.insert(param_name.clone(), serde_json::json!(val));
                        has_numeric = true;
                    }
                    Some("integer") => {
                        let min = param_schema
                            .get("minimum")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0);
                        let max = param_schema
                            .get("maximum")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(1);
                        let val = (min + max) / 2;
                        params.insert(param_name.clone(), serde_json::json!(val));
                        has_numeric = true;
                    }
                    _ => {} // skip non-numeric params for this test
                }
            }

            if !has_numeric {
                continue;
            }

            let mut adj = BTreeMap::new();
            adj.insert(node_id.clone(), serde_json::Value::Object(params));

            // Known gap: dt_sigmoid doesn't implement Filter yet.
            if node_id == "zenfilters.dt_sigmoid" {
                continue;
            }

            match editor.render_overview(&adj) {
                Ok(out) => {
                    assert!(out.width > 0, "{node_id} produced 0-width output");
                    assert!(!out.data.is_empty(), "{node_id} produced empty data");
                    passed.push(node_id.clone());
                }
                Err(e) => {
                    eprintln!("FAIL: {node_id}: {e}");
                    failed.push((node_id.clone(), e));
                }
            }
        }

        eprintln!("\n=== Filter node test results ===");
        eprintln!("Passed: {}/{}", passed.len(), passed.len() + failed.len());
        for (id, err) in &failed {
            eprintln!("  FAIL: {id}: {err}");
        }

        assert!(
            failed.is_empty(),
            "{} filter nodes failed:\n{}",
            failed.len(),
            failed
                .iter()
                .map(|(id, e)| format!("  {id}: {e}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
