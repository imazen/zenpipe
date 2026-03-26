//! Bridge from [`zennode`] node instances to [`PipelineGraph`] node operations.
//!
//! Converts a list of [`zennode::NodeInstance`] objects into a [`PipelineGraph`]
//! by coalescing fusable groups and mapping each node to a [`NodeOp`].
//!
//! Encode/decode-phase nodes are separated out and returned alongside the graph
//! since they configure the encoder/decoder rather than pixel operations.
//! Their params are extracted into [`DecodeConfig`] and [`EncodeConfig`] for
//! convenient access without downcasting.
//!
//! # Extensibility
//!
//! For nodes that require crate-specific types (e.g., zenfilters pipelines),
//! callers provide [`NodeConverter`] implementations. The bridge handles
//! geometry/layout nodes (crop, orient, resize, flip, rotate) directly via
//! param introspection — no extra dependencies needed.
//!
//! # Example
//!
//! ```ignore
//! use zenpipe::bridge::{compile_nodes, CompileResult};
//!
//! let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![/* ... */];
//! let result = compile_nodes(&nodes, &[], source_w, source_h)?;
//! // result.graph has Source → ops → Output wired up
//! // result.encode_nodes has any Encode-phase nodes
//! // result.decode_config has extracted decoder params
//! // result.encode_config has extracted encoder params
//! ```

mod config;
mod convert;
mod dag;
mod geometry;
pub mod ordering;
mod parse;

use alloc::boxed::Box;
use alloc::vec::Vec;

use zennode::{NodeInstance, NodeRole};

use crate::error::PipeError;
use crate::graph::{EdgeKind, NodeOp, PipelineGraph};

// Re-export public types.
pub use config::{DecodeConfig, EncodeConfig};
pub use dag::{DagNode, build_pipeline_dag};
pub use ordering::{OptimizationLevel, canonical_sort, optimize_node_order};

// Sub-module items used by this module.
use convert::{coalesce, convert_step};

// Re-export for tests.
#[cfg(test)]
use geometry::is_geometry_node;
#[cfg(test)]
use parse::{parse_constraint_mode, parse_filter_opt};

// ─── CompileResult ───

/// Result of compiling zennode nodes into a pipeline graph.
pub struct CompileResult {
    /// The compiled pipeline graph with Source → ops → Output.
    pub graph: PipelineGraph,
    /// Encode-phase nodes separated out (they configure the encoder, not pixels).
    pub encode_nodes: Vec<Box<dyn NodeInstance>>,
    /// Decode-phase nodes separated out (they configure the decoder, not pixels).
    pub decode_nodes: Vec<Box<dyn NodeInstance>>,
    /// Metadata extracted from decode node params (hdr_mode, color_intent, min_size).
    pub decode_config: DecodeConfig,
    /// Encode settings extracted from encode-phase nodes (quality, format, dpr).
    pub encode_config: EncodeConfig,
}

/// Result of building a streaming pipeline from zennode nodes.
///
/// Contains a streaming [`Source`](crate::Source) that can be connected directly to an
/// encoder sink via [`execute()`](crate::execute), plus extracted decode
/// and encode configuration.
pub struct PipelineResult {
    /// Streaming pixel source — connect to an `EncoderSink` via
    /// [`execute(source, sink)`](crate::execute) for zero-materialization.
    pub source: Box<dyn crate::Source>,
    /// Decode configuration extracted from nodes.
    pub decode_config: DecodeConfig,
    /// Encode configuration extracted from nodes.
    pub encode_config: EncodeConfig,
}

impl PipelineResult {
    /// Materialize the streaming source into a pixel buffer.
    ///
    /// Use only when you genuinely need random-access pixels
    /// (quality analysis, non-streaming encoder).
    pub fn materialize(self) -> Result<MaterializedImage, PipeError> {
        let mat = crate::sources::MaterializedSource::from_source(self.source)?;
        Ok(MaterializedImage {
            pixels: mat,
            decode_config: self.decode_config,
            encode_config: self.encode_config,
        })
    }
}

/// A fully materialized image with its decode/encode configuration.
///
/// Created by [`PipelineResult::materialize()`]. Prefer streaming via
/// [`PipelineResult::source`] when possible.
pub struct MaterializedImage {
    /// The materialized pixel buffer.
    pub pixels: crate::sources::MaterializedSource,
    /// Decode configuration extracted from nodes.
    pub decode_config: DecodeConfig,
    /// Encode configuration extracted from nodes.
    pub encode_config: EncodeConfig,
}

/// A step in the compiled pipeline, either a single node or a coalesced group.
pub(crate) enum PipelineStep<'a> {
    /// A single node that wasn't coalesced.
    Single(&'a dyn NodeInstance),
    /// Adjacent fusable nodes merged into one step.
    Coalesced {
        group: &'static str,
        nodes: Vec<&'a dyn NodeInstance>,
    },
}

/// Trait for extending the bridge with crate-specific node conversions.
///
/// Implementations handle nodes that require types from optional dependencies
/// (e.g., zenfilters `Pipeline`, zenresize `ResizeConfig`). The bridge calls
/// converters in order, using the first one that claims the node.
pub trait NodeConverter: Send + Sync {
    /// Whether this converter handles the given schema ID.
    fn can_convert(&self, schema_id: &str) -> bool;

    /// Convert a single node instance to a [`NodeOp`].
    fn convert(&self, node: &dyn NodeInstance) -> Result<NodeOp, PipeError>;

    /// Convert a coalesced group of nodes into a single [`NodeOp`].
    ///
    /// Called when adjacent fusable nodes share a coalesce group and the
    /// converter claims at least one of them.
    fn convert_group(&self, nodes: &[&dyn NodeInstance]) -> Result<NodeOp, PipeError>;

    /// Fuse a group of adjacent compatible nodes into a single [`NodeOp`].
    ///
    /// This is the preferred fusion API. Unlike [`convert_group`](Self::convert_group)
    /// which requires nodes to share a coalesce group, `fuse_group` is called
    /// with any adjacent run of nodes that this converter claims. The converter
    /// can build an optimized combined operation (e.g., a `zenfilters::Pipeline`
    /// with `FusedAdjust`).
    ///
    /// Returns `Ok(None)` if fusion is not possible for this group — the bridge
    /// will fall back to converting each node individually.
    fn fuse_group(&self, nodes: &[&dyn NodeInstance]) -> Result<Option<NodeOp>, PipeError> {
        let _ = nodes;
        Ok(None)
    }
}

// ─── Public API ───

/// Compile zennode node instances into a [`PipelineGraph`].
///
/// Preserves user-specified node order (no reordering). Separates
/// encode/decode phase nodes, coalesces adjacent fusable nodes in the same
/// group, and converts each step to a [`NodeOp`].
///
/// When adjacent geometry nodes are detected and source dimensions are known,
/// they are fused into a single `NodeOp::Layout` via [`geometry::compile_geometry_run()`].
/// When source dimensions are not known (0, 0), geometry nodes are emitted
/// individually.
///
/// Decode and encode node params are extracted into [`DecodeConfig`] and
/// [`EncodeConfig`] for convenient typed access.
///
/// # Arguments
///
/// * `nodes` — node instances in user-declared order
/// * `converters` — optional extension converters for crate-specific nodes
/// * `source_w` — source image width (0 if unknown)
/// * `source_h` — source image height (0 if unknown)
///
/// # Errors
///
/// Returns `PipeError::Op` if a node cannot be converted and no converter
/// handles it.
pub fn compile_nodes(
    nodes: &[Box<dyn NodeInstance>],
    converters: &[&dyn NodeConverter],
    source_w: u32,
    source_h: u32,
) -> Result<CompileResult, PipeError> {
    // 1. Separate encode/decode nodes from pixel-processing nodes.
    let mut pixel_nodes: Vec<&dyn NodeInstance> = Vec::new();
    let mut encode_nodes: Vec<Box<dyn NodeInstance>> = Vec::new();
    let mut decode_nodes: Vec<Box<dyn NodeInstance>> = Vec::new();

    for node in nodes {
        match node.schema().role {
            NodeRole::Encode => encode_nodes.push(node.clone_boxed()),
            NodeRole::Decode => decode_nodes.push(node.clone_boxed()),
            _ => pixel_nodes.push(node.as_ref()),
        }
    }

    let decode_config = DecodeConfig::from_nodes(&decode_nodes);
    let encode_config = EncodeConfig::from_nodes(&encode_nodes);

    // 2. Coalesce, then attempt geometry fusion and filter fusion.
    let steps = coalesce(&pixel_nodes);

    // 3. Build the graph: Source → ops → Output.
    let mut graph = PipelineGraph::new();
    let source_id = graph.add_node(NodeOp::Source);
    let mut prev_id = source_id;

    for step in &steps {
        let ops = convert_step(step, converters, source_w, source_h)?;
        for node_op in ops {
            let node_id = graph.add_node(node_op);
            graph.add_edge(prev_id, node_id, EdgeKind::Input);
            prev_id = node_id;
        }
    }

    let output_id = graph.add_node(NodeOp::Output);
    graph.add_edge(prev_id, output_id, EdgeKind::Input);

    Ok(CompileResult {
        graph,
        encode_nodes,
        decode_nodes,
        decode_config,
        encode_config,
    })
}

/// Build a streaming pipeline from zennode nodes.
///
/// Returns a [`PipelineResult`] containing a streaming [`Source`](crate::Source)
/// that can be connected directly to an encoder sink via
/// [`execute()`](crate::execute) for zero-materialization processing.
///
/// This is the primary API — prefer it over materializing unless you
/// genuinely need the full image in memory.
///
/// # Arguments
///
/// * `source` — decoded pixel source (the caller has already decoded the image)
/// * `nodes` — zennode node instances in user-declared order
/// * `converters` — extension converters for crate-specific nodes
///
/// # Errors
///
/// Returns [`PipeError`] if node compilation or graph compilation fails.
pub fn build_pipeline(
    source: Box<dyn crate::Source>,
    nodes: &[Box<dyn NodeInstance>],
    converters: &[&dyn NodeConverter],
) -> Result<PipelineResult, PipeError> {
    let source_w = source.width();
    let source_h = source.height();

    let CompileResult {
        graph,
        decode_config,
        encode_config,
        ..
    } = compile_nodes(nodes, converters, source_w, source_h)?;

    let mut sources = hashbrown::HashMap::new();
    sources.insert(0, source);
    let pipeline_source = graph.compile(sources)?;

    Ok(PipelineResult {
        source: pipeline_source,
        decode_config,
        encode_config,
    })
}

// ─── Tests using mock NodeInstance implementations ───

#[cfg(test)]
mod core_tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;
    use crate::format::RGBA8_SRGB;
    use crate::strip::Strip;
    use core::any::Any;

    // A mock NodeInstance backed by a BTreeMap of params.
    struct MockNode {
        schema: &'static zennode::NodeSchema,
        params: zennode::ParamMap,
    }

    impl MockNode {
        fn boxed(
            schema: &'static zennode::NodeSchema,
            params: zennode::ParamMap,
        ) -> Box<dyn NodeInstance> {
            Box::new(Self { schema, params })
        }
    }

    impl NodeInstance for MockNode {
        fn schema(&self) -> &'static zennode::NodeSchema {
            self.schema
        }

        fn to_params(&self) -> zennode::ParamMap {
            self.params.clone()
        }

        fn get_param(&self, name: &str) -> Option<zennode::ParamValue> {
            self.params.get(name).cloned()
        }

        fn set_param(&mut self, name: &str, value: zennode::ParamValue) -> bool {
            self.params.insert(name.into(), value);
            true
        }

        fn as_any(&self) -> &dyn Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }

        fn clone_boxed(&self) -> Box<dyn NodeInstance> {
            Box::new(Self {
                schema: self.schema,
                params: self.params.clone(),
            })
        }
    }

    // ─── Static schemas ───

    static CROP_SCHEMA: zennode::NodeSchema = zennode::NodeSchema {
        id: "zenlayout.crop",
        label: "Crop",
        description: "Crop to rectangle",
        group: zennode::NodeGroup::Geometry,
        role: zennode::NodeRole::Geometry,
        params: &[],
        tags: &[],
        coalesce: Some(zennode::CoalesceInfo {
            group: "layout_plan",
            fusable: true,
            is_target: false,
        }),
        format: zennode::FormatHint {
            preferred: zennode::PixelFormatPreference::Any,
            alpha: zennode::AlphaHandling::Process,
            changes_dimensions: true,
            is_neighborhood: false,
        },
        version: 1,
        compat_version: 1,
        json_key: "",
        deny_unknown_fields: false,
    };

    static ORIENT_SCHEMA: zennode::NodeSchema = zennode::NodeSchema {
        id: "zenlayout.orient",
        label: "Orient",
        description: "Auto-orient from EXIF",
        group: zennode::NodeGroup::Geometry,
        role: zennode::NodeRole::Geometry,
        params: &[],
        tags: &[],
        coalesce: Some(zennode::CoalesceInfo {
            group: "layout_plan",
            fusable: true,
            is_target: false,
        }),
        format: zennode::FormatHint {
            preferred: zennode::PixelFormatPreference::Any,
            alpha: zennode::AlphaHandling::Process,
            changes_dimensions: true,
            is_neighborhood: false,
        },
        version: 1,
        compat_version: 1,
        json_key: "",
        deny_unknown_fields: false,
    };

    static CONSTRAIN_SCHEMA: zennode::NodeSchema = zennode::NodeSchema {
        id: "zenresize.constrain",
        label: "Constrain",
        description: "Resize within constraints",
        group: zennode::NodeGroup::Geometry,
        role: zennode::NodeRole::Geometry,
        params: &[],
        tags: &[],
        coalesce: Some(zennode::CoalesceInfo {
            group: "layout_plan",
            fusable: true,
            is_target: false,
        }),
        format: zennode::FormatHint {
            preferred: zennode::PixelFormatPreference::Any,
            alpha: zennode::AlphaHandling::Process,
            changes_dimensions: true,
            is_neighborhood: false,
        },
        version: 1,
        compat_version: 1,
        json_key: "",
        deny_unknown_fields: false,
    };

    static FLIP_H_SCHEMA: zennode::NodeSchema = zennode::NodeSchema {
        id: "zenlayout.flip_h",
        label: "Flip Horizontal",
        description: "Mirror horizontally",
        group: zennode::NodeGroup::Geometry,
        role: zennode::NodeRole::Geometry,
        params: &[],
        tags: &[],
        coalesce: Some(zennode::CoalesceInfo {
            group: "layout_plan",
            fusable: true,
            is_target: false,
        }),
        format: zennode::FormatHint {
            preferred: zennode::PixelFormatPreference::Any,
            alpha: zennode::AlphaHandling::Process,
            changes_dimensions: false,
            is_neighborhood: false,
        },
        version: 1,
        compat_version: 1,
        json_key: "",
        deny_unknown_fields: false,
    };

    static ROTATE_90_SCHEMA: zennode::NodeSchema = zennode::NodeSchema {
        id: "zenlayout.rotate_90",
        label: "Rotate 90",
        description: "Rotate 90 degrees",
        group: zennode::NodeGroup::Geometry,
        role: zennode::NodeRole::Geometry,
        params: &[],
        tags: &[],
        coalesce: Some(zennode::CoalesceInfo {
            group: "layout_plan",
            fusable: true,
            is_target: false,
        }),
        format: zennode::FormatHint {
            preferred: zennode::PixelFormatPreference::Any,
            alpha: zennode::AlphaHandling::Process,
            changes_dimensions: true,
            is_neighborhood: false,
        },
        version: 1,
        compat_version: 1,
        json_key: "",
        deny_unknown_fields: false,
    };

    static DECODE_SCHEMA: zennode::NodeSchema = zennode::NodeSchema {
        id: "zennode.decode",
        label: "Decode",
        description: "Decode",
        group: zennode::NodeGroup::Decode,
        role: zennode::NodeRole::Decode,
        params: &[],
        tags: &[],
        coalesce: None,
        format: zennode::FormatHint {
            preferred: zennode::PixelFormatPreference::Any,
            alpha: zennode::AlphaHandling::Process,
            changes_dimensions: false,
            is_neighborhood: false,
        },
        version: 1,
        compat_version: 1,
        json_key: "",
        deny_unknown_fields: false,
    };

    // A fake non-geometry schema for testing mixed groups.
    static FILTER_SCHEMA: zennode::NodeSchema = zennode::NodeSchema {
        id: "zenfilters.exposure",
        label: "Exposure",
        description: "Adjust exposure",
        group: zennode::NodeGroup::Tone,
        role: zennode::NodeRole::Filter,
        params: &[],
        tags: &[],
        coalesce: Some(zennode::CoalesceInfo {
            group: "filter_pipeline",
            fusable: true,
            is_target: false,
        }),
        format: zennode::FormatHint {
            preferred: zennode::PixelFormatPreference::OklabF32,
            alpha: zennode::AlphaHandling::Process,
            changes_dimensions: false,
            is_neighborhood: false,
        },
        version: 1,
        compat_version: 1,
        json_key: "",
        deny_unknown_fields: false,
    };

    // ─── Helper constructors ───

    fn crop_node(x: u32, y: u32, w: u32, h: u32) -> Box<dyn NodeInstance> {
        let mut params = zennode::ParamMap::new();
        params.insert("x".into(), zennode::ParamValue::U32(x));
        params.insert("y".into(), zennode::ParamValue::U32(y));
        params.insert("w".into(), zennode::ParamValue::U32(w));
        params.insert("h".into(), zennode::ParamValue::U32(h));
        MockNode::boxed(&CROP_SCHEMA, params)
    }

    fn orient_node(orientation: i32) -> Box<dyn NodeInstance> {
        let mut params = zennode::ParamMap::new();
        params.insert("orientation".into(), zennode::ParamValue::I32(orientation));
        MockNode::boxed(&ORIENT_SCHEMA, params)
    }

    fn constrain_node(w: u32, h: u32, mode: &str, filter: &str) -> Box<dyn NodeInstance> {
        let mut params = zennode::ParamMap::new();
        params.insert("w".into(), zennode::ParamValue::U32(w));
        params.insert("h".into(), zennode::ParamValue::U32(h));
        params.insert("mode".into(), zennode::ParamValue::Str(mode.into()));
        params.insert("filter".into(), zennode::ParamValue::Str(filter.into()));
        MockNode::boxed(&CONSTRAIN_SCHEMA, params)
    }

    fn flip_h_node() -> Box<dyn NodeInstance> {
        MockNode::boxed(&FLIP_H_SCHEMA, zennode::ParamMap::new())
    }

    fn rotate_90_node() -> Box<dyn NodeInstance> {
        MockNode::boxed(&ROTATE_90_SCHEMA, zennode::ParamMap::new())
    }

    fn decode_node() -> Box<dyn NodeInstance> {
        MockNode::boxed(&DECODE_SCHEMA, zennode::ParamMap::new())
    }

    fn filter_node() -> Box<dyn NodeInstance> {
        MockNode::boxed(&FILTER_SCHEMA, zennode::ParamMap::new())
    }

    // ─── Test source ───

    struct SolidSource {
        width: u32,
        height: u32,
        format: crate::PixelFormat,
        y: u32,
    }

    impl SolidSource {
        fn new(width: u32, height: u32) -> Self {
            Self {
                width,
                height,
                format: RGBA8_SRGB,
                y: 0,
            }
        }
    }

    impl crate::Source for SolidSource {
        fn next(&mut self) -> Result<Option<Strip<'_>>, PipeError> {
            if self.y >= self.height {
                return Ok(None);
            }
            let rows = 16.min(self.height - self.y);
            let stride = self.format.aligned_stride(self.width);
            let data = alloc::vec![128u8; stride * rows as usize];
            self.y += rows;
            let leaked: &'static [u8] = alloc::vec::Vec::leak(data);
            Ok(Some(Strip::new(
                leaked,
                self.width,
                rows,
                stride,
                self.format,
            )?))
        }

        fn width(&self) -> u32 {
            self.width
        }

        fn height(&self) -> u32 {
            self.height
        }

        fn format(&self) -> crate::PixelFormat {
            self.format
        }
    }

    // ─── compile_nodes tests ───

    #[test]
    fn compile_empty_nodes() {
        let nodes: Vec<Box<dyn NodeInstance>> = Vec::new();
        let result = compile_nodes(&nodes, &[], 0, 0).unwrap();
        assert!(result.encode_nodes.is_empty());
        assert!(result.decode_nodes.is_empty());
    }

    #[test]
    fn compile_single_crop_node() {
        let nodes: Vec<Box<dyn NodeInstance>> = vec![crop_node(10, 20, 100, 80)];
        let result = compile_nodes(&nodes, &[], 0, 0).unwrap();
        assert!(result.encode_nodes.is_empty());
        assert!(result.decode_nodes.is_empty());
    }

    #[test]
    fn compile_orient_node() {
        let nodes: Vec<Box<dyn NodeInstance>> = vec![orient_node(6)];
        let result = compile_nodes(&nodes, &[], 0, 0).unwrap();
        assert!(result.encode_nodes.is_empty());
    }

    #[test]
    fn compile_constrain_node() {
        let nodes: Vec<Box<dyn NodeInstance>> = vec![constrain_node(800, 600, "within", "lanczos")];
        let result = compile_nodes(&nodes, &[], 0, 0).unwrap();
        assert!(result.encode_nodes.is_empty());
    }

    #[test]
    fn decode_nodes_separated() {
        let nodes: Vec<Box<dyn NodeInstance>> = vec![decode_node()];
        let result = compile_nodes(&nodes, &[], 0, 0).unwrap();
        assert_eq!(result.decode_nodes.len(), 1);
    }

    // ─── Geometry fusion tests ───

    #[test]
    fn geometry_fusion_crop_plus_constrain() {
        let nodes: Vec<Box<dyn NodeInstance>> = vec![
            crop_node(100, 100, 2000, 2000),
            constrain_node(800, 600, "within", "robidoux"),
        ];

        let result = compile_nodes(&nodes, &[], 4000, 3000).unwrap();
        let mut sources = hashbrown::HashMap::new();
        sources.insert(
            0,
            Box::new(SolidSource::new(4000, 3000)) as Box<dyn crate::Source>,
        );
        let pipeline = result.graph.compile(sources).unwrap();

        assert_eq!(pipeline.width(), 600);
        assert_eq!(pipeline.height(), 600);
    }

    #[test]
    fn geometry_fusion_orient_plus_constrain() {
        let nodes: Vec<Box<dyn NodeInstance>> =
            vec![orient_node(6), constrain_node(800, 600, "fit", "")];

        let result = compile_nodes(&nodes, &[], 4000, 3000).unwrap();
        let mut sources = hashbrown::HashMap::new();
        sources.insert(
            0,
            Box::new(SolidSource::new(4000, 3000)) as Box<dyn crate::Source>,
        );
        let pipeline = result.graph.compile(sources).unwrap();

        assert_eq!(pipeline.width(), 450);
        assert_eq!(pipeline.height(), 600);
    }

    #[test]
    fn geometry_fusion_flip_rotate_crop() {
        let nodes: Vec<Box<dyn NodeInstance>> =
            vec![flip_h_node(), rotate_90_node(), crop_node(0, 0, 500, 500)];

        let result = compile_nodes(&nodes, &[], 1000, 800).unwrap();
        let mut sources = hashbrown::HashMap::new();
        sources.insert(
            0,
            Box::new(SolidSource::new(1000, 800)) as Box<dyn crate::Source>,
        );
        let pipeline = result.graph.compile(sources).unwrap();

        assert_eq!(pipeline.width(), 500);
        assert_eq!(pipeline.height(), 500);
    }

    #[test]
    fn geometry_fusion_single_constrain() {
        let nodes: Vec<Box<dyn NodeInstance>> = vec![constrain_node(200, 150, "fit", "lanczos")];

        let result = compile_nodes(&nodes, &[], 800, 600).unwrap();
        let mut sources = hashbrown::HashMap::new();
        sources.insert(
            0,
            Box::new(SolidSource::new(800, 600)) as Box<dyn crate::Source>,
        );
        let pipeline = result.graph.compile(sources).unwrap();

        assert_eq!(pipeline.width(), 200);
        assert_eq!(pipeline.height(), 150);
    }

    #[test]
    fn geometry_fusion_fallback_without_dimensions() {
        let nodes: Vec<Box<dyn NodeInstance>> = vec![
            crop_node(10, 10, 100, 100),
            constrain_node(50, 50, "fit", ""),
        ];

        let result = compile_nodes(&nodes, &[], 0, 0).unwrap();
        assert!(result.encode_nodes.is_empty());
    }

    // ─── build_pipeline streaming tests ───

    #[test]
    fn build_pipeline_returns_streaming_source() {
        let source = Box::new(SolidSource::new(200, 150));
        let nodes: Vec<Box<dyn NodeInstance>> = Vec::new();

        let result = build_pipeline(source, &nodes, &[]).unwrap();
        assert_eq!(result.source.width(), 200);
        assert_eq!(result.source.height(), 150);
    }

    #[test]
    fn build_pipeline_with_crop() {
        let source = Box::new(SolidSource::new(400, 300));
        let nodes: Vec<Box<dyn NodeInstance>> = vec![crop_node(10, 10, 200, 150)];

        let result = build_pipeline(source, &nodes, &[]).unwrap();
        assert_eq!(result.source.width(), 200);
        assert_eq!(result.source.height(), 150);
    }

    #[test]
    fn build_pipeline_streams_all_strips() {
        let source = Box::new(SolidSource::new(100, 100));
        let nodes: Vec<Box<dyn NodeInstance>> = Vec::new();

        let mut result = build_pipeline(source, &nodes, &[]).unwrap();
        let mut total_rows = 0u32;
        while let Some(strip) = result.source.next().unwrap() {
            total_rows += strip.rows();
        }
        assert_eq!(total_rows, 100);
    }

    #[test]
    fn build_pipeline_decode_config_extracted() {
        let source = Box::new(SolidSource::new(100, 100));
        let mut params = zennode::ParamMap::new();
        params.insert(
            "hdr_mode".into(),
            zennode::ParamValue::Str("preserve".into()),
        );
        let nodes: Vec<Box<dyn NodeInstance>> = vec![MockNode::boxed(&DECODE_SCHEMA, params)];

        let result = build_pipeline(source, &nodes, &[]).unwrap();
        assert_eq!(result.decode_config.hdr_mode, "preserve");
    }

    // ─── DAG tests ───

    #[test]
    fn dag_simple_linear_chain() {
        let dag = vec![
            DagNode {
                instance: crop_node(0, 0, 100, 100),
                inputs: vec![],
            },
            DagNode {
                instance: crop_node(10, 10, 50, 50),
                inputs: vec![0],
            },
        ];

        let sources = vec![(
            0,
            Box::new(SolidSource::new(200, 200)) as Box<dyn crate::Source>,
        )];

        let result = build_pipeline_dag(sources, &dag, &[]).unwrap();
        assert_eq!(result.source.width(), 50);
        assert_eq!(result.source.height(), 50);
    }

    #[test]
    fn dag_with_decode_node() {
        let dag = vec![
            DagNode {
                instance: decode_node(),
                inputs: vec![],
            },
            DagNode {
                instance: crop_node(0, 0, 100, 100),
                inputs: vec![],
            },
            DagNode {
                instance: crop_node(5, 5, 50, 50),
                inputs: vec![1],
            },
        ];

        let sources = vec![(
            1,
            Box::new(SolidSource::new(200, 200)) as Box<dyn crate::Source>,
        )];

        let result = build_pipeline_dag(sources, &dag, &[]).unwrap();
        assert_eq!(result.decode_config.hdr_mode, "sdr_only");
    }

    // ─── NodeConverter fuse_group tests ───

    struct TestFilterConverter;

    impl NodeConverter for TestFilterConverter {
        fn can_convert(&self, schema_id: &str) -> bool {
            schema_id == "zenfilters.exposure"
        }

        fn convert(&self, _node: &dyn NodeInstance) -> Result<NodeOp, PipeError> {
            Ok(NodeOp::PixelTransform(Box::new(IdentityOp)))
        }

        fn convert_group(&self, nodes: &[&dyn NodeInstance]) -> Result<NodeOp, PipeError> {
            self.convert(nodes[0])
        }

        fn fuse_group(&self, nodes: &[&dyn NodeInstance]) -> Result<Option<NodeOp>, PipeError> {
            if nodes.len() > 1 {
                Ok(Some(NodeOp::PixelTransform(Box::new(IdentityOp))))
            } else {
                Ok(None)
            }
        }
    }

    struct IdentityOp;

    impl crate::ops::PixelOp for IdentityOp {
        fn apply(&mut self, input: &[u8], output: &mut [u8], _width: u32, _height: u32) {
            let len = output.len();
            output[..len].copy_from_slice(&input[..len]);
        }

        fn input_format(&self) -> crate::PixelFormat {
            RGBA8_SRGB
        }

        fn output_format(&self) -> crate::PixelFormat {
            RGBA8_SRGB
        }
    }

    #[test]
    fn fuse_group_called_for_filter_nodes() {
        let nodes: Vec<Box<dyn NodeInstance>> = vec![filter_node(), filter_node()];
        let converter = TestFilterConverter;
        let converters: &[&dyn NodeConverter] = &[&converter];

        let result = compile_nodes(&nodes, converters, 100, 100).unwrap();

        let mut sources = hashbrown::HashMap::new();
        sources.insert(
            0,
            Box::new(SolidSource::new(100, 100)) as Box<dyn crate::Source>,
        );
        let pipeline = result.graph.compile(sources).unwrap();
        assert_eq!(pipeline.width(), 100);
        assert_eq!(pipeline.height(), 100);
    }

    #[test]
    fn single_filter_node_uses_convert() {
        let nodes: Vec<Box<dyn NodeInstance>> = vec![filter_node()];
        let converter = TestFilterConverter;
        let converters: &[&dyn NodeConverter] = &[&converter];

        let result = compile_nodes(&nodes, converters, 100, 100).unwrap();
        let mut sources = hashbrown::HashMap::new();
        sources.insert(
            0,
            Box::new(SolidSource::new(100, 100)) as Box<dyn crate::Source>,
        );
        let pipeline = result.graph.compile(sources).unwrap();
        assert_eq!(pipeline.width(), 100);
    }

    // ─── parse_filter / parse_constraint_mode tests ───

    #[test]
    fn parse_filter_variants() {
        assert!(parse_filter_opt("").is_none());
        assert_eq!(
            parse_filter_opt("lanczos"),
            Some(zenresize::Filter::Lanczos)
        );
        assert_eq!(
            parse_filter_opt("robidoux"),
            Some(zenresize::Filter::Robidoux)
        );
        assert!(parse_filter_opt("unknown_filter").is_none());
    }

    #[test]
    fn parse_constraint_mode_aliases() {
        assert_eq!(
            parse_constraint_mode("crop").unwrap(),
            zenresize::ConstraintMode::FitCrop
        );
        assert_eq!(
            parse_constraint_mode("pad").unwrap(),
            zenresize::ConstraintMode::FitPad
        );
        assert_eq!(
            parse_constraint_mode("within").unwrap(),
            zenresize::ConstraintMode::Within
        );
    }

    #[test]
    fn unknown_constraint_mode_errors() {
        let err = parse_constraint_mode("bogus").unwrap_err();
        assert!(err.to_string().contains("bogus"));
    }

    // ─── DecodeConfig / EncodeConfig tests ───

    #[test]
    fn decode_config_defaults() {
        let config = DecodeConfig::default();
        assert_eq!(config.hdr_mode, "sdr_only");
        assert_eq!(config.color_intent, "preserve");
        assert_eq!(config.min_size, 0);
    }

    #[test]
    fn encode_config_defaults() {
        let config = EncodeConfig::default();
        assert!(config.quality_profile.is_none());
        assert!(config.format.is_none());
        assert_eq!(config.dpr, 1.0);
        assert!(config.lossless.is_none());
        assert!(config.codec_params.is_none());
    }

    #[test]
    fn decode_config_from_empty_nodes() {
        let nodes: Vec<Box<dyn NodeInstance>> = Vec::new();
        let config = DecodeConfig::from_nodes(&nodes);
        assert_eq!(config.hdr_mode, "sdr_only");
    }

    #[test]
    fn encode_config_from_empty_nodes() {
        let nodes: Vec<Box<dyn NodeInstance>> = Vec::new();
        let config = EncodeConfig::from_nodes(&nodes);
        assert!(config.quality_profile.is_none());
    }

    // ─── is_geometry_node tests ───

    #[test]
    fn geometry_node_detection() {
        assert!(is_geometry_node("zenlayout.crop"));
        assert!(is_geometry_node("zenlayout.orient"));
        assert!(is_geometry_node("zenlayout.flip_h"));
        assert!(is_geometry_node("zenlayout.flip_v"));
        assert!(is_geometry_node("zenlayout.rotate_90"));
        assert!(is_geometry_node("zenlayout.rotate_180"));
        assert!(is_geometry_node("zenlayout.rotate_270"));
        assert!(is_geometry_node("zenresize.constrain"));
        assert!(is_geometry_node("zenlayout.constrain"));
        assert!(!is_geometry_node("zenfilters.exposure"));
        assert!(!is_geometry_node("zennode.decode"));
    }

    // ─── materialize tests ───

    #[test]
    fn materialize_produces_pixel_buffer() {
        use crate::Source as _;

        let source = Box::new(SolidSource::new(64, 64));
        let nodes: Vec<Box<dyn NodeInstance>> = Vec::new();

        let result = build_pipeline(source, &nodes, &[]).unwrap();
        let mat = result.materialize().unwrap();
        assert_eq!(mat.pixels.width(), 64);
        assert_eq!(mat.pixels.height(), 64);
        assert!(!mat.pixels.data().is_empty());
    }

    #[test]
    fn materialize_preserves_configs() {
        let source = Box::new(SolidSource::new(100, 100));
        let mut params = zennode::ParamMap::new();
        params.insert(
            "hdr_mode".into(),
            zennode::ParamValue::Str("preserve".into()),
        );
        let nodes: Vec<Box<dyn NodeInstance>> = vec![MockNode::boxed(&DECODE_SCHEMA, params)];

        let result = build_pipeline(source, &nodes, &[]).unwrap();
        let mat = result.materialize().unwrap();
        assert_eq!(mat.decode_config.hdr_mode, "preserve");
        assert!(mat.encode_config.quality_profile.is_none());
    }

    #[test]
    fn materialize_preserves_pixel_data() {
        let source = Box::new(SolidSource::new(64, 64));
        let nodes: Vec<Box<dyn NodeInstance>> = Vec::new();

        let result = build_pipeline(source, &nodes, &[]).unwrap();
        let mat = result.materialize().unwrap();
        // SolidSource fills with 128. Check the materialized data.
        let expected_size = mat.pixels.stride() * 64;
        assert_eq!(mat.pixels.data().len(), expected_size);
        // First pixel should be 128 (the fill value).
        assert_eq!(mat.pixels.data()[0], 128);
    }
}

// Bridge tests requiring `zennode_defs` modules in zenresize and zenlayout.
#[cfg(all(test, feature = "zennode-defs"))]
mod tests {
    use super::*;
    use zennode::NodeDef;

    use zenresize::zennode_defs as resize_nodes;

    #[test]
    fn compile_empty() {
        let nodes: Vec<Box<dyn NodeInstance>> = Vec::new();
        let result = compile_nodes(&nodes, &[], 0, 0).unwrap();
        assert!(result.encode_nodes.is_empty());
        assert!(result.decode_nodes.is_empty());
    }

    #[test]
    fn compile_single_crop() {
        let mut params = zennode::ParamMap::new();
        params.insert("x".into(), zennode::ParamValue::U32(10));
        params.insert("y".into(), zennode::ParamValue::U32(20));
        params.insert("w".into(), zennode::ParamValue::U32(100));
        params.insert("h".into(), zennode::ParamValue::U32(80));

        let crop_node = zenlayout::zennode_defs::CROP_NODE.create(&params).unwrap();
        let nodes: Vec<Box<dyn NodeInstance>> = vec![crop_node];
        let result = compile_nodes(&nodes, &[], 0, 0).unwrap();
        assert!(result.encode_nodes.is_empty());
        assert!(result.decode_nodes.is_empty());
    }

    #[test]
    fn compile_orient() {
        let mut params = zennode::ParamMap::new();
        params.insert("orientation".into(), zennode::ParamValue::I32(6));

        let orient_node = zenlayout::zennode_defs::ORIENT_NODE
            .create(&params)
            .unwrap();
        let nodes: Vec<Box<dyn NodeInstance>> = vec![orient_node];
        let result = compile_nodes(&nodes, &[], 0, 0).unwrap();
        assert!(result.encode_nodes.is_empty());
    }

    #[test]
    fn compile_constrain() {
        let mut params = zennode::ParamMap::new();
        params.insert("w".into(), zennode::ParamValue::U32(800));
        params.insert("h".into(), zennode::ParamValue::U32(600));
        params.insert("mode".into(), zennode::ParamValue::Str("within".into()));
        params.insert("filter".into(), zennode::ParamValue::Str("lanczos".into()));

        let node = resize_nodes::CONSTRAIN_NODE.create(&params).unwrap();
        let nodes: Vec<Box<dyn NodeInstance>> = vec![node];
        let result = compile_nodes(&nodes, &[], 0, 0).unwrap();
        assert!(result.encode_nodes.is_empty());
    }

    #[test]
    fn decode_nodes_separated() {
        let decode_node = zennode::nodes::DECODE_NODE.create_default().unwrap();
        let nodes: Vec<Box<dyn NodeInstance>> = vec![decode_node];
        let result = compile_nodes(&nodes, &[], 0, 0).unwrap();
        assert_eq!(result.decode_nodes.len(), 1);
        assert_eq!(result.decode_nodes[0].schema().id, "zennode.decode");
    }

    #[test]
    fn unknown_node_errors() {
        let err = parse_constraint_mode("bogus").unwrap_err();
        assert!(err.to_string().contains("bogus"));
    }

    #[test]
    fn order_preserved() {
        let rot90 = zenlayout::zennode_defs::ROTATE90_NODE
            .create_default()
            .unwrap();
        let rot270 = zenlayout::zennode_defs::ROTATE270_NODE
            .create_default()
            .unwrap();

        let nodes: Vec<Box<dyn NodeInstance>> = vec![rot270, rot90];
        let result = compile_nodes(&nodes, &[], 0, 0).unwrap();
        assert!(result.encode_nodes.is_empty());
        assert!(result.decode_nodes.is_empty());
    }

    #[test]
    fn parse_filter_variants() {
        assert!(parse_filter_opt("").is_none());
        assert_eq!(
            parse_filter_opt("lanczos"),
            Some(zenresize::Filter::Lanczos)
        );
        assert_eq!(
            parse_filter_opt("robidoux"),
            Some(zenresize::Filter::Robidoux)
        );
        assert_eq!(
            parse_filter_opt("ginseng"),
            Some(zenresize::Filter::Ginseng)
        );
        assert!(parse_filter_opt("unknown_filter").is_none());
    }

    #[test]
    fn parse_constraint_mode_aliases() {
        assert_eq!(
            parse_constraint_mode("crop").unwrap(),
            zenresize::ConstraintMode::FitCrop
        );
        assert_eq!(
            parse_constraint_mode("pad").unwrap(),
            zenresize::ConstraintMode::FitPad
        );
        assert_eq!(
            parse_constraint_mode("within").unwrap(),
            zenresize::ConstraintMode::Within
        );
    }

    // ─── DecodeConfig tests ───

    #[test]
    fn decode_config_defaults() {
        let config = DecodeConfig::default();
        assert_eq!(config.hdr_mode, "sdr_only");
        assert_eq!(config.color_intent, "preserve");
        assert_eq!(config.min_size, 0);
    }

    #[test]
    fn decode_config_from_default_node() {
        let decode_node = zennode::nodes::DECODE_NODE.create_default().unwrap();
        let nodes: Vec<Box<dyn NodeInstance>> = vec![decode_node];
        let config = DecodeConfig::from_nodes(&nodes);
        assert_eq!(config.hdr_mode, "sdr_only");
        assert_eq!(config.color_intent, "preserve");
        assert_eq!(config.min_size, 0);
    }

    #[test]
    fn decode_config_from_custom_params() {
        let mut params = zennode::ParamMap::new();
        params.insert(
            "hdr_mode".into(),
            zennode::ParamValue::Str("hdr_reconstruct".into()),
        );
        params.insert(
            "color_intent".into(),
            zennode::ParamValue::Str("srgb".into()),
        );
        params.insert("min_size".into(), zennode::ParamValue::U32(400));

        let decode_node = zennode::nodes::DECODE_NODE.create(&params).unwrap();
        let nodes: Vec<Box<dyn NodeInstance>> = vec![decode_node];
        let config = DecodeConfig::from_nodes(&nodes);
        assert_eq!(config.hdr_mode, "hdr_reconstruct");
        assert_eq!(config.color_intent, "srgb");
        assert_eq!(config.min_size, 400);
    }

    #[test]
    fn decode_config_no_decode_node_returns_defaults() {
        let nodes: Vec<Box<dyn NodeInstance>> = Vec::new();
        let config = DecodeConfig::from_nodes(&nodes);
        assert_eq!(config.hdr_mode, "sdr_only");
        assert_eq!(config.color_intent, "preserve");
        assert_eq!(config.min_size, 0);
    }

    #[test]
    fn decode_config_extracted_in_compile() {
        let mut params = zennode::ParamMap::new();
        params.insert(
            "hdr_mode".into(),
            zennode::ParamValue::Str("preserve".into()),
        );
        params.insert("min_size".into(), zennode::ParamValue::U32(256));

        let decode_node = zennode::nodes::DECODE_NODE.create(&params).unwrap();
        let nodes: Vec<Box<dyn NodeInstance>> = vec![decode_node];
        let result = compile_nodes(&nodes, &[], 0, 0).unwrap();
        assert_eq!(result.decode_config.hdr_mode, "preserve");
        assert_eq!(result.decode_config.min_size, 256);
    }

    // ─── EncodeConfig tests ───

    #[test]
    fn encode_config_defaults() {
        let config = EncodeConfig::default();
        assert!(config.quality_profile.is_none());
        assert!(config.format.is_none());
        assert_eq!(config.dpr, 1.0);
        assert!(config.lossless.is_none());
        assert!(config.codec_params.is_none());
    }

    #[test]
    fn encode_config_no_nodes_returns_defaults() {
        let nodes: Vec<Box<dyn NodeInstance>> = Vec::new();
        let config = EncodeConfig::from_nodes(&nodes);
        assert!(config.quality_profile.is_none());
        assert!(config.format.is_none());
        assert_eq!(config.dpr, 1.0);
    }

    #[test]
    fn encode_config_extracted_in_compile_empty() {
        let nodes: Vec<Box<dyn NodeInstance>> = Vec::new();
        let result = compile_nodes(&nodes, &[], 0, 0).unwrap();
        assert!(result.encode_config.quality_profile.is_none());
        assert!(result.encode_config.format.is_none());
    }

    // ─── Full flow tests ───

    #[test]
    fn full_flow_decode_crop_encode() {
        let decode_node = zennode::nodes::DECODE_NODE.create_default().unwrap();

        let mut crop_params = zennode::ParamMap::new();
        crop_params.insert("x".into(), zennode::ParamValue::U32(0));
        crop_params.insert("y".into(), zennode::ParamValue::U32(0));
        crop_params.insert("w".into(), zennode::ParamValue::U32(200));
        crop_params.insert("h".into(), zennode::ParamValue::U32(150));
        let crop_node = zenlayout::zennode_defs::CROP_NODE
            .create(&crop_params)
            .unwrap();

        let nodes: Vec<Box<dyn NodeInstance>> = vec![decode_node, crop_node];
        let result = compile_nodes(&nodes, &[], 0, 0).unwrap();

        assert_eq!(result.decode_nodes.len(), 1);
        assert!(result.encode_nodes.is_empty());
        assert_eq!(result.decode_config.hdr_mode, "sdr_only");
        assert!(result.encode_config.quality_profile.is_none());
    }
}
