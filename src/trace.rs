//! Pipeline tracing and debugging — multi-layer trace system.
//!
//! Captures decisions and data flow at every pipeline layer:
//!
//! - **Layer 1 (RIAPI)**: Querystring parsing — which keys mapped to which nodes.
//! - **Layer 2 (Bridge)**: Node compilation — separation, coalescing, converter selection.
//! - **Layer 3 (Graph)**: Graph compilation — per-node format/dims, implicit `ensure_format`.
//! - **Layer 4 (Execution)**: Runtime — strip timing, memory (populated as pipeline drains).
//!
//! Core data structures use only `alloc` (no_std compatible). I/O methods (text/SVG/JSON
//! output, pixel dump) require `std`.
//!
//! # Usage
//!
//! ```ignore
//! let config = TraceConfig::metadata_only();
//! let (source, trace) = graph.compile_traced(sources, &config)?;
//! // drain pipeline...
//! let trace = trace.lock().unwrap();
//! println!("{}", trace.to_text());
//! ```

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::format::PixelFormat;

// ─── Configuration ───

/// What to trace at each pipeline layer.
#[derive(Clone, Debug)]
pub struct TraceConfig {
    /// Enable graph-level metadata (format, dims, alpha at each node).
    pub metadata: bool,

    /// Enable bridge-level tracing (separation, coalescing, converter selection).
    pub bridge: bool,

    /// Enable per-node execution timing.
    #[cfg(feature = "std")]
    pub timing: bool,

    /// Directory to dump pixel snapshots per node.
    #[cfg(feature = "std")]
    pub pixel_dump_dir: Option<std::path::PathBuf>,

    /// Specific node indices to dump (empty = dump all when pixel_dump_dir is set).
    pub dump_nodes: Vec<usize>,
}

impl TraceConfig {
    /// Trace graph-level metadata only. Near-zero cost.
    pub fn metadata_only() -> Self {
        Self {
            metadata: true,
            bridge: false,
            #[cfg(feature = "std")]
            timing: false,
            #[cfg(feature = "std")]
            pixel_dump_dir: None,
            dump_nodes: Vec::new(),
        }
    }

    /// Trace all layers including bridge decisions and execution timing.
    #[cfg(feature = "std")]
    pub fn full() -> Self {
        Self {
            metadata: true,
            bridge: true,
            timing: true,
            pixel_dump_dir: None,
            dump_nodes: Vec::new(),
        }
    }

    /// Trace all layers + dump pixel snapshots to a directory.
    #[cfg(feature = "std")]
    pub fn with_pixel_dump(dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            metadata: true,
            bridge: true,
            timing: true,
            pixel_dump_dir: Some(dir.into()),
            dump_nodes: Vec::new(),
        }
    }

    /// Should node at this index dump pixels?
    pub fn should_dump(&self, index: usize) -> bool {
        #[cfg(feature = "std")]
        {
            self.pixel_dump_dir.is_some()
                && (self.dump_nodes.is_empty() || self.dump_nodes.contains(&index))
        }
        #[cfg(not(feature = "std"))]
        {
            let _ = index;
            false
        }
    }
}

// ─── Layer 1: RIAPI Trace ───

/// Trace of RIAPI querystring parsing.
///
/// Records which keys were consumed by which nodes, what node instances
/// were created, and any warnings from unrecognized or invalid keys.
#[derive(Clone, Debug, Default)]
pub struct RiapiTrace {
    /// The original querystring.
    pub querystring: String,
    /// Every key-value pair and its consumption status.
    pub keys: Vec<RiapiKeyTrace>,
    /// Warnings generated during parsing.
    pub warnings: Vec<String>,
    /// Node instances created, in creation order.
    pub created_nodes: Vec<RiapiNodeTrace>,
}

/// A single key-value pair from the querystring with consumption info.
#[derive(Clone, Debug)]
pub struct RiapiKeyTrace {
    /// The key (lowercased).
    pub key: String,
    /// The value (percent-decoded).
    pub value: String,
    /// Which node schema consumed this key, or None if unconsumed.
    pub consumed_by: Option<String>,
}

/// A node instance created during RIAPI parsing.
#[derive(Clone, Debug)]
pub struct RiapiNodeTrace {
    /// Schema ID of the created node (e.g., "zenlayout.crop").
    pub schema_id: String,
    /// Keys this node consumed.
    pub consumed_keys: Vec<String>,
    /// Whether the node reports is_identity() = true.
    pub is_identity: bool,
}

// ─── Layer 2: Bridge Trace ───

/// Trace of the bridge layer (NodeInstance → PipelineGraph conversion).
///
/// Records node separation, coalescing decisions, converter selection,
/// and optimization snapshots showing node order at each transformation step.
#[derive(Clone, Debug, Default)]
pub struct BridgeTrace {
    /// Input nodes before separation.
    pub input_nodes: Vec<BridgeNodeInfo>,
    /// Schema IDs of nodes separated to decode phase.
    pub decode_nodes: Vec<String>,
    /// Schema IDs of nodes separated to encode phase.
    pub encode_nodes: Vec<String>,
    /// Schema IDs of pixel-processing nodes after separation.
    pub pixel_nodes: Vec<String>,
    /// Source dimensions used for geometry fusion.
    pub source_dims: (u32, u32),
    /// Steps after coalescing, with converter info.
    pub steps: Vec<BridgeStepTrace>,
    /// DAG snapshots at each transformation step.
    ///
    /// Each snapshot captures the full graph topology (nodes + edges)
    /// after a transformation (e.g., "input", "after canonical_sort",
    /// "compiled graph"). Enables timeline visualization of how the
    /// pipeline was built and optimized.
    pub snapshots: Vec<DagSnapshot>,
}

/// A DAG snapshot at a point in the pipeline transformation.
///
/// Captures both nodes and edges so the full graph topology is preserved,
/// not just a linear ordering. Even for linear pipelines, storing edges
/// makes compositing/fan-out/watermark branches visible.
#[derive(Clone, Debug)]
pub struct DagSnapshot {
    /// Label for this snapshot (e.g., "input", "after canonical_sort", "compiled graph").
    pub label: String,
    /// Nodes in the DAG. Index = node ID.
    pub nodes: Vec<DagSnapshotNode>,
    /// Edges connecting nodes.
    pub edges: Vec<DagSnapshotEdge>,
}

/// A node in a DAG snapshot.
#[derive(Clone, Debug)]
pub struct DagSnapshotNode {
    /// Node ID (index in the snapshot's node list).
    pub id: usize,
    /// Short label (schema ID, NodeOp name, or description).
    pub label: String,
    /// Node kind for rendering (e.g., "source", "geometry", "filter", "encode", "implicit").
    pub kind: String,
}

/// An edge in a DAG snapshot.
#[derive(Clone, Debug)]
pub struct DagSnapshotEdge {
    /// Source node ID.
    pub from: usize,
    /// Target node ID.
    pub to: usize,
    /// Edge kind ("input" or "canvas").
    pub kind: String,
}

/// Info about a node before bridge processing.
#[derive(Clone, Debug)]
pub struct BridgeNodeInfo {
    /// Schema ID (e.g., "zenlayout.crop", "zenresize.constrain").
    pub schema_id: String,
    /// Node role as string ("Decode", "Encode", "Geometry", "Filter", etc.).
    pub role: String,
    /// Coalesce group, if any.
    pub coalesce_group: Option<String>,
}

/// A single step in the bridge compilation pipeline.
#[derive(Clone, Debug)]
pub struct BridgeStepTrace {
    /// Whether this step is a single node or a coalesced/fused group.
    pub kind: String,
    /// Schema IDs of source nodes in this step.
    pub source_nodes: Vec<String>,
    /// What handled this step (e.g., "builtin:geometry", "ext:zenfilters").
    pub converter: String,
    /// NodeOp variant names produced.
    pub produced_ops: Vec<String>,
    /// Notes about fusion/optimization decisions.
    pub notes: Vec<String>,
}

// ─── Layer 3: Graph Trace (enhanced TraceEntry) ───

/// One entry per node boundary in the compiled pipeline.
#[derive(Clone, Debug)]
pub struct TraceEntry {
    /// Node index in the graph (from `add_node`).
    pub index: usize,
    /// Sequential position in the trace (0, 1, 2, ...).
    pub trace_order: usize,
    /// Short name (e.g., "Resize", "ConvertFormat").
    pub name: String,
    /// Detailed description (e.g., "800x600 → 400x300 Robidoux").
    pub description: String,

    /// Whether this is an implicit node (inserted by ensure_format).
    pub implicit: bool,
    /// What triggered this implicit insertion.
    pub implicit_reason: Option<String>,

    /// Input format BEFORE this node.
    pub input_format: PixelFormat,
    /// Input dimensions BEFORE this node.
    pub input_width: u32,
    pub input_height: u32,

    /// Output format AFTER this node.
    pub output_format: PixelFormat,
    /// Output dimensions AFTER this node.
    pub output_width: u32,
    pub output_height: u32,

    /// Whether this node materializes (full-frame buffer).
    pub materializes: bool,

    /// Runtime notes — content-adaptive decisions, detection results, etc.
    /// Populated during compilation for nodes like CropWhitespace, Analyze.
    pub notes: Vec<String>,

    /// Execution timing (populated after pipeline drains).
    #[cfg(feature = "std")]
    pub timing: Option<alloc::sync::Arc<std::sync::Mutex<NodeTiming>>>,
}

impl TraceEntry {
    /// Did the format change at this node?
    pub fn format_changed(&self) -> bool {
        self.input_format != self.output_format
    }

    /// Did the dimensions change?
    pub fn dims_changed(&self) -> bool {
        self.input_width != self.output_width || self.input_height != self.output_height
    }

    /// Did alpha mode change?
    pub fn alpha_changed(&self) -> bool {
        self.input_format.has_alpha() != self.output_format.has_alpha()
    }
}

// ─── Layer 4: Execution Trace ───

/// Per-node execution timing.
///
/// Timing is cumulative — it includes time spent in this node AND all
/// upstream nodes. To get per-node time, compute the differential:
/// `node[n].time - node[n-1].time`.
#[cfg(feature = "std")]
#[derive(Clone, Debug, Default)]
pub struct NodeTiming {
    /// Total wall-clock time for all strip pulls through this node.
    pub total_duration: std::time::Duration,
    /// Number of strips pulled.
    pub strip_count: u32,
    /// Bytes processed (strips * width * strip_height * bpp).
    pub bytes_processed: u64,
}

/// Execution-level trace (populated after pipeline drains).
#[cfg(feature = "std")]
#[derive(Clone, Debug, Default)]
pub struct ExecutionTrace {
    /// Total wall-clock time for the full pipeline execution.
    pub total_duration: std::time::Duration,
    /// Total strips produced.
    pub total_strips: u32,
}

// ─── Trace appender (for runtime annotations) ───

/// Lightweight handle for pushing trace entries and notes during compilation.
///
/// Passed to content-adaptive closures (e.g., `AnalyzeBuilder`) so they can
/// record internal decisions and sub-chains. Cloneable and `Send` — safe to
/// move into closures.
#[cfg(feature = "std")]
#[derive(Clone)]
pub struct TraceAppender {
    trace: alloc::sync::Arc<std::sync::Mutex<PipelineTrace>>,
}

#[cfg(feature = "std")]
impl TraceAppender {
    /// Create from a shared trace.
    pub fn new(trace: alloc::sync::Arc<std::sync::Mutex<PipelineTrace>>) -> Self {
        Self { trace }
    }

    /// Push a trace entry for a runtime-created node.
    pub fn push_entry(&self, mut entry: TraceEntry) {
        let mut t = self.trace.lock().unwrap();
        entry.trace_order = t.len();
        t.push(entry);
    }

    /// Add a note to the most recent trace entry.
    ///
    /// Used by content-adaptive nodes to record detection results
    /// (e.g., "detected 10px border left, reference=#FFFFFF").
    pub fn add_note(&self, note: String) {
        let mut t = self.trace.lock().unwrap();
        if let Some(last) = t.entries.last_mut() {
            last.notes.push(note);
        }
    }

    /// Push a sub-chain entry (implicit, with the given name and reason).
    pub fn push_sub_node(
        &self,
        name: &str,
        description: String,
        source: &dyn crate::Source,
    ) {
        let fmt = source.format();
        let w = source.width();
        let h = source.height();
        self.push_entry(TraceEntry {
            index: usize::MAX,
            trace_order: 0, // assigned by push_entry
            name: alloc::string::String::from(name),
            description,
            implicit: true,
            implicit_reason: Some(alloc::string::String::from("Analyze sub-chain")),
            input_format: fmt,
            input_width: w,
            input_height: h,
            output_format: fmt,
            output_width: w,
            output_height: h,
            materializes: false,
            notes: Vec::new(),
            #[cfg(feature = "std")]
            timing: None,
        });
    }
}

// ─── Collected trace ───

/// Collected pipeline trace data (graph layer).
///
/// This is the primary trace type, populated during `compile_traced()`.
/// Contains per-node entries with format/dims transitions and the
/// graph edge topology.
#[derive(Clone, Debug, Default)]
pub struct PipelineTrace {
    /// Per-node trace entries (in compile order).
    pub entries: Vec<TraceEntry>,
    /// Graph edges capturing the DAG topology.
    /// Populated from `PipelineGraph.edges` during `compile_traced()`.
    pub edges: Vec<DagSnapshotEdge>,
}

impl PipelineTrace {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            edges: Vec::new(),
        }
    }

    pub fn push(&mut self, entry: TraceEntry) {
        self.entries.push(entry);
    }

    /// Number of entries (including implicit format conversions).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the trace is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Count of implicit (ensure_format) entries.
    pub fn implicit_count(&self) -> usize {
        self.entries.iter().filter(|e| e.implicit).count()
    }
}

/// Full pipeline trace spanning all layers.
///
/// Assembled by the orchestration layer from per-layer traces.
#[derive(Clone, Debug, Default)]
pub struct FullPipelineTrace {
    /// Layer 1: RIAPI querystring parsing (if applicable).
    pub riapi: Option<RiapiTrace>,
    /// Layer 2: Bridge compilation (if tracing enabled).
    pub bridge: Option<BridgeTrace>,
    /// Layer 3: Graph compilation (always present when tracing).
    pub graph: PipelineTrace,
    /// Layer 4: Execution metrics (populated after pipeline drains).
    #[cfg(feature = "std")]
    pub execution: Option<ExecutionTrace>,
}

// ─── Output formatters ───

#[cfg(feature = "std")]
impl PipelineTrace {
    /// Human-readable text summary of the pipeline.
    pub fn to_text(&self) -> String {
        let implicit = self.implicit_count();
        let mut out = if implicit > 0 {
            format!(
                "Pipeline Trace ({} nodes, {} implicit)\n",
                self.entries.len(),
                implicit
            )
        } else {
            format!("Pipeline Trace ({} nodes)\n", self.entries.len())
        };
        out.push_str(&"=".repeat(90));
        out.push('\n');

        for e in &self.entries {
            let prefix = if e.implicit { "*" } else { " " };

            let dims = if e.dims_changed() {
                format!(
                    "{}x{} -> {}x{}",
                    e.input_width, e.input_height, e.output_width, e.output_height
                )
            } else {
                format!("{}x{}", e.output_width, e.output_height)
            };

            let fmt = if e.format_changed() {
                format!(
                    "{} -> {}",
                    format_short(&e.input_format),
                    format_short(&e.output_format)
                )
            } else {
                format_short(&e.output_format)
            };

            let flags = if e.materializes { " [MAT]" } else { "" };

            let desc = if e.description.is_empty() {
                String::new()
            } else {
                format!("  {}", e.description)
            };

            out.push_str(&format!(
                "[{prefix}{:2}] {:<20} {:>20}  {}{}{}\n",
                e.trace_order, e.name, dims, fmt, flags, desc
            ));

            // Show runtime notes indented below the entry.
            for note in &e.notes {
                out.push_str(&format!("        note: {note}\n"));
            }
        }

        out.push_str(&"=".repeat(90));
        out.push('\n');

        let format_changes = self.entries.iter().filter(|e| e.format_changed()).count();
        let alpha_changes = self.entries.iter().filter(|e| e.alpha_changed()).count();
        let materializations = self.entries.iter().filter(|e| e.materializes).count();
        out.push_str(&format!(
            "Format changes: {} | Alpha changes: {} | Materializations: {} | Implicit: {}\n",
            format_changes, alpha_changes, materializations, implicit
        ));

        out
    }

    /// Generate SVG visualization of the pipeline.
    pub fn to_svg(&self) -> String {
        use core::fmt::Write;
        let node_w = 240u32;
        let node_h = 80u32;
        let gap = 30u32;
        let margin = 20u32;
        let n = self.entries.len().max(1) as u32;
        let total_w = n * (node_w + gap) + margin * 2;
        let total_h = node_h + margin * 2 + 30;

        let mut s = String::with_capacity(4096);
        let _ = write!(
            s,
            "<svg xmlns='http://www.w3.org/2000/svg' width='{total_w}' height='{total_h}' \
             font-family='monospace' font-size='11'>"
        );
        let _ = write!(
            s,
            "<style>\
             rect{{rx:8;ry:8}} \
             .fmt{{fill:#666}} \
             .dim{{fill:#333;font-weight:bold}} \
             .name{{fill:#000;font-weight:bold;font-size:13px}} \
             .desc{{fill:#888;font-size:9px}} \
             .implicit rect{{stroke-dasharray:6,3}}\
             </style>"
        );

        for (i, e) in self.entries.iter().enumerate() {
            let x = margin + i as u32 * (node_w + gap);
            let y = margin;

            let fill = if e.materializes {
                "#ffe0e0"
            } else if e.format_changed() {
                "#fff3e0"
            } else {
                "#e8f5e9"
            };

            let stroke = if e.implicit { "#999" } else { "#ccc" };
            let dash = if e.implicit {
                " stroke-dasharray='6,3'"
            } else {
                ""
            };

            let _ = write!(
                s,
                "<rect x='{x}' y='{y}' width='{node_w}' height='{node_h}' \
                 fill='{fill}' stroke='{stroke}'{dash}/>"
            );

            let label = if e.implicit {
                format!("*{}", e.name)
            } else {
                e.name.clone()
            };
            let _ = write!(
                s,
                "<text x='{}' y='{}' class='name'>{label}</text>",
                x + 8,
                y + 16
            );

            let dims = if e.dims_changed() {
                format!(
                    "{}x{}->{}x{}",
                    e.input_width, e.input_height, e.output_width, e.output_height
                )
            } else {
                format!("{}x{}", e.output_width, e.output_height)
            };
            let _ = write!(
                s,
                "<text x='{}' y='{}' class='dim'>{dims}</text>",
                x + 8,
                y + 33
            );

            let fmt_text = if e.format_changed() {
                format!(
                    "{} -> {}",
                    format_short(&e.input_format),
                    format_short(&e.output_format)
                )
            } else {
                format_short(&e.output_format)
            };
            let _ = write!(
                s,
                "<text x='{}' y='{}' class='fmt'>{fmt_text}</text>",
                x + 8,
                y + 49
            );

            if !e.description.is_empty() {
                // Truncate long descriptions for SVG
                let desc = if e.description.len() > 35 {
                    format!("{}...", &e.description[..32])
                } else {
                    e.description.clone()
                };
                let _ = write!(
                    s,
                    "<text x='{}' y='{}' class='desc'>{desc}</text>",
                    x + 8,
                    y + 65
                );
            }

            if e.alpha_changed() {
                let label = if e.output_format.has_alpha() {
                    "+a"
                } else {
                    "-a"
                };
                let _ = write!(
                    s,
                    "<text x='{}' y='{}' fill='#e63946' font-weight='bold'>{label}</text>",
                    x + node_w - 30,
                    y + 16
                );
            }

        }

        // Draw edges from the graph topology.
        // Build a map from node graph-index to trace_order for positioning.
        let index_to_order: hashbrown::HashMap<usize, usize> = self
            .entries
            .iter()
            .map(|e| (e.index, e.trace_order))
            .collect();

        if !self.edges.is_empty() {
            // Use explicit edges from the graph.
            for edge in &self.edges {
                let Some(&from_order) = index_to_order.get(&edge.from) else {
                    continue;
                };
                let Some(&to_order) = index_to_order.get(&edge.to) else {
                    continue;
                };
                let from_entry = &self.entries[from_order];
                let x1 = margin + from_order as u32 * (node_w + gap) + node_w;
                let x2 = margin + to_order as u32 * (node_w + gap);
                let cy = margin + node_h / 2;

                let color = if from_entry.alpha_changed() {
                    "#e63946"
                } else if from_entry.format_changed() {
                    "#f4a261"
                } else {
                    "#999"
                };

                let dash = if edge.kind == "canvas" {
                    " stroke-dasharray='4,2'"
                } else {
                    ""
                };

                let _ = write!(
                    s,
                    "<line x1='{x1}' y1='{cy}' x2='{x2}' y2='{cy}' \
                     stroke='{color}' stroke-width='2'{dash}/>"
                );
            }
        } else {
            // Fallback: draw linear edges between adjacent entries.
            for i in 0..self.entries.len().saturating_sub(1) {
                let e = &self.entries[i];
                let x1 = margin + i as u32 * (node_w + gap) + node_w;
                let x2 = margin + (i as u32 + 1) * (node_w + gap);
                let cy = margin + node_h / 2;

                let color = if e.alpha_changed() {
                    "#e63946"
                } else if e.format_changed() {
                    "#f4a261"
                } else {
                    "#999"
                };
                let _ = write!(
                    s,
                    "<line x1='{x1}' y1='{cy}' x2='{x2}' y2='{cy}' \
                     stroke='{color}' stroke-width='2'/>"
                );
            }
        }

        s.push_str("</svg>");
        s
    }
}

#[cfg(feature = "std")]
impl FullPipelineTrace {
    /// Human-readable multi-section text output.
    pub fn to_text(&self) -> String {
        let mut out = String::with_capacity(2048);

        // RIAPI section
        if let Some(ref riapi) = self.riapi {
            out.push_str("RIAPI Trace\n");
            out.push_str(&"-".repeat(60));
            out.push('\n');
            out.push_str(&format!("Query: {}\n", riapi.querystring));

            if !riapi.keys.is_empty() {
                out.push_str("Keys:\n");
                for k in &riapi.keys {
                    let status = match &k.consumed_by {
                        Some(node) => format!("-> {node}"),
                        None => "UNCONSUMED".to_string(),
                    };
                    out.push_str(&format!("  {}={} {}\n", k.key, k.value, status));
                }
            }

            if !riapi.created_nodes.is_empty() {
                out.push_str("Nodes created:\n");
                for n in &riapi.created_nodes {
                    let identity = if n.is_identity { " [identity]" } else { "" };
                    out.push_str(&format!(
                        "  {} (keys: {}){}\n",
                        n.schema_id,
                        n.consumed_keys.join(", "),
                        identity
                    ));
                }
            }

            if !riapi.warnings.is_empty() {
                out.push_str("Warnings:\n");
                for w in &riapi.warnings {
                    out.push_str(&format!("  {w}\n"));
                }
            }
            out.push('\n');
        }

        // Bridge section
        if let Some(ref bridge) = self.bridge {
            out.push_str("Bridge Trace\n");
            out.push_str(&"-".repeat(60));
            out.push('\n');
            out.push_str(&format!(
                "Source dims: {}x{}\n",
                bridge.source_dims.0, bridge.source_dims.1
            ));

            if !bridge.decode_nodes.is_empty() {
                out.push_str(&format!("Decode nodes: {}\n", bridge.decode_nodes.join(", ")));
            }
            if !bridge.encode_nodes.is_empty() {
                out.push_str(&format!("Encode nodes: {}\n", bridge.encode_nodes.join(", ")));
            }
            out.push_str(&format!(
                "Pixel nodes: {} total\n",
                bridge.pixel_nodes.len()
            ));

            // DAG snapshots (show pipeline topology at each transformation step).
            if !bridge.snapshots.is_empty() {
                out.push_str("Pipeline DAG timeline:\n");
                for snap in &bridge.snapshots {
                    let node_labels: Vec<&str> =
                        snap.nodes.iter().map(|n| n.label.as_str()).collect();
                    out.push_str(&format!("  {}:\n", snap.label));
                    out.push_str(&format!("    nodes: [{}]\n", node_labels.join(", ")));
                    if !snap.edges.is_empty() {
                        let edge_strs: Vec<String> = snap
                            .edges
                            .iter()
                            .map(|e| {
                                let from = snap
                                    .nodes
                                    .get(e.from)
                                    .map(|n| n.label.as_str())
                                    .unwrap_or("?");
                                let to = snap
                                    .nodes
                                    .get(e.to)
                                    .map(|n| n.label.as_str())
                                    .unwrap_or("?");
                                if e.kind == "canvas" {
                                    format!("{from} =canvas=> {to}")
                                } else {
                                    format!("{from} -> {to}")
                                }
                            })
                            .collect();
                        out.push_str(&format!("    edges: [{}]\n", edge_strs.join(", ")));
                    }
                }
            }

            if !bridge.steps.is_empty() {
                out.push_str("Steps:\n");
                for (i, step) in bridge.steps.iter().enumerate() {
                    out.push_str(&format!(
                        "  [{}] {} ({}) -> {}\n",
                        i,
                        step.kind,
                        step.source_nodes.join("+"),
                        step.produced_ops.join(", ")
                    ));
                    if !step.converter.is_empty() {
                        out.push_str(&format!("      converter: {}\n", step.converter));
                    }
                    for note in &step.notes {
                        out.push_str(&format!("      note: {note}\n"));
                    }
                }
            }
            out.push('\n');
        }

        // Graph section
        out.push_str(&self.graph.to_text());

        // Execution section
        if let Some(ref exec) = self.execution {
            out.push('\n');
            out.push_str("Execution Trace\n");
            out.push_str(&"-".repeat(60));
            out.push('\n');
            out.push_str(&format!(
                "Total: {:?} | Strips: {}\n",
                exec.total_duration, exec.total_strips
            ));

            // Per-node timing (differential)
            let timed: Vec<_> = self
                .graph
                .entries
                .iter()
                .filter_map(|e| {
                    e.timing.as_ref().map(|t| {
                        let t = t.lock().unwrap();
                        (e.name.clone(), e.trace_order, t.clone())
                    })
                })
                .collect();

            if !timed.is_empty() {
                out.push_str("Per-node timing (cumulative):\n");
                for (name, order, timing) in &timed {
                    out.push_str(&format!(
                        "  [{:2}] {:<20} {:>10?}  {} strips  {} bytes\n",
                        order, name, timing.total_duration, timing.strip_count, timing.bytes_processed
                    ));
                }
            }
        }

        out
    }

    /// Generate JSON representation for programmatic analysis.
    pub fn to_json(&self) -> String {
        use core::fmt::Write;
        let mut s = String::with_capacity(4096);
        s.push('{');

        // RIAPI
        if let Some(ref riapi) = self.riapi {
            let _ = write!(s, "\"riapi\":{{\"querystring\":");
            json_string(&mut s, &riapi.querystring);
            s.push_str(",\"keys\":[");
            for (i, k) in riapi.keys.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push_str("{\"key\":");
                json_string(&mut s, &k.key);
                s.push_str(",\"value\":");
                json_string(&mut s, &k.value);
                s.push_str(",\"consumed_by\":");
                match &k.consumed_by {
                    Some(c) => json_string(&mut s, c),
                    None => s.push_str("null"),
                }
                s.push('}');
            }
            s.push_str("],\"nodes\":[");
            for (i, n) in riapi.created_nodes.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push_str("{\"schema_id\":");
                json_string(&mut s, &n.schema_id);
                let _ = write!(s, ",\"is_identity\":{}", n.is_identity);
                s.push('}');
            }
            s.push_str("]},");
        }

        // Graph entries
        s.push_str("\"graph\":[");
        for (i, e) in self.graph.entries.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push('{');
            let _ = write!(s, "\"order\":{}", e.trace_order);
            s.push_str(",\"name\":");
            json_string(&mut s, &e.name);
            let _ = write!(
                s,
                ",\"implicit\":{},\"materializes\":{}",
                e.implicit, e.materializes
            );
            s.push_str(",\"input\":{");
            s.push_str("\"format\":");
            json_string(&mut s, &format_short(&e.input_format));
            let _ = write!(s, ",\"w\":{},\"h\":{}", e.input_width, e.input_height);
            s.push_str("},\"output\":{\"format\":");
            json_string(&mut s, &format_short(&e.output_format));
            let _ = write!(s, ",\"w\":{},\"h\":{}", e.output_width, e.output_height);
            s.push('}');
            if !e.description.is_empty() {
                s.push_str(",\"description\":");
                json_string(&mut s, &e.description);
            }
            s.push('}');
        }
        s.push(']');

        s.push('}');
        s
    }
}

// ─── Helpers ───

/// Short human-readable format description.
pub fn format_short(fmt: &PixelFormat) -> String {
    let layout = match (fmt.channels(), fmt.bytes_per_pixel()) {
        (4, 4) => "RGBA8",
        (3, 3) => "RGB8",
        (4, 16) => "RGBAF32",
        (3, 12) => "RGBF32",
        (4, 8) => "RGBA16",
        (1, 1) => "Gray8",
        _ => "?",
    };
    let transfer = if fmt.is_linear() { "Lin" } else { "sRGB" };
    let alpha = if fmt.has_alpha() { " +a" } else { "" };
    format!("{layout} {transfer}{alpha}")
}

/// Map a NodeOp variant to a short name.
pub fn node_op_name(op: &crate::graph::NodeOp) -> &'static str {
    use crate::graph::NodeOp;
    match op {
        NodeOp::Source => "Source",
        NodeOp::Output => "Output",
        NodeOp::Layout { .. } => "Layout",
        NodeOp::LayoutComposite { .. } => "LayoutComposite",
        NodeOp::Crop { .. } => "Crop",
        NodeOp::Resize { .. } => "Resize",
        NodeOp::Constrain { .. } => "Constrain",
        NodeOp::ResizeAdvanced(_) => "ResizeAdvanced",
        NodeOp::Orient(_) => "Orient",
        NodeOp::AutoOrient(_) => "AutoOrient",
        NodeOp::PixelTransform(_) => "PixelTransform",
        NodeOp::Composite { .. } => "Composite",
        #[cfg(feature = "std")]
        NodeOp::Filter(_) => "Filter",
        #[cfg(feature = "std")]
        NodeOp::IccTransform { .. } => "IccTransform",
        NodeOp::RemoveAlpha { .. } => "RemoveAlpha",
        NodeOp::AddAlpha => "AddAlpha",
        NodeOp::Overlay { .. } => "Overlay",
        NodeOp::Analyze(_) => "Analyze",
        NodeOp::CropWhitespace { .. } => "CropWhitespace",
        NodeOp::ExpandCanvas { .. } => "ExpandCanvas",
        NodeOp::FillRect { .. } => "FillRect",
        NodeOp::Materialize(_) => "Materialize",
    }
}

/// Generate a description string from a NodeOp's parameters.
pub fn node_op_description(op: &crate::graph::NodeOp) -> String {
    use crate::graph::NodeOp;
    match op {
        NodeOp::Source | NodeOp::Output => String::new(),
        NodeOp::Layout { filter, .. } => format!("{filter:?}"),
        NodeOp::LayoutComposite { filter, .. } => format!("composite {filter:?}"),
        NodeOp::Crop { x, y, w, h } => format!("({x},{y}) {w}x{h}"),
        NodeOp::Resize {
            w,
            h,
            filter,
            sharpen_percent,
        } => {
            let f = filter
                .map(|f| format!("{f:?}"))
                .unwrap_or_else(|| "Robidoux".into());
            let s = sharpen_percent
                .map(|p| format!(" sharpen={p:.0}%"))
                .unwrap_or_default();
            format!("-> {w}x{h} {f}{s}")
        }
        NodeOp::Constrain {
            mode,
            w,
            h,
            orientation,
            filter,
        } => {
            let f = filter
                .map(|f| format!("{f:?}"))
                .unwrap_or_else(|| "Robidoux".into());
            let orient = orientation
                .map(|o| format!(" orient={o}"))
                .unwrap_or_default();
            format!("{mode:?} {w}x{h} {f}{orient}")
        }
        NodeOp::ResizeAdvanced(config) => {
            format!("-> {}x{}", config.out_width, config.out_height)
        }
        NodeOp::Orient(o) => format!("{o:?}"),
        NodeOp::AutoOrient(exif) => format!("exif={exif}"),
        NodeOp::PixelTransform(_) => "pixel op".into(),
        NodeOp::Composite { fg_x, fg_y, .. } => format!("at ({fg_x},{fg_y})"),
        #[cfg(feature = "std")]
        NodeOp::Filter(_) => "filter pipeline".into(),
        #[cfg(feature = "std")]
        NodeOp::IccTransform { .. } => "ICC transform".into(),
        NodeOp::RemoveAlpha { matte } => {
            format!("matte=#{:02x}{:02x}{:02x}", matte[0], matte[1], matte[2])
        }
        NodeOp::AddAlpha => String::new(),
        NodeOp::Overlay {
            x,
            y,
            width,
            height,
            opacity,
            ..
        } => {
            format!("{width}x{height} at ({x},{y}) opacity={opacity:.2}")
        }
        NodeOp::Analyze(_) => "content-adaptive".into(),
        NodeOp::CropWhitespace {
            threshold,
            percent_padding,
        } => {
            format!("threshold={threshold} padding={percent_padding:.1}%")
        }
        NodeOp::ExpandCanvas {
            left,
            top,
            right,
            bottom,
            ..
        } => {
            format!("L={left} T={top} R={right} B={bottom}")
        }
        NodeOp::FillRect {
            x1, y1, x2, y2, ..
        } => {
            format!("({x1},{y1})-({x2},{y2})")
        }
        NodeOp::Materialize(_) => "custom transform".into(),
    }
}

/// Whether a NodeOp variant materializes (requires full-frame buffer).
pub fn node_op_materializes(op: &crate::graph::NodeOp) -> bool {
    use crate::graph::NodeOp;
    matches!(
        op,
        NodeOp::Orient(_)
            | NodeOp::AutoOrient(_)
            | NodeOp::CropWhitespace { .. }
            | NodeOp::Analyze(_)
            | NodeOp::FillRect { .. }
            | NodeOp::Materialize(_)
    )
}

/// Escape and quote a string for JSON output.
#[cfg(feature = "std")]
fn json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' => {
                use core::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
