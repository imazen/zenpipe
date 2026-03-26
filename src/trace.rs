//! Pipeline tracing and debugging.
//!
//! Opt-in runtime tracing that records every format conversion, dimension change,
//! and alpha mode transition at each node boundary. Optionally dumps pixel data
//! to uncompressed 16-bit PNGs at any pipeline node.
//!
//! # Usage
//!
//! ```ignore
//! let config = TraceConfig::metadata_only();
//! let (source, trace) = graph.compile_traced(sources, &config)?;
//! // drain pipeline...
//! println!("{}", trace.to_text());
//! ```

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::format::PixelFormat;

/// What to trace at each node boundary.
#[derive(Clone, Debug)]
pub struct TraceConfig {
    /// Enable metadata collection (format, dims, alpha at each node).
    pub metadata: bool,

    /// Directory to dump PNG16 pixel snapshots.
    /// When Some, every node boundary saves pixels.
    #[cfg(feature = "std")]
    pub pixel_dump_dir: Option<std::path::PathBuf>,

    /// Specific node indices to dump (empty = dump all when pixel_dump_dir is set).
    pub dump_nodes: Vec<usize>,
}

impl TraceConfig {
    /// Trace metadata only (format/dims/alpha transitions). Near-zero cost.
    pub fn metadata_only() -> Self {
        Self {
            metadata: true,
            #[cfg(feature = "std")]
            pixel_dump_dir: None,
            dump_nodes: Vec::new(),
        }
    }

    /// Trace metadata + dump all nodes to PNG16.
    #[cfg(feature = "std")]
    pub fn with_pixel_dump(dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            metadata: true,
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
        { false }
    }
}

/// One entry per node boundary in the compiled pipeline.
#[derive(Clone, Debug)]
pub struct TraceEntry {
    /// Node index in compile order.
    pub index: usize,
    /// Short name (e.g., "Resize", "ConvertFormat").
    pub name: String,
    /// Detailed description (e.g., "Resize 800x600 → 400x300 Robidoux").
    pub description: String,
    /// Input format (from upstream).
    pub input_format: PixelFormat,
    /// Input dimensions.
    pub input_width: u32,
    pub input_height: u32,
    /// Output format (after this node).
    pub output_format: PixelFormat,
    /// Output dimensions.
    pub output_width: u32,
    pub output_height: u32,
    /// Whether this node materializes (full-frame buffer).
    pub materializes: bool,
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

/// Collected pipeline trace data.
#[derive(Clone, Debug, Default)]
pub struct PipelineTrace {
    pub entries: Vec<TraceEntry>,
}

impl PipelineTrace {
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    pub fn push(&mut self, entry: TraceEntry) {
        self.entries.push(entry);
    }

    /// Human-readable text summary of the pipeline.
    pub fn to_text(&self) -> String {
        let mut out = format!("Pipeline Trace ({} nodes)\n", self.entries.len());
        out.push_str(&"=".repeat(60));
        out.push('\n');

        for e in &self.entries {
            let dims = if e.dims_changed() {
                format!("{}x{} → {}x{}", e.input_width, e.input_height, e.output_width, e.output_height)
            } else {
                format!("{}x{}", e.output_width, e.output_height)
            };

            let fmt = if e.format_changed() {
                format!("{} → {}", format_short(&e.input_format), format_short(&e.output_format))
            } else {
                format_short(&e.output_format)
            };

            let flags = if e.materializes { " [MAT]" } else { "" };

            out.push_str(&format!("[{:2}] {:<20} {:>15}  {}{}\n",
                e.index, e.name, dims, fmt, flags));
        }

        out.push_str(&"=".repeat(60));
        out.push('\n');

        let format_changes = self.entries.iter().filter(|e| e.format_changed()).count();
        let alpha_changes = self.entries.iter().filter(|e| e.alpha_changed()).count();
        let materializations = self.entries.iter().filter(|e| e.materializes).count();
        out.push_str(&format!("Format changes: {} | Alpha changes: {} | Materializations: {}\n",
            format_changes, alpha_changes, materializations));

        out
    }

    /// Generate SVG visualization of the pipeline.
    pub fn to_svg(&self) -> String {
        use core::fmt::Write;
        let node_w = 220u32;
        let node_h = 70u32;
        let gap = 40u32;
        let margin = 20u32;
        let total_w = self.entries.len() as u32 * (node_w + gap) + margin * 2;
        let total_h = node_h + margin * 2 + 30;

        let mut s = String::with_capacity(4096);
        let _ = write!(s, "<svg xmlns='http://www.w3.org/2000/svg' width='{total_w}' height='{total_h}' font-family='monospace' font-size='11'>");
        let _ = write!(s, "<style>rect{{rx:8;ry:8}} .fmt{{fill:#666}} .dim{{fill:#333;font-weight:bold}} .name{{fill:#000;font-weight:bold;font-size:13px}}</style>");

        for (i, e) in self.entries.iter().enumerate() {
            let x = margin + i as u32 * (node_w + gap);
            let y = margin;

            let fill = if e.materializes { "#ffe0e0" }
                else if e.format_changed() { "#fff3e0" }
                else { "#e8f5e9" };
            let _ = write!(s, "<rect x='{x}' y='{y}' width='{node_w}' height='{node_h}' fill='{fill}' stroke='#ccc'/>");
            let _ = write!(s, "<text x='{}' y='{}' class='name'>{}</text>", x + 8, y + 18, e.name);

            let dims = if e.dims_changed() {
                format!("{}x{} -> {}x{}", e.input_width, e.input_height, e.output_width, e.output_height)
            } else {
                format!("{}x{}", e.output_width, e.output_height)
            };
            let _ = write!(s, "<text x='{}' y='{}' class='dim'>{dims}</text>", x + 8, y + 35);

            let fmt = format_short(&e.output_format);
            let _ = write!(s, "<text x='{}' y='{}' class='fmt'>{fmt}</text>", x + 8, y + 52);

            if e.alpha_changed() {
                let label = if e.output_format.has_alpha() { "+a" } else { "-a" };
                let _ = write!(s, "<text x='{}' y='{}' fill='#e63946' font-weight='bold'>{label}</text>", x + node_w - 30, y + 18);
            }

            if i + 1 < self.entries.len() {
                let x1 = x + node_w;
                let x2 = x + node_w + gap;
                let cy = y + node_h / 2;
                let color = if e.alpha_changed() { "#e63946" }
                    else if e.format_changed() { "#f4a261" }
                    else { "#999" };
                let _ = write!(s, "<line x1='{x1}' y1='{cy}' x2='{x2}' y2='{cy}' stroke='{color}' stroke-width='2'/>");
            }
        }

        s.push_str("</svg>");
        s
    }
}

/// Short human-readable format description.
fn format_short(fmt: &PixelFormat) -> String {
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
        _ => "Unknown",
    }
}
