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
/// Captures nodes, edges, and change reasons so the full graph topology
/// and mutation history are preserved across snapshots. Nodes carry stable
/// UIDs that persist across snapshots for tracking movement/coalescence.
#[derive(Clone, Debug)]
pub struct DagSnapshot {
    /// Label for this snapshot (e.g., "input", "after canonical_sort", "compiled graph").
    pub label: String,
    /// Why this snapshot differs from the previous one.
    pub reason: String,
    /// Nodes in the DAG.
    pub nodes: Vec<DagSnapshotNode>,
    /// Edges connecting nodes.
    pub edges: Vec<DagSnapshotEdge>,
}

/// A node in a DAG snapshot.
#[derive(Clone, Debug)]
pub struct DagSnapshotNode {
    /// Stable unique ID (monotonic u32) that persists across snapshots.
    /// When a node is reordered, its uid stays the same.
    /// When nodes are coalesced, the new node gets a new uid
    /// and `merged_from` lists the source uids.
    pub uid: u32,
    /// Position index within this snapshot (for layout).
    pub position: usize,
    /// Short label (schema ID, NodeOp name, or description).
    pub label: String,
    /// Node kind for rendering (e.g., "source", "geometry", "filter", "encode", "implicit").
    pub kind: String,
    /// Where this node originated (e.g., "Ir4Expand:Resample2D", "RIAPI:w=800").
    pub origin: Option<String>,
    /// UIDs of nodes that were merged into this one (empty if not coalesced).
    pub merged_from: Vec<u32>,
    /// True if this node was added in this snapshot (not present in previous).
    pub added: bool,
    /// True if this node was removed in this snapshot (present in previous, gone now).
    pub removed: bool,
}

/// An edge in a DAG snapshot.
#[derive(Clone, Debug)]
pub struct DagSnapshotEdge {
    /// Source node UID.
    pub from: u32,
    /// Target node UID.
    pub to: u32,
    /// Edge kind ("input" or "canvas").
    pub kind: String,
}

impl DagSnapshotEdge {
    /// Create an input edge.
    pub fn input(from: u32, to: u32) -> Self {
        Self {
            from,
            to,
            kind: String::from("input"),
        }
    }

    /// Create a canvas edge.
    pub fn canvas(from: u32, to: u32) -> Self {
        Self {
            from,
            to,
            kind: String::from("canvas"),
        }
    }
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

    /// Where this node originated — provenance chain.
    /// E.g., "Ir4Expand:Resample2D → translate:Resample2DNode → bridge:Resize"
    pub origin: Option<String>,

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
            origin: None,
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

// ─── Tracer (aspect-oriented trace facade) ───

/// Zero-cost trace facade for the graph compiler.
///
/// When inactive (`inner: None`), all methods are no-ops with zero allocations.
/// When active, records nodes, implicit format conversions, and runtime notes
/// into a shared `PipelineTrace`.
///
/// This replaces scattered `#[cfg(feature = "std")]` blocks and `if let Some(trace)`
/// checks throughout graph compilation. The compiler sees through the `is_none()`
/// early returns and eliminates dead code.
#[cfg(feature = "std")]
#[derive(Clone)]
pub struct Tracer {
    inner: Option<alloc::sync::Arc<std::sync::Mutex<PipelineTrace>>>,
    timing: bool,
}

#[cfg(feature = "std")]
impl Tracer {
    /// Inactive tracer — all methods are no-ops, zero allocations.
    pub fn inactive() -> Self {
        Self {
            inner: None,
            timing: false,
        }
    }

    /// Active tracer backed by a shared `PipelineTrace`.
    pub fn active(
        trace: alloc::sync::Arc<std::sync::Mutex<PipelineTrace>>,
        timing: bool,
    ) -> Self {
        Self {
            inner: Some(trace),
            timing,
        }
    }

    /// Whether tracing is active.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.inner.is_some()
    }

    /// Get a `TraceAppender` for content-adaptive closures. Returns `None` if inactive.
    pub fn appender(&self) -> Option<TraceAppender> {
        self.inner.as_ref().map(|t| TraceAppender::new(t.clone()))
    }

    /// Record a compiled node in the trace.
    ///
    /// Called by `compile_node` after `compile_node_inner` returns.
    /// Only allocates (name string, description, timing arc) when active.
    /// Returns a `TracingSource`-wrapped source when active, or the source unchanged.
    pub fn wrap_compiled_node(
        &self,
        source: Box<dyn crate::Source>,
        node_id: usize,
        op_name: &'static str,
        op_desc_fn: impl FnOnce() -> String,
        materializes: bool,
        upstream: Option<&UpstreamMeta>,
    ) -> Box<dyn crate::Source> {
        let Some(trace_arc) = &self.inner else {
            return source;
        };

        let (in_fmt, in_w, in_h) = match upstream {
            Some(meta) => (meta.format, meta.width, meta.height),
            None => (source.format(), source.width(), source.height()),
        };

        let timing_arc = if self.timing {
            Some(alloc::sync::Arc::new(std::sync::Mutex::new(
                NodeTiming::default(),
            )))
        } else {
            None
        };

        let trace_order = trace_arc.lock().unwrap().len();
        let entry = TraceEntry {
            index: node_id,
            trace_order,
            name: alloc::string::String::from(op_name),
            description: op_desc_fn(), // only called when active
            origin: None,
            implicit: false,
            implicit_reason: None,
            input_format: in_fmt,
            input_width: in_w,
            input_height: in_h,
            output_format: source.format(),
            output_width: source.width(),
            output_height: source.height(),
            materializes,
            notes: Vec::new(),
            timing: timing_arc.clone(),
        };
        trace_arc.lock().unwrap().push(entry.clone());
        let mut wrapped = crate::sources::TracingSource::new(source, &entry, None);
        if let Some(timing) = timing_arc {
            wrapped = wrapped.with_timing(timing);
        }
        Box::new(wrapped)
    }

    /// Insert a format conversion, recording an implicit trace entry when active.
    ///
    /// When inactive, equivalent to a plain `ensure_format` — checks if conversion
    /// is needed and inserts a `RowConverterOp` if so. Zero allocations when inactive.
    pub fn ensure_format(
        &self,
        source: Box<dyn crate::Source>,
        target: crate::format::PixelFormat,
        reason: &str,
    ) -> Result<Box<dyn crate::Source>, crate::error::PipeError> {
        let current = source.format();
        if current == target {
            return Ok(source);
        }

        let in_w = source.width();
        let in_h = source.height();

        let op = crate::ops::RowConverterOp::new(current, target).ok_or_else(|| {
            crate::error::PipeError::Op(alloc::format!(
                "no conversion path from {current} to {target}"
            ))
        })?;
        let result: Box<dyn crate::Source> =
            Box::new(crate::sources::TransformSource::new(source).push(op));

        // Record implicit conversion — only allocates when active.
        if let Some(trace_arc) = &self.inner {
            let mut trace = trace_arc.lock().unwrap();
            let trace_order = trace.len();
            trace.push(TraceEntry {
                index: usize::MAX,
                trace_order,
                name: alloc::string::String::from("ConvertFormat"),
                description: alloc::format!(
                    "{} -> {} (for {})",
                    format_short(&current),
                    format_short(&target),
                    reason
                ),
                origin: None,
                implicit: true,
                implicit_reason: Some(alloc::format!(
                    "{reason} requires {}",
                    format_short(&target)
                )),
                input_format: current,
                input_width: in_w,
                input_height: in_h,
                output_format: target,
                output_width: in_w,
                output_height: in_h,
                materializes: false,
                notes: Vec::new(),
                timing: None,
            });
        }

        Ok(result)
    }

    /// Add a runtime note to the trace (e.g., content-adaptive detection results).
    ///
    /// Pushes an implicit entry with the given name and description. No-op when inactive.
    pub fn note_implicit(
        &self,
        name: &str,
        description: String,
        reason: &str,
        input_format: crate::format::PixelFormat,
        input_w: u32,
        input_h: u32,
        output_w: u32,
        output_h: u32,
        materializes: bool,
    ) {
        let Some(trace_arc) = &self.inner else {
            return;
        };
        let mut trace = trace_arc.lock().unwrap();
        let trace_order = trace.len();
        trace.push(TraceEntry {
            index: usize::MAX,
            trace_order,
            name: alloc::string::String::from(name),
            description,
            origin: None,
            implicit: true,
            implicit_reason: Some(alloc::string::String::from(reason)),
            input_format,
            input_width: input_w,
            input_height: input_h,
            output_format: input_format,
            output_width: output_w,
            output_height: output_h,
            materializes,
            notes: Vec::new(),
            timing: None,
        });
    }
}

/// Metadata about a node's upstream state, captured before ensure_format.
pub struct UpstreamMeta {
    pub format: crate::format::PixelFormat,
    pub width: u32,
    pub height: u32,
}

impl UpstreamMeta {
    /// Capture from a Source reference.
    #[inline]
    pub fn from_source(source: &dyn crate::Source) -> Self {
        Self {
            format: source.format(),
            width: source.width(),
            height: source.height(),
        }
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

            // Show origin (provenance) if present.
            if let Some(ref origin) = e.origin {
                out.push_str(&format!("        origin: {origin}\n"));
            }
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
        // Build a map from node graph-index (as u32) to trace_order for positioning.
        let index_to_order: hashbrown::HashMap<u32, usize> = self
            .entries
            .iter()
            .map(|e| (e.index as u32, e.trace_order))
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
                    let node_labels: Vec<String> = snap
                        .nodes
                        .iter()
                        .map(|n| format!("{}(#{})", n.label, n.uid))
                        .collect();
                    out.push_str(&format!("  {}:", snap.label));
                    if !snap.reason.is_empty() {
                        out.push_str(&format!(" — {}", snap.reason));
                    }
                    out.push('\n');
                    out.push_str(&format!("    nodes: [{}]\n", node_labels.join(", ")));
                    if !snap.edges.is_empty() {
                        // Build UID → label lookup.
                        let uid_to_label: hashbrown::HashMap<u32, &str> = snap
                            .nodes
                            .iter()
                            .map(|n| (n.uid, n.label.as_str()))
                            .collect();
                        let edge_strs: Vec<String> = snap
                            .edges
                            .iter()
                            .map(|e| {
                                let from = uid_to_label.get(&e.from).copied().unwrap_or("?");
                                let to = uid_to_label.get(&e.to).copied().unwrap_or("?");
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

// ─── Animated SVG ───

#[cfg(feature = "std")]
impl FullPipelineTrace {
    /// Generate an animated SVG showing pipeline transformation over time.
    ///
    /// Each `DagSnapshot` in `bridge.snapshots` becomes a frame. Nodes with
    /// the same UID slide to new positions; new nodes fade in; removed nodes
    /// fade out. Edges follow their endpoints. A timeline bar shows frame labels.
    ///
    /// Uses CSS `@keyframes` — no JavaScript required. Works in browsers and
    /// `feh`/`eog` for static frames.
    pub fn to_animated_svg(&self) -> String {
        use alloc::collections::BTreeMap;
        use core::fmt::Write;

        let snapshots: Vec<&DagSnapshot> = self
            .bridge
            .as_ref()
            .map(|b| b.snapshots.iter().collect())
            .unwrap_or_default();

        if snapshots.is_empty() {
            return String::from("<svg xmlns='http://www.w3.org/2000/svg'><text>No snapshots</text></svg>");
        }

        let node_w = 180u32;
        let node_h = 40u32;
        let gap_x = 30u32;
        let gap_y = 60u32;
        let margin = 30u32;
        let timeline_h = 50u32;
        let frame_dur_s = 2.0f32;
        let num_frames = snapshots.len();
        let total_dur = frame_dur_s * num_frames as f32;

        // Collect all unique UIDs across all snapshots for global positioning.
        let mut all_uids: Vec<u32> = Vec::new();
        for snap in &snapshots {
            for node in &snap.nodes {
                if !all_uids.contains(&node.uid) {
                    all_uids.push(node.uid);
                }
            }
        }

        // Find max nodes in any single snapshot for width.
        let max_nodes_per_snap = snapshots.iter().map(|s| s.nodes.len()).max().unwrap_or(1);
        let svg_w = margin * 2 + max_nodes_per_snap as u32 * (node_w + gap_x);
        let svg_h = margin * 2 + node_h + gap_y + timeline_h;

        let mut s = String::with_capacity(8192);
        let _ = write!(
            s,
            "<svg xmlns='http://www.w3.org/2000/svg' width='{svg_w}' height='{svg_h}' \
             font-family='monospace' font-size='11'>\n"
        );

        // Color map for node kinds.
        let kind_color = |kind: &str| -> &str {
            match kind {
                k if k.contains("Geometry") => "#e3f2fd"
                , k if k.contains("Filter") => "#f3e5f5"
                , k if k.contains("Encode") => "#fff3e0"
                , k if k.contains("Decode") => "#e8f5e9"
                , k if k.contains("implicit") => "#fafafa"
                , _ => "#f5f5f5"
            }
        };

        // Build per-UID keyframes: for each frame, record (x, y, opacity).
        // A node's x is its position index * (node_w + gap_x), y is constant for linear.
        #[derive(Clone)]
        struct NodeFrame {
            x: u32,
            y: u32,
            opacity: f32,
            label: String,
            kind: String,
        }

        let mut uid_frames: BTreeMap<u32, Vec<Option<NodeFrame>>> = BTreeMap::new();
        for uid in &all_uids {
            uid_frames.insert(*uid, vec![None; num_frames]);
        }

        for (fi, snap) in snapshots.iter().enumerate() {
            for node in &snap.nodes {
                let x = margin + node.position as u32 * (node_w + gap_x);
                let y = margin;
                if let Some(frames) = uid_frames.get_mut(&node.uid) {
                    frames[fi] = Some(NodeFrame {
                        x,
                        y,
                        opacity: if node.removed { 0.3 } else { 1.0 },
                        label: node.label.clone(),
                        kind: node.kind.clone(),
                    });
                }
            }
        }

        // Generate CSS keyframes for each UID.
        let _ = write!(s, "<defs><style>\n");
        for (uid, frames) in &uid_frames {
            let _ = write!(s, "@keyframes n{uid} {{\n");
            for (fi, frame) in frames.iter().enumerate() {
                let pct_start = (fi as f32 / num_frames as f32 * 100.0) as u32;
                let pct_end = ((fi as f32 + 0.9) / num_frames as f32 * 100.0).min(100.0) as u32;
                let (x, y, opacity) = match frame {
                    Some(f) => (f.x, f.y, f.opacity),
                    None => (0, 0, 0.0), // not present in this frame
                };
                let _ = write!(
                    s,
                    "  {pct_start}%,{pct_end}% {{ transform:translate({x}px,{y}px); opacity:{opacity}; }}\n"
                );
            }
            let _ = write!(s, "}}\n");
        }

        // Timeline indicator animation.
        let _ = write!(s, "@keyframes timeline {{");
        for fi in 0..num_frames {
            let pct = (fi as f32 / num_frames as f32 * 100.0) as u32;
            let x = margin + fi as u32 * (svg_w - margin * 2) / num_frames.max(1) as u32;
            let _ = write!(s, " {pct}% {{ transform:translateX({x}px); }}");
        }
        let _ = write!(s, " }}\n");

        let _ = write!(s, ".node {{ animation-duration:{total_dur}s; animation-iteration-count:infinite; animation-timing-function:ease-in-out; }}\n");
        let _ = write!(s, ".tmark {{ animation:timeline {total_dur}s infinite steps({num_frames}); }}\n");
        let _ = write!(s, "rect.nb {{ rx:6; ry:6; stroke:#bbb; stroke-width:1; }}\n");
        let _ = write!(s, "</style></defs>\n");

        // Render nodes — each is a group with CSS animation.
        for (uid, frames) in &uid_frames {
            // Use the first visible frame for the label and color.
            let first_visible = frames.iter().flatten().next();
            let Some(fv) = first_visible else {
                continue;
            };
            let fill = kind_color(&fv.kind);
            let label = &fv.label;
            // Truncate label for display.
            let display_label = if label.len() > 22 {
                &label[..22]
            } else {
                label
            };

            let _ = write!(
                s,
                "<g class='node' style='animation-name:n{uid}'>\
                 <rect class='nb' x='0' y='0' width='{node_w}' height='{node_h}' fill='{fill}'/>\
                 <text x='6' y='15' font-weight='bold' font-size='10'>{display_label}</text>\
                 <text x='6' y='30' fill='#888' font-size='9'>#{uid}</text>\
                 </g>\n"
            );
        }

        // Timeline bar at bottom.
        let tl_y = svg_h - timeline_h;
        let _ = write!(
            s,
            "<rect x='{margin}' y='{tl_y}' width='{}' height='30' fill='#f0f0f0' rx='4'/>\n",
            svg_w - margin * 2
        );

        // Frame labels on timeline.
        for (fi, snap) in snapshots.iter().enumerate() {
            let x = margin
                + fi as u32 * (svg_w - margin * 2) / num_frames.max(1) as u32
                + 4;
            let label = if snap.label.len() > 16 {
                &snap.label[..16]
            } else {
                &snap.label
            };
            let _ = write!(
                s,
                "<text x='{x}' y='{}' font-size='9' fill='#666'>{label}</text>\n",
                tl_y + 18
            );
        }

        // Animated timeline marker.
        let _ = write!(
            s,
            "<circle class='tmark' cx='0' cy='{}' r='5' fill='#e63946'/>\n",
            tl_y + 15
        );

        s.push_str("</svg>\n");
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
