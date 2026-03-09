//! Graph-based pipeline compiler.
//!
//! Compiles a DAG of image operations into a pull-based [`Source`] chain.
//! The caller builds a [`PipelineGraph`], adds nodes and edges, then calls
//! [`compile`](PipelineGraph::compile) to produce an executable source.
//!
//! # Design
//!
//! - **No estimate phase**: dimensions propagate naturally through `Source::width()`/`height()`
//!   during compilation. Each node queries its upstream source's dimensions at construction time.
//! - **No expand phase**: zen crates handle internal optimization (zenresize handles
//!   sRGB→linear→premul internally, zenlayout computes layouts). The graph compiler just wires
//!   sources together.
//! - **Automatic format conversion**: the compiler inserts `SrgbToLinearPremul` /
//!   `UnpremulLinearToSrgb` where required (e.g., before composite, before resize).
//! - **Op fusion**: adjacent [`PixelTransform`](NodeOp::PixelTransform) nodes are fused into
//!   a single [`TransformSource`](crate::sources::TransformSource) — one pass, no intermediate
//!   buffers between fused ops.
//!
//! # Example
//!
//! ```ignore
//! let mut g = PipelineGraph::new();
//! let src = g.add_node(NodeOp::Source);
//! let crop = g.add_node(NodeOp::Crop { x: 10, y: 10, w: 200, h: 200 });
//! let resize = g.add_node(NodeOp::Resize { w: 100, h: 100 });
//! let out = g.add_node(NodeOp::Output);
//!
//! g.add_edge(src, crop, EdgeKind::Input);
//! g.add_edge(crop, resize, EdgeKind::Input);
//! g.add_edge(resize, out, EdgeKind::Input);
//!
//! let mut sources = HashMap::new();
//! sources.insert(src, my_decoder_source);
//!
//! let pipeline = g.compile(sources)?;
//! // pipeline is a Box<dyn Source> — drain into a sink/encoder
//! ```

use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::Source;
use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::ops::{self, PixelOp, SrgbToLinearPremul, UnpremulLinearToSrgb};
use crate::sources::{
    CompositeSource, CropSource, FlipHSource, MaterializedSource, ResizeSource, TransformSource,
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

    // === Streamable geometry ===
    /// Crop to a rectangle. Streamed via [`CropSource`].
    Crop { x: u32, y: u32, w: u32, h: u32 },

    /// Resize to target dimensions. Streamed via [`ResizeSource`].
    /// Input must be `Rgba8` (inserted automatically if needed).
    Resize { w: u32, h: u32 },

    // === Per-pixel ops (fusible into TransformSource) ===
    /// A per-pixel operation. Adjacent `PixelTransform` nodes are fused
    /// into a single [`TransformSource`] — one pass, no intermediate buffers.
    PixelTransform(Box<dyn PixelOp>),

    // === Multi-input ===
    /// Porter-Duff source-over composite.
    ///
    /// - `Canvas` edge = background
    /// - `Input` edge = foreground (placed at `fg_x, fg_y`)
    ///
    /// Both inputs are automatically converted to `Rgbaf32LinearPremul`.
    Composite { fg_x: u32, fg_y: u32 },

    // === Geometry requiring materialization ===
    /// Flip horizontal — streaming, no materialization.
    FlipH,
    /// Flip vertical — requires materialization.
    FlipV,
    /// Rotate 90° clockwise — requires materialization.
    Rotate90,
    /// Rotate 180° — requires materialization.
    Rotate180,
    /// Rotate 270° clockwise — requires materialization.
    Rotate270,
    /// Transpose (swap x/y) — requires materialization.
    Transpose,

    /// Expand canvas with padding. Fill color is `Rgba8`.
    ExpandCanvas {
        left: u32,
        top: u32,
        right: u32,
        bottom: u32,
        color: [u8; 4],
    },

    /// Custom materialization barrier — drain upstream, transform, re-stream.
    ///
    /// The closure receives `(data, width, height, format)` and may mutate all of them.
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
        // Find the output node
        let output_id = self
            .nodes
            .iter()
            .position(|n| matches!(&n.op, Some(NodeOp::Output)))
            .ok_or_else(|| PipeError::Op("graph has no Output node".to_string()))?;

        self.compile_node(output_id, &mut sources)
    }

    /// Find the upstream node connected to `node_id` via an edge of the given kind.
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

    /// Count how many output edges a node has.
    fn output_count(&self, node_id: NodeId) -> usize {
        self.edges.iter().filter(|e| e.from == node_id).count()
    }

    /// Peek at a node's operation without taking it.
    fn peek_op(&self, id: NodeId) -> Option<&NodeOp> {
        self.nodes[id].op.as_ref()
    }

    /// Take a node's operation (consumes it — can only be called once per node).
    fn take_op(&mut self, id: NodeId) -> Result<NodeOp, PipeError> {
        self.nodes[id]
            .op
            .take()
            .ok_or_else(|| PipeError::Op(alloc::format!("node {id} already compiled")))
    }

    /// Recursively compile a node into a Source.
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

            NodeOp::PixelTransform(pixel_op) => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                // Collect chain of adjacent PixelTransform nodes for fusion
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

            NodeOp::FlipH => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources)?;
                Ok(Box::new(FlipHSource::new(upstream)))
            }

            NodeOp::FlipV => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources)?;
                Ok(Box::new(MaterializedSource::from_source_with_transform(
                    upstream,
                    |data, w, h, fmt| flip_vertical(data, *w, *h, *fmt),
                )?))
            }

            NodeOp::Rotate90 => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources)?;
                Ok(Box::new(MaterializedSource::from_source_with_transform(
                    upstream,
                    |data, w, h, fmt| rotate_90(data, w, h, *fmt),
                )?))
            }

            NodeOp::Rotate180 => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources)?;
                Ok(Box::new(MaterializedSource::from_source_with_transform(
                    upstream,
                    |data, w, h, fmt| {
                        flip_horizontal(data, *w, *h, *fmt);
                        flip_vertical(data, *w, *h, *fmt);
                    },
                )?))
            }

            NodeOp::Rotate270 => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources)?;
                Ok(Box::new(MaterializedSource::from_source_with_transform(
                    upstream,
                    |data, w, h, fmt| rotate_270(data, w, h, *fmt),
                )?))
            }

            NodeOp::Transpose => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources)?;
                Ok(Box::new(MaterializedSource::from_source_with_transform(
                    upstream,
                    |data, w, h, fmt| transpose(data, w, h, *fmt),
                )?))
            }

            NodeOp::ExpandCanvas {
                left,
                top,
                right,
                bottom,
                color,
            } => {
                let input_id = self.find_input(node_id, EdgeKind::Input)?;
                let upstream = self.compile_node(input_id, sources)?;
                let upstream = ensure_format(upstream, PixelFormat::Rgba8);
                Ok(Box::new(MaterializedSource::from_source_with_transform(
                    upstream,
                    move |data, w, h, _fmt| {
                        expand_canvas(data, w, h, left, top, right, bottom, color);
                    },
                )?))
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
    ///
    /// Returns `(first_non_pixel_upstream_id, collected_ops)`.
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

// =============================================================================
// Geometry transforms for materialized buffers
// =============================================================================

fn flip_horizontal(data: &mut [u8], width: u32, height: u32, fmt: PixelFormat) {
    let bpp = fmt.bytes_per_pixel();
    let stride = width as usize * bpp;
    let w = width as usize;
    for y in 0..height as usize {
        let row = &mut data[y * stride..(y + 1) * stride];
        for x in 0..w / 2 {
            let a = x * bpp;
            let b = (w - 1 - x) * bpp;
            for c in 0..bpp {
                row.swap(a + c, b + c);
            }
        }
    }
}

fn flip_vertical(data: &mut [u8], width: u32, height: u32, fmt: PixelFormat) {
    let stride = width as usize * fmt.bytes_per_pixel();
    let h = height as usize;
    for y in 0..h / 2 {
        let top = y * stride;
        let bot = (h - 1 - y) * stride;
        // Swap rows using split_at_mut
        let (first, second) = data.split_at_mut(bot);
        first[top..top + stride].swap_with_slice(&mut second[..stride]);
    }
}

fn rotate_90(data: &mut alloc::vec::Vec<u8>, w: &mut u32, h: &mut u32, fmt: PixelFormat) {
    let bpp = fmt.bytes_per_pixel();
    let old_w = *w as usize;
    let old_h = *h as usize;
    let mut out = alloc::vec![0u8; data.len()];
    let new_w = old_h;
    let new_stride = new_w * bpp;

    for y in 0..old_h {
        for x in 0..old_w {
            let src = y * old_w * bpp + x * bpp;
            // 90° CW: (x, y) → (h-1-y, x) in new coords, where new_w=old_h
            let dst_x = old_h - 1 - y;
            let dst_y = x;
            let dst = dst_y * new_stride + dst_x * bpp;
            out[dst..dst + bpp].copy_from_slice(&data[src..src + bpp]);
        }
    }

    *data = out;
    *w = old_h as u32;
    *h = old_w as u32;
}

fn rotate_270(data: &mut alloc::vec::Vec<u8>, w: &mut u32, h: &mut u32, fmt: PixelFormat) {
    let bpp = fmt.bytes_per_pixel();
    let old_w = *w as usize;
    let old_h = *h as usize;
    let mut out = alloc::vec![0u8; data.len()];
    let new_w = old_h;
    let new_stride = new_w * bpp;

    for y in 0..old_h {
        for x in 0..old_w {
            let src = y * old_w * bpp + x * bpp;
            // 270° CW: (x, y) → (y, w-1-x) in new coords
            let dst_x = y;
            let dst_y = old_w - 1 - x;
            let dst = dst_y * new_stride + dst_x * bpp;
            out[dst..dst + bpp].copy_from_slice(&data[src..src + bpp]);
        }
    }

    *data = out;
    *w = old_h as u32;
    *h = old_w as u32;
}

fn transpose(data: &mut alloc::vec::Vec<u8>, w: &mut u32, h: &mut u32, fmt: PixelFormat) {
    let bpp = fmt.bytes_per_pixel();
    let old_w = *w as usize;
    let old_h = *h as usize;
    let mut out = alloc::vec![0u8; data.len()];
    let new_w = old_h;
    let new_stride = new_w * bpp;

    for y in 0..old_h {
        for x in 0..old_w {
            let src = y * old_w * bpp + x * bpp;
            let dst = x * new_stride + y * bpp;
            out[dst..dst + bpp].copy_from_slice(&data[src..src + bpp]);
        }
    }

    *data = out;
    *w = old_h as u32;
    *h = old_w as u32;
}

#[allow(clippy::too_many_arguments)]
fn expand_canvas(
    data: &mut alloc::vec::Vec<u8>,
    w: &mut u32,
    h: &mut u32,
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
    color: [u8; 4],
) {
    let old_w = *w as usize;
    let old_h = *h as usize;
    let new_w = old_w + left as usize + right as usize;
    let new_h = old_h + top as usize + bottom as usize;
    let bpp = 4usize; // Rgba8
    let new_stride = new_w * bpp;
    let old_stride = old_w * bpp;

    let mut out = alloc::vec![0u8; new_w * new_h * bpp];

    // Fill entire output with background color
    for px in out.chunks_exact_mut(bpp) {
        px.copy_from_slice(&color);
    }

    // Copy source pixels at offset (left, top)
    for y in 0..old_h {
        let src_start = y * old_stride;
        let dst_start = (y + top as usize) * new_stride + left as usize * bpp;
        out[dst_start..dst_start + old_stride]
            .copy_from_slice(&data[src_start..src_start + old_stride]);
    }

    *data = out;
    *w = new_w as u32;
    *h = new_h as u32;
}
