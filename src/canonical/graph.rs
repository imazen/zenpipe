//! Canonical 10-node typed DAG IR (`Graph`, `GraphBuilder`, `ImageOp`, `OutputOp`).
//!
//! Pure `no_std + alloc` validated IR with branded `ImageId` handles, cycle-free
//! construction, bounded node/slot counts, and zero backend crate types.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use zenlayout::{Constraint, Orientation, Rect, Size};
use zenpixels::{ColorSpace, DiffuseWhite, PixelFormat, Primaries};

#[cfg(feature = "zennode")]
use zencodecs::engine::{
    DecodeOrientation, GainMapAction, OutputIntent, ToneMap,
};
#[cfg(all(feature = "zennode", feature = "media"))]
use zencodecs::media::select::DecodeSelection;

static NEXT_GRAPH_KEY: AtomicU64 = AtomicU64::new(1);

/// Maximum nodes permitted in one canonical graph (DoS bound).
pub const MAX_GRAPH_NODES: usize = 256;
/// Maximum outputs permitted in one canonical graph.
pub const MAX_GRAPH_OUTPUTS: usize = 32;

/// Validated non-empty ASCII slot identifier (`[a-zA-Z0-9._-]{1,64}`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SlotName(String);

impl SlotName {
    /// Validate and construct a [`SlotName`].
    pub fn new(name: &str) -> Result<Self, GraphError> {
        if name.is_empty() || name.len() > 64 {
            return Err(GraphError::InvalidSlotName(name.to_string()));
        }
        if !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        {
            return Err(GraphError::InvalidSlotName(name.to_string()));
        }
        Ok(Self(name.to_string()))
    }

    /// Borrow slot name text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SlotName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Branded handle referencing a node within its owning [`GraphBuilder`] / [`Graph`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ImageId {
    graph_key: u64,
    index: u32,
}

impl ImageId {
    /// Zero-based topological node index inside its owning graph.
    #[must_use]
    pub const fn index(self) -> usize {
        self.index as usize
    }
}

/// Validation errors returned when authoring or importing a canonical [`Graph`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GraphError {
    /// Slot name failed validation.
    InvalidSlotName(String),
    /// An [`ImageId`] came from a different [`GraphBuilder`] or is out of range.
    ForeignOrInvalidHandle,
    /// Graph exceeded [`MAX_GRAPH_NODES`] or [`MAX_GRAPH_OUTPUTS`].
    GraphTooLarge,
    /// Two outputs targeted the same output slot name.
    DuplicateOutputSlot(String),
    /// Graph has zero terminal outputs.
    NoOutputs,
    /// Invalid numeric domain (zero dimension, NaN/Inf parameter, out-of-range opacity).
    InvalidParameter(String),
    /// Cycle detected during external graph import.
    CycleDetected,
    /// Node arity or port mismatch during external graph import.
    InvalidEdge(String),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSlotName(s) => write!(f, "invalid slot name: '{s}'"),
            Self::ForeignOrInvalidHandle => write!(f, "ImageId belongs to another graph or is invalid"),
            Self::GraphTooLarge => write!(f, "graph exceeds maximum node/output count"),
            Self::DuplicateOutputSlot(s) => write!(f, "duplicate output slot: '{s}'"),
            Self::NoOutputs => write!(f, "graph must declare at least one terminal output"),
            Self::InvalidParameter(s) => write!(f, "invalid node parameter: {s}"),
            Self::CycleDetected => write!(f, "cycle detected in pipeline graph"),
            Self::InvalidEdge(s) => write!(f, "invalid graph edge: {s}"),
        }
    }
}

impl core::error::Error for GraphError {}

/// Resampling kernel selection for canonical `geometry` resize/constrain ops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResizeKernel {
    /// Default high-quality perceptual downscale/upscale kernel (Robidoux / Mitchell family).
    #[default]
    Robidoux,
    /// Sharp photographic Lanczos-3 kernel.
    Lanczos3,
    /// Crisp Catmull-Rom cubic kernel.
    CatmullRom,
    /// Bilinear interpolation.
    Linear,
    /// Exact nearest-neighbor pixel replication.
    Nearest,
}

/// Canonical `decode` node payload (node 1 of 10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeNodeOp {
    /// Input slot name bound at execution time.
    pub slot: SlotName,
    /// Embedded orientation handling at decode ingress.
    #[cfg(feature = "zennode")]
    pub orientation: DecodeOrientation,
    /// Optional presentation/frame/time/page selection (default: all presentations).
    #[cfg(all(feature = "zennode", feature = "media"))]
    pub select: Option<DecodeSelection>,
    /// Optional zero-based frame selection when `media` feature is disabled.
    #[cfg(not(all(feature = "zennode", feature = "media")))]
    pub select_frame: Option<u64>,
}

/// Mode for canonical `color_ingest` node (node 2 of 10).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColorIngestMode {
    /// Keep active decoder-established tags and color space.
    #[default]
    KeepTagged,
    /// Explicitly assign a compatible color space tag without altering sample values.
    Assign,
    /// Convert ingested samples into `target`.
    Convert,
}

/// Canonical `color_ingest` node payload (node 2 of 10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColorIngestOp {
    /// Upstream image handle.
    pub src: ImageId,
    /// Ingest mode.
    pub mode: ColorIngestMode,
    /// Target color space (used by `Assign` and `Convert`).
    pub target: Option<ColorSpace>,
    /// Whether to refuse invalid/corrupt embedded ICC profiles (`true` default).
    pub strict_invalid_icc: bool,
}

/// Canonical `color_space` node payload (node 3 of 10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColorSpaceOp {
    /// Upstream image handle.
    pub src: ImageId,
    /// Target pixel format / color space.
    pub to: PixelFormat,
}

/// Canonical `tone_map` node payload (node 4 of 10).
#[derive(Clone, Debug, PartialEq)]
pub struct ToneMapNodeOp {
    /// Upstream HDR image handle.
    pub src: ImageId,
    /// Tone-mapping render record (when `zennode` is enabled).
    #[cfg(feature = "zennode")]
    pub render: ToneMap,
    /// Reference diffuse white fallback when `zennode` is disabled.
    #[cfg(not(feature = "zennode"))]
    pub white: DiffuseWhite,
}

/// Canonical `gain_map` node payload (node 5 of 10).
#[derive(Clone, Debug, PartialEq)]
pub struct GainMapNodeOp {
    /// Upstream image handle.
    pub src: ImageId,
    /// Requested gain-map action.
    #[cfg(feature = "zennode")]
    pub action: GainMapAction,
    /// Drop gain map flag when `zennode` is disabled.
    #[cfg(not(feature = "zennode"))]
    pub drop_map: bool,
}

/// Canonical `make_gain_map` node payload (node 6 of 10).
#[derive(Clone, Debug, PartialEq)]
pub struct MakeGainMapOp {
    /// Base rendition input (typically SDR).
    pub base: ImageId,
    /// Alternate rendition input (typically HDR).
    pub alternate: ImageId,
    /// Target working primaries for gain-map derivation.
    pub primaries: Primaries,
    /// Reference diffuse white for derivation.
    pub white: DiffuseWhite,
}

/// Tagged canonical `geometry` operations (node 7 of 10).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum GeometryOp {
    /// Apply a D4 orientation permutation (`zenpixels::Orientation`).
    Orient {
        /// Upstream image handle.
        src: ImageId,
        /// Exact D4 transform.
        orientation: Orientation,
    },
    /// Crop to an in-bounds half-open pixel rectangle (`zenlayout::Rect`).
    Crop {
        /// Upstream image handle.
        src: ImageId,
        /// Half-open pixel rectangle.
        rect: Rect,
    },
    /// Resize to exact dimensions using a declared resampling kernel and linear-light flag.
    Resize {
        /// Upstream image handle.
        src: ImageId,
        /// Target pixel dimensions (`width` or `height` may be 0 to preserve aspect ratio).
        width: Option<u32>,
        /// Target height (`None` = preserve aspect ratio).
        height: Option<u32>,
        /// Resampling kernel.
        kernel: ResizeKernel,
        /// Resample in linear light (`true` default; `false` = encoded gamma domain).
        linear_light: bool,
    },
    /// Constrain image dimensions using a `zenlayout::Constraint`.
    Constrain {
        /// Upstream image handle.
        src: ImageId,
        /// Layout constraint.
        constraint: Constraint,
        /// Resampling kernel.
        kernel: ResizeKernel,
        /// Resample in linear light.
        linear_light: bool,
    },
    /// Pad / extend canvas around the image with an explicit sRGB RGBA fill color.
    ExtendCanvas {
        /// Upstream image handle.
        src: ImageId,
        /// Target canvas size.
        canvas: Size,
        /// Horizontal offset of source inside canvas.
        offset_x: i32,
        /// Vertical offset of source inside canvas.
        offset_y: i32,
        /// Background fill RGBA8.
        fill_rgba: [u8; 4],
    },
}

/// Portable semantic filter operation for canonical `filter` node (node 8 of 10).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum FilterNodeOp {
    /// Exposure adjustment in photographic stops (`-5.0..=5.0`).
    Exposure {
        /// Upstream image handle.
        src: ImageId,
        /// Stops adjustment.
        stops: f32,
    },
    /// Perceptual contrast (`-1.0..=1.0`).
    Contrast {
        /// Upstream image handle.
        src: ImageId,
        /// Contrast amount.
        amount: f32,
    },
    /// Oklab saturation (`-1.0..=2.0`).
    Saturation {
        /// Upstream image handle.
        src: ImageId,
        /// Saturation amount.
        amount: f32,
    },
    /// Vibrance (`-1.0..=2.0`).
    Vibrance {
        /// Upstream image handle.
        src: ImageId,
        /// Vibrance amount.
        amount: f32,
    },
    /// Gaussian blur (`sigma > 0.0`).
    Blur {
        /// Upstream image handle.
        src: ImageId,
        /// Blur sigma in pixels.
        sigma: f32,
    },
    /// Unsharp mask sharpening.
    Sharpen {
        /// Upstream image handle.
        src: ImageId,
        /// Sharpen strength (`0.0..=5.0`).
        amount: f32,
        /// Radius in pixels (`> 0.0`).
        radius: f32,
    },
    /// Grayscale conversion honoring active primaries.
    Grayscale {
        /// Upstream image handle.
        src: ImageId,
    },
    /// Sepia tone (`0.0..=1.0`).
    Sepia {
        /// Upstream image handle.
        src: ImageId,
        /// Intensity.
        amount: f32,
    },
}

/// Blend mode for canonical `composite` node (node 9 of 10).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BlendKind {
    /// Porter-Duff source-over in linear premultiplied light.
    #[default]
    SourceOver,
    /// Overwrite destination pixels in placement rectangle.
    Copy,
    /// Multiply blend mode.
    Multiply,
    /// Screen blend mode.
    Screen,
}

/// Canonical `composite` node payload (node 9 of 10).
#[derive(Clone, Debug, PartialEq)]
pub struct CompositeOp {
    /// Background canvas image handle.
    pub canvas: ImageId,
    /// Foreground overlay image handle.
    pub foreground: ImageId,
    /// Placement X offset on canvas.
    pub x: i32,
    /// Placement Y offset on canvas.
    pub y: i32,
    /// Opacity (`0.0..=1.0`).
    pub opacity: f32,
    /// Blend operator.
    pub blend: BlendKind,
}

/// All 9 non-terminal image-valued operations in the canonical 10-node graph
/// (terminal node 10 is [`OutputOp::Encode`]).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum ImageOp {
    /// 1. Decode encoded bytes (still, animation, video container, or page).
    Decode(DecodeNodeOp),
    /// 2. Color ingress interpretation / assignment / conversion.
    ColorIngest(ColorIngestOp),
    /// 3. Working color space / format conversion.
    ColorSpace(ColorSpaceOp),
    /// 4. Explicit HDR ↔ SDR tone mapping.
    ToneMap(ToneMapNodeOp),
    /// 5. Gain-map disposition (base-only, reconstruct, carry).
    GainMap(GainMapNodeOp),
    /// 6. Synthesize a fresh gain map from co-registered base + alternate renditions.
    MakeGainMap(MakeGainMapOp),
    /// 7. Geometry transform (orient, crop, resize, constrain, extend canvas).
    Geometry(GeometryOp),
    /// 8. Portable semantic photo/neighborhood filter.
    Filter(FilterNodeOp),
    /// 9. Compositing of foreground onto canvas.
    Composite(CompositeOp),
    /// Synthetic solid-color canvas root (for canvas-only graphs).
    Canvas {
        /// Canvas dimensions.
        size: Size,
        /// Fill color RGBA8.
        fill_rgba: [u8; 4],
    },
}

/// Canonical terminal `encode` node payload (node 10 of 10).
#[derive(Clone, Debug, PartialEq)]
pub struct EncodeNodeOp {
    /// Upstream image/presentation handle.
    pub src: ImageId,
    /// Output slot name.
    pub slot: SlotName,
    /// Portable encode intent shared identically with `zencodecs`.
    #[cfg(feature = "zennode")]
    pub intent: OutputIntent,
}

/// Terminal output operations.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum OutputOp {
    /// 10. Encode to a still image, animation, or video container via `zencodecs::Engine`.
    Encode(Box<EncodeNodeOp>),
    /// Deliver decoded/processed raw pixels to a host pixel output slot.
    Pixels {
        /// Upstream image handle.
        src: ImageId,
        /// Output slot name.
        slot: SlotName,
        /// Optional required pixel format.
        format: Option<PixelFormat>,
    },
}

/// Validated, immutable canonical DAG ready for compilation and execution.
#[derive(Clone, Debug, PartialEq)]
pub struct Graph {
    graph_key: u64,
    nodes: Vec<ImageOp>,
    outputs: Vec<OutputOp>,
}

impl Graph {
    /// Borrow all image nodes in topological order.
    #[must_use]
    pub fn nodes(&self) -> &[ImageOp] {
        &self.nodes
    }

    /// Borrow all terminal outputs.
    #[must_use]
    pub fn outputs(&self) -> &[OutputOp] {
        &self.outputs
    }

    /// Look up an image node by handle.
    #[must_use]
    pub fn node(&self, id: ImageId) -> Option<&ImageOp> {
        if id.graph_key != self.graph_key {
            return None;
        }
        self.nodes.get(id.index as usize)
    }

    /// Collect all input slot names referenced by `Decode` nodes in declaration order.
    #[must_use]
    pub fn input_slots(&self) -> Vec<&SlotName> {
        let mut out = Vec::new();
        for op in &self.nodes {
            if let ImageOp::Decode(d) = op
                && !out.contains(&&d.slot)
            {
                out.push(&d.slot);
            }
        }
        out
    }

    /// Collect all output slot names referenced by terminal outputs.
    #[must_use]
    pub fn output_slots(&self) -> Vec<&SlotName> {
        self.outputs
            .iter()
            .map(|o| match o {
                OutputOp::Encode(e) => &e.slot,
                OutputOp::Pixels { slot, .. } => slot,
            })
            .collect()
    }
}

/// Checked builder for [`Graph`] ensuring acyclic construction, branded handle ownership,
/// parameter domain validation, and unique output slots.
#[derive(Debug)]
pub struct GraphBuilder {
    graph_key: u64,
    nodes: Vec<ImageOp>,
    outputs: Vec<OutputOp>,
}

impl Default for GraphBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphBuilder {
    /// Create a fresh empty builder with a unique graph brand.
    #[must_use]
    pub fn new() -> Self {
        Self {
            graph_key: NEXT_GRAPH_KEY.fetch_add(1, Ordering::Relaxed),
            nodes: Vec::new(),
            outputs: Vec::new(),
        }
    }

    fn check_handle(&self, id: ImageId) -> Result<(), GraphError> {
        if id.graph_key != self.graph_key || (id.index as usize) >= self.nodes.len() {
            return Err(GraphError::ForeignOrInvalidHandle);
        }
        Ok(())
    }

    /// Insert a validated [`ImageOp`] and return its branded [`ImageId`].
    pub fn image(&mut self, op: ImageOp) -> Result<ImageId, GraphError> {
        if self.nodes.len() >= MAX_GRAPH_NODES {
            return Err(GraphError::GraphTooLarge);
        }
        match &op {
            ImageOp::Decode(_) => {}
            ImageOp::Canvas { size, .. } => {
                if size.width == 0 || size.height == 0 {
                    return Err(GraphError::InvalidParameter("canvas dimensions must be > 0".into()));
                }
            }
            ImageOp::ColorIngest(c) => self.check_handle(c.src)?,
            ImageOp::ColorSpace(c) => self.check_handle(c.src)?,
            ImageOp::ToneMap(t) => self.check_handle(t.src)?,
            ImageOp::GainMap(g) => self.check_handle(g.src)?,
            ImageOp::MakeGainMap(m) => {
                self.check_handle(m.base)?;
                self.check_handle(m.alternate)?;
            }
            ImageOp::Geometry(g) => match g {
                GeometryOp::Orient { src, .. } => self.check_handle(*src)?,
                GeometryOp::Crop { src, rect } => {
                    self.check_handle(*src)?;
                    if rect.width == 0 || rect.height == 0 {
                        return Err(GraphError::InvalidParameter("crop width/height must be > 0".into()));
                    }
                }
                GeometryOp::Resize {
                    src, width, height, ..
                } => {
                    self.check_handle(*src)?;
                    if width.is_none() && height.is_none() {
                        return Err(GraphError::InvalidParameter(
                            "resize requires at least width or height".into(),
                        ));
                    }
                    if width == &Some(0) || height == &Some(0) {
                        return Err(GraphError::InvalidParameter(
                            "resize width/height must be > 0".into(),
                        ));
                    }
                }
                GeometryOp::Constrain { src, .. } => self.check_handle(*src)?,
                GeometryOp::ExtendCanvas { src, canvas, .. } => {
                    self.check_handle(*src)?;
                    if canvas.width == 0 || canvas.height == 0 {
                        return Err(GraphError::InvalidParameter(
                            "extend canvas dimensions must be > 0".into(),
                        ));
                    }
                }
            },
            ImageOp::Filter(f) => match f {
                FilterNodeOp::Exposure { src, stops } => {
                    self.check_handle(*src)?;
                    if !stops.is_finite() || !(-10.0..=10.0).contains(stops) {
                        return Err(GraphError::InvalidParameter("exposure stops out of range".into()));
                    }
                }
                FilterNodeOp::Contrast { src, amount } => {
                    self.check_handle(*src)?;
                    if !amount.is_finite() || !(-1.0..=1.0).contains(amount) {
                        return Err(GraphError::InvalidParameter("contrast out of range".into()));
                    }
                }
                FilterNodeOp::Saturation { src, amount }
                | FilterNodeOp::Vibrance { src, amount } => {
                    self.check_handle(*src)?;
                    if !amount.is_finite() || !(-1.0..=2.0).contains(amount) {
                        return Err(GraphError::InvalidParameter("saturation/vibrance out of range".into()));
                    }
                }
                FilterNodeOp::Blur { src, sigma } => {
                    self.check_handle(*src)?;
                    if !sigma.is_finite() || *sigma <= 0.0 || *sigma > 256.0 {
                        return Err(GraphError::InvalidParameter("blur sigma must be in (0, 256]".into()));
                    }
                }
                FilterNodeOp::Sharpen {
                    src,
                    amount,
                    radius,
                } => {
                    self.check_handle(*src)?;
                    if !amount.is_finite()
                        || !(0.0..=5.0).contains(amount)
                        || !radius.is_finite()
                        || *radius <= 0.0
                        || *radius > 64.0
                    {
                        return Err(GraphError::InvalidParameter("sharpen amount/radius out of range".into()));
                    }
                }
                FilterNodeOp::Grayscale { src } => self.check_handle(*src)?,
                FilterNodeOp::Sepia { src, amount } => {
                    self.check_handle(*src)?;
                    if !amount.is_finite() || !(0.0..=1.0).contains(amount) {
                        return Err(GraphError::InvalidParameter("sepia amount must be in [0, 1]".into()));
                    }
                }
            },
            ImageOp::Composite(c) => {
                self.check_handle(c.canvas)?;
                self.check_handle(c.foreground)?;
                if !c.opacity.is_finite() || !(0.0..=1.0).contains(&c.opacity) {
                    return Err(GraphError::InvalidParameter("composite opacity must be in [0, 1]".into()));
                }
            }
        }
        let index = self.nodes.len() as u32;
        self.nodes.push(op);
        Ok(ImageId {
            graph_key: self.graph_key,
            index,
        })
    }

    /// Attach a terminal [`OutputOp`]. Rejects duplicate output slot names.
    pub fn output(&mut self, op: OutputOp) -> Result<(), GraphError> {
        if self.outputs.len() >= MAX_GRAPH_OUTPUTS {
            return Err(GraphError::GraphTooLarge);
        }
        let (src, slot_str) = match &op {
            OutputOp::Encode(e) => {
                #[cfg(feature = "zennode")]
                if let Err(err) = e.intent.validate() {
                    return Err(GraphError::InvalidParameter(err.error().message().to_string()));
                }
                (e.src, e.slot.as_str())
            }
            OutputOp::Pixels { src, slot, .. } => (*src, slot.as_str()),
        };
        self.check_handle(src)?;
        let dup = self.outputs.iter().any(|existing| {
            let ex = match existing {
                OutputOp::Encode(e) => e.slot.as_str(),
                OutputOp::Pixels { slot, .. } => slot.as_str(),
            };
            ex == slot_str
        });
        if dup {
            return Err(GraphError::DuplicateOutputSlot(slot_str.to_string()));
        }
        self.outputs.push(op);
        Ok(())
    }

    /// Finalize into an immutable validated [`Graph`].
    pub fn finish(self) -> Result<Graph, GraphError> {
        if self.outputs.is_empty() {
            return Err(GraphError::NoOutputs);
        }
        Ok(Graph {
            graph_key: self.graph_key,
            nodes: self.nodes,
            outputs: self.outputs,
        })
    }
}

/// Complete portable job carrying a validated [`Graph`] plus optional tightened limits and diagnostics.
#[derive(Clone, Debug)]
pub struct CanonicalJob {
    graph: Graph,
    /// Optional tightened limits parsed from wire envelope.
    pub max_output_bytes: Option<u64>,
    /// Optional tightened dimension cap parsed from wire envelope.
    pub max_dimension: Option<u32>,
    /// Non-fatal ingress diagnostics.
    pub diagnostics: Vec<String>,
}

impl CanonicalJob {
    /// Wrap a validated [`Graph`] as a [`CanonicalJob`].
    #[must_use]
    pub fn from_graph(graph: Graph) -> Self {
        Self {
            graph,
            max_output_bytes: None,
            max_dimension: None,
            diagnostics: Vec::new(),
        }
    }

    /// Borrow the underlying immutable [`Graph`].
    #[must_use]
    pub const fn graph(&self) -> &Graph {
        &self.graph
    }
}
