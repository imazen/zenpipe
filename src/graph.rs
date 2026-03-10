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
use crate::sources::{
    CompositeSource, CropSource, ExpandCanvasSource, FlipHSource, MaterializedSource, ResizeSource,
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
    /// placement via decomposed streaming steps where possible.
    ///
    /// Input must be `Rgba8`. Decomposes into:
    /// 1. `CropSource` (streaming) if trim is needed
    /// 2. `FlipHSource` (streaming) or `MaterializedSource(orient_image)` for orientation
    /// 3. `ResizeSource` (streaming) if resize is needed
    /// 4. Canvas expansion (materialized) if canvas differs from resize output
    ///
    /// Common case (no orientation, no canvas padding) is fully streaming.
    /// Falls back to materializing `execute_layout` for edge extension
    /// (`content_size`) or `Linear` canvas colors.
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

                let needs_canvas = plan.canvas.width != plan.resize_to.width
                    || plan.canvas.height != plan.resize_to.height
                    || plan.placement != (0, 0);
                let has_linear_canvas =
                    matches!(plan.canvas_color, zenresize::CanvasColor::Linear { .. });

                // Fall back to materializing for features that can't stream
                if plan.content_size.is_some() || (needs_canvas && has_linear_canvas) {
                    let in_w = source.width();
                    let in_h = source.height();
                    let canvas_w = plan.canvas.width;
                    let canvas_h = plan.canvas.height;
                    return Ok(Box::new(MaterializedSource::from_source_with_transform(
                        source,
                        move |data, w, h, _fmt| {
                            let out = zenresize::execute_layout(
                                data,
                                in_w,
                                in_h,
                                &plan,
                                zenresize::PixelDescriptor::RGBA8_SRGB,
                                filter,
                            );
                            *data = out;
                            *w = canvas_w;
                            *h = canvas_h;
                        },
                    )?));
                }

                // Step 1: Trim (streaming crop)
                if let Some(trim) = plan.trim {
                    source = Box::new(CropSource::new(
                        source,
                        trim.x,
                        trim.y,
                        trim.width,
                        trim.height,
                    )?);
                }

                // Step 2: Orientation
                let orientation = plan.remaining_orientation;
                if !orientation.is_identity() {
                    if matches!(orientation, zenresize::Orientation::FlipH) {
                        source = Box::new(FlipHSource::new(source));
                    } else {
                        let in_w = source.width();
                        let in_h = source.height();
                        source = Box::new(MaterializedSource::from_source_with_transform(
                            source,
                            move |data, w, h, _fmt| {
                                let (result, new_w, new_h) =
                                    zenresize::orient_image(data, in_w, in_h, orientation, 4);
                                *data = result;
                                *w = new_w;
                                *h = new_h;
                            },
                        )?);
                    }
                }

                // Step 3: Resize (streaming via StreamingResize)
                if !plan.resize_is_identity {
                    let in_w = source.width();
                    let in_h = source.height();
                    let config = zenresize::ResizeConfig::builder(
                        in_w,
                        in_h,
                        plan.resize_to.width,
                        plan.resize_to.height,
                    )
                    .filter(filter)
                    .build();
                    source = Box::new(ResizeSource::new(source, &config, 16)?);
                }

                // Step 4: Canvas expansion (streaming)
                if needs_canvas {
                    let (px, py) = plan.placement;
                    let bg = match plan.canvas_color {
                        zenresize::CanvasColor::Transparent => [0u8, 0, 0, 0],
                        zenresize::CanvasColor::Srgb { r, g, b, a } => [r, g, b, a],
                        _ => [0, 0, 0, 0],
                    };
                    source = Box::new(ExpandCanvasSource::new(
                        source,
                        plan.canvas.width,
                        plan.canvas.height,
                        px,
                        py,
                        bg,
                    ));
                }

                Ok(source)
            }

            NodeOp::LayoutComposite { plan, filter } => {
                let fg_id = self.find_input(node_id, EdgeKind::Input)?;
                let bg_id = self.find_input(node_id, EdgeKind::Canvas)?;
                let fg_upstream = self.compile_node(fg_id, sources)?;
                let bg_upstream = self.compile_node(bg_id, sources)?;
                let fg_upstream = ensure_format(fg_upstream, PixelFormat::Rgba8);
                let bg_upstream = ensure_format(bg_upstream, PixelFormat::Rgba8);

                // Materialize both to get pixel data
                let mut fg_mat = MaterializedSource::from_source(fg_upstream)?;
                let bg_mat = MaterializedSource::from_source(bg_upstream)?;

                // Collect foreground pixels
                let fg_w = fg_mat.width();
                let fg_h = fg_mat.height();
                let mut fg_data = Vec::new();
                while let Some(strip) = fg_mat.next()? {
                    fg_data.extend_from_slice(strip.data);
                }

                // Collect background pixels and build SliceBackground
                let bg_w = bg_mat.width();
                let bg_h = bg_mat.height();
                let mut bg_data_u8 = Vec::new();
                let mut bg_src = bg_mat;
                while let Some(strip) = bg_src.next()? {
                    bg_data_u8.extend_from_slice(strip.data);
                }

                // Convert background to premultiplied linear f32 for SliceBackground
                let bg_pixels = bg_w as usize * bg_h as usize;
                let mut bg_f32 = alloc::vec![0.0f32; bg_pixels * 4];
                linear_srgb::default::srgb_u8_to_linear_rgba_slice(&bg_data_u8, &mut bg_f32);
                garb::bytes::premultiply_alpha_f32(bytemuck::cast_slice_mut(&mut bg_f32))
                    .expect("aligned");

                let row_len = bg_w as usize * 4;
                let background = zenresize::SliceBackground::new(&bg_f32, row_len);

                let canvas_w = plan.canvas.width;
                let canvas_h = plan.canvas.height;

                let result = zenresize::execute_layout_with_background(
                    &fg_data,
                    fg_w,
                    fg_h,
                    &plan,
                    zenresize::PixelDescriptor::RGBA8_SRGB,
                    filter,
                    background,
                )
                .map_err(|e| PipeError::Op(e.to_string()))?;

                Ok(Box::new(MaterializedSource::from_data(
                    result,
                    canvas_w,
                    canvas_h,
                    PixelFormat::Rgba8,
                )))
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
        (PixelFormat::Rgba8, PixelFormat::Rgbaf32LinearPremul) => {
            Box::new(TransformSource::new(source).push(SrgbToLinearPremul))
        }
        (PixelFormat::Rgbaf32LinearPremul, PixelFormat::Rgba8) => {
            Box::new(TransformSource::new(source).push(UnpremulLinearToSrgb))
        }
        (PixelFormat::Rgba8, PixelFormat::Rgbaf32Srgb) => {
            Box::new(TransformSource::new(source).push(ops::NormalizeU8ToF32))
        }
        (PixelFormat::Rgbaf32Srgb, PixelFormat::Rgba8) => {
            Box::new(TransformSource::new(source).push(ops::QuantizeF32ToU8))
        }
        (PixelFormat::Rgbaf32Linear, PixelFormat::Rgbaf32LinearPremul) => {
            Box::new(TransformSource::new(source).push(ops::Premultiply))
        }
        (PixelFormat::Rgbaf32LinearPremul, PixelFormat::Rgbaf32Linear) => {
            Box::new(TransformSource::new(source).push(ops::Unpremultiply))
        }
        _ => {
            // Multi-hop: go through Rgba8 as intermediate
            let intermediate = ensure_format(source, PixelFormat::Rgba8);
            ensure_format(intermediate, target)
        }
    }
}
