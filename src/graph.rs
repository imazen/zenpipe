//! Graph-based pipeline compiler.
//!
//! Compiles a DAG of image operations into a pull-based [`Source`] chain.
//! The caller builds a [`PipelineGraph`], adds nodes and edges, then calls
//! [`compile`](PipelineGraph::compile) to produce an executable source.
//!
//! Call [`estimate`](PipelineGraph::estimate) first to check resource usage
//! before committing to compilation (which may decode pixels for content-adaptive
//! nodes like [`CropWhitespace`](NodeOp::CropWhitespace)).
//!
//! # Design
//!
//! - **Delegates to zen crates**: zenresize handles orient + resize. Layout nodes
//!   decompose into streaming steps (crop → orient → resize → canvas) with
//!   materialization only when unavoidable (axis-swapping orientations, canvas expansion).
//! - **Estimate before compile**: [`estimate()`](PipelineGraph::estimate) propagates
//!   worst-case dimensions through the graph without allocating or decoding.
//! - **No expand phase**: zen crates handle internal optimization.
//! - **Automatic format conversion**: inserts [`RowConverterOp`](crate::ops::RowConverterOp)
//!   for any format pair supported by zenpixels-convert (sRGB, P3, BT.2020, PQ, HLG, etc.).
//! - **Op fusion**: adjacent [`PixelTransform`](NodeOp::PixelTransform) nodes are fused into
//!   a single [`TransformSource`](crate::sources::TransformSource).
//!
//! # Example
//!
//! ```ignore
//! use zenresize::{Filter, LayoutPlan, Orientation, Size};
//!
//! let mut g = PipelineGraph::new();
//! let src = g.add_node(NodeOp::Source);
//! // Single node handles crop + orient + resize + canvas via zenresize
//! let layout = g.add_node(NodeOp::Layout {
//!     plan: my_layout_plan,
//!     filter: Filter::Robidoux,
//! });
//! let out = g.add_node(NodeOp::Output);
//!
//! g.add_edge(src, layout, EdgeKind::Input);
//! g.add_edge(layout, out, EdgeKind::Input);
//!
//! let pipeline = g.compile(sources)?;
//! ```

use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::Source;
#[allow(unused_imports)]
use whereat::at;

use crate::error::PipeError;
use crate::format::{self, PixelFormat};
use crate::ops::{PixelOp, RowConverterOp};
#[cfg(feature = "std")]
use crate::sources::FilterSource;
use crate::sources::{
    CompositeSource, CropSource, EdgeReplicateSource, ExpandCanvasSource, MaterializedSource,
    ResizeSource, TransformSource,
};

/// Node identifier (index into the graph's node list).
pub type NodeId = usize;

/// A transform applied to a fully materialized pixel buffer.
pub type MaterializeTransform =
    Box<dyn FnOnce(&mut alloc::vec::Vec<u8>, &mut u32, &mut u32, &mut PixelFormat) + Send>;

/// A closure that analyzes a materialized buffer and returns a new source chain.
///
/// Used by [`NodeOp::Analyze`] for content-adaptive operations (e.g., face
/// detection, image classification) that need full-frame pixel access to
/// decide what downstream operations to apply.
///
/// The optional [`TraceAppender`](crate::trace::TraceAppender) allows the
/// closure to record internal decisions and sub-chain nodes in the trace.
/// When tracing is disabled, the appender is `None`.
#[cfg(feature = "std")]
pub type AnalyzeBuilder = Box<
    dyn FnOnce(
            MaterializedSource,
            Option<crate::trace::TraceAppender>,
        ) -> crate::PipeResult<Box<dyn Source>>
        + Send,
>;

/// A closure that analyzes a materialized buffer and returns a new source chain (no-std).
#[cfg(not(feature = "std"))]
pub type AnalyzeBuilder =
    Box<dyn FnOnce(MaterializedSource) -> crate::PipeResult<Box<dyn Source>> + Send>;

/// Metadata about a source, used by [`PipelineGraph::estimate`].
///
/// Provide one per [`NodeOp::Source`] node so the estimator can propagate
/// dimensions through the graph without decoding any pixels.
#[derive(Clone, Debug)]
pub struct SourceInfo {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Pixel format.
    pub format: PixelFormat,
}

/// Resource usage estimate for a compiled pipeline.
///
/// All values are worst-case upper bounds. Actual usage may be lower
/// (e.g., `CropWhitespace` may shrink dimensions, reducing downstream memory).
#[derive(Clone, Debug)]
pub struct ResourceEstimate {
    /// Peak memory for strip buffers, resize ring buffers, etc.
    /// Does NOT include materialization buffers (see [`materialization_bytes`]).
    pub streaming_bytes: u64,
    /// Worst-case materialization buffer (largest single full-frame allocation).
    /// Zero if no node materializes.
    pub materialization_bytes: u64,
    /// Whether any node requires full-frame materialization.
    pub materializes: bool,
    /// Output image width (worst-case).
    pub output_width: u32,
    /// Output image height (worst-case).
    pub output_height: u32,
    /// Output pixel format.
    pub output_format: PixelFormat,
}

impl Default for ResourceEstimate {
    fn default() -> Self {
        Self {
            streaming_bytes: 0,
            materialization_bytes: 0,
            materializes: false,
            output_width: 0,
            output_height: 0,
            output_format: format::RGBA8_SRGB,
        }
    }
}

impl ResourceEstimate {
    /// Total worst-case peak memory (streaming + materialization).
    pub fn peak_memory_bytes(&self) -> u64 {
        self.streaming_bytes + self.materialization_bytes
    }

    /// Check this estimate against resource limits.
    pub fn check(&self, limits: &crate::Limits) -> crate::PipeResult<()> {
        limits.check(self.output_width, self.output_height, self.output_format)?;
        if let Some(max_mem) = limits.max_memory_bytes {
            if self.peak_memory_bytes() > max_mem {
                return Err(at!(PipeError::LimitExceeded(alloc::format!(
                    "estimated peak memory {} bytes exceeds limit {max_mem}",
                    self.peak_memory_bytes()
                ))));
            }
        }
        Ok(())
    }
}

/// Edge type connecting two nodes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    /// Primary pixel data flowing downstream.
    Input,
    /// Background/canvas for compositing (e.g., the background in source-over).
    Canvas,
}

/// An operation that a graph node performs.
pub enum NodeOp {
    // === Sources (leaf nodes — no upstream edges) ===
    /// External source provided by the caller via the `sources` map.
    Source,

    // === Layout: crop + orient + resize + canvas via zenresize ===
    /// Execute a [`LayoutPlan`] — handles crop, orientation, resize, and canvas
    /// placement in a single streaming pass via [`zenresize::streaming_from_plan_batched`].
    ///
    /// Input must be `Rgba8`. All steps (crop, resize, padding, orientation)
    /// are handled inside one [`StreamingResize`](zenresize::StreamingResize)
    /// — no intermediate materialization.
    ///
    /// Falls back to materializing `execute_layout` only for edge extension
    /// (`content_size`).
    Layout {
        plan: zenresize::LayoutPlan,
        filter: zenresize::Filter,
    },

    /// Execute a [`LayoutPlan`] with background compositing.
    ///
    /// - `Input` edge = foreground image
    /// - `Canvas` edge = background image (composited under the resized foreground)
    ///
    /// Both inputs must be `Rgba8`. Delegates to
    /// [`zenresize::execute_layout_with_background`].
    LayoutComposite {
        plan: zenresize::LayoutPlan,
        filter: zenresize::Filter,
    },

    // === Streaming geometry ===
    /// Crop to a rectangle. Streamed via [`CropSource`] — no materialization.
    Crop { x: u32, y: u32, w: u32, h: u32 },

    /// Resize to target dimensions. Streamed via [`ResizeSource`].
    /// Input must be `Rgba8` (converted automatically if needed).
    ///
    /// Optional filter (default: Robidoux), sharpen percentage, and
    /// kernel width scale for zero-cost sharpening during resize.
    Resize {
        w: u32,
        h: u32,
        /// Resampling filter (default: Robidoux if None).
        filter: Option<zenresize::Filter>,
        /// Post-resize sharpening percentage (0-100, applied during resize).
        sharpen_percent: Option<f32>,
    },

    /// Constrain to target dimensions using a zenlayout constraint mode.
    ///
    /// Builds a [`LayoutPlan`] internally from the upstream dimensions,
    /// constraint mode, target size, and optional EXIF orientation.
    /// Equivalent to [`Layout`](NodeOp::Layout) but without requiring
    /// manual [`LayoutPlan`] construction.
    Constrain {
        mode: zenresize::ConstraintMode,
        w: u32,
        h: u32,
        /// Optional EXIF orientation (1-8) to apply during layout.
        orientation: Option<u8>,
        /// Resampling filter (default Robidoux if None).
        filter: Option<zenresize::Filter>,
        /// Post-resize unsharp mask percentage (0-100). None = no unsharp mask.
        unsharp_percent: Option<f32>,
        /// Gravity for crop/pad modes (x: 0.0=left..1.0=right, y: 0.0=top..1.0=bottom).
        /// None = center (0.5, 0.5).
        gravity: Option<(f32, f32)>,
        /// Canvas fill color for pad modes. None = transparent.
        canvas_color: Option<zenresize::CanvasColor>,
        /// Matte color for alpha compositing during resize. None = no matte.
        /// Execution pending zenresize support for matte compositing during resize.
        matte_color: Option<zenresize::CanvasColor>,
        /// Scaling colorspace: true = linear light, false = sRGB gamma. None = linear (default).
        scaling_linear: Option<bool>,
        /// Kernel width scale factor (>1.0 = softer, <1.0 = sharper). None = 1.0.
        kernel_width_scale: Option<f32>,
        /// Negative-lobe ratio for kernel sharpening (zero-cost). None = filter default.
        kernel_lobe_ratio: Option<f32>,
        /// Post-resize Gaussian blur sigma. None = no blur.
        post_blur: Option<f32>,
        /// Upscale resampling filter. None = use the main `filter` for both directions.
        /// When set, this filter is used for axes where output > input (upscaling),
        /// while the main `filter` is used for downscaling axes.
        up_filter: Option<zenresize::Filter>,
        /// When to resample: None = when size differs (default).
        /// Values: "size_differs", "size_differs_or_sharpening_requested", "always".
        resample_when: Option<String>,
        /// When to sharpen: None = when downscaling (default).
        /// Values: "downscaling", "upscaling", "size_differs", "always".
        sharpen_when: Option<String>,
    },

    /// Advanced resize with a pre-built [`ResizeConfig`](zenresize::ResizeConfig).
    ///
    /// Provides access to all zenresize options: kernel_width_scale,
    /// post_blur, kernel_lobe_ratio, padding, source_region, etc.
    /// The config's `in_width`/`in_height` are overridden from upstream at compile time.
    ResizeAdvanced(zenresize::ResizeConfig),

    /// Apply an orientation transform (any of the 8 EXIF orientations).
    /// Delegates to [`zenresize::orient_image`]. Materializes.
    Orient(zenresize::Orientation),

    /// Auto-orient from raw EXIF orientation tag value (1-8).
    ///
    /// Values outside 1-8 are treated as identity (no-op).
    /// Equivalent to `Orient(Orientation::from_exif(value))`.
    AutoOrient(u8),

    // === Per-pixel ops (fusible into TransformSource) ===
    /// A per-pixel operation. Adjacent `PixelTransform` nodes are fused
    /// into a single [`TransformSource`] — one pass, no intermediate buffers.
    PixelTransform(Box<dyn PixelOp>),

    // === Multi-input (streaming) ===
    /// Porter-Duff source-over composite (streaming).
    ///
    /// - `Canvas` edge = background
    /// - `Input` edge = foreground (placed at `fg_x, fg_y`)
    ///
    /// Both inputs are automatically converted to `Rgbaf32LinearPremul`.
    /// Optional blend mode (default: SrcOver if None).
    Composite {
        fg_x: u32,
        fg_y: u32,
        blend_mode: Option<zenblend::BlendMode>,
    },

    // === Filters (zenfilters integration) ===
    /// Apply a [`zenfilters::Pipeline`] of photo filters.
    ///
    /// Input is auto-converted to [`Rgbaf32Linear`](format::RGBAF32_LINEAR).
    /// Per-pixel-only pipelines stream strip-by-strip via [`FilterSource`].
    /// Pipelines with neighborhood filters (blur, clarity, sharpen) use
    /// windowed materialization via [`WindowedFilterSource`] — only
    /// `strip_height + 2 * overlap` rows are buffered at a time instead
    /// of the full image.
    #[cfg(feature = "std")]
    Filter(zenfilters::Pipeline),

    // === ICC color management (requires std for moxcms) ===
    /// Apply an ICC profile transform to the pixel data.
    ///
    /// Converts pixels from the source ICC profile's color space to the
    /// destination ICC profile's color space, row-by-row via moxcms.
    /// The pixel format (layout, depth) is preserved — only color values change.
    ///
    /// Provide the raw ICC profile bytes for source and destination.
    /// The transform is built at compile time from the upstream format.
    #[cfg(feature = "std")]
    IccTransform {
        /// Source ICC profile bytes.
        src_icc: alloc::sync::Arc<[u8]>,
        /// Destination ICC profile bytes.
        dst_icc: alloc::sync::Arc<[u8]>,
    },

    // === Alpha operations ===
    /// Remove alpha channel by compositing onto a solid matte color.
    ///
    /// Produces RGB output (3 bytes/pixel) suitable for JPEG encoding.
    /// Alpha blending is in sRGB space (matching browser behavior).
    RemoveAlpha {
        /// Matte color [R, G, B] in sRGB.
        matte: [u8; 3],
    },

    /// Add an opaque alpha channel to an RGB source.
    ///
    /// RGB → RGBA with A=255. No-op if upstream is already RGBA.
    AddAlpha,

    // === Overlay / watermark ===
    /// Overlay a small image on top of the pipeline output.
    ///
    /// The overlay image is provided as raw pixel data and composited
    /// via Porter-Duff source-over at the given position with opacity.
    /// Both the upstream and overlay are auto-converted to premultiplied
    /// linear f32 for correct blending.
    Overlay {
        /// Raw pixel data of the overlay image (row-major, tightly packed).
        image_data: alloc::vec::Vec<u8>,
        /// Overlay image width.
        width: u32,
        /// Overlay image height.
        height: u32,
        /// Pixel format of the overlay data.
        format: PixelFormat,
        /// X position on the background (clamped to ≥0).
        x: i32,
        /// Y position on the background (clamped to ≥0).
        y: i32,
        /// Opacity factor (0.0 = invisible, 1.0 = full).
        opacity: f32,
        /// Blend mode (default: SrcOver if None).
        blend_mode: Option<zenblend::BlendMode>,
    },

    // === Content-adaptive (materialize + analyze) ===
    /// Analyze materialized pixels, then build a downstream source chain.
    ///
    /// The closure receives the fully materialized upstream image and must
    /// return a [`Source`] to continue the pipeline. This is the low-level
    /// primitive for content-adaptive operations — face detection, image
    /// classification, or any analysis that needs full-frame pixel access
    /// before deciding what operations to apply.
    ///
    /// Not representable in JSON — use named variants like [`CropWhitespace`]
    /// for declarative pipelines.
    ///
    /// During [`estimate()`](PipelineGraph::estimate), treated as worst-case
    /// pass-through (upstream dimensions unchanged).
    Analyze(AnalyzeBuilder),

    /// Detect and crop uniform borders (whitespace trimming).
    ///
    /// Materializes the upstream image, scans inward from each edge to find
    /// where pixel values diverge from the border color by more than
    /// `threshold`, then crops to the content bounds plus `percent_padding`.
    ///
    /// During [`estimate()`](PipelineGraph::estimate), treated as worst-case
    /// no-op (dimensions unchanged — actual crop can only be smaller).
    CropWhitespace {
        /// Color distance threshold (0–255). Pixels within this distance
        /// of the border color are considered "whitespace".
        threshold: u8,
        /// Padding to add around detected content, as a percentage of the
        /// content dimensions (0.0 = tight crop, 0.05 = 5% padding).
        percent_padding: f32,
    },

    // === Canvas ===
    /// Expand the canvas by adding padding around the content.
    ///
    /// Streaming — no materialization. Uses [`ExpandCanvasSource`] internally.
    /// Background color is RGBA [u8; 4].
    ExpandCanvas {
        left: u32,
        top: u32,
        right: u32,
        bottom: u32,
        /// Background color as RGBA bytes.
        bg_color: [u8; 4],
    },

    /// Fill a rectangle on the image with a solid color.
    ///
    /// Materializes the upstream, draws the rectangle, re-streams.
    FillRect {
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
        /// Fill color as RGBA bytes.
        color: [u8; 4],
    },

    // === Barriers ===
    /// Custom materialization barrier — drain upstream, transform, re-stream.
    ///
    /// The closure receives `(data, width, height, format)` and may mutate all.
    /// The label is shown in pipeline traces (e.g., "watermark_overlay", "white_balance").
    Materialize {
        label: &'static str,
        transform: MaterializeTransform,
    },

    /// Terminal output node. `compile()` returns the Source feeding this node.
    Output,
}

struct GraphNode {
    op: Option<NodeOp>,
}

struct Edge {
    from: NodeId,
    to: NodeId,
    kind: EdgeKind,
}

/// Maximum recursion depth for graph compilation/estimation.
/// Prevents stack overflow from cycles or pathologically deep graphs.
const MAX_GRAPH_DEPTH: usize = 256;

// Re-export UpstreamMeta from trace module when std is available.
#[cfg(feature = "std")]
use crate::trace::UpstreamMeta;

// Minimal UpstreamMeta for no_std builds.
#[cfg(not(feature = "std"))]
struct UpstreamMeta {
    format: PixelFormat,
    width: u32,
    height: u32,
}

/// A directed acyclic graph of image operations.
///
/// Build the graph by adding nodes and edges, then call [`compile`](Self::compile)
/// to produce an executable [`Source`] chain.
pub struct PipelineGraph {
    nodes: Vec<GraphNode>,
    edges: Vec<Edge>,
    /// Trace facade — inactive by default, zero-cost no-ops.
    #[cfg(feature = "std")]
    tracer: crate::trace::Tracer,
}

impl PipelineGraph {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            #[cfg(feature = "std")]
            tracer: crate::trace::Tracer::inactive(),
        }
    }

    /// Add a node and return its ID.
    pub fn add_node(&mut self, op: NodeOp) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(GraphNode { op: Some(op) });
        id
    }

    /// Connect `from` → `to` with the given edge kind.
    pub fn add_edge(&mut self, from: NodeId, to: NodeId, kind: EdgeKind) {
        self.edges.push(Edge { from, to, kind });
    }

    /// Validate graph structure: no cycles, valid node references, exactly one output.
    ///
    /// Called automatically by [`estimate()`](Self::estimate) and [`compile()`](Self::compile).
    pub fn validate(&self) -> crate::PipeResult<()> {
        // Must have at least one node
        if self.nodes.is_empty() {
            return Err(at!(PipeError::Op("graph has no nodes".to_string())));
        }

        // Exactly one Output node
        let output_count = self
            .nodes
            .iter()
            .filter(|n| matches!(&n.op, Some(NodeOp::Output)))
            .count();
        if output_count == 0 {
            return Err(at!(PipeError::Op("graph has no Output node".to_string())));
        }
        if output_count > 1 {
            return Err(at!(PipeError::Op(alloc::format!(
                "graph has {output_count} Output nodes, expected 1"
            ))));
        }

        // All edge references must be valid node indices
        for (i, e) in self.edges.iter().enumerate() {
            if e.from >= self.nodes.len() {
                return Err(at!(PipeError::Op(alloc::format!(
                    "edge {i}: source node {} out of range (graph has {} nodes)",
                    e.from,
                    self.nodes.len()
                ))));
            }
            if e.to >= self.nodes.len() {
                return Err(at!(PipeError::Op(alloc::format!(
                    "edge {i}: target node {} out of range (graph has {} nodes)",
                    e.to,
                    self.nodes.len()
                ))));
            }
            if e.from == e.to {
                return Err(at!(PipeError::Op(alloc::format!(
                    "edge {i}: self-loop on node {}",
                    e.from
                ))));
            }
        }

        // Cycle detection via DFS with coloring (white/gray/black)
        // 0=white (unvisited), 1=gray (in progress), 2=black (done)
        let mut color = alloc::vec![0u8; self.nodes.len()];
        for start in 0..self.nodes.len() {
            if color[start] == 0 {
                self.dfs_cycle_check(start, &mut color)?;
            }
        }

        Ok(())
    }

    /// DFS cycle detection. Traverses edges in reverse (to→from is our direction).
    fn dfs_cycle_check(&self, node: usize, color: &mut [u8]) -> crate::PipeResult<()> {
        color[node] = 1; // gray — in progress
        // Follow edges where this node is the target (upstream nodes)
        for e in &self.edges {
            if e.to == node {
                let upstream = e.from;
                if color[upstream] == 1 {
                    return Err(at!(PipeError::Op(alloc::format!(
                        "cycle detected: node {upstream} → node {node}"
                    ))));
                }
                if color[upstream] == 0 {
                    self.dfs_cycle_check(upstream, color)?;
                }
            }
        }
        color[node] = 2; // black — done
        Ok(())
    }

    /// Estimate resource usage without decoding any pixels.
    ///
    /// Propagates worst-case dimensions through the graph and computes
    /// peak memory estimates. Call this before [`compile()`](Self::compile)
    /// to reject oversized requests cheaply.
    ///
    /// `source_info` maps [`NodeOp::Source`] node IDs to their dimensions
    /// (typically from decoder header probes).
    ///
    /// Content-adaptive nodes ([`CropWhitespace`](NodeOp::CropWhitespace),
    /// [`Analyze`](NodeOp::Analyze)) estimate worst-case (upstream dimensions
    /// unchanged), since their actual output can only be smaller.
    pub fn estimate(
        &self,
        source_info: &hashbrown::HashMap<NodeId, SourceInfo>,
    ) -> crate::PipeResult<ResourceEstimate> {
        self.validate()?;
        let output_id = self
            .nodes
            .iter()
            .position(|n| matches!(&n.op, Some(NodeOp::Output)))
            .unwrap(); // safe: validate() ensures exactly one Output

        let mut estimate = ResourceEstimate::default();
        let dims = self.estimate_node(output_id, source_info, &mut estimate, 0)?;
        estimate.output_width = dims.width;
        estimate.output_height = dims.height;
        estimate.output_format = dims.format;
        Ok(estimate)
    }

    fn estimate_node(
        &self,
        node_id: NodeId,
        source_info: &hashbrown::HashMap<NodeId, SourceInfo>,
        est: &mut ResourceEstimate,
        depth: usize,
    ) -> crate::PipeResult<SourceInfo> {
        if depth > MAX_GRAPH_DEPTH {
            return Err(at!(PipeError::Op(alloc::format!(
                "graph depth exceeds {MAX_GRAPH_DEPTH} at node {node_id}"
            ))));
        }
        let op = self
            .nodes
            .get(node_id)
            .and_then(|n| n.op.as_ref())
            .ok_or_else(|| at!(PipeError::Op(alloc::format!("node {node_id} has no op"))))?;

        match op {
            NodeOp::Source => source_info.get(&node_id).cloned().ok_or_else(|| {
                at!(PipeError::Op(alloc::format!(
                    "no source info for node {node_id}"
                )))
            }),

            NodeOp::Output => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                self.estimate_node(input_id, source_info, est, depth + 1)
            }

            NodeOp::Crop { w, h, .. } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.estimate_node(input_id, source_info, est, depth + 1)?;
                // Strip buffer for crop output
                est.streaming_bytes += strip_mem(*w, upstream.format);
                Ok(SourceInfo {
                    width: *w,
                    height: *h,
                    format: upstream.format,
                })
            }

            NodeOp::Resize { w, h, .. } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.estimate_node(input_id, source_info, est, depth + 1)?;
                // Cost-aware: f32 linear sources use ResizeF32Source (no u8 roundtrip).
                let out_fmt = if upstream.format == format::RGBAF32_LINEAR {
                    format::RGBAF32_LINEAR
                } else {
                    format::RGBA8_SRGB
                };
                // Resize ring buffer: kernel_height rows of input width
                let kernel_rows = 16u64; // conservative for most filters
                est.streaming_bytes +=
                    kernel_rows * upstream.width as u64 * upstream.format.bytes_per_pixel() as u64;
                // Output strip buffer
                est.streaming_bytes += strip_mem(*w, out_fmt);
                Ok(SourceInfo {
                    width: *w,
                    height: *h,
                    format: out_fmt,
                })
            }

            NodeOp::Constrain {
                w,
                h,
                mode,
                orientation,
                ..
            } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.estimate_node(input_id, source_info, est, depth + 1)?;
                let (in_w, in_h) = if let Some(exif) = orientation {
                    let o = zenresize::Orientation::from_exif(*exif)
                        .unwrap_or(zenresize::Orientation::Identity);
                    if o.swaps_axes() {
                        (upstream.height, upstream.width)
                    } else {
                        (upstream.width, upstream.height)
                    }
                } else {
                    (upstream.width, upstream.height)
                };
                // Use zenlayout to compute output dimensions
                let mut pipeline = zenresize::Pipeline::new(in_w, in_h);
                if let Some(exif) = orientation {
                    pipeline = pipeline.auto_orient(*exif);
                }
                pipeline = pipeline.constrain(zenresize::Constraint::new(*mode, *w, *h));
                let (ideal, request) = pipeline.plan().map_err(|e| {
                    PipeError::Op(alloc::format!("estimate layout plan failed: {e}"))
                })?;
                let offer = zenresize::DecoderOffer::full_decode(in_w, in_h);
                let plan = ideal.finalize(&request, &offer);
                let out_w = plan.canvas.width;
                let out_h = plan.canvas.height;
                // Resize ring buffer + output strip
                let kernel_rows = 16u64;
                est.streaming_bytes +=
                    kernel_rows * upstream.width as u64 * upstream.format.bytes_per_pixel() as u64;
                est.streaming_bytes += strip_mem(out_w, format::RGBA8_SRGB);
                // Orient may need materialization
                if orientation.is_some() {
                    let o = zenresize::Orientation::from_exif(orientation.unwrap())
                        .unwrap_or(zenresize::Orientation::Identity);
                    if o.swaps_axes() {
                        est.materializes = true;
                        let mat = upstream.width as u64
                            * upstream.height as u64
                            * upstream.format.bytes_per_pixel() as u64;
                        est.materialization_bytes = est.materialization_bytes.max(mat);
                    }
                }
                Ok(SourceInfo {
                    width: out_w,
                    height: out_h,
                    format: format::RGBA8_SRGB,
                })
            }

            NodeOp::Layout { plan, .. } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.estimate_node(input_id, source_info, est, depth + 1)?;
                let out_w = plan.canvas.width;
                let out_h = plan.canvas.height;
                let kernel_rows = 16u64;
                est.streaming_bytes +=
                    kernel_rows * upstream.width as u64 * upstream.format.bytes_per_pixel() as u64;
                est.streaming_bytes += strip_mem(out_w, format::RGBA8_SRGB);
                Ok(SourceInfo {
                    width: out_w,
                    height: out_h,
                    format: format::RGBA8_SRGB,
                })
            }

            NodeOp::LayoutComposite { plan, .. } => {
                let fg_id = self.find_input(node_id, EdgeKind::Input)?;
                let bg_id = self.find_input(node_id, EdgeKind::Canvas)?;
                let fg = self.estimate_node(fg_id, source_info, est, depth + 1)?;
                let bg = self.estimate_node(bg_id, source_info, est, depth + 1)?;
                let out_w = plan.canvas.width.max(bg.width);
                let out_h = plan.canvas.height.max(bg.height);
                let kernel_rows = 16u64;
                est.streaming_bytes +=
                    kernel_rows * fg.width as u64 * fg.format.bytes_per_pixel() as u64;
                est.streaming_bytes += strip_mem(out_w, format::RGBAF32_LINEAR_PREMUL);
                Ok(SourceInfo {
                    width: out_w,
                    height: out_h,
                    format: format::RGBAF32_LINEAR_PREMUL,
                })
            }

            NodeOp::ResizeAdvanced(config) => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.estimate_node(input_id, source_info, est, depth + 1)?;
                let out_w = config.total_output_width();
                let out_h = config.total_output_height();
                let kernel_rows = 16u64;
                est.streaming_bytes +=
                    kernel_rows * upstream.width as u64 * upstream.format.bytes_per_pixel() as u64;
                est.streaming_bytes += strip_mem(out_w, format::RGBA8_SRGB);
                Ok(SourceInfo {
                    width: out_w,
                    height: out_h,
                    format: format::RGBA8_SRGB,
                })
            }

            NodeOp::Orient(_) | NodeOp::AutoOrient(_) => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.estimate_node(input_id, source_info, est, depth + 1)?;
                let orientation = match op {
                    NodeOp::Orient(o) => *o,
                    NodeOp::AutoOrient(exif) => zenresize::Orientation::from_exif(*exif)
                        .unwrap_or(zenresize::Orientation::Identity),
                    _ => unreachable!(),
                };
                let (out_w, out_h) = if orientation.swaps_axes() {
                    (upstream.height, upstream.width)
                } else {
                    (upstream.width, upstream.height)
                };
                if !orientation.is_identity() {
                    est.materializes = true;
                    let mat = upstream.width as u64
                        * upstream.height as u64
                        * format::RGBA8_SRGB.bytes_per_pixel() as u64;
                    est.materialization_bytes = est.materialization_bytes.max(mat);
                }
                Ok(SourceInfo {
                    width: out_w,
                    height: out_h,
                    format: format::RGBA8_SRGB,
                })
            }

            NodeOp::PixelTransform(_) => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.estimate_node(input_id, source_info, est, depth + 1)?;
                // Strip buffer for transform output (format may change)
                est.streaming_bytes += strip_mem(upstream.width, upstream.format);
                Ok(upstream)
            }

            NodeOp::Composite { .. } => {
                let bg_id = self.find_input(node_id, EdgeKind::Canvas)?;
                let fg_id = self.find_input(node_id, EdgeKind::Input)?;
                let bg = self.estimate_node(bg_id, source_info, est, depth + 1)?;
                let _fg = self.estimate_node(fg_id, source_info, est, depth + 1)?;
                est.streaming_bytes += strip_mem(bg.width, format::RGBAF32_LINEAR_PREMUL);
                Ok(SourceInfo {
                    width: bg.width,
                    height: bg.height,
                    format: format::RGBAF32_LINEAR_PREMUL,
                })
            }

            #[cfg(feature = "std")]
            NodeOp::Filter(pipeline) => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.estimate_node(input_id, source_info, est, depth + 1)?;
                if pipeline.has_neighborhood_filter() {
                    let overlap = pipeline.max_neighborhood_radius(upstream.width, upstream.height);
                    // Windowed filter: strip_height + 2*overlap rows
                    let rows = 16u64 + 2 * overlap as u64;
                    est.streaming_bytes += rows
                        * upstream.width as u64
                        * format::RGBAF32_LINEAR.bytes_per_pixel() as u64;
                }
                est.streaming_bytes += strip_mem(upstream.width, format::RGBAF32_LINEAR);
                Ok(SourceInfo {
                    width: upstream.width,
                    height: upstream.height,
                    format: format::RGBAF32_LINEAR,
                })
            }

            #[cfg(feature = "std")]
            NodeOp::IccTransform { .. } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.estimate_node(input_id, source_info, est, depth + 1)?;
                est.streaming_bytes += strip_mem(upstream.width, upstream.format);
                Ok(upstream)
            }

            NodeOp::RemoveAlpha { .. } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.estimate_node(input_id, source_info, est, depth + 1)?;
                let out_fmt = format::RGB8_SRGB;
                est.streaming_bytes += strip_mem(upstream.width, out_fmt);
                Ok(SourceInfo {
                    width: upstream.width,
                    height: upstream.height,
                    format: out_fmt,
                })
            }

            NodeOp::AddAlpha => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.estimate_node(input_id, source_info, est, depth + 1)?;
                let out_fmt = format::RGBA8_SRGB;
                est.streaming_bytes += strip_mem(upstream.width, out_fmt);
                Ok(SourceInfo {
                    width: upstream.width,
                    height: upstream.height,
                    format: out_fmt,
                })
            }

            NodeOp::Overlay {
                width: ov_w,
                height: ov_h,
                format: ov_fmt,
                ..
            } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.estimate_node(input_id, source_info, est, depth + 1)?;
                // Overlay image is held in memory
                let ov_mem = *ov_w as u64 * *ov_h as u64 * ov_fmt.bytes_per_pixel() as u64;
                est.streaming_bytes += ov_mem;
                est.streaming_bytes += strip_mem(upstream.width, format::RGBAF32_LINEAR_PREMUL);
                Ok(SourceInfo {
                    width: upstream.width,
                    height: upstream.height,
                    format: format::RGBAF32_LINEAR_PREMUL,
                })
            }

            NodeOp::ExpandCanvas {
                left,
                top,
                right,
                bottom,
                ..
            } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.estimate_node(input_id, source_info, est, depth + 1)?;
                Ok(SourceInfo {
                    width: upstream.width + left + right,
                    height: upstream.height + top + bottom,
                    format: upstream.format,
                })
            }

            NodeOp::FillRect { .. } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.estimate_node(input_id, source_info, est, depth + 1)?;
                est.materializes = true;
                let mat = upstream.width as u64
                    * upstream.height as u64
                    * upstream.format.bytes_per_pixel() as u64;
                est.materialization_bytes = est.materialization_bytes.max(mat);
                Ok(upstream)
            }

            // Worst case: dimensions unchanged, full materialization
            NodeOp::CropWhitespace { .. } | NodeOp::Analyze(_) => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.estimate_node(input_id, source_info, est, depth + 1)?;
                est.materializes = true;
                let mat = upstream.width as u64
                    * upstream.height as u64
                    * upstream.format.bytes_per_pixel() as u64;
                est.materialization_bytes = est.materialization_bytes.max(mat);
                Ok(upstream)
            }

            NodeOp::Materialize { .. } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.estimate_node(input_id, source_info, est, depth + 1)?;
                est.materializes = true;
                let mat = upstream.width as u64
                    * upstream.height as u64
                    * upstream.format.bytes_per_pixel() as u64;
                est.materialization_bytes = est.materialization_bytes.max(mat);
                Ok(upstream)
            }
        }
    }

    /// Compile the graph into a single output [`Source`].
    ///
    /// `sources` maps [`Source`](NodeOp::Source) node IDs to their concrete sources
    /// (e.g., decoder outputs). Each source is consumed exactly once.
    ///
    /// The graph must have exactly one [`Output`](NodeOp::Output) node.
    pub fn compile(
        mut self,
        mut sources: hashbrown::HashMap<NodeId, Box<dyn Source>>,
    ) -> crate::PipeResult<Box<dyn Source>> {
        self.validate()?;
        let output_id = self
            .nodes
            .iter()
            .position(|n| matches!(&n.op, Some(NodeOp::Output)))
            .unwrap();

        self.compile_node(output_id, &mut sources, 0)
    }

    /// Compile with tracing enabled. Returns the pipeline source and collected trace.
    ///
    /// Every node boundary gets a `TracingSource` wrapper that records format,
    /// dimensions, and alpha changes. The trace is collected in a shared
    /// `PipelineTrace` accessible after the pipeline is drained.
    #[cfg(feature = "std")]
    pub fn compile_traced(
        mut self,
        mut sources: hashbrown::HashMap<NodeId, Box<dyn Source>>,
        config: &crate::trace::TraceConfig,
    ) -> crate::PipeResult<(
        Box<dyn Source>,
        alloc::sync::Arc<std::sync::Mutex<crate::trace::PipelineTrace>>,
    )> {
        self.validate()?;
        let trace =
            alloc::sync::Arc::new(std::sync::Mutex::new(crate::trace::PipelineTrace::new()));

        // Capture graph edges into the trace.
        {
            let edge_traces: Vec<crate::trace::DagSnapshotEdge> = self
                .edges
                .iter()
                .map(|e| crate::trace::DagSnapshotEdge {
                    from: e.from as u32,
                    to: e.to as u32,
                    kind: match e.kind {
                        EdgeKind::Input => alloc::string::String::from("input"),
                        EdgeKind::Canvas => alloc::string::String::from("canvas"),
                    },
                })
                .collect();
            trace.lock().unwrap().edges = edge_traces;
        }

        self.tracer = crate::trace::Tracer::active(trace.clone(), config.timing);

        let output_id = self
            .nodes
            .iter()
            .position(|n| matches!(&n.op, Some(NodeOp::Output)))
            .unwrap();

        let source = self.compile_node(output_id, &mut sources, 0)?;
        Ok((source, trace))
    }

    fn find_input(&self, node_id: NodeId, kind: EdgeKind) -> crate::PipeResult<NodeId> {
        for e in &self.edges {
            if e.to == node_id && e.kind == kind {
                return Ok(e.from);
            }
        }
        Err(at!(PipeError::Op(alloc::format!(
            "node {node_id} has no {kind:?} input edge"
        ))))
    }

    fn output_count(&self, node_id: NodeId) -> usize {
        self.edges.iter().filter(|e| e.from == node_id).count()
    }

    fn peek_op(&self, id: NodeId) -> Option<&NodeOp> {
        self.nodes[id].op.as_ref()
    }

    fn take_op(&mut self, id: NodeId) -> crate::PipeResult<NodeOp> {
        self.nodes[id]
            .op
            .take()
            .ok_or_else(|| at!(PipeError::Op(alloc::format!("node {id} already compiled"))))
    }

    fn compile_node(
        &mut self,
        node_id: NodeId,
        sources: &mut hashbrown::HashMap<NodeId, Box<dyn Source>>,
        depth: usize,
    ) -> crate::PipeResult<Box<dyn Source>> {
        // Capture op metadata before it's consumed — only when tracer is active.
        // Description is computed eagerly here (before take_op consumes it),
        // but only when tracing is on — zero alloc when inactive.
        #[cfg(feature = "std")]
        let trace_meta = if self.tracer.is_active() {
            self.peek_op(node_id).map(|op| {
                (
                    crate::trace::node_op_name(op),
                    crate::trace::node_op_description(op),
                    crate::trace::node_op_materializes(op),
                )
            })
        } else {
            None
        };

        let result = self.compile_node_inner(node_id, sources, depth);

        #[cfg(feature = "std")]
        if let Ok((source, upstream_meta)) = result {
            if let Some((name, description, materializes)) = trace_meta {
                let source = self.tracer.wrap_compiled_node(
                    source,
                    node_id,
                    name,
                    || description,
                    materializes,
                    upstream_meta.as_ref(),
                );
                return Ok(source);
            }
            return Ok(source);
        }

        #[cfg(not(feature = "std"))]
        return result;

        #[cfg(feature = "std")]
        result.map(|(source, _meta)| source)
    }

    fn compile_node_inner(
        &mut self,
        node_id: NodeId,
        sources: &mut hashbrown::HashMap<NodeId, Box<dyn Source>>,
        depth: usize,
    ) -> crate::PipeResult<(Box<dyn Source>, Option<UpstreamMeta>)> {
        if depth > MAX_GRAPH_DEPTH {
            return Err(at!(PipeError::Op(alloc::format!(
                "graph depth exceeds {MAX_GRAPH_DEPTH} at node {node_id}"
            ))));
        }
        let op = self.take_op(node_id)?;

        // Clone tracer to avoid borrow conflicts with &mut self.
        #[cfg(feature = "std")]
        let tracer = self.tracer.clone();

        /// Capture upstream metadata before any ensure_format.
        macro_rules! capture_meta {
            ($source:expr) => {{
                #[cfg(feature = "std")]
                {
                    Some(UpstreamMeta::from_source($source.as_ref()))
                }
                #[cfg(not(feature = "std"))]
                {
                    let s: &dyn Source = $source.as_ref();
                    Some(UpstreamMeta {
                        format: s.format(),
                        width: s.width(),
                        height: s.height(),
                    })
                }
            }};
        }

        /// Call ensure_format through the tracer (no-op recording when inactive).
        /// Use for operations that require an exact format (MustBe).
        macro_rules! ensure_fmt {
            ($source:expr, $target:expr, $reason:expr) => {{
                #[cfg(feature = "std")]
                {
                    tracer.ensure_format($source, $target, $reason)
                }
                #[cfg(not(feature = "std"))]
                {
                    ensure_format($source, $target)
                }
            }};
        }

        /// Cost-aware format conversion via intent negotiation.
        /// Uses [`ideal_format()`](zenpixels_convert::ideal_format) to pick the
        /// cheapest compatible format instead of forcing a specific target.
        /// If the source already satisfies the intent, no conversion is inserted.
        #[allow(unused_macros)]
        macro_rules! ensure_fmt_negotiated {
            ($source:expr, $intent:expr, $reason:expr) => {{
                #[cfg(feature = "std")]
                {
                    tracer.ensure_format_negotiated($source, $intent, $reason)
                }
                #[cfg(not(feature = "std"))]
                {
                    ensure_format_negotiated($source, $intent)
                }
            }};
        }

        match op {
            NodeOp::Source => {
                let source = sources.remove(&node_id).ok_or_else(|| {
                    PipeError::Op(alloc::format!("no source provided for node {node_id}"))
                })?;
                Ok((source, None))
            }

            NodeOp::Output => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources, depth + 1)?;
                let meta = capture_meta!(upstream);
                Ok((upstream, meta))
            }

            NodeOp::Layout { plan, filter } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let mut source = self.compile_node(input_id, sources, depth + 1)?;
                let meta = capture_meta!(source);
                source = ensure_fmt!(source, format::RGBA8_SRGB, "Layout")?;

                let content_size = plan.content_size;
                let in_w = source.width();
                let in_h = source.height();
                let resizer = zenresize::streaming_from_plan_batched(
                    in_w,
                    in_h,
                    &plan,
                    zenresize::PixelDescriptor::RGBA8_SRGB,
                    filter,
                    16,
                );
                source = Box::new(ResizeSource::from_streaming(source, resizer, 16)?);

                if let Some(cs) = content_size {
                    let out_w = source.width();
                    let out_h = source.height();
                    if cs.width < out_w || cs.height < out_h {
                        source = Box::new(EdgeReplicateSource::new(
                            source, cs.width, cs.height, out_w, out_h,
                        ));
                    }
                }

                Ok((source, meta))
            }

            NodeOp::LayoutComposite { mut plan, filter } => {
                let fg_id = self.find_input(node_id, EdgeKind::Input)?;
                let bg_id = self.find_input(node_id, EdgeKind::Canvas)?;

                plan.canvas_color = zenresize::CanvasColor::Transparent;

                let mut fg = self.compile_node(fg_id, sources, depth + 1)?;
                let meta = capture_meta!(fg);
                fg = ensure_fmt!(fg, format::RGBA8_SRGB, "LayoutComposite")?;
                let fg_w = fg.width();
                let fg_h = fg.height();
                let resizer = zenresize::streaming_from_plan_batched(
                    fg_w,
                    fg_h,
                    &plan,
                    zenresize::PixelDescriptor::RGBA8_SRGB,
                    filter,
                    16,
                );
                fg = Box::new(ResizeSource::from_streaming(fg, resizer, 16)?);

                let bg = self.compile_node(bg_id, sources, depth + 1)?;

                let fg = ensure_fmt!(fg, format::RGBAF32_LINEAR_PREMUL, "LayoutComposite")?;
                let bg = ensure_fmt!(bg, format::RGBAF32_LINEAR_PREMUL, "LayoutComposite")?;

                Ok((Box::new(CompositeSource::over_at(bg, fg, 0, 0)?), meta))
            }

            NodeOp::Crop { x, y, w, h } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources, depth + 1)?;
                let meta = capture_meta!(upstream);
                Ok((Box::new(CropSource::new(upstream, x, y, w, h)?), meta))
            }

            NodeOp::Resize {
                w,
                h,
                filter,
                sharpen_percent,
            } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources, depth + 1)?;
                let meta = capture_meta!(upstream);

                // Cost-aware format selection: if upstream is already f32 linear
                // (4 channels), use ResizeF32Source to avoid an unnecessary
                // f32->u8->resize roundtrip. Otherwise convert to RGBA8_SRGB.
                let use_f32 = upstream.format() == format::RGBAF32_LINEAR;
                let upstream = if use_f32 {
                    upstream
                } else {
                    ensure_fmt!(upstream, format::RGBA8_SRGB, "Resize")?
                };

                // Skip identity resize (same dimensions, no sharpening).
                // ensure_format already ran, so format conversion is preserved.
                if upstream.width() == w && upstream.height() == h && sharpen_percent.is_none() {
                    return Ok((upstream, meta));
                }

                let mut builder =
                    zenresize::ResizeConfig::builder(upstream.width(), upstream.height(), w, h);
                if let Some(f) = filter {
                    builder = builder.filter(f);
                }
                if let Some(pct) = sharpen_percent {
                    builder = builder.resize_sharpen(pct);
                }

                if use_f32 {
                    let config = builder
                        .format(zenresize::PixelDescriptor::RGBAF32_LINEAR)
                        .build();
                    Ok((
                        Box::new(crate::sources::ResizeF32Source::new(upstream, &config, 16)?),
                        meta,
                    ))
                } else {
                    let config = builder.build();
                    Ok((Box::new(ResizeSource::new(upstream, &config, 16)?), meta))
                }
            }

            NodeOp::Constrain {
                mode,
                w,
                h,
                orientation,
                filter,
                unsharp_percent,
                gravity,
                canvas_color,
                matte_color: _, // TODO: pending zenresize support for matte compositing
                scaling_linear,
                kernel_width_scale,
                kernel_lobe_ratio,
                post_blur,
                up_filter,
                resample_when,
                sharpen_when,
            } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources, depth + 1)?;
                let meta = capture_meta!(upstream);
                let upstream = ensure_fmt!(upstream, format::RGBA8_SRGB, "Constrain")?;
                let in_w = upstream.width();
                let in_h = upstream.height();

                let mut pipeline = zenresize::Pipeline::new(in_w, in_h);
                if let Some(exif) = orientation {
                    pipeline = pipeline.auto_orient(exif);
                }

                // Build Constraint with gravity and canvas_color
                let mut constraint = zenresize::Constraint::new(mode, w, h);
                if let Some((gx, gy)) = gravity {
                    constraint = constraint.gravity(zenresize::Gravity::Percentage(gx, gy));
                }
                if let Some(cc) = canvas_color {
                    constraint = constraint.canvas_color(cc);
                }
                pipeline = pipeline.constrain(constraint);

                let (ideal, request) = pipeline
                    .plan()
                    .map_err(|e| at!(PipeError::Op(alloc::format!("layout plan failed: {e}"))))?;
                let offer = zenresize::DecoderOffer::full_decode(in_w, in_h);
                let plan = ideal.finalize(&request, &offer);
                let f = filter.unwrap_or(zenresize::Filter::Robidoux);

                // ── resample_when: skip resize if not needed ──
                let plan_out_w = plan.canvas.width;
                let plan_out_h = plan.canvas.height;
                let size_differs = plan_out_w != in_w || plan_out_h != in_h;
                let has_padding =
                    plan.content_size.is_some() || plan.placement != (0, 0) || plan.trim.is_some();
                let should_resample = match resample_when.as_deref() {
                    Some("always") => true,
                    Some("size_differs_or_sharpening_requested") => {
                        size_differs || unsharp_percent.filter(|&v| v > 0.0).is_some()
                    }
                    _ => size_differs, // "size_differs" is the default
                };

                if !should_resample && !has_padding {
                    return Ok((upstream, meta));
                }

                // ── sharpen_when: conditionally disable sharpening ──
                let is_downscaling =
                    (plan_out_w as u64 * plan_out_h as u64) < (in_w as u64 * in_h as u64);
                let is_upscaling =
                    (plan_out_w as u64 * plan_out_h as u64) > (in_w as u64 * in_h as u64);
                let effective_sharpen = unsharp_percent.and_then(|pct| {
                    if pct <= 0.0 {
                        return None;
                    }
                    let should_sharpen = match sharpen_when.as_deref() {
                        Some("upscaling") => is_upscaling,
                        Some("size_differs") => size_differs,
                        Some("always") => true,
                        _ => is_downscaling, // "downscaling" is the default
                    };
                    if should_sharpen { Some(pct) } else { None }
                });

                // Check if we need advanced resize params beyond what
                // streaming_from_plan_batched supports
                let has_advanced = effective_sharpen.is_some()
                    || scaling_linear.is_some()
                    || kernel_width_scale.is_some()
                    || kernel_lobe_ratio.is_some()
                    || post_blur.is_some()
                    || up_filter.is_some();

                let mut source: Box<dyn Source> = if has_advanced {
                    // Build ResizeConfig from plan, then layer on advanced params
                    let mut config = zenresize::config_from_plan(
                        in_w,
                        in_h,
                        &plan,
                        zenresize::PixelDescriptor::RGBA8_SRGB,
                        f,
                    );
                    if let Some(pct) = effective_sharpen {
                        config.post_sharpen = pct;
                    }
                    if let Some(false) = scaling_linear {
                        config.linear = false;
                    }
                    if let Some(kws) = kernel_width_scale {
                        config.kernel_width_scale = Some(kws as f64);
                    }
                    if let Some(lr) = kernel_lobe_ratio {
                        config.lobe_ratio = zenresize::LobeRatio::Exact(lr);
                    }
                    if let Some(blur) = post_blur {
                        config.post_blur_sigma = blur;
                    }
                    if let Some(uf) = up_filter {
                        config.up_filter = Some(uf);
                    }
                    Box::new(ResizeSource::new(upstream, &config, 16)?)
                } else {
                    // Simple path — use streaming_from_plan_batched
                    let resizer = zenresize::streaming_from_plan_batched(
                        in_w,
                        in_h,
                        &plan,
                        zenresize::PixelDescriptor::RGBA8_SRGB,
                        f,
                        16,
                    );
                    Box::new(ResizeSource::from_streaming(upstream, resizer, 16)?)
                };

                // Apply edge replication if needed
                if let Some(cs) = plan.content_size {
                    let out_w = source.width();
                    let out_h = source.height();
                    if cs.width < out_w || cs.height < out_h {
                        source = Box::new(EdgeReplicateSource::new(
                            source, cs.width, cs.height, out_w, out_h,
                        ));
                    }
                }

                Ok((source, meta))
            }

            NodeOp::ResizeAdvanced(mut config) => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources, depth + 1)?;
                let meta = capture_meta!(upstream);
                let upstream = ensure_fmt!(upstream, format::RGBA8_SRGB, "ResizeAdvanced")?;
                config.in_width = upstream.width();
                config.in_height = upstream.height();
                Ok((Box::new(ResizeSource::new(upstream, &config, 16)?), meta))
            }

            NodeOp::Orient(orientation) => compile_orient(
                self,
                node_id,
                sources,
                orientation,
                depth,
                #[cfg(feature = "std")]
                &tracer,
            ),

            NodeOp::AutoOrient(exif) => {
                let orientation = zenresize::Orientation::from_exif(exif)
                    .unwrap_or(zenresize::Orientation::Identity);
                compile_orient(
                    self,
                    node_id,
                    sources,
                    orientation,
                    depth,
                    #[cfg(feature = "std")]
                    &tracer,
                )
            }

            NodeOp::PixelTransform(pixel_op) => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let (upstream_id, mut ops) = self.collect_pixel_op_chain(input_id);
                ops.push(pixel_op);
                let upstream = self.compile_node(upstream_id, sources, depth + 1)?;
                let meta = capture_meta!(upstream);
                let mut transform = TransformSource::new(upstream);
                for op in ops {
                    transform = transform.push_boxed(op);
                }
                Ok((Box::new(transform), meta))
            }

            NodeOp::Composite {
                fg_x,
                fg_y,
                blend_mode,
            } => {
                let bg_id = self.find_input(node_id, EdgeKind::Canvas)?;
                let fg_id = self.find_input(node_id, EdgeKind::Input)?;
                let bg = self.compile_node(bg_id, sources, depth + 1)?;
                let fg = self.compile_node(fg_id, sources, depth + 1)?;
                let meta = capture_meta!(fg);
                let bg = ensure_fmt!(bg, format::RGBAF32_LINEAR_PREMUL, "Composite")?;
                let fg = ensure_fmt!(fg, format::RGBAF32_LINEAR_PREMUL, "Composite")?;
                let mut comp = CompositeSource::over_at(bg, fg, fg_x, fg_y)?;
                if let Some(mode) = blend_mode {
                    comp = comp.with_blend_mode(mode);
                }
                Ok((Box::new(comp), meta))
            }

            #[cfg(feature = "std")]
            NodeOp::Filter(pipeline) => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;

                // Fuse resize→filter: if upstream is Resize, use f32 path
                // to avoid sRGB encode→decode roundtrip (~15% savings).
                let (upstream, meta) =
                    if matches!(self.peek_op(input_id), Some(NodeOp::Resize { .. }))
                        && self.output_count(input_id) <= 1
                    {
                        let op = self.take_op(input_id)?;
                        let NodeOp::Resize { w, h, .. } = op else {
                            unreachable!()
                        };
                        let resize_input_id = self.find_input(input_id, EdgeKind::Input)?;
                        let resize_upstream =
                            self.compile_node(resize_input_id, sources, depth + 1)?;
                        let meta = capture_meta!(resize_upstream);
                        let resize_upstream =
                            ensure_fmt!(resize_upstream, format::RGBAF32_LINEAR, "Filter+Resize")?;
                        let config = zenresize::ResizeConfig::builder(
                            resize_upstream.width(),
                            resize_upstream.height(),
                            w,
                            h,
                        )
                        .format(zenresize::PixelDescriptor::RGBAF32_LINEAR)
                        .build();
                        let source = Box::new(crate::sources::ResizeF32Source::new(
                            resize_upstream,
                            &config,
                            16,
                        )?) as Box<dyn Source>;
                        (source, meta)
                    } else {
                        let upstream = self.compile_node(input_id, sources, depth + 1)?;
                        let meta = capture_meta!(upstream);
                        let upstream = ensure_fmt!(upstream, format::RGBAF32_LINEAR, "Filter")?;
                        (upstream, meta)
                    };

                let result: Box<dyn Source> = if pipeline.has_neighborhood_filter() {
                    let overlap =
                        pipeline.max_neighborhood_radius(upstream.width(), upstream.height());
                    Box::new(crate::sources::WindowedFilterSource::new(
                        upstream, pipeline, overlap,
                    )?)
                } else {
                    Box::new(FilterSource::new(upstream, pipeline)?)
                };
                Ok((result, meta))
            }

            #[cfg(feature = "std")]
            NodeOp::IccTransform { src_icc, dst_icc } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources, depth + 1)?;
                let meta = capture_meta!(upstream);
                Ok((
                    Box::new(crate::sources::IccTransformSource::new(
                        upstream,
                        &src_icc,
                        &dst_icc,
                        &zenpixels_convert::cms_moxcms::MoxCms,
                    )?),
                    meta,
                ))
            }

            NodeOp::RemoveAlpha { matte } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources, depth + 1)?;
                let meta = capture_meta!(upstream);
                let upstream = ensure_fmt!(upstream, format::RGBA8_SRGB, "RemoveAlpha")?;
                let options = zenpixels_convert::policy::ConvertOptions::permissive()
                    .with_alpha_policy(zenpixels_convert::policy::AlphaPolicy::CompositeOnto {
                        r: matte[0],
                        g: matte[1],
                        b: matte[2],
                    });
                let op =
                    RowConverterOp::new_explicit(format::RGBA8_SRGB, format::RGB8_SRGB, &options)
                        .ok_or_else(|| {
                        PipeError::Op("no conversion path for alpha removal".to_string())
                    })?;
                Ok((Box::new(TransformSource::new(upstream).push(op)), meta))
            }

            NodeOp::AddAlpha => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources, depth + 1)?;
                let meta = capture_meta!(upstream);
                Ok((ensure_fmt!(upstream, format::RGBA8_SRGB, "AddAlpha")?, meta))
            }

            NodeOp::Overlay {
                image_data,
                width: ov_w,
                height: ov_h,
                format: ov_fmt,
                x,
                y,
                opacity,
                blend_mode,
            } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let bg = self.compile_node(input_id, sources, depth + 1)?;
                let meta = capture_meta!(bg);
                let bg = ensure_fmt!(bg, format::RGBAF32_LINEAR_PREMUL, "Overlay")?;

                let mut fg: Box<dyn Source> = Box::new(MaterializedSource::from_data(
                    image_data, ov_w, ov_h, ov_fmt,
                ));
                fg = ensure_fmt!(fg, format::RGBAF32_LINEAR_PREMUL, "Overlay")?;

                if opacity < 1.0 {
                    fg = Box::new(
                        TransformSource::new(fg).push(crate::ops::ScaleAlphaOp::new(opacity)),
                    );
                }

                let fg_x = x.max(0) as u32;
                let fg_y = y.max(0) as u32;
                let mut comp = CompositeSource::over_at(bg, fg, fg_x, fg_y)?;
                if let Some(mode) = blend_mode {
                    comp = comp.with_blend_mode(mode);
                }
                Ok((Box::new(comp), meta))
            }

            NodeOp::Analyze(builder) => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources, depth + 1)?;
                let meta = capture_meta!(upstream);
                let mat = MaterializedSource::from_source(upstream)?;
                #[cfg(feature = "std")]
                {
                    Ok((builder(mat, tracer.appender())?, meta))
                }
                #[cfg(not(feature = "std"))]
                Ok((builder(mat)?, meta))
            }

            NodeOp::CropWhitespace {
                threshold,
                percent_padding,
            } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources, depth + 1)?;
                let meta = capture_meta!(upstream);
                let upstream = ensure_fmt!(upstream, format::RGBA8_SRGB, "CropWhitespace")?;
                let mat = MaterializedSource::from_source(upstream)?;
                let orig_w = mat.width();
                let orig_h = mat.height();
                let (x, y, w, h) = detect_content_bounds(&mat, threshold, percent_padding);

                // Record detection result in trace — no-op when tracer is inactive.
                #[cfg(feature = "std")]
                {
                    let desc = if w == orig_w && h == orig_h && x == 0 && y == 0 {
                        alloc::format!(
                            "no whitespace detected ({}x{}, threshold={})",
                            orig_w,
                            orig_h,
                            threshold
                        )
                    } else {
                        alloc::format!(
                            "detected content at ({},{}) {}x{} from {}x{} (borders: L={} T={} R={} B={})",
                            x,
                            y,
                            w,
                            h,
                            orig_w,
                            orig_h,
                            x,
                            y,
                            orig_w - w - x,
                            orig_h - h - y
                        )
                    };
                    tracer.note_implicit(
                        "WhitespaceDetect",
                        desc,
                        "CropWhitespace detection result",
                        format::RGBA8_SRGB,
                        orig_w,
                        orig_h,
                        w,
                        h,
                        true,
                    );
                }

                if w == orig_w && h == orig_h && x == 0 && y == 0 {
                    return Ok((Box::new(mat), meta));
                }
                Ok((Box::new(CropSource::new(Box::new(mat), x, y, w, h)?), meta))
            }

            NodeOp::ExpandCanvas {
                left,
                top,
                right,
                bottom,
                bg_color,
            } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources, depth + 1)?;
                let meta = capture_meta!(upstream);
                let src_w = upstream.width();
                let src_h = upstream.height();
                let canvas_w = src_w + left + right;
                let canvas_h = src_h + top + bottom;
                Ok((
                    Box::new(ExpandCanvasSource::new(
                        upstream,
                        canvas_w,
                        canvas_h,
                        left as i32,
                        top as i32,
                        bg_color,
                    )),
                    meta,
                ))
            }

            NodeOp::FillRect {
                x1,
                y1,
                x2,
                y2,
                color,
            } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources, depth + 1)?;
                let meta = capture_meta!(upstream);
                let mut mat = MaterializedSource::from_source(upstream)?;
                let w = mat.width();
                let h = mat.height();
                let bpp = mat.format().bytes_per_pixel();
                let data = mat.data_mut();
                let x1 = (x1 as usize).min(w as usize);
                let x2 = (x2 as usize).min(w as usize);
                let y1 = (y1 as usize).min(h as usize);
                let y2 = (y2 as usize).min(h as usize);
                let stride = w as usize * bpp;
                for y in y1..y2 {
                    for x in x1..x2 {
                        let off = y * stride + x * bpp;
                        if off + bpp <= data.len() {
                            for c in 0..bpp.min(4) {
                                data[off + c] = color[c];
                            }
                        }
                    }
                }
                Ok((Box::new(mat), meta))
            }

            NodeOp::Materialize { transform, .. } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources, depth + 1)?;
                let meta = capture_meta!(upstream);
                Ok((
                    Box::new(MaterializedSource::from_source_with_transform(
                        upstream, transform,
                    )?),
                    meta,
                ))
            }
        }
    }

    /// Walk backward through consecutive PixelTransform nodes with single outputs,
    /// collecting their ops in upstream-first order.
    fn collect_pixel_op_chain(&mut self, node_id: NodeId) -> (NodeId, Vec<Box<dyn PixelOp>>) {
        let is_pixel = matches!(self.peek_op(node_id), Some(NodeOp::PixelTransform(_)));
        let single_output = self.output_count(node_id) <= 1;

        if is_pixel && single_output {
            let op = self.take_op(node_id).unwrap();
            let NodeOp::PixelTransform(pixel_op) = op else {
                unreachable!()
            };
            let input_id = self.find_input(node_id, EdgeKind::Input).unwrap();
            let (upstream_id, mut ops) = self.collect_pixel_op_chain(input_id);
            ops.push(pixel_op);
            (upstream_id, ops)
        } else {
            (node_id, Vec::new())
        }
    }
}

impl Default for PipelineGraph {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Format conversion helper — generic via RowConverterOp
// =============================================================================

/// Compile an orientation transform (shared by Orient and AutoOrient).
fn compile_orient(
    graph: &mut PipelineGraph,
    node_id: NodeId,
    sources: &mut hashbrown::HashMap<NodeId, Box<dyn Source>>,
    orientation: zenresize::Orientation,
    depth: usize,
    #[cfg(feature = "std")] tracer: &crate::trace::Tracer,
) -> crate::PipeResult<(Box<dyn Source>, Option<crate::trace::UpstreamMeta>)> {
    let input_id = graph.find_input(node_id, EdgeKind::Input)?;
    let upstream = graph.compile_node(input_id, sources, depth + 1)?;
    #[cfg(feature = "std")]
    let meta = Some(crate::trace::UpstreamMeta::from_source(upstream.as_ref()));
    #[cfg(not(feature = "std"))]
    let meta = None;

    if orientation.is_identity() {
        return Ok((upstream, meta));
    }
    #[cfg(feature = "std")]
    let upstream = tracer.ensure_format(upstream, format::RGBA8_SRGB, "Orient")?;
    #[cfg(not(feature = "std"))]
    let upstream = ensure_format(upstream, format::RGBA8_SRGB)?;

    let in_w = upstream.width();
    let in_h = upstream.height();
    Ok((
        Box::new(MaterializedSource::from_source_with_transform(
            upstream,
            move |data, w, h, _fmt| {
                let (result, new_w, new_h) =
                    zenresize::orient_image(data, in_w, in_h, orientation, 4);
                *data = result;
                *w = new_w;
                *h = new_h;
            },
        )?),
        meta,
    ))
}

/// Insert a format conversion source if needed (no-std path).
#[cfg(not(feature = "std"))]
fn ensure_format(
    source: Box<dyn Source>,
    target: PixelFormat,
) -> crate::PipeResult<Box<dyn Source>> {
    let current = source.format();
    if current == target {
        return Ok(source);
    }
    let op = RowConverterOp::new(current, target).ok_or_else(|| {
        PipeError::Op(alloc::format!(
            "no conversion path from {current} to {target}"
        ))
    })?;
    Ok(Box::new(TransformSource::new(source).push(op)))
}

/// Cost-aware format conversion using [`zenpixels_convert::ideal_format`] (no-std path).
///
/// Computes the ideal working format for the given intent and only converts
/// if the source doesn't already satisfy the requirement.
#[cfg(not(feature = "std"))]
fn ensure_format_negotiated(
    source: Box<dyn Source>,
    intent: zenpixels_convert::ConvertIntent,
) -> Result<Box<dyn Source>, PipeError> {
    let current = source.format();
    let ideal = zenpixels_convert::ideal_format(current, intent);
    ensure_format(source, ideal)
}

// ensure_format_traced is now Tracer::ensure_format — see trace.rs

// =============================================================================
// Estimation helper
// =============================================================================

/// Estimate memory for one strip buffer (16 rows at the given width and format).
fn strip_mem(width: u32, fmt: PixelFormat) -> u64 {
    16 * width as u64 * fmt.bytes_per_pixel() as u64
}

// =============================================================================
// Whitespace detection (used by CropWhitespace)
// =============================================================================

/// Scan a materialized RGBA8 image for uniform borders and return content bounds.
///
/// Scans inward from each edge. A row/column is "whitespace" if every pixel
/// is within `threshold` per-channel distance of the top-left corner pixel.
/// Returns `(x, y, w, h)` with `percent_padding` applied.
fn detect_content_bounds(
    mat: &MaterializedSource,
    threshold: u8,
    percent_padding: f32,
) -> (u32, u32, u32, u32) {
    let w = mat.width();
    let h = mat.height();
    if w == 0 || h == 0 {
        return (0, 0, w, h);
    }

    let bpp = mat.format().bytes_per_pixel();

    // Reference color: top-left pixel
    let row0 = mat.row(0);
    let ref_color: &[u8] = &row0[..bpp];
    let thresh = threshold as i16;

    let pixel_matches = |row: &[u8], x: u32| -> bool {
        let start = x as usize * bpp;
        // Compare each channel independently (skip alpha for 4-channel)
        let channels = bpp.min(3);
        for c in 0..channels {
            let diff = (row[start + c] as i16 - ref_color[c] as i16).abs();
            if diff > thresh {
                return false;
            }
        }
        true
    };

    let row_is_whitespace = |y: u32| -> bool {
        let row = mat.row(y);
        (0..w).all(|x| pixel_matches(row, x))
    };

    let col_is_whitespace = |x: u32| -> bool { (0..h).all(|y| pixel_matches(mat.row(y), x)) };

    // Scan from each edge
    let mut top = 0u32;
    while top < h && row_is_whitespace(top) {
        top += 1;
    }

    let mut bottom = h;
    while bottom > top && row_is_whitespace(bottom - 1) {
        bottom -= 1;
    }

    let mut left = 0u32;
    while left < w && col_is_whitespace(left) {
        left += 1;
    }

    let mut right = w;
    while right > left && col_is_whitespace(right - 1) {
        right -= 1;
    }

    // Handle fully uniform image
    if top >= bottom || left >= right {
        return (0, 0, w, h);
    }

    let content_w = right - left;
    let content_h = bottom - top;

    // Apply padding.
    // percent_padding is in percentage units (0.5 = 0.5%), matching imageflow v2's
    // RIAPI trim.percentpadding parameter. V2 divides by 100 and uses the average
    // of content dimensions as the base: padding = (pct/100) * (w+h)/2.
    // Padding is uniform (same for all sides).
    if percent_padding > 0.0 {
        let avg_dim = (content_w + content_h) as f32 / 2.0;
        let pad = (percent_padding / 100.0 * avg_dim).ceil() as u32;
        let x = left.saturating_sub(pad);
        let y = top.saturating_sub(pad);
        let r = (right + pad).min(w);
        let b = (bottom + pad).min(h);
        (x, y, r - x, b - y)
    } else {
        (left, top, content_w, content_h)
    }
}
