//! Graph-based pipeline compiler.
//!
//! Compiles a DAG of image operations into a pull-based [`Source`] chain.
//! The caller builds a [`PipelineGraph`], adds nodes and edges, then calls
//! [`compile`](PipelineGraph::compile) to produce an executable source.
//!
//! # Design
//!
//! - **Delegates to zen crates**: zenresize handles orient + resize. Layout nodes
//!   decompose into streaming steps (crop → orient → resize → canvas) with
//!   materialization only when unavoidable (axis-swapping orientations, canvas expansion).
//! - **No estimate phase**: dimensions propagate naturally through `Source::width()`/`height()`
//!   during compilation.
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
use crate::error::PipeError;
use crate::format::{self, PixelFormat};
use crate::ops::{PixelOp, RowConverterOp};
use crate::sources::FilterSource;
use crate::sources::{
    CompositeSource, CropSource, EdgeReplicateSource, MaterializedSource, ResizeSource,
    TransformSource,
};

/// Node identifier (index into the graph's node list).
pub type NodeId = usize;

/// A transform applied to a fully materialized pixel buffer.
pub type MaterializeTransform =
    Box<dyn FnOnce(&mut alloc::vec::Vec<u8>, &mut u32, &mut u32, &mut PixelFormat) + Send>;

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
    Composite { fg_x: u32, fg_y: u32 },

    // === Filters (zenfilters integration) ===
    /// Apply a [`zenfilters::Pipeline`] of photo filters.
    ///
    /// Input is auto-converted to [`Rgbaf32Linear`](format::RGBAF32_LINEAR).
    /// Per-pixel-only pipelines stream strip-by-strip via [`FilterSource`].
    /// Pipelines with neighborhood filters (blur, clarity, sharpen) use
    /// windowed materialization via [`WindowedFilterSource`] — only
    /// `strip_height + 2 * overlap` rows are buffered at a time instead
    /// of the full image.
    Filter(zenfilters::Pipeline),

    // === ICC color management (requires `cms` feature) ===
    /// Apply an ICC profile transform to the pixel data.
    ///
    /// Converts pixels from the source ICC profile's color space to the
    /// destination ICC profile's color space, row-by-row via moxcms.
    /// The pixel format (layout, depth) is preserved — only color values change.
    ///
    /// Provide the raw ICC profile bytes for source and destination.
    /// The transform is built at compile time from the upstream format.
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
    },

    // === Barriers ===
    /// Custom materialization barrier — drain upstream, transform, re-stream.
    ///
    /// The closure receives `(data, width, height, format)` and may mutate all.
    Materialize(MaterializeTransform),

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

/// A directed acyclic graph of image operations.
///
/// Build the graph by adding nodes and edges, then call [`compile`](Self::compile)
/// to produce an executable [`Source`] chain.
pub struct PipelineGraph {
    nodes: Vec<GraphNode>,
    edges: Vec<Edge>,
}

impl PipelineGraph {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
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

    /// Compile the graph into a single output [`Source`].
    ///
    /// `sources` maps [`Source`](NodeOp::Source) node IDs to their concrete sources
    /// (e.g., decoder outputs). Each source is consumed exactly once.
    ///
    /// The graph must have exactly one [`Output`](NodeOp::Output) node.
    pub fn compile(
        mut self,
        mut sources: hashbrown::HashMap<NodeId, Box<dyn Source>>,
    ) -> Result<Box<dyn Source>, PipeError> {
        let output_id = self
            .nodes
            .iter()
            .position(|n| matches!(&n.op, Some(NodeOp::Output)))
            .ok_or_else(|| PipeError::Op("graph has no Output node".to_string()))?;

        self.compile_node(output_id, &mut sources)
    }

    fn find_input(&self, node_id: NodeId, kind: EdgeKind) -> Result<NodeId, PipeError> {
        for e in &self.edges {
            if e.to == node_id && e.kind == kind {
                return Ok(e.from);
            }
        }
        Err(PipeError::Op(alloc::format!(
            "node {node_id} has no {kind:?} input edge"
        )))
    }

    fn output_count(&self, node_id: NodeId) -> usize {
        self.edges.iter().filter(|e| e.from == node_id).count()
    }

    fn peek_op(&self, id: NodeId) -> Option<&NodeOp> {
        self.nodes[id].op.as_ref()
    }

    fn take_op(&mut self, id: NodeId) -> Result<NodeOp, PipeError> {
        self.nodes[id]
            .op
            .take()
            .ok_or_else(|| PipeError::Op(alloc::format!("node {id} already compiled")))
    }

    fn compile_node(
        &mut self,
        node_id: NodeId,
        sources: &mut hashbrown::HashMap<NodeId, Box<dyn Source>>,
    ) -> Result<Box<dyn Source>, PipeError> {
        let op = self.take_op(node_id)?;

        match op {
            NodeOp::Source => sources.remove(&node_id).ok_or_else(|| {
                PipeError::Op(alloc::format!("no source provided for node {node_id}"))
            }),

            NodeOp::Output => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                self.compile_node(input_id, sources)
            }

            NodeOp::Layout { plan, filter } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let mut source = self.compile_node(input_id, sources)?;
                source = ensure_format(source, format::RGBA8_SRGB)?;

                let content_size = plan.content_size;

                // Single streaming pass: crop + resize + padding + orientation
                // all handled by zenresize's StreamingResize via config_from_plan.
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

                // Streaming edge replication for MCU alignment (content_size).
                // Replaces solid-color padding at the content boundary with
                // replicated edge pixels for better encoder output quality.
                if let Some(cs) = content_size {
                    let out_w = source.width();
                    let out_h = source.height();
                    if cs.width < out_w || cs.height < out_h {
                        source = Box::new(EdgeReplicateSource::new(
                            source, cs.width, cs.height, out_w, out_h,
                        ));
                    }
                }

                Ok(source)
            }

            NodeOp::LayoutComposite { mut plan, filter } => {
                let fg_id = self.find_input(node_id, EdgeKind::Input)?;
                let bg_id = self.find_input(node_id, EdgeKind::Canvas)?;

                // Stream the foreground through Layout with transparent canvas,
                // then composite over the background — fully streaming.
                plan.canvas_color = zenresize::CanvasColor::Transparent;

                let mut fg = self.compile_node(fg_id, sources)?;
                fg = ensure_format(fg, format::RGBA8_SRGB)?;
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

                let bg = self.compile_node(bg_id, sources)?;

                // Composite foreground over background in premultiplied linear space.
                let fg = ensure_format(fg, format::RGBAF32_LINEAR_PREMUL)?;
                let bg = ensure_format(bg, format::RGBAF32_LINEAR_PREMUL)?;

                Ok(Box::new(CompositeSource::over_at(bg, fg, 0, 0)?))
            }

            NodeOp::Crop { x, y, w, h } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources)?;
                Ok(Box::new(CropSource::new(upstream, x, y, w, h)?))
            }

            NodeOp::Resize {
                w,
                h,
                filter,
                sharpen_percent,
            } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources)?;
                let upstream = ensure_format(upstream, format::RGBA8_SRGB)?;
                let mut builder =
                    zenresize::ResizeConfig::builder(upstream.width(), upstream.height(), w, h);
                if let Some(f) = filter {
                    builder = builder.filter(f);
                }
                if let Some(pct) = sharpen_percent {
                    builder = builder.resize_sharpen(pct);
                }
                let config = builder.build();
                Ok(Box::new(ResizeSource::new(upstream, &config, 16)?))
            }

            NodeOp::Orient(orientation) => {
                return compile_orient(self, node_id, sources, orientation);
            }

            NodeOp::AutoOrient(exif) => {
                let orientation = zenresize::Orientation::from_exif(exif)
                    .unwrap_or(zenresize::Orientation::Identity);
                return compile_orient(self, node_id, sources, orientation);
            }

            NodeOp::PixelTransform(pixel_op) => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let (upstream_id, mut ops) = self.collect_pixel_op_chain(input_id);
                ops.push(pixel_op);
                let upstream = self.compile_node(upstream_id, sources)?;
                let mut transform = TransformSource::new(upstream);
                for op in ops {
                    transform = transform.push_boxed(op);
                }
                Ok(Box::new(transform))
            }

            NodeOp::Composite { fg_x, fg_y } => {
                let bg_id = self.find_input(node_id, EdgeKind::Canvas)?;
                let fg_id = self.find_input(node_id, EdgeKind::Input)?;
                let bg = self.compile_node(bg_id, sources)?;
                let fg = self.compile_node(fg_id, sources)?;
                let bg = ensure_format(bg, format::RGBAF32_LINEAR_PREMUL)?;
                let fg = ensure_format(fg, format::RGBAF32_LINEAR_PREMUL)?;
                Ok(Box::new(CompositeSource::over_at(bg, fg, fg_x, fg_y)?))
            }

            NodeOp::Filter(pipeline) => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;

                // Fuse resize→filter: if upstream is Resize, use f32 path
                // to avoid sRGB encode→decode roundtrip (~15% savings).
                let upstream = if matches!(self.peek_op(input_id), Some(NodeOp::Resize { .. }))
                    && self.output_count(input_id) <= 1
                {
                    let op = self.take_op(input_id)?;
                    let NodeOp::Resize { w, h, .. } = op else {
                        unreachable!()
                    };
                    let resize_input_id = self.find_input(input_id, EdgeKind::Input)?;
                    let resize_upstream = self.compile_node(resize_input_id, sources)?;
                    let resize_upstream = ensure_format(resize_upstream, format::RGBAF32_LINEAR)?;
                    let config = zenresize::ResizeConfig::builder(
                        resize_upstream.width(),
                        resize_upstream.height(),
                        w,
                        h,
                    )
                    .format(zenresize::PixelDescriptor::RGBAF32_LINEAR)
                    .build();
                    Box::new(crate::sources::ResizeF32Source::new(
                        resize_upstream,
                        &config,
                        16,
                    )?) as Box<dyn Source>
                } else {
                    let upstream = self.compile_node(input_id, sources)?;
                    ensure_format(upstream, format::RGBAF32_LINEAR)?
                };

                if pipeline.has_neighborhood_filter() {
                    let overlap =
                        pipeline.max_neighborhood_radius(upstream.width(), upstream.height());
                    Ok(Box::new(crate::sources::WindowedFilterSource::new(
                        upstream, pipeline, overlap,
                    )?))
                } else {
                    Ok(Box::new(FilterSource::new(upstream, pipeline)?))
                }
            }

            NodeOp::IccTransform { src_icc, dst_icc } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources)?;
                Ok(Box::new(crate::sources::IccTransformSource::new(
                    upstream, &src_icc, &dst_icc,
                )?))
            }

            NodeOp::RemoveAlpha { matte } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources)?;
                let upstream = ensure_format(upstream, format::RGBA8_SRGB)?;
                let flatten = crate::ops::MatteFlattenOp::new(matte[0], matte[1], matte[2]);
                Ok(Box::new(TransformSource::new(upstream).push(flatten)))
            }

            NodeOp::AddAlpha => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources)?;
                // If already RGBA, this is a no-op. ensure_format handles
                // RGB→RGBA via RowConverter (adds opaque alpha).
                ensure_format(upstream, format::RGBA8_SRGB)
            }

            NodeOp::Overlay {
                image_data,
                width: ov_w,
                height: ov_h,
                format: ov_fmt,
                x,
                y,
                opacity,
            } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let bg = self.compile_node(input_id, sources)?;
                let bg = ensure_format(bg, format::RGBAF32_LINEAR_PREMUL)?;

                let mut fg: Box<dyn Source> = Box::new(MaterializedSource::from_data(
                    image_data, ov_w, ov_h, ov_fmt,
                ));
                fg = ensure_format(fg, format::RGBAF32_LINEAR_PREMUL)?;

                if opacity < 1.0 {
                    fg = Box::new(
                        TransformSource::new(fg).push(crate::ops::ScaleAlphaOp::new(opacity)),
                    );
                }

                let fg_x = x.max(0) as u32;
                let fg_y = y.max(0) as u32;
                Ok(Box::new(CompositeSource::over_at(bg, fg, fg_x, fg_y)?))
            }

            NodeOp::Materialize(transform_fn) => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources)?;
                Ok(Box::new(MaterializedSource::from_source_with_transform(
                    upstream,
                    transform_fn,
                )?))
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

/// Insert a format conversion source if needed.
///
/// Compile an orientation transform (shared by Orient and AutoOrient).
fn compile_orient(
    graph: &mut PipelineGraph,
    node_id: NodeId,
    sources: &mut hashbrown::HashMap<NodeId, Box<dyn Source>>,
    orientation: zenresize::Orientation,
) -> Result<Box<dyn Source>, PipeError> {
    let input_id = graph.find_input(node_id, EdgeKind::Input)?;
    let upstream = graph.compile_node(input_id, sources)?;
    if orientation.is_identity() {
        return Ok(upstream);
    }
    let upstream = ensure_format(upstream, format::RGBA8_SRGB)?;
    let in_w = upstream.width();
    let in_h = upstream.height();
    Ok(Box::new(MaterializedSource::from_source_with_transform(
        upstream,
        move |data, w, h, _fmt| {
            let (result, new_w, new_h) = zenresize::orient_image(data, in_w, in_h, orientation, 4);
            *data = result;
            *w = new_w;
            *h = new_h;
        },
    )?))
}

/// Uses [`RowConverterOp`] to handle any conversion that zenpixels-convert
/// supports, including gamut changes (BT.709 ↔ P3 ↔ BT.2020), transfer
/// function changes (sRGB ↔ Linear ↔ PQ ↔ HLG), alpha mode changes,
/// and depth changes.
fn ensure_format(
    source: Box<dyn Source>,
    target: PixelFormat,
) -> Result<Box<dyn Source>, PipeError> {
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
