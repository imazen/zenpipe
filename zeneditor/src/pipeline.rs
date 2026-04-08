//! Pipeline execution — rendering via zenpipe Session, filter conversion,
//! and RGBA packing.
//!
//! Moved from demo/crate/src/editor.rs to be platform-agnostic.

use std::collections::BTreeMap;

use zenpipe::Source;
use zenpipe::format::RGBA8_SRGB;
use zenpipe::orchestrate::SourceImageInfo;
use zenpipe::sources::MaterializedSource;

/// Rendered output: RGBA8 pixels + dimensions.
pub struct RenderOutput {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Emit pipeline nodes from GeometryModel state.
///
/// Translates the editor's geometry representation to zenpipe node instances.
/// Order: orient → crop → rotate → flip → pad. This matches the pipeline
/// stage order in SPEC.md §11.1.
#[cfg(feature = "std")]
pub(crate) fn geometry_to_nodes(
    geo: &crate::model::geometry::GeometryModel,
    metadata: Option<&zencodec::Metadata>,
    nodes: &mut Vec<Box<dyn zennode::NodeInstance>>,
) {
    use crate::model::geometry::*;

    // 1. Orientation (must be first — corrects EXIF rotation before crop applies)
    match geo.orientation {
        OrientMode::Auto => {
            // Extract EXIF orientation from metadata (Orientation repr(u8) = EXIF 1-8)
            let orient_val = metadata.map(|m| m.orientation as i32).unwrap_or(1);
            if orient_val != 1 {
                nodes.push(Box::new(zenpipe::zennode_defs::Orient {
                    orientation: orient_val,
                }));
            }
        }
        OrientMode::Explicit { value } => {
            if value != 1 {
                nodes.push(Box::new(zenpipe::zennode_defs::Orient {
                    orientation: value as i32,
                }));
            }
        }
        OrientMode::None => {}
    }

    // 2. Crop
    match &geo.crop {
        CropMode::None | CropMode::Auto => {}
        CropMode::Pixels { x, y, w, h } => {
            nodes.push(Box::new(zenpipe::zennode_defs::Crop {
                x: *x,
                y: *y,
                w: *w,
                h: *h,
            }));
        }
        CropMode::Percent { x, y, w, h } => {
            nodes.push(Box::new(zenpipe::zennode_defs::CropPercent {
                x: *x,
                y: *y,
                w: *w,
                h: *h,
            }));
        }
    }

    // Auto crop via CropWhitespace node (if/when available in pipeline)
    if matches!(geo.crop, CropMode::Auto) {
        // TODO: push CropWhitespace node when zenpipe exposes it as a zennode_def.
        // Currently crop_whitespace exists as NodeOp but not as a Node derive struct.
    }

    // 3. Rotation
    match geo.rotation {
        RotationMode::None | RotationMode::AutoDeskew => {}
        RotationMode::Cardinal { degrees } => match degrees {
            90 => nodes.push(Box::new(zenpipe::zennode_defs::Rotate90 {})),
            180 => nodes.push(Box::new(zenpipe::zennode_defs::Rotate180 {})),
            270 => nodes.push(Box::new(zenpipe::zennode_defs::Rotate270 {})),
            _ => {} // invalid cardinal, ignore
        },
        RotationMode::Arbitrary { degrees, .. } => {
            if degrees.abs() > 0.001 {
                // Arbitrary rotation goes through zenfilters.rotate (experimental).
                // For now, we emit it via the adjustments path — the node registry
                // handles "zenfilters.rotate" if the feature is enabled.
                // TODO: emit RotateDef directly when zenfilters experimental is stable.
                let _ = degrees; // acknowledged but not yet wired
            }
        }
    }

    // AutoDeskew: would need analysis pass to detect angle, then emit RotateDef.
    // Deferred to when document analysis pipeline is integrated.

    // 4. Flip
    if geo.flip_h {
        nodes.push(Box::new(zenpipe::zennode_defs::FlipH {}));
    }
    if geo.flip_v {
        nodes.push(Box::new(zenpipe::zennode_defs::FlipV {}));
    }

    // 5. Padding
    if !geo.padding.is_empty() {
        nodes.push(Box::new(zenpipe::zennode_defs::ExpandCanvas {
            left: geo.padding.left,
            top: geo.padding.top,
            right: geo.padding.right,
            bottom: geo.padding.bottom,
            color: geo.padding.bg_color.clone(),
        }));
    }
}

/// NodeConverter that bridges zenfilters zennode definitions to pipeline NodeOps.
#[cfg(feature = "std")]
pub(crate) struct FiltersConverter;

#[cfg(feature = "std")]
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
#[cfg(feature = "std")]
pub(crate) static FILTERS_CONVERTER: FiltersConverter = FiltersConverter;

/// Append a film look node if a preset is specified.
///
/// `film_look_strength` is extracted from the adjustments map if present;
/// defaults to the given intensity. The preset is applied before individual
/// filter nodes.
#[cfg(feature = "std")]
pub(crate) fn append_film_look(
    nodes: &mut Vec<Box<dyn zennode::NodeInstance>>,
    adjustments: &BTreeMap<String, serde_json::Value>,
    film_look_preset: Option<&str>,
    intensity: f32,
) {
    if let Some(preset) = film_look_preset {
        if !preset.is_empty() && preset != "none" {
            let strength = adjustments
                .get("film_look_strength")
                .and_then(|v| v.as_f64())
                .map(|s| s as f32)
                .unwrap_or(intensity);
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
/// objects of param name → value. Non-zenfilters keys and `film_look` keys
/// are skipped. Unknown nodes are silently ignored.
#[cfg(feature = "std")]
pub(crate) fn append_filter_nodes(
    nodes: &mut Vec<Box<dyn zennode::NodeInstance>>,
    adjustments: &BTreeMap<String, serde_json::Value>,
) {
    let registry = zenpipe::full_registry();

    for (node_id, params_json) in adjustments {
        if !node_id.starts_with("zenfilters.") || node_id == "zenfilters.film_look" {
            continue;
        }
        if !params_json.is_object() {
            continue;
        }

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

/// Create a SourceImageInfo for pipeline configuration.
pub(crate) fn make_source_info(width: u32, height: u32) -> SourceImageInfo {
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

/// Convert a MaterializedSource to tightly-packed RGBA8 sRGB pixels
/// using zenpixels-convert's SIMD-accelerated RowConverter.
///
/// Handles any pipeline output format (RGBA8, RGB8, RgbaF32 linear, etc.)
/// via automatic conversion planning. Fast path when already RGBA8 sRGB.
pub(crate) fn pack_rgba(mat: &MaterializedSource) -> Vec<u8> {
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
