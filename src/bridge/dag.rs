//! DAG-based pipeline building for non-linear processing graphs.
//!
//! Applies the same coalescing and geometry fusion as the linear
//! [`compile_nodes()`](super::compile_nodes) path, but supports
//! multi-input nodes (compositing, watermarking).
//!
//! Linear sub-chains within the DAG are detected and fused identically
//! to the linear path — adjacent geometry nodes become a single
//! `NodeOp::Layout`, adjacent filter nodes fuse via `NodeConverter`.
//! Fan-out points (one source feeding multiple consumers) are
//! materialized by the underlying graph compiler automatically.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use zennode::{NodeInstance, NodeRole};

#[allow(unused_imports)]
use whereat::at;

use crate::error::PipeError;
use crate::graph::{EdgeKind, NodeOp, PipelineGraph};

use super::config::{DecodeConfig, EncodeConfig};
use super::convert::{coalesce_and_append_chain, separate_by_role};
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

/// Build a streaming pipeline from a DAG of zennode nodes.
///
/// Linear sub-chains within the DAG receive the same coalescing and
/// geometry fusion as [`build_pipeline()`](super::build_pipeline).
/// Multi-input nodes (composite, watermark) act as chain break points.
/// Fan-out points are materialized automatically by the graph compiler.
///
/// Nodes must be in **topological order** (sources first, output last).
///
/// # Arguments
///
/// * `sources` — map from DAG node index to decoded pixel source
/// * `dag` — nodes in topological order
/// * `converters` — extension converters for crate-specific nodes
///
/// # Errors
///
/// Returns [`PipeError`] if compilation fails.
pub fn build_pipeline_dag(
    sources: Vec<(usize, Box<dyn crate::Source>)>,
    dag: &[DagNode],
    converters: &[&dyn NodeConverter],
) -> crate::PipeResult<PipelineResult> {
    let mut decode_nodes: Vec<Box<dyn NodeInstance>> = Vec::new();
    let mut encode_nodes: Vec<Box<dyn NodeInstance>> = Vec::new();

    // Build a successor count for each node to detect fan-out.
    let mut successor_count = alloc::vec![0usize; dag.len()];
    for dag_node in dag.iter() {
        for &input_idx in &dag_node.inputs {
            successor_count[input_idx] += 1;
        }
    }

    // Identify linear chains: runs of nodes where each has exactly one
    // input and its predecessor has exactly one successor (no fan-out).
    // Multi-input nodes and fan-out points break chains.
    let chains = identify_chains(dag, &successor_count);

    // Get source dimensions from the first source (for geometry fusion).
    let (source_w, source_h) = sources
        .first()
        .map(|(_, s)| (s.width(), s.height()))
        .unwrap_or((0, 0));

    let mut graph = PipelineGraph::new();
    // Map from DAG index to graph NodeId.
    let mut dag_to_graph: Vec<Option<crate::graph::NodeId>> = alloc::vec![None; dag.len()];

    // Process each chain or individual node.
    for chain in &chains {
        match chain {
            Chain::Single(idx) => {
                let dag_node = &dag[*idx];
                let role = dag_node.instance.schema().role;

                match role {
                    NodeRole::Decode => {
                        decode_nodes.push(dag_node.instance.clone_boxed());
                        // Still register a Source graph node so downstream
                        // nodes can reference this decode position.
                        let gid = graph.add_node(NodeOp::Source);
                        dag_to_graph[*idx] = Some(gid);
                        continue;
                    }
                    NodeRole::Encode => {
                        encode_nodes.push(dag_node.instance.clone_boxed());
                        continue;
                    }
                    _ => {}
                }

                let node_op = if dag_node.inputs.is_empty() {
                    NodeOp::Source
                } else {
                    let node_ref: &dyn NodeInstance = dag_node.instance.as_ref();
                    super::convert::convert_single(node_ref, converters)?
                };

                let gid = graph.add_node(node_op);
                dag_to_graph[*idx] = Some(gid);
            }
            Chain::Linear(indices) => {
                let sep = separate_by_role(indices.iter().map(|&idx| dag[idx].instance.as_ref()));
                decode_nodes.extend(sep.decode);
                encode_nodes.extend(sep.encode);

                if sep.pixel.is_empty() {
                    continue;
                }

                if let Some((first, last)) = coalesce_and_append_chain(
                    &sep.pixel, converters, source_w, source_h, &mut graph,
                )? {
                    dag_to_graph[indices[0]] = Some(first);
                    dag_to_graph[*indices.last().unwrap()] = Some(last);
                    for &idx in &indices[1..indices.len() - 1] {
                        dag_to_graph[idx] = Some(last);
                    }
                }
            }
        }
    }

    // Wire cross-chain edges (multi-input nodes connecting to their predecessors).
    for (i, dag_node) in dag.iter().enumerate() {
        let Some(to_gid) = dag_to_graph[i] else {
            continue;
        };
        for (edge_idx, &input_idx) in dag_node.inputs.iter().enumerate() {
            // Skip intra-chain edges (already wired above).
            if is_same_chain(i, input_idx, &chains) {
                continue;
            }
            // Find the graph ID for the predecessor.
            // For chain nodes, use the last node in the chain.
            let from_gid =
                find_chain_output(input_idx, &dag_to_graph, &chains).ok_or_else(|| {
                    PipeError::Op(alloc::format!(
                        "DAG node {i} references input {input_idx} which has no graph node"
                    ))
                })?;
            let kind = if edge_idx == 0 {
                EdgeKind::Input
            } else {
                EdgeKind::Canvas
            };
            graph.add_edge(from_gid, to_gid, kind);
        }
    }

    // Find the last pixel-processing node and add Output.
    let last_gid = dag_to_graph
        .iter()
        .rev()
        .find_map(|opt| *opt)
        .ok_or_else(|| at!(PipeError::Op("DAG has no pixel-processing nodes".into())))?;

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

// ─── Chain identification ───

enum Chain {
    /// A single node that's a chain break point (multi-input, fan-out source, or isolated).
    Single(usize),
    /// A linear sub-chain of consecutive nodes (each has one input, predecessor has one successor).
    Linear(Vec<usize>),
}

/// Identify linear sub-chains within the DAG.
///
/// A linear chain is a maximal sequence of nodes where:
/// - Each node (except the first) has exactly one input
/// - Each node (except the last) has exactly one successor
/// - The single input of node[i+1] is node[i]
///
/// Multi-input nodes and fan-out points break chains.
fn identify_chains(dag: &[DagNode], successor_count: &[usize]) -> Vec<Chain> {
    let mut chains = Vec::new();
    let mut visited = alloc::vec![false; dag.len()];

    for start in 0..dag.len() {
        if visited[start] {
            continue;
        }

        // Can this node start a chain?
        // A chain starts at: source nodes (no inputs), multi-input nodes,
        // or nodes whose predecessor has fan-out.
        // Source nodes (no inputs) are always single — they map to NodeOp::Source.
        if dag[start].inputs.is_empty() {
            visited[start] = true;
            chains.push(Chain::Single(start));
            continue;
        }

        // A chain starts at: multi-input nodes, or nodes whose predecessor
        // has fan-out (more than one successor), or nodes whose predecessor
        // is a source (no inputs — already processed as Single).
        let pred_is_source =
            dag[start].inputs.len() == 1 && dag[dag[start].inputs[0]].inputs.is_empty();
        let pred_has_fanout =
            dag[start].inputs.len() == 1 && successor_count[dag[start].inputs[0]] > 1;
        let is_chain_start = dag[start].inputs.len() > 1 || pred_has_fanout || pred_is_source;

        if !is_chain_start && !visited[start] {
            // This node will be picked up as part of another chain.
            continue;
        }

        // Walk forward to build the chain.
        let mut chain = vec![start];
        visited[start] = true;
        let mut current = start;

        loop {
            // Find the unique successor of `current` that has `current` as its only input.
            let next = find_unique_linear_successor(current, dag, successor_count);
            match next {
                Some(n) if !visited[n] => {
                    visited[n] = true;
                    chain.push(n);
                    current = n;
                }
                _ => break,
            }
        }

        if chain.len() == 1 {
            chains.push(Chain::Single(chain[0]));
        } else {
            chains.push(Chain::Linear(chain));
        }
    }

    // Pick up any unvisited nodes (isolated or in cycles — shouldn't happen in valid DAGs).
    for i in 0..dag.len() {
        if !visited[i] {
            visited[i] = true;
            chains.push(Chain::Single(i));
        }
    }

    chains
}

/// Find the unique successor of `node_idx` that forms a linear chain continuation.
///
/// Returns `Some(successor)` if:
/// - `node_idx` has exactly one successor
/// - That successor has exactly one input (which is `node_idx`)
fn find_unique_linear_successor(
    node_idx: usize,
    dag: &[DagNode],
    successor_count: &[usize],
) -> Option<usize> {
    // Must have exactly one successor.
    if successor_count[node_idx] != 1 {
        return None;
    }

    // Find which node has `node_idx` as its input.
    for (i, dag_node) in dag.iter().enumerate() {
        if dag_node.inputs.len() == 1 && dag_node.inputs[0] == node_idx {
            return Some(i);
        }
    }
    None
}

/// Check if two DAG indices are in the same chain.
fn is_same_chain(a: usize, b: usize, chains: &[Chain]) -> bool {
    for chain in chains {
        if let Chain::Linear(indices) = chain {
            let has_a = indices.contains(&a);
            let has_b = indices.contains(&b);
            if has_a && has_b {
                return true;
            }
        }
    }
    false
}

/// Find the graph output NodeId for a DAG node, handling chains.
///
/// For chain nodes, returns the graph ID of the last node in the chain
/// (since the chain compiles to a single fused sub-graph).
fn find_chain_output(
    dag_idx: usize,
    dag_to_graph: &[Option<crate::graph::NodeId>],
    chains: &[Chain],
) -> Option<crate::graph::NodeId> {
    // Direct lookup.
    if let Some(Some(gid)) = dag_to_graph.get(dag_idx) {
        return Some(*gid);
    }
    // Search chains for this index — use the last node's graph ID.
    for chain in chains {
        if let Chain::Linear(indices) = chain {
            if indices.contains(&dag_idx) {
                if let Some(&last) = indices.last() {
                    return dag_to_graph[last];
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use zennode::*;

    // Simple test node that accepts any schema.
    fn mock_node(id: &'static str, role: NodeRole) -> Box<dyn NodeInstance> {
        MockNode::boxed(id, role)
    }

    struct MockNode {
        schema: &'static NodeSchema,
    }

    static FILTER_SCHEMA: NodeSchema = NodeSchema {
        id: "test.filter",
        label: "Filter",
        description: "",
        group: NodeGroup::Other,
        role: NodeRole::Filter,
        params: &[],
        tags: &[],
        coalesce: None,
        format: FormatHint {
            preferred: PixelFormatPreference::Any,
            alpha: AlphaHandling::Process,
            changes_dimensions: false,
            is_neighborhood: false,
        },
        version: 1,
        compat_version: 1,
        json_key: "",
        deny_unknown_fields: false,
        inputs: &[],
    };

    static CROP_SCHEMA: NodeSchema = NodeSchema {
        id: "zenlayout.crop",
        label: "Crop",
        description: "",
        group: NodeGroup::Geometry,
        role: NodeRole::Geometry,
        params: &[],
        tags: &[],
        coalesce: Some(CoalesceInfo {
            group: "layout_plan",
            fusable: true,
            is_target: false,
        }),
        format: FormatHint {
            preferred: PixelFormatPreference::Any,
            alpha: AlphaHandling::Process,
            changes_dimensions: true,
            is_neighborhood: false,
        },
        version: 1,
        compat_version: 1,
        json_key: "",
        deny_unknown_fields: false,
        inputs: &[],
    };

    impl MockNode {
        fn boxed(id: &'static str, role: NodeRole) -> Box<dyn NodeInstance> {
            let schema: &'static NodeSchema = match (id, role) {
                ("zenlayout.crop", _) => &CROP_SCHEMA,
                _ => &FILTER_SCHEMA,
            };
            Box::new(Self { schema })
        }
    }

    impl NodeInstance for MockNode {
        fn schema(&self) -> &'static NodeSchema {
            self.schema
        }
        fn to_params(&self) -> ParamMap {
            ParamMap::new()
        }
        fn get_param(&self, _: &str) -> Option<ParamValue> {
            None
        }
        fn set_param(&mut self, _: &str, _: ParamValue) -> bool {
            false
        }
        fn as_any(&self) -> &dyn core::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
            self
        }
        fn clone_boxed(&self) -> Box<dyn NodeInstance> {
            Box::new(Self {
                schema: self.schema,
            })
        }
    }

    fn make_source(w: u32, h: u32) -> Box<dyn crate::Source> {
        use crate::sources::MaterializedSource;
        let bpp = crate::format::RGBA8_SRGB.bytes_per_pixel() as usize;
        let data = alloc::vec![128u8; w as usize * h as usize * bpp];
        let mat = MaterializedSource::from_data(data, w, h, crate::format::RGBA8_SRGB);
        Box::new(mat)
    }

    #[test]
    fn dag_linear_chain_identified() {
        // Source → A → B: linear chain should be detected.
        let dag = vec![
            DagNode {
                instance: mock_node("source", NodeRole::Filter),
                inputs: vec![],
            },
            DagNode {
                instance: mock_node("zenlayout.crop", NodeRole::Geometry),
                inputs: vec![0],
            },
            DagNode {
                instance: mock_node("zenlayout.crop", NodeRole::Geometry),
                inputs: vec![1],
            },
        ];

        let sc = {
            let mut sc = alloc::vec![0usize; dag.len()];
            for dn in &dag {
                for &i in &dn.inputs {
                    sc[i] += 1;
                }
            }
            sc
        };

        let chains = identify_chains(&dag, &sc);
        // Node 0 is source (Single). Nodes 1-2 should form a Linear chain.
        let has_linear = chains
            .iter()
            .any(|c| matches!(c, Chain::Linear(v) if v.len() == 2));
        assert!(
            has_linear,
            "expected a 2-node linear chain, chains: {}",
            chains.len()
        );
    }

    #[test]
    fn dag_fan_out_breaks_chain() {
        // A → B, A → C: fan-out at A breaks the chain.
        let dag = vec![
            DagNode {
                instance: mock_node("source", NodeRole::Decode),
                inputs: vec![],
            },
            DagNode {
                instance: mock_node("test.filter", NodeRole::Filter),
                inputs: vec![0],
            },
            DagNode {
                instance: mock_node("test.filter", NodeRole::Filter),
                inputs: vec![0],
            },
        ];

        let chains = identify_chains(&dag, &{
            let mut sc = alloc::vec![0usize; dag.len()];
            for dn in &dag {
                for &i in &dn.inputs {
                    sc[i] += 1;
                }
            }
            sc
        });

        // Node 0 has 2 successors → not a linear chain start for extension.
        // Should produce 3 singles (or 1 single + 2 singles).
        let chain_count = chains.len();
        assert!(
            chain_count >= 2,
            "fan-out should break chains, got {chain_count}"
        );
    }

    #[test]
    fn dag_multi_input_breaks_chain() {
        // A, B → C (composite): multi-input at C breaks the chain.
        let dag = vec![
            DagNode {
                instance: mock_node("source1", NodeRole::Decode),
                inputs: vec![],
            },
            DagNode {
                instance: mock_node("source2", NodeRole::Decode),
                inputs: vec![],
            },
            DagNode {
                instance: mock_node("test.filter", NodeRole::Filter),
                inputs: vec![0, 1],
            },
        ];

        let chains = identify_chains(&dag, &{
            let mut sc = alloc::vec![0usize; dag.len()];
            for dn in &dag {
                for &i in &dn.inputs {
                    sc[i] += 1;
                }
            }
            sc
        });

        // Node 2 has 2 inputs → must be a Single, not part of a Linear chain.
        for chain in &chains {
            if let Chain::Linear(indices) = chain {
                assert!(
                    !indices.contains(&2),
                    "multi-input node should not be in a linear chain"
                );
            }
        }
    }
}
