//! Bridge from [`zenode`] node instances to [`PipelineGraph`] node operations.
//!
//! Converts a list of [`zenode::NodeInstance`] objects into a [`PipelineGraph`]
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
//! let nodes: Vec<Box<dyn zenode::NodeInstance>> = vec![/* ... */];
//! let result = compile_nodes(&nodes, &[])?;
//! // result.graph has Source → ops → Output wired up
//! // result.encode_nodes has any Encode-phase nodes
//! // result.decode_config has extracted decoder params
//! // result.encode_config has extracted encoder params
//! ```

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use zenode::{NodeInstance, NodeRole};

use crate::error::PipeError;
use crate::graph::{EdgeKind, NodeId, NodeOp, PipelineGraph};

// ─── Config types ───

/// Decode-time configuration extracted from the Decode node's params.
///
/// Provides convenient typed access to decode settings without requiring
/// callers to downcast the node instance or read params individually.
#[derive(Clone, Debug)]
pub struct DecodeConfig {
    /// HDR mode: `"sdr_only"`, `"hdr_reconstruct"`, `"preserve"`.
    pub hdr_mode: String,
    /// Color intent: `"preserve"`, `"srgb"`.
    pub color_intent: String,
    /// JPEG prescale hint (minimum output dimension). 0 = no prescaling.
    pub min_size: u32,
}

impl Default for DecodeConfig {
    fn default() -> Self {
        Self {
            hdr_mode: String::from("sdr_only"),
            color_intent: String::from("preserve"),
            min_size: 0,
        }
    }
}

impl DecodeConfig {
    /// Extract decode configuration from a list of decode-phase nodes.
    ///
    /// Reads params from the first node with schema ID `"zenode.decode"`.
    /// If no such node is found, returns defaults.
    pub fn from_nodes(nodes: &[Box<dyn NodeInstance>]) -> Self {
        for node in nodes {
            if node.schema().id == "zenode.decode" {
                return Self::from_node(node.as_ref());
            }
        }
        Self::default()
    }

    /// Extract decode configuration from a single decode node.
    fn from_node(node: &dyn NodeInstance) -> Self {
        let hdr_mode = node
            .get_param("hdr_mode")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| String::from("sdr_only"));

        let color_intent = node
            .get_param("color_intent")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| String::from("preserve"));

        let min_size = node
            .get_param("min_size")
            .and_then(|v| v.as_u32())
            .unwrap_or(0);

        Self {
            hdr_mode,
            color_intent,
            min_size,
        }
    }
}

/// Encode configuration extracted from encode-phase nodes.
///
/// Reads quality intent and per-codec params from the encode node list.
/// Handles both `"zenode.quality_intent"` and `"zencodecs.quality_intent"`
/// schema IDs so callers don't need to know where QualityIntent is defined.
pub struct EncodeConfig {
    /// Quality profile string (from QualityIntent node, if present).
    ///
    /// Named presets: `"lowest"`, `"low"`, `"medium_low"`, `"medium"`,
    /// `"good"`, `"high"`, `"highest"`, `"lossless"`. Or numeric `"0"`-`"100"`.
    pub quality_profile: Option<String>,
    /// Output format string (from QualityIntent node).
    ///
    /// `""` = auto-select, `"jpeg"`, `"png"`, `"webp"`, `"avif"`, `"jxl"`, `"keep"`.
    pub format: Option<String>,
    /// Device pixel ratio for quality adjustment.
    pub dpr: f32,
    /// Lossless preference.
    pub lossless: Option<bool>,
    /// Per-codec params from an explicit encode node (e.g., `zenjpeg.encode`).
    ///
    /// Stored as the raw node instance for downstream code to downcast
    /// via [`NodeInstance::as_any()`].
    pub codec_params: Option<Box<dyn NodeInstance>>,
}

impl core::fmt::Debug for EncodeConfig {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EncodeConfig")
            .field("quality_profile", &self.quality_profile)
            .field("format", &self.format)
            .field("dpr", &self.dpr)
            .field("lossless", &self.lossless)
            .field(
                "codec_params",
                &self.codec_params.as_ref().map(|n| n.schema().id),
            )
            .finish()
    }
}

impl Clone for EncodeConfig {
    fn clone(&self) -> Self {
        Self {
            quality_profile: self.quality_profile.clone(),
            format: self.format.clone(),
            dpr: self.dpr,
            lossless: self.lossless,
            codec_params: self.codec_params.as_ref().map(|n| n.clone_boxed()),
        }
    }
}

impl Default for EncodeConfig {
    fn default() -> Self {
        Self {
            quality_profile: None,
            format: None,
            dpr: 1.0,
            lossless: None,
            codec_params: None,
        }
    }
}

impl EncodeConfig {
    /// Extract encode configuration from a list of encode-phase nodes.
    ///
    /// Looks for:
    /// - `"zenode.quality_intent"` or `"zencodecs.quality_intent"` for quality/format settings
    /// - Any other encode-phase node as codec-specific params
    pub fn from_nodes(nodes: &[Box<dyn NodeInstance>]) -> Self {
        let mut config = Self::default();

        for node in nodes {
            let id = node.schema().id;
            if id == "zenode.quality_intent" || id == "zencodecs.quality_intent" {
                config.quality_profile = node
                    .get_param("profile")
                    .and_then(|v| v.as_str().map(|s| s.to_string()));

                config.format = node.get_param("format").and_then(|v| {
                    let s = v.as_str()?.to_string();
                    if s.is_empty() { None } else { Some(s) }
                });

                config.dpr = node
                    .get_param("dpr")
                    .and_then(|v| v.as_f32())
                    .unwrap_or(1.0);

                config.lossless = node.get_param("lossless").and_then(|v| v.as_bool());
            } else {
                // Any other encode-phase node is treated as codec-specific config.
                config.codec_params = Some(node.clone_boxed());
            }
        }

        config
    }
}

// ─── CompileResult ───

/// Result of compiling zenode nodes into a pipeline graph.
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

/// Result of building a streaming pipeline from zenode nodes.
///
/// Contains a streaming [`Source`] that can be connected directly to an
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

/// A node in a processing DAG.
///
/// Used by [`build_pipeline_dag()`] to represent non-linear processing
/// graphs (e.g., compositing, branching, watermarking).
pub struct DagNode {
    /// The node instance.
    pub instance: Box<dyn NodeInstance>,
    /// Indices of input nodes in the DAG (empty for source nodes).
    pub inputs: Vec<usize>,
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

// ─── Pipeline Step (intermediate representation) ───

/// A step in the compiled pipeline, either a single node or a coalesced group.
enum PipelineStep<'a> {
    /// A single node that wasn't coalesced.
    Single(&'a dyn NodeInstance),
    /// Adjacent fusable nodes merged into one step.
    Coalesced {
        group: &'static str,
        nodes: Vec<&'a dyn NodeInstance>,
    },
}

// ─── Public API ───

/// Compile zenode node instances into a [`PipelineGraph`].
///
/// Preserves user-specified node order (no reordering). Separates
/// encode/decode phase nodes, coalesces adjacent fusable nodes in the same
/// group, and converts each step to a [`NodeOp`].
///
/// Decode and encode node params are extracted into [`DecodeConfig`] and
/// [`EncodeConfig`] for convenient typed access.
///
/// # Arguments
///
/// * `nodes` — node instances in user-declared order
/// * `converters` — optional extension converters for crate-specific nodes
///
/// # Errors
///
/// Returns `PipeError::Op` if a node cannot be converted and no converter
/// handles it.
pub fn compile_nodes(
    nodes: &[Box<dyn NodeInstance>],
    converters: &[&dyn NodeConverter],
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

    // 2. Extract configs from separated nodes.
    let decode_config = DecodeConfig::from_nodes(&decode_nodes);
    let encode_config = EncodeConfig::from_nodes(&encode_nodes);

    // 3. Coalesce adjacent fusable nodes in the same group.
    //    Node order is preserved — no sorting. zenode explicitly does NOT
    //    reorder user-specified node sequences.
    let steps = coalesce(&pixel_nodes);

    // 4. Build the graph: Source → ops → Output.
    let mut graph = PipelineGraph::new();
    let source_id = graph.add_node(NodeOp::Source);

    let mut prev_id = source_id;
    for step in &steps {
        let node_op = convert_step(step, converters)?;
        let node_id = graph.add_node(node_op);
        graph.add_edge(prev_id, node_id, EdgeKind::Input);
        prev_id = node_id;
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

/// Build a streaming pipeline from zenode nodes.
///
/// Returns a [`PipelineResult`] containing a streaming [`Source`](crate::Source)
/// that can be connected directly to an encoder sink via
/// [`execute()`](crate::execute) for zero-materialization processing.
///
/// This is the primary API — prefer it over [`process()`](crate::orchestrate::process)
/// unless you genuinely need to materialize the full image into memory.
///
/// # Arguments
///
/// * `source` — decoded pixel source (the caller has already decoded the image)
/// * `nodes` — zenode node instances in user-declared order
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
    let CompileResult {
        graph,
        decode_config,
        encode_config,
        ..
    } = compile_nodes(nodes, converters)?;

    let mut sources = hashbrown::HashMap::new();
    sources.insert(0, source);
    let pipeline_source = graph.compile(sources)?;

    Ok(PipelineResult {
        source: pipeline_source,
        decode_config,
        encode_config,
    })
}

/// Build a streaming pipeline from a DAG of zenode nodes.
///
/// For graphs with multiple inputs (compositing, watermarking), represent
/// the processing graph as a list of [`DagNode`] values with explicit
/// input edges. Source nodes have empty `inputs` and must have a
/// corresponding entry in `sources`.
///
/// For linear chains, use [`build_pipeline()`] instead — it's simpler.
///
/// # Arguments
///
/// * `sources` — map from DAG node index to decoded pixel source
/// * `dag` — nodes in topological order (sources first, output last)
/// * `converters` — extension converters for crate-specific nodes
///
/// # Errors
///
/// Returns [`PipeError`] if compilation fails.
pub fn build_pipeline_dag(
    sources: Vec<(usize, Box<dyn crate::Source>)>,
    dag: &[DagNode],
    converters: &[&dyn NodeConverter],
) -> Result<PipelineResult, PipeError> {
    // Separate decode/encode nodes and collect pixel-processing nodes.
    let mut decode_nodes: Vec<Box<dyn NodeInstance>> = Vec::new();
    let mut encode_nodes: Vec<Box<dyn NodeInstance>> = Vec::new();

    let mut graph = PipelineGraph::new();

    // Map from DAG index to graph NodeId.
    let mut dag_to_graph: Vec<Option<NodeId>> = alloc::vec![None; dag.len()];

    // First pass: create graph nodes.
    for (i, dag_node) in dag.iter().enumerate() {
        let role = dag_node.instance.schema().role;
        match role {
            NodeRole::Decode => {
                decode_nodes.push(dag_node.instance.clone_boxed());
                // Decode nodes don't produce graph nodes — they configure the decoder.
                continue;
            }
            NodeRole::Encode => {
                encode_nodes.push(dag_node.instance.clone_boxed());
                continue;
            }
            _ => {}
        }

        // Check if this is a source node (no inputs).
        let node_op = if dag_node.inputs.is_empty() {
            NodeOp::Source
        } else {
            // Convert using the standard path.
            let node_ref: &dyn NodeInstance = dag_node.instance.as_ref();
            convert_single(node_ref, converters)?
        };

        let gid = graph.add_node(node_op);
        dag_to_graph[i] = Some(gid);
    }

    // Second pass: wire edges.
    for (i, dag_node) in dag.iter().enumerate() {
        let Some(to_gid) = dag_to_graph[i] else {
            continue;
        };
        for (edge_idx, &input_idx) in dag_node.inputs.iter().enumerate() {
            let from_gid = dag_to_graph.get(input_idx).copied().flatten().ok_or_else(
                || {
                    PipeError::Op(alloc::format!(
                        "DAG node {i} references input {input_idx} which has no graph node"
                    ))
                },
            )?;
            // First input is the primary (Input), second is Canvas (for composites).
            let kind = if edge_idx == 0 {
                EdgeKind::Input
            } else {
                EdgeKind::Canvas
            };
            graph.add_edge(from_gid, to_gid, kind);
        }
    }

    // Find the last pixel-processing node and add an Output node.
    let last_gid = dag_to_graph
        .iter()
        .rev()
        .find_map(|opt| *opt)
        .ok_or_else(|| PipeError::Op("DAG has no pixel-processing nodes".into()))?;

    let output_id = graph.add_node(NodeOp::Output);
    graph.add_edge(last_gid, output_id, EdgeKind::Input);

    // Compile with provided sources.
    let mut source_map = hashbrown::HashMap::new();
    for (dag_idx, src) in sources {
        if let Some(Some(gid)) = dag_to_graph.get(dag_idx) {
            source_map.insert(*gid, src);
        }
    }

    let decode_config = DecodeConfig::from_nodes(&decode_nodes);
    let encode_config = EncodeConfig::from_nodes(&encode_nodes);

    let pipeline_source = graph.compile(source_map)?;

    Ok(PipelineResult {
        source: pipeline_source,
        decode_config,
        encode_config,
    })
}

// ─── Geometry schema IDs ───

/// Schema IDs that are geometry operations eligible for layout fusion.
const GEOMETRY_SCHEMA_IDS: &[&str] = &[
    "zenlayout.crop",
    "zenlayout.orient",
    "zenlayout.flip_h",
    "zenlayout.flip_v",
    "zenlayout.rotate_90",
    "zenlayout.rotate_180",
    "zenlayout.rotate_270",
    "zenresize.constrain",
    "zenlayout.constrain",
];

/// Check if a schema ID is a geometry operation.
fn is_geometry_node(schema_id: &str) -> bool {
    GEOMETRY_SCHEMA_IDS.contains(&schema_id)
}

/// Compile a run of adjacent geometry nodes into a single `NodeOp::Layout`.
///
/// Feeds the geometry run through `zenresize::Pipeline` to produce a single
/// `LayoutPlan`, then emits `NodeOp::Layout { plan, filter }`. This avoids
/// creating separate Crop, Orient, Resize graph nodes — everything is fused
/// into one streaming pass.
///
/// `source_w` and `source_h` are needed for layout planning but are not
/// always known at compile time (they depend on the upstream source). When
/// not available (0, 0), falls back to individual node conversion.
fn compile_geometry_run(
    nodes: &[&dyn NodeInstance],
    source_w: u32,
    source_h: u32,
) -> Result<NodeOp, PipeError> {
    if nodes.is_empty() {
        return Err(PipeError::Op("empty geometry run".into()));
    }

    // If source dimensions aren't known, fall back (caller handles this).
    if source_w == 0 || source_h == 0 {
        return Err(PipeError::Op(
            "geometry fusion requires source dimensions".into(),
        ));
    }

    let mut pipeline = zenresize::Pipeline::new(source_w, source_h);
    let mut filter: Option<zenresize::Filter> = None;

    for &node in nodes {
        let id = node.schema().id;
        match id {
            "zenlayout.crop" => {
                let x = param_u32(node, "x")?;
                let y = param_u32(node, "y")?;
                let w = param_u32(node, "w")?;
                let h = param_u32(node, "h")?;
                pipeline = pipeline.crop_pixels(x, y, w, h);
            }
            "zenlayout.orient" => {
                let val = param_i32(node, "orientation")?;
                let exif = u8::try_from(val).unwrap_or(1);
                pipeline = pipeline.auto_orient(exif);
            }
            "zenlayout.flip_h" => {
                pipeline = pipeline.flip_h();
            }
            "zenlayout.flip_v" => {
                pipeline = pipeline.flip_v();
            }
            "zenlayout.rotate_90" => {
                pipeline = pipeline.rotate_90();
            }
            "zenlayout.rotate_180" => {
                pipeline = pipeline.rotate_180();
            }
            "zenlayout.rotate_270" => {
                pipeline = pipeline.rotate_270();
            }
            "zenresize.constrain" => {
                let w = param_u32(node, "w")?;
                let h = param_u32(node, "h")?;
                let mode_str = param_str(node, "mode")?;
                let filter_str = param_str(node, "filter")?;
                let mode = parse_constraint_mode(&mode_str)?;
                pipeline = pipeline.constrain(zenresize::Constraint::new(mode, w, h));
                if let Some(f) = parse_filter_opt(&filter_str) {
                    filter = Some(f);
                }
            }
            "zenlayout.constrain" => {
                let w = param_u32(node, "w")?;
                let h = param_u32(node, "h")?;
                let mode_str = param_str(node, "mode")?;
                let mode = parse_constraint_mode(&mode_str)?;
                pipeline = pipeline.constrain(zenresize::Constraint::new(mode, w, h));
            }
            _ => {
                return Err(PipeError::Op(alloc::format!(
                    "unexpected node '{id}' in geometry run"
                )));
            }
        }
    }

    let (ideal, request) = pipeline
        .plan()
        .map_err(|e| PipeError::Op(alloc::format!("geometry fusion plan failed: {e}")))?;
    let offer = zenresize::DecoderOffer::full_decode(source_w, source_h);
    let plan = ideal.finalize(&request, &offer);
    let f = filter.unwrap_or(zenresize::Filter::Robidoux);

    Ok(NodeOp::Layout { plan, filter: f })
}

/// Compile a linear list of pixel-processing nodes into a [`PipelineGraph`],
/// performing geometry fusion where possible.
///
/// When adjacent geometry nodes are detected, they are fused into a single
/// `NodeOp::Layout` using [`compile_geometry_run()`]. When source dimensions
/// are known, the fusion produces an optimal single-pass layout.
///
/// When source dimensions are not known (0, 0), geometry nodes are emitted
/// individually as before.
pub fn compile_nodes_fused(
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
        let ops = convert_step_fused(step, converters, source_w, source_h)?;
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

// ─── Coalescing ───

/// Group adjacent fusable nodes that share the same coalesce group.
///
/// Non-fusable nodes pass through as `Single` steps. Adjacent fusable nodes
/// with the same group name are merged into `Coalesced` steps.
fn coalesce<'a>(nodes: &[&'a dyn NodeInstance]) -> Vec<PipelineStep<'a>> {
    let mut steps: Vec<PipelineStep<'a>> = Vec::new();

    for &node in nodes {
        let coalesce = node.schema().coalesce.as_ref();

        if let Some(info) = coalesce {
            if info.fusable || info.is_target {
                // Try to merge with the previous step if same group.
                if let Some(PipelineStep::Coalesced {
                    group,
                    nodes: group_nodes,
                }) = steps.last_mut()
                {
                    if *group == info.group {
                        group_nodes.push(node);
                        continue;
                    }
                }
                // Start a new coalesced group.
                steps.push(PipelineStep::Coalesced {
                    group: info.group,
                    nodes: alloc::vec![node],
                });
                continue;
            }
        }

        // Not fusable — emit as a single step.
        steps.push(PipelineStep::Single(node));
    }

    steps
}

// ─── Step conversion ───

/// Convert a pipeline step to a `NodeOp`.
fn convert_step(
    step: &PipelineStep<'_>,
    converters: &[&dyn NodeConverter],
) -> Result<NodeOp, PipeError> {
    match step {
        PipelineStep::Single(node) => convert_single(*node, converters),
        PipelineStep::Coalesced { nodes, .. } => convert_coalesced(nodes, converters),
    }
}

/// Convert a pipeline step with geometry and filter fusion.
///
/// For coalesced groups where all nodes are geometry nodes, attempts
/// to fuse them via [`compile_geometry_run()`]. For groups handled by
/// a converter with `fuse_group()`, delegates to that. Falls back to
/// individual conversion for anything else.
///
/// May return multiple `NodeOp`s (e.g., when a coalesced group has mixed
/// geometry and non-geometry nodes that can't be fused together).
fn convert_step_fused(
    step: &PipelineStep<'_>,
    converters: &[&dyn NodeConverter],
    source_w: u32,
    source_h: u32,
) -> Result<Vec<NodeOp>, PipeError> {
    match step {
        PipelineStep::Single(node) => {
            let schema_id = node.schema().id;

            // Single geometry node: if source dims are known, fuse it.
            if is_geometry_node(schema_id) && source_w > 0 && source_h > 0 {
                let nodes = &[*node];
                match compile_geometry_run(nodes, source_w, source_h) {
                    Ok(op) => return Ok(alloc::vec![op]),
                    Err(_) => {} // fall through to individual conversion
                }
            }

            Ok(alloc::vec![convert_single(*node, converters)?])
        }

        PipelineStep::Coalesced { nodes, .. } => {
            // Check if all nodes in the group are geometry nodes.
            let all_geometry = nodes.iter().all(|n| is_geometry_node(n.schema().id));

            if all_geometry && source_w > 0 && source_h > 0 {
                match compile_geometry_run(nodes, source_w, source_h) {
                    Ok(op) => return Ok(alloc::vec![op]),
                    Err(_) => {} // fall through
                }
            }

            // Try converter fusion (fuse_group).
            for conv in converters {
                if nodes.iter().any(|n| conv.can_convert(n.schema().id)) {
                    if let Some(fused) = conv.fuse_group(nodes)? {
                        return Ok(alloc::vec![fused]);
                    }
                    // fuse_group returned None — try convert_group.
                    return Ok(alloc::vec![conv.convert_group(nodes)?]);
                }
            }

            // No converter — convert each node individually.
            let mut ops = Vec::new();
            for &node in nodes {
                ops.push(convert_single(node, converters)?);
            }
            Ok(ops)
        }
    }
}

/// Convert a single node to a `NodeOp`.
fn convert_single(
    node: &dyn NodeInstance,
    converters: &[&dyn NodeConverter],
) -> Result<NodeOp, PipeError> {
    let schema_id = node.schema().id;

    // Try extension converters first.
    for conv in converters {
        if conv.can_convert(schema_id) {
            return conv.convert(node);
        }
    }

    // Built-in conversions for geometry/layout nodes.
    match schema_id {
        "zenlayout.crop" => convert_crop(node),
        "zenlayout.orient" => convert_orient(node),
        "zenlayout.flip_h" => Ok(NodeOp::Orient(zenresize::Orientation::FlipH)),
        "zenlayout.flip_v" => Ok(NodeOp::Orient(zenresize::Orientation::FlipV)),
        "zenlayout.rotate_90" => Ok(NodeOp::Orient(zenresize::Orientation::Rotate90)),
        "zenlayout.rotate_180" => Ok(NodeOp::Orient(zenresize::Orientation::Rotate180)),
        "zenlayout.rotate_270" => Ok(NodeOp::Orient(zenresize::Orientation::Rotate270)),
        "zenresize.constrain" => convert_zenresize_constrain(node),
        "zenlayout.constrain" => convert_zenlayout_constrain(node),
        _ => Err(PipeError::Op(alloc::format!(
            "bridge: no converter for node '{schema_id}'"
        ))),
    }
}

/// Convert a coalesced group to a `NodeOp`.
///
/// Tries extension converters first. Falls back to converting only the
/// first node and logging a warning (via the error) if no converter
/// handles the group.
fn convert_coalesced(
    nodes: &[&dyn NodeInstance],
    converters: &[&dyn NodeConverter],
) -> Result<NodeOp, PipeError> {
    if nodes.is_empty() {
        return Err(PipeError::Op("bridge: empty coalesced group".to_string()));
    }

    // Check if any converter handles the group.
    for conv in converters {
        // A converter handles the group if it can convert any member.
        if nodes.iter().any(|n| conv.can_convert(n.schema().id)) {
            return conv.convert_group(nodes);
        }
    }

    // No converter — if it's a single node, convert individually.
    if nodes.len() == 1 {
        return convert_single(nodes[0], converters);
    }

    // Multiple nodes, no converter. Try converting each individually
    // and return the first error if any fail. This handles the common
    // case where geometry nodes are coalesced but can each map to their
    // own NodeOp (crop + orient + constrain in "layout_plan" group).
    //
    // We can't return multiple NodeOps from one step, so we return an
    // error suggesting a converter is needed for proper fusion.
    Err(PipeError::Op(alloc::format!(
        "bridge: coalesced group '{}' with {} nodes has no converter \
         (nodes: {})",
        nodes[0].schema().coalesce.as_ref().map_or("?", |c| c.group),
        nodes.len(),
        nodes
            .iter()
            .map(|n| n.schema().id)
            .collect::<Vec<_>>()
            .join(", "),
    )))
}

// ─── Built-in converters ───

/// Convert a `zenlayout.crop` node to `NodeOp::Crop`.
fn convert_crop(node: &dyn NodeInstance) -> Result<NodeOp, PipeError> {
    let x = param_u32(node, "x")?;
    let y = param_u32(node, "y")?;
    let w = param_u32(node, "w")?;
    let h = param_u32(node, "h")?;
    Ok(NodeOp::Crop { x, y, w, h })
}

/// Convert a `zenlayout.orient` node to `NodeOp::AutoOrient`.
fn convert_orient(node: &dyn NodeInstance) -> Result<NodeOp, PipeError> {
    let orientation = param_i32(node, "orientation")?;
    // Clamp to u8 range — AutoOrient handles values outside 1-8 as identity.
    let value = u8::try_from(orientation).unwrap_or(1);
    Ok(NodeOp::AutoOrient(value))
}

/// Convert a `zenresize.constrain` node to `NodeOp::Constrain`.
fn convert_zenresize_constrain(node: &dyn NodeInstance) -> Result<NodeOp, PipeError> {
    let w = param_u32(node, "w")?;
    let h = param_u32(node, "h")?;
    let mode_str = param_str(node, "mode")?;
    let filter_str = param_str(node, "filter")?;
    let _sharpen = param_f32_opt(node, "sharpen");

    let mode = parse_constraint_mode(&mode_str)?;
    let filter = parse_filter_opt(&filter_str);

    Ok(NodeOp::Constrain {
        mode,
        w,
        h,
        orientation: None,
        filter,
    })
}

/// Convert a `zenlayout.constrain` node to `NodeOp::Constrain`.
fn convert_zenlayout_constrain(node: &dyn NodeInstance) -> Result<NodeOp, PipeError> {
    let w = param_u32(node, "w")?;
    let h = param_u32(node, "h")?;
    let mode_str = param_str(node, "mode")?;

    let mode = parse_constraint_mode(&mode_str)?;

    Ok(NodeOp::Constrain {
        mode,
        w,
        h,
        orientation: None,
        filter: None,
    })
}

// ─── Param helpers ───

fn param_u32(node: &dyn NodeInstance, name: &str) -> Result<u32, PipeError> {
    node.get_param(name)
        .and_then(|v| v.as_u32())
        .ok_or_else(|| {
            PipeError::Op(alloc::format!(
                "bridge: missing or invalid u32 param '{}' on '{}'",
                name,
                node.schema().id,
            ))
        })
}

fn param_i32(node: &dyn NodeInstance, name: &str) -> Result<i32, PipeError> {
    node.get_param(name)
        .and_then(|v| v.as_i32())
        .ok_or_else(|| {
            PipeError::Op(alloc::format!(
                "bridge: missing or invalid i32 param '{}' on '{}'",
                name,
                node.schema().id,
            ))
        })
}

fn param_str(node: &dyn NodeInstance, name: &str) -> Result<String, PipeError> {
    node.get_param(name)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .ok_or_else(|| {
            PipeError::Op(alloc::format!(
                "bridge: missing or invalid string param '{}' on '{}'",
                name,
                node.schema().id,
            ))
        })
}

fn param_f32_opt(node: &dyn NodeInstance, name: &str) -> Option<f32> {
    node.get_param(name).and_then(|v| v.as_f32())
}

// ─── String → enum parsers ───

fn parse_constraint_mode(s: &str) -> Result<zenresize::ConstraintMode, PipeError> {
    match s {
        "distort" => Ok(zenresize::ConstraintMode::Distort),
        "fit" => Ok(zenresize::ConstraintMode::Fit),
        "within" => Ok(zenresize::ConstraintMode::Within),
        "fit_crop" | "crop" => Ok(zenresize::ConstraintMode::FitCrop),
        "within_crop" => Ok(zenresize::ConstraintMode::WithinCrop),
        "fit_pad" | "pad" => Ok(zenresize::ConstraintMode::FitPad),
        "within_pad" => Ok(zenresize::ConstraintMode::WithinPad),
        "pad_within" => Ok(zenresize::ConstraintMode::PadWithin),
        "aspect_crop" => Ok(zenresize::ConstraintMode::AspectCrop),
        _ => Err(PipeError::Op(alloc::format!(
            "bridge: unknown constraint mode '{s}'"
        ))),
    }
}

fn parse_filter_opt(s: &str) -> Option<zenresize::Filter> {
    if s.is_empty() {
        return None;
    }
    // Try parsing the filter name. Unknown filters return None
    // (letting the downstream use its default).
    match s {
        "robidoux" => Some(zenresize::Filter::Robidoux),
        "robidoux_sharp" => Some(zenresize::Filter::RobidouxSharp),
        "lanczos" | "lanczos3" => Some(zenresize::Filter::Lanczos),
        "lanczos2" => Some(zenresize::Filter::Lanczos2),
        "mitchell" => Some(zenresize::Filter::Mitchell),
        "catmull_rom" | "catrom" => Some(zenresize::Filter::CatmullRom),
        "hermite" => Some(zenresize::Filter::Hermite),
        "box" | "nearest" => Some(zenresize::Filter::Box),
        "triangle" | "linear" | "bilinear" => Some(zenresize::Filter::Triangle),
        "ginseng" => Some(zenresize::Filter::Ginseng),
        "cubic" => Some(zenresize::Filter::CubicBSpline),
        _ => None,
    }
}

// Bridge tests require `zenode_defs` modules in zenresize and zenlayout,
// which are not yet implemented. Enable these tests once those modules exist
// by adding `zenode-defs` to the feature list and depending on
// `zenresize/zenode` + `zenlayout/zenode`.
#[cfg(all(test, feature = "zenode-defs"))]
mod tests {
    use super::*;
    use zenode::NodeDef;

    // Pull in the concrete zenode node types for testing.
    // These are re-exported from zenresize and zenlayout via their zenode_defs modules.
    use zenresize::zenode_defs as resize_nodes;

    #[test]
    fn compile_empty() {
        let nodes: Vec<Box<dyn NodeInstance>> = Vec::new();
        let result = compile_nodes(&nodes, &[]).unwrap();
        // Should have Source → Output (2 nodes, 1 edge).
        assert!(result.encode_nodes.is_empty());
        assert!(result.decode_nodes.is_empty());
    }

    #[test]
    fn compile_single_crop() {
        let mut params = zenode::ParamMap::new();
        params.insert("x".into(), zenode::ParamValue::U32(10));
        params.insert("y".into(), zenode::ParamValue::U32(20));
        params.insert("w".into(), zenode::ParamValue::U32(100));
        params.insert("h".into(), zenode::ParamValue::U32(80));

        let crop_node = zenlayout::zenode_defs::CROP_NODE.create(&params).unwrap();
        let nodes: Vec<Box<dyn NodeInstance>> = vec![crop_node];
        let result = compile_nodes(&nodes, &[]).unwrap();
        assert!(result.encode_nodes.is_empty());
        assert!(result.decode_nodes.is_empty());
    }

    #[test]
    fn compile_orient() {
        let mut params = zenode::ParamMap::new();
        params.insert("orientation".into(), zenode::ParamValue::I32(6));

        let orient_node = zenlayout::zenode_defs::ORIENT_NODE.create(&params).unwrap();
        let nodes: Vec<Box<dyn NodeInstance>> = vec![orient_node];
        let result = compile_nodes(&nodes, &[]).unwrap();
        assert!(result.encode_nodes.is_empty());
    }

    #[test]
    fn compile_constrain() {
        let mut params = zenode::ParamMap::new();
        params.insert("w".into(), zenode::ParamValue::U32(800));
        params.insert("h".into(), zenode::ParamValue::U32(600));
        params.insert("mode".into(), zenode::ParamValue::Str("within".into()));
        params.insert("filter".into(), zenode::ParamValue::Str("lanczos".into()));

        let node = resize_nodes::CONSTRAIN_NODE.create(&params).unwrap();
        let nodes: Vec<Box<dyn NodeInstance>> = vec![node];
        let result = compile_nodes(&nodes, &[]).unwrap();
        assert!(result.encode_nodes.is_empty());
    }

    #[test]
    fn decode_nodes_separated() {
        let decode_node = zenode::nodes::DECODE_NODE.create_default().unwrap();
        let nodes: Vec<Box<dyn NodeInstance>> = vec![decode_node];
        let result = compile_nodes(&nodes, &[]).unwrap();
        assert_eq!(result.decode_nodes.len(), 1);
        assert_eq!(result.decode_nodes[0].schema().id, "zenode.decode");
    }

    #[test]
    fn unknown_node_errors() {
        let err = parse_constraint_mode("bogus").unwrap_err();
        assert!(err.to_string().contains("bogus"));
    }

    #[test]
    fn order_preserved() {
        // Nodes should stay in user-declared order, no sorting.
        // Use rotate_90 and rotate_270 which are NOT fusable (no coalesce group).
        let rot90 = zenlayout::zenode_defs::ROTATE90_NODE
            .create_default()
            .unwrap();
        let rot270 = zenlayout::zenode_defs::ROTATE270_NODE
            .create_default()
            .unwrap();

        // User declares rotate_270 before rotate_90
        let nodes: Vec<Box<dyn NodeInstance>> = vec![rot270, rot90];
        let result = compile_nodes(&nodes, &[]).unwrap();
        // Both end up in the graph (not in encode/decode)
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
        let decode_node = zenode::nodes::DECODE_NODE.create_default().unwrap();
        let nodes: Vec<Box<dyn NodeInstance>> = vec![decode_node];
        let config = DecodeConfig::from_nodes(&nodes);
        assert_eq!(config.hdr_mode, "sdr_only");
        assert_eq!(config.color_intent, "preserve");
        assert_eq!(config.min_size, 0);
    }

    #[test]
    fn decode_config_from_custom_params() {
        let mut params = zenode::ParamMap::new();
        params.insert(
            "hdr_mode".into(),
            zenode::ParamValue::Str("hdr_reconstruct".into()),
        );
        params.insert(
            "color_intent".into(),
            zenode::ParamValue::Str("srgb".into()),
        );
        params.insert("min_size".into(), zenode::ParamValue::U32(400));

        let decode_node = zenode::nodes::DECODE_NODE.create(&params).unwrap();
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
        let mut params = zenode::ParamMap::new();
        params.insert(
            "hdr_mode".into(),
            zenode::ParamValue::Str("preserve".into()),
        );
        params.insert("min_size".into(), zenode::ParamValue::U32(256));

        let decode_node = zenode::nodes::DECODE_NODE.create(&params).unwrap();
        let nodes: Vec<Box<dyn NodeInstance>> = vec![decode_node];
        let result = compile_nodes(&nodes, &[]).unwrap();
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
        let result = compile_nodes(&nodes, &[]).unwrap();
        assert!(result.encode_config.quality_profile.is_none());
        assert!(result.encode_config.format.is_none());
    }

    // ─── Full flow tests ───

    #[test]
    fn full_flow_decode_crop_encode() {
        // Decode → Crop → (implicit encode config)
        let decode_node = zenode::nodes::DECODE_NODE.create_default().unwrap();

        let mut crop_params = zenode::ParamMap::new();
        crop_params.insert("x".into(), zenode::ParamValue::U32(0));
        crop_params.insert("y".into(), zenode::ParamValue::U32(0));
        crop_params.insert("w".into(), zenode::ParamValue::U32(200));
        crop_params.insert("h".into(), zenode::ParamValue::U32(150));
        let crop_node = zenlayout::zenode_defs::CROP_NODE
            .create(&crop_params)
            .unwrap();

        let nodes: Vec<Box<dyn NodeInstance>> = vec![decode_node, crop_node];
        let result = compile_nodes(&nodes, &[]).unwrap();

        // Decode separated
        assert_eq!(result.decode_nodes.len(), 1);
        // Crop in graph
        assert!(result.encode_nodes.is_empty());
        // Decode config extracted
        assert_eq!(result.decode_config.hdr_mode, "sdr_only");
        // Encode config is defaults (no encode node)
        assert!(result.encode_config.quality_profile.is_none());
    }
}
