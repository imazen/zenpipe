//! Core editor: manages two Sessions (overview + detail) and renders
//! from cached geometry prefixes when only filters change.

use std::collections::BTreeMap;

use zenpipe::Source;
use zenpipe::format::RGBA8_SRGB;
use zenpipe::orchestrate::{ProcessConfig, SourceImageInfo};
use zenpipe::session::Session;
use zenpipe::sources::MaterializedSource;

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
        }
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
    pub fn render_overview(
        &mut self,
        adjustments: &BTreeMap<String, f64>,
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

        // Filters from adjustments
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

        let output = self
            .overview_session
            .stream(source, &config, None, self.source_hash)
            .map_err(|e| format!("overview render: {e}"))?;

        let mat = MaterializedSource::from_source(output.source)
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
        adjustments: &BTreeMap<String, f64>,
        region: &Region,
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

        nodes.push(Box::new(zenpipe::zennode_defs::Constrain {
            w: Some(self.detail_max),
            h: Some(self.detail_max),
            mode: "within".into(),
            ..Default::default()
        }));

        // Filters from adjustments
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

        let output = self
            .detail_session
            .stream(source, &config, None, region_hash)
            .map_err(|e| format!("detail render: {e}"))?;

        let mat = MaterializedSource::from_source(output.source)
            .map_err(|e| format!("detail materialize: {e}"))?;

        Ok(RenderOutput {
            width: mat.width(),
            height: mat.height(),
            data: pack_rgba(&mat),
        })
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

/// Extract tightly-packed RGBA8 pixels from a MaterializedSource,
/// stripping any stride padding and expanding RGB→RGBA if needed.
fn pack_rgba(mat: &MaterializedSource) -> Vec<u8> {
    let w = mat.width() as usize;
    let h = mat.height() as usize;
    let bpp = mat.format().bytes_per_pixel();
    let stride = mat.stride();

    if bpp == 4 {
        // RGBA8 — just strip stride padding.
        let row_bytes = w * 4;
        if stride == row_bytes {
            return mat.data()[..row_bytes * h].to_vec();
        }
        let mut packed = Vec::with_capacity(row_bytes * h);
        for y in 0..h {
            let start = y * stride;
            packed.extend_from_slice(&mat.data()[start..start + row_bytes]);
        }
        packed
    } else if bpp == 3 {
        // RGB8 → expand to RGBA8 with alpha=255.
        let mut packed = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            let row_start = y * stride;
            for x in 0..w {
                let px = row_start + x * 3;
                packed.push(mat.data()[px]);
                packed.push(mat.data()[px + 1]);
                packed.push(mat.data()[px + 2]);
                packed.push(255);
            }
        }
        packed
    } else {
        // Unsupported format — return empty (shouldn't happen).
        vec![0u8; w * h * 4]
    }
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

/// Map adjustment key-value pairs to zenfilters node instances.
///
/// Keys are zenfilters node IDs without the "zenfilters." prefix
/// (e.g., "exposure", "contrast"). Values are the primary parameter.
fn append_filter_nodes(
    nodes: &mut Vec<Box<dyn zennode::NodeInstance>>,
    adjustments: &BTreeMap<String, f64>,
) {
    for (key, &value) in adjustments {
        // Skip identity values (no-op filters).
        if value == 0.0 {
            continue;
        }

        let node: Option<Box<dyn zennode::NodeInstance>> = match key.as_str() {
            "exposure" => Some(Box::new(zenfilters::zennode_defs::Exposure {
                stops: value as f32,
            })),
            "contrast" => Some(Box::new(zenfilters::zennode_defs::Contrast {
                amount: value as f32,
            })),
            "highlights" => Some(Box::new(zenfilters::zennode_defs::HighlightsShadows {
                highlights: value as f32,
                ..Default::default()
            })),
            "shadows" => Some(Box::new(zenfilters::zennode_defs::HighlightsShadows {
                shadows: value as f32,
                ..Default::default()
            })),
            "saturation" => Some(Box::new(zenfilters::zennode_defs::Saturation {
                factor: 1.0 + value as f32,
            })),
            "vibrance" => Some(Box::new(zenfilters::zennode_defs::Vibrance {
                amount: value as f32,
                ..Default::default()
            })),
            "clarity" => Some(Box::new(zenfilters::zennode_defs::Clarity {
                amount: value as f32,
                ..Default::default()
            })),
            "sharpen" => Some(Box::new(zenfilters::zennode_defs::Sharpen {
                amount: value as f32,
                ..Default::default()
            })),
            "temperature" => Some(Box::new(zenfilters::zennode_defs::Temperature {
                shift: value as f32,
            })),
            "dehaze" => Some(Box::new(zenfilters::zennode_defs::Dehaze {
                strength: value as f32,
            })),
            _ => None,
        };

        if let Some(n) = node {
            nodes.push(n);
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
        adj.insert("exposure".into(), 1.0);
        let out = editor.render_overview(&adj).unwrap();
        assert!(out.width > 0);
        assert!(out.height > 0);
    }

    #[test]
    fn render_overview_cache_hit() {
        let pixels = solid_rgba(400, 300, 128, 128, 128);
        let mut editor = Editor::from_rgba(pixels, 400, 300, 200, 400);
        let mut adj = BTreeMap::new();
        adj.insert("exposure".into(), 0.5);

        // First render — cache miss.
        let _out1 = editor.render_overview(&adj).unwrap();
        assert_eq!(editor.overview_session.cache_len(), 1);

        // Second render with different filter — cache hit on geometry.
        adj.insert("exposure".into(), 1.0);
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
        adj.insert("contrast".into(), 0.5);
        let _out1 = editor.render_region(&adj, &region).unwrap();
        assert_eq!(editor.detail_session.cache_len(), 1);

        // Same region, different filter → cache hit.
        adj.insert("contrast".into(), -0.3);
        let _out2 = editor.render_region(&adj, &region).unwrap();
        assert_eq!(editor.detail_session.cache_len(), 1);
    }

    #[test]
    fn render_region_cache_miss_on_region_change() {
        let pixels = solid_rgba(400, 300, 128, 128, 128);
        let mut editor = Editor::from_rgba(pixels, 400, 300, 200, 400);
        // Need a filter so the session has a geometry/filter split to cache.
        let mut adj = BTreeMap::new();
        adj.insert("exposure".into(), 0.5);

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
        adj.insert("exposure".into(), 0.0); // Should be skipped
        adj.insert("contrast".into(), 0.0); // Should be skipped
        let out = editor.render_overview(&adj).unwrap();
        assert!(out.width > 0);
    }

    #[test]
    fn multiple_filters_combined() {
        let pixels = solid_rgba(200, 200, 128, 128, 128);
        let mut editor = Editor::from_rgba(pixels, 200, 200, 100, 200);
        let mut adj = BTreeMap::new();
        adj.insert("exposure".into(), 0.5);
        adj.insert("contrast".into(), 0.3);
        adj.insert("saturation".into(), 0.2);
        let out = editor.render_overview(&adj).unwrap();
        assert!(out.width > 0);
    }
}
