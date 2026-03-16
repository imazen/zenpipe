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
//! - **Automatic format conversion**: inserts `SrgbToLinearPremul` /
//!   `UnpremulLinearToSrgb` where required (e.g., before streaming composite).
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
use crate::format::PixelFormat;
use crate::ops::{self, PixelOp, SrgbToLinearPremul, UnpremulLinearToSrgb};
#[cfg(feature = "filters")]
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
    Resize { w: u32, h: u32 },

    /// Apply an orientation transform (any of the 8 EXIF orientations).
    /// Delegates to [`zenresize::orient_image`]. Materializes.
    Orient(zenresize::Orientation),

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
    /// Input is auto-converted to [`Rgbaf32Linear`](PixelFormat::Rgbaf32Linear).
    /// Per-pixel-only pipelines stream strip-by-strip. Pipelines with
    /// neighborhood filters (blur, clarity, sharpen) materialize the full
    /// image, apply filters, and re-stream.
    #[cfg(feature = "filters")]
    Filter(zenfilters::Pipeline),

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
                source = ensure_format(source, PixelFormat::Rgba8);

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
                fg = ensure_format(fg, PixelFormat::Rgba8);
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
                let fg = ensure_format(fg, PixelFormat::Rgbaf32LinearPremul);
                let bg = ensure_format(bg, PixelFormat::Rgbaf32LinearPremul);

                Ok(Box::new(CompositeSource::over_at(bg, fg, 0, 0)?))
            }

            NodeOp::Crop { x, y, w, h } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources)?;
                Ok(Box::new(CropSource::new(upstream, x, y, w, h)?))
            }

            NodeOp::Resize { w, h } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources)?;
                let upstream = ensure_format(upstream, PixelFormat::Rgba8);
                let config =
                    zenresize::ResizeConfig::builder(upstream.width(), upstream.height(), w, h)
                        .build();
                Ok(Box::new(ResizeSource::new(upstream, &config, 16)?))
            }

            NodeOp::Orient(orientation) => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources)?;
                if orientation.is_identity() {
                    return Ok(upstream);
                }
                let upstream = ensure_format(upstream, PixelFormat::Rgba8);
                let in_w = upstream.width();
                let in_h = upstream.height();
                Ok(Box::new(MaterializedSource::from_source_with_transform(
                    upstream,
                    move |data, w, h, _fmt| {
                        let (result, new_w, new_h) =
                            zenresize::orient_image(data, in_w, in_h, orientation, 4);
                        *data = result;
                        *w = new_w;
                        *h = new_h;
                    },
                )?))
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
                let bg = ensure_format(bg, PixelFormat::Rgbaf32LinearPremul);
                let fg = ensure_format(fg, PixelFormat::Rgbaf32LinearPremul);
                Ok(Box::new(CompositeSource::over_at(bg, fg, fg_x, fg_y)?))
            }

            #[cfg(feature = "filters")]
            NodeOp::Filter(pipeline) => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources)?;
                let upstream = ensure_format(upstream, PixelFormat::Rgbaf32Linear);

                if pipeline.has_neighborhood_filter() {
                    // Neighborhood filters need full-frame access — materialize,
                    // apply, then stream the result back.
                    let w = upstream.width();
                    let h = upstream.height();
                    Ok(Box::new(MaterializedSource::from_source_with_transform(
                        upstream,
                        move |data, _w, _h, _fmt| {
                            let mut ctx = zenfilters::FilterContext::new();
                            let src_copy = data.clone();
                            let in_f32: &[f32] = bytemuck::cast_slice(&src_copy);
                            let out_f32: &mut [f32] = bytemuck::cast_slice_mut(data);
                            pipeline
                                .apply(in_f32, out_f32, w, h, 4, &mut ctx)
                                .expect("filter pipeline apply failed");
                        },
                    )?))
                } else {
                    // Per-pixel only — stream strip by strip.
                    Ok(Box::new(FilterSource::new(upstream, pipeline)?))
                }
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
// Format conversion helpers
// =============================================================================

/// Insert a format conversion source if needed.
fn ensure_format(source: Box<dyn Source>, target: PixelFormat) -> Box<dyn Source> {
    let current = source.format();
    if current == target {
        return source;
    }
    match (current, target) {
        // === Rgba8 ↔ premul linear ===
        (PixelFormat::Rgba8, PixelFormat::Rgbaf32LinearPremul) => {
            Box::new(TransformSource::new(source).push(SrgbToLinearPremul))
        }
        (PixelFormat::Rgbaf32LinearPremul, PixelFormat::Rgba8) => {
            Box::new(TransformSource::new(source).push(UnpremulLinearToSrgb))
        }
        // === Rgba8 ↔ straight linear ===
        (PixelFormat::Rgba8, PixelFormat::Rgbaf32Linear) => {
            Box::new(TransformSource::new(source).push(ops::SrgbToLinear))
        }
        (PixelFormat::Rgbaf32Linear, PixelFormat::Rgba8) => {
            Box::new(TransformSource::new(source).push(ops::LinearToSrgb))
        }
        // === Rgba8 ↔ f32 sRGB ===
        (PixelFormat::Rgba8, PixelFormat::Rgbaf32Srgb) => {
            Box::new(TransformSource::new(source).push(ops::NormalizeU8ToF32))
        }
        (PixelFormat::Rgbaf32Srgb, PixelFormat::Rgba8) => {
            Box::new(TransformSource::new(source).push(ops::QuantizeF32ToU8))
        }
        // === f32 sRGB ↔ f32 linear (gamma only, no premul) ===
        (PixelFormat::Rgbaf32Srgb, PixelFormat::Rgbaf32Linear) => {
            Box::new(TransformSource::new(source).push(ops::LinearizeF32))
        }
        (PixelFormat::Rgbaf32Linear, PixelFormat::Rgbaf32Srgb) => {
            Box::new(TransformSource::new(source).push(ops::DelinearizeF32))
        }
        // === premul ↔ straight linear ===
        (PixelFormat::Rgbaf32Linear, PixelFormat::Rgbaf32LinearPremul) => {
            Box::new(TransformSource::new(source).push(ops::Premultiply))
        }
        (PixelFormat::Rgbaf32LinearPremul, PixelFormat::Rgbaf32Linear) => {
            Box::new(TransformSource::new(source).push(ops::Unpremultiply))
        }
        // === Multi-hop for remaining conversions ===
        _ => {
            // Route through Rgbaf32Linear as the hub format — it connects
            // to all other formats in one hop.
            let intermediate = ensure_format(source, PixelFormat::Rgbaf32Linear);
            ensure_format(intermediate, target)
        }
    }
}
