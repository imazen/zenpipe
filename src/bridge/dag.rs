//! DAG-based pipeline building for non-linear processing graphs.

use alloc::boxed::Box;
use alloc::vec::Vec;

use zenode::{NodeInstance, NodeRole};

use crate::error::PipeError;
use crate::graph::{EdgeKind, NodeId, NodeOp, PipelineGraph};

use super::config::{DecodeConfig, EncodeConfig};
use super::convert::convert_single;
use super::{NodeConverter, PipelineResult};

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

/// Build a streaming pipeline from a DAG of zenode nodes.
///
/// For graphs with multiple inputs (compositing, watermarking), represent
/// the processing graph as a list of [`DagNode`] values with explicit
/// input edges. Source nodes have empty `inputs` and must have a
/// corresponding entry in `sources`.
///
/// For linear chains, use [`build_pipeline()`](super::build_pipeline) instead — it's simpler.
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
            let from_gid = dag_to_graph
                .get(input_idx)
                .copied()
                .flatten()
                .ok_or_else(|| {
                    PipeError::Op(alloc::format!(
                        "DAG node {i} references input {input_idx} which has no graph node"
                    ))
                })?;
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
