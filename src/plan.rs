//! Command pipeline and decoder negotiation.
//!
//! Two-phase layout planning:
//! 1. [`compute_layout()`] — compute ideal layout from commands + source dimensions → [`IdealLayout`] + [`DecoderRequest`]
//! 2. [`IdealLayout::finalize()`] — given what the decoder actually did ([`DecoderOffer`]), compute remaining work → [`LayoutPlan`]
//!
//! ```text
//!     Commands + Source
//!           │
//!           ▼
//!     ┌──────────────┐     ┌──────────────┐
//!     │compute_layout│────►│DecoderRequest│───► Decoder
//!     └──────────────┘     └──────────────┘        │
//!           │                                      │
//!           ▼                                      ▼
//!     ┌───────────┐       ┌─────────────┐    ┌───────────┐
//!     │IdealLayout│──────►│ finalize()  │◄───│DecoderOffer│
//!     └───────────┘       └─────────────┘    └───────────┘
//!                               │
//!                               ▼
//!                         ┌──────────┐
//!                         │LayoutPlan│ ── final operations
//!                         └──────────┘
//! ```

use crate::constraint::{
    CanvasColor, Constraint, ConstraintMode, Layout, LayoutError, Rect, Size, SourceCrop,
};
use crate::orientation::Orientation;

/// Rotation amount for manual rotation commands.
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Rotation {
    /// 90 degrees clockwise.
    Rotate90,
    /// 180 degrees.
    Rotate180,
    /// 270 degrees clockwise (90 counter-clockwise).
    Rotate270,
}

/// Axis for manual flip commands.
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FlipAxis {
    /// Flip left-right.
    Horizontal,
    /// Flip top-bottom.
    Vertical,
}

/// A coordinate in the region viewport, expressed as a percentage of the
/// source dimension plus a pixel offset.
///
/// Resolved as: `source_dimension * percent + pixels`
///
/// # Examples
///
/// - `RegionCoord::px(0)` — source origin
/// - `RegionCoord::px(-50)` — 50px before source origin (padding area)
/// - `RegionCoord::pct(1.0)` — source far edge
/// - `RegionCoord::pct_px(1.0, 50)` — 50px past source far edge (padding area)
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RegionCoord {
    /// Fraction of source dimension (0.0 = origin, 1.0 = far edge).
    pub percent: f32,
    /// Additional pixel offset (can be negative).
    pub pixels: i32,
}

impl RegionCoord {
    /// Coordinate at a pixel offset from the source origin.
    pub const fn px(pixels: i32) -> Self {
        Self {
            percent: 0.0,
            pixels,
        }
    }

    /// Coordinate at a percentage of source dimension.
    pub const fn pct(percent: f32) -> Self {
        Self { percent, pixels: 0 }
    }

    /// Coordinate at a percentage plus pixel offset.
    pub const fn pct_px(percent: f32, pixels: i32) -> Self {
        Self { percent, pixels }
    }

    /// Resolve to absolute pixel coordinate given source dimension.
    pub fn resolve(self, source_dim: u32) -> i32 {
        (source_dim as f64 * self.percent as f64).round() as i32 + self.pixels
    }
}

/// A viewport rectangle in source coordinates defining a window into an
/// infinite canvas.
///
/// The source image occupies `[0, source_w) × [0, source_h)`. Areas of the
/// viewport outside the source are filled with `color`. Areas of the source
/// outside the viewport are cropped.
///
/// This unifies crop and pad into a single operation:
/// - Viewport smaller than source → crop
/// - Viewport extending beyond source → pad
/// - Viewport entirely outside source → blank canvas
///
/// Coordinates are **edge-based** (left, top, right, bottom), not origin + size.
/// A viewport from `(10, 10, 90, 90)` is 80×80 pixels. This differs from
/// [`SourceCrop::pixels`](crate::SourceCrop::pixels) which uses origin + size:
/// `(10, 10, 80, 80)` for the same region.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Region {
    /// Left edge of viewport in source x-coordinates.
    pub left: RegionCoord,
    /// Top edge of viewport in source y-coordinates.
    pub top: RegionCoord,
    /// Right edge of viewport in source x-coordinates.
    pub right: RegionCoord,
    /// Bottom edge of viewport in source y-coordinates.
    pub bottom: RegionCoord,
    /// Fill color for areas outside the source image.
    pub color: CanvasColor,
}

impl Region {
    /// Viewport from pixel edge coordinates (transparent fill).
    ///
    /// `Region::crop(10, 10, 90, 90)` selects an 80×80 region.
    pub const fn crop(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left: RegionCoord::px(left),
            top: RegionCoord::px(top),
            right: RegionCoord::px(right),
            bottom: RegionCoord::px(bottom),
            color: CanvasColor::Transparent,
        }
    }

    /// Uniform padding around the full source.
    pub const fn padded(amount: u32, color: CanvasColor) -> Self {
        Self {
            left: RegionCoord::px(-(amount as i32)),
            top: RegionCoord::px(-(amount as i32)),
            right: RegionCoord::pct_px(1.0, amount as i32),
            bottom: RegionCoord::pct_px(1.0, amount as i32),
            color,
        }
    }

    /// Blank canvas (viewport entirely outside source).
    ///
    /// Creates a viewport in negative coordinate space with the given
    /// dimensions, guaranteeing zero overlap with the source image.
    pub const fn blank(width: u32, height: u32, color: CanvasColor) -> Self {
        // Place viewport so right edge = -1, bottom edge = -1.
        // Source occupies [0, source_w) × [0, source_h), so a viewport
        // ending at -1 can never overlap regardless of source dimensions.
        let w = width as i32;
        let h = height as i32;
        Self {
            left: RegionCoord::px(-w - 1),
            top: RegionCoord::px(-h - 1),
            right: RegionCoord::px(-1),
            bottom: RegionCoord::px(-1),
            color,
        }
    }

    /// Resolve all coordinates against source dimensions.
    /// Returns `(left, top, right, bottom)` in absolute pixels.
    fn resolve(self, source_w: u32, source_h: u32) -> (i32, i32, i32, i32) {
        (
            self.left.resolve(source_w),
            self.top.resolve(source_h),
            self.right.resolve(source_w),
            self.bottom.resolve(source_h),
        )
    }
}

impl SourceCrop {
    /// Convert to an equivalent Region (viewport coordinates, transparent fill).
    pub fn to_region(self) -> Region {
        match self {
            Self::Pixels(r) => Region {
                left: RegionCoord::px(r.x as i32),
                top: RegionCoord::px(r.y as i32),
                right: RegionCoord::px((r.x + r.width) as i32),
                bottom: RegionCoord::px((r.y + r.height) as i32),
                color: CanvasColor::Transparent,
            },
            Self::Percent {
                x,
                y,
                width,
                height,
            } => Region {
                left: RegionCoord::pct(x),
                top: RegionCoord::pct(y),
                right: RegionCoord::pct(x + width),
                bottom: RegionCoord::pct(y + height),
                color: CanvasColor::Transparent,
            },
        }
    }
}

/// A single image processing command.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Apply EXIF orientation correction (value 1-8).
    AutoOrient(u8),
    /// Manual rotation. Composes with all orientation commands into
    /// a single source transform.
    Rotate(Rotation),
    /// Manual flip. Composes with all orientation commands into
    /// a single source transform.
    Flip(FlipAxis),
    /// Crop in post-orientation coordinates.
    Crop(SourceCrop),
    /// Viewport region in post-orientation coordinates. Unifies crop and pad.
    Region(Region),
    /// Constrain dimensions in post-orientation coordinates.
    Constrain(Constraint),
    /// Add padding around the image.
    Pad(Padding),
}

/// Result of the first phase of layout planning.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct IdealLayout {
    /// Net orientation to apply.
    pub orientation: Orientation,
    /// Layout computed in post-orientation space.
    pub layout: Layout,
    /// Source crop transformed back to pre-orientation source coordinates.
    pub source_crop: Option<Rect>,
    /// Padding to add around the final image.
    pub padding: Option<Padding>,
    /// If [`Align::Extend`] was used, crop to these dimensions after encoding.
    /// Canvas was extended with replicated edges; this records the real content size.
    pub content_size: Option<Size>,
}

/// Explicit padding specification.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Padding {
    /// Top padding in pixels.
    pub top: u32,
    /// Right padding in pixels.
    pub right: u32,
    /// Bottom padding in pixels.
    pub bottom: u32,
    /// Left padding in pixels.
    pub left: u32,
    /// Padding color.
    pub color: CanvasColor,
}

impl Padding {
    /// Create padding with per-side values (CSS order: top, right, bottom, left).
    pub const fn new(top: u32, right: u32, bottom: u32, left: u32, color: CanvasColor) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
            color,
        }
    }

    /// Create uniform padding on all sides.
    pub const fn uniform(amount: u32, color: CanvasColor) -> Self {
        Self::new(amount, amount, amount, amount, color)
    }
}

/// What the layout engine wants the decoder to do.
///
/// The decoder should apply the requested crop and orientation if possible,
/// then produce output at or near `target_size`. The resize engine handles
/// all quality-sensitive downscaling — decoders should decode at full
/// resolution (or the nearest size their format supports).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DecoderRequest {
    /// Crop region in pre-orientation source coordinates.
    pub crop: Option<Rect>,
    /// Target dimensions for the resize step.
    ///
    /// This is the size the layout engine will resize to. The decoder
    /// should produce output at or above this size; the resize engine
    /// handles the final downscale at high quality.
    pub target_size: Size,
    /// Orientation the engine would like the decoder to handle.
    pub orientation: Orientation,
}

impl DecoderRequest {
    /// Create a new decoder request.
    pub fn new(target_size: Size, orientation: Orientation) -> Self {
        Self {
            crop: None,
            target_size,
            orientation,
        }
    }

    /// Add a crop region.
    pub fn with_crop(mut self, crop: Rect) -> Self {
        self.crop = Some(crop);
        self
    }
}

/// What the decoder actually did.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DecoderOffer {
    /// Dimensions of the decoded output.
    pub dimensions: Size,
    /// Crop the decoder applied (in source coordinates).
    pub crop_applied: Option<Rect>,
    /// Orientation the decoder applied.
    pub orientation_applied: Orientation,
}

impl DecoderOffer {
    /// Default offer: decoder did nothing special, just decoded at full size.
    pub fn full_decode(w: u32, h: u32) -> Self {
        Self {
            dimensions: Size::new(w, h),
            crop_applied: None,
            orientation_applied: Orientation::Identity,
        }
    }

    /// Set the orientation the decoder applied.
    pub fn with_orientation_applied(mut self, orientation: Orientation) -> Self {
        self.orientation_applied = orientation;
        self
    }

    /// Set the crop the decoder applied.
    pub fn with_crop_applied(mut self, crop: Rect) -> Self {
        self.crop_applied = Some(crop);
        self
    }
}

/// Final layout plan after decoder negotiation.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LayoutPlan {
    /// What was requested of the decoder.
    pub decoder_request: DecoderRequest,
    /// Trim rect to apply to decoder output (for block-aligned overshoot).
    pub trim: Option<Rect>,
    /// Dimensions to resize to.
    pub resize_to: Size,
    /// Orientation remaining after decoder's contribution.
    pub remaining_orientation: Orientation,
    /// Final canvas dimensions (may be extended for alignment).
    pub canvas: Size,
    /// Placement offset on canvas (negative = content extends past top-left edge).
    pub placement: (i32, i32),
    /// Canvas background color.
    pub canvas_color: CanvasColor,
    /// True when no resize is needed (enables lossless path).
    pub resize_is_identity: bool,
    /// If [`Align::Extend`] was used, crop to these dimensions after encoding.
    /// Renderer should replicate edge pixels into the extension area.
    pub content_size: Option<Size>,
}

impl LayoutPlan {
    /// Create a no-op identity plan (no resize, no trim, no orientation).
    pub fn identity(size: Size) -> Self {
        Self {
            decoder_request: DecoderRequest {
                crop: None,
                target_size: size,
                orientation: Orientation::Identity,
            },
            trim: None,
            resize_to: size,
            remaining_orientation: Orientation::Identity,
            canvas: size,
            placement: (0, 0),
            canvas_color: CanvasColor::Transparent,
            resize_is_identity: true,
            content_size: None,
        }
    }

    /// Set the decoder request.
    pub fn with_decoder_request(mut self, request: DecoderRequest) -> Self {
        self.decoder_request = request;
        self
    }

    /// Set a trim rect to apply to decoder output.
    pub fn with_trim(mut self, trim: Rect) -> Self {
        self.trim = Some(trim);
        self
    }

    /// Set the resize target dimensions.
    pub fn with_resize_to(mut self, size: Size) -> Self {
        self.resize_to = size;
        self.resize_is_identity = false;
        self
    }

    /// Set the remaining orientation after decoder contribution.
    pub fn with_remaining_orientation(mut self, orientation: Orientation) -> Self {
        self.remaining_orientation = orientation;
        self
    }

    /// Set the canvas dimensions.
    pub fn with_canvas(mut self, size: Size) -> Self {
        self.canvas = size;
        self
    }

    /// Set the placement offset on canvas.
    pub fn with_placement(mut self, x: i32, y: i32) -> Self {
        self.placement = (x, y);
        self
    }

    /// Set the canvas background color.
    pub fn with_canvas_color(mut self, color: CanvasColor) -> Self {
        self.canvas_color = color;
        self
    }

    /// Set the content size (for extend alignment).
    pub fn with_content_size(mut self, size: Size) -> Self {
        self.content_size = Some(size);
        self
    }
}

/// How to align output dimensions to codec-required multiples.
///
/// All variants take `(x_align, y_align)` for per-axis alignment.
/// Use [`Subsampling::mcu_align()`] for JPEG MCU-aligned extend.
///
/// ```text
///     Source: 801x601, align to mod-16
///
///     Crop:     800x592  --  round down, lose edge pixels
///     Extend:   816x608  --  round up, replicate edges, content_size=(801,601)
///     Distort:  800x608  --  round to nearest, slight stretch
/// ```
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Align {
    /// Round canvas down to nearest multiple per axis. Loses up to `n-1`
    /// edge pixels per axis. Simple, lossy.
    Crop(u32, u32),
    /// Extend canvas up to nearest multiple per axis. **Resets placement
    /// to `(0, 0)`** — renderer replicates edge pixels into the extension
    /// area at the bottom-right. Original content dimensions stored in
    /// [`IdealLayout::content_size`] / [`LayoutPlan::content_size`].
    /// No content loss. This is how JPEG MCU padding works.
    Extend(u32, u32),
    /// Round `resize_to` to nearest multiple per axis, stretching content
    /// slightly to fit. Minimal distortion, no pixel loss, no padding.
    /// Canvas follows `resize_to` in non-pad modes; in pad modes the image
    /// is recentered within the existing canvas.
    Distort(u32, u32),
}

impl Align {
    /// Uniform crop alignment (same for both axes).
    pub const fn uniform_crop(n: u32) -> Self {
        Self::Crop(n, n)
    }

    /// Uniform extend alignment (same for both axes).
    pub const fn uniform_extend(n: u32) -> Self {
        Self::Extend(n, n)
    }

    /// Uniform distort alignment (same for both axes).
    pub const fn uniform_distort(n: u32) -> Self {
        Self::Distort(n, n)
    }
}

/// Chroma subsampling scheme.
///
/// Describes the relationship between luma and chroma plane dimensions.
/// Use [`Subsampling::mcu_align()`] to get the [`Align`] needed for JPEG encoding.
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Subsampling {
    /// 4:4:4 — no subsampling. Chroma same size as luma. MCU = 8×8.
    S444,
    /// 4:2:2 — chroma half width, full height. MCU = 16×8.
    S422,
    /// 4:2:0 — chroma half width and height. MCU = 16×16.
    S420,
}

impl Subsampling {
    /// Horizontal and vertical subsampling factors.
    ///
    /// Returns `(h, v)` where chroma dimensions = luma dimensions / factor.
    pub const fn factors(self) -> (u32, u32) {
        match self {
            Self::S444 => (1, 1),
            Self::S422 => (2, 1),
            Self::S420 => (2, 2),
        }
    }

    /// MCU dimensions in luma pixels for this subsampling scheme.
    pub const fn mcu_size(self) -> Size {
        let (h, v) = self.factors();
        Size::new(8 * h, 8 * v)
    }

    /// [`Align::Extend`] for JPEG MCU alignment with this subsampling.
    ///
    /// Use with [`OutputLimits::align`] to extend the canvas to
    /// MCU boundaries with edge replication.
    pub const fn mcu_align(self) -> Align {
        let mcu = self.mcu_size();
        Align::Extend(mcu.width, mcu.height)
    }
}

/// Geometry for a single image plane (luma or chroma).
///
/// All dimensions in pixels. Block size is always 8×8 (DCT block).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct PlaneLayout {
    /// Content dimensions in pixels.
    pub content: Size,
    /// Allocated/encoded dimensions (extended to block boundary).
    pub extended: Size,
    /// Number of 8×8 blocks per row.
    pub blocks_w: u32,
    /// Number of 8×8 blocks per column.
    pub blocks_h: u32,
}

/// Codec-ready geometry for a YCbCr image.
///
/// Computed from canvas dimensions + subsampling scheme. Provides everything
/// a JPEG or video encoder needs for direct streaming without buffering:
/// per-plane dimensions, block/MCU grid, and row group size.
///
/// # Example
///
/// ```
/// use zenlayout::{Pipeline, Subsampling, CodecLayout, OutputLimits, Size};
///
/// let (ideal, _) = Pipeline::new(4000, 3000)
///     .fit(800, 600)
///     .output_limits(OutputLimits::default().with_align(Subsampling::S420.mcu_align()))
///     .plan()
///     .unwrap();
///
/// let codec = CodecLayout::new(ideal.layout.canvas, Subsampling::S420);
/// assert_eq!(codec.mcu_size, Size::new(16, 16));
/// assert_eq!(codec.luma.extended, ideal.layout.canvas);
/// assert_eq!(codec.chroma.extended, Size::new(400, 304));
/// // Feed resize output in chunks of codec.luma_rows_per_mcu rows
/// assert_eq!(codec.luma_rows_per_mcu, 16);
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct CodecLayout {
    /// Luma (Y) plane layout.
    pub luma: PlaneLayout,
    /// Chroma (Cb, Cr) plane layout — both chroma planes share this geometry.
    pub chroma: PlaneLayout,
    /// Subsampling scheme.
    pub subsampling: Subsampling,
    /// MCU dimensions in luma pixels.
    pub mcu_size: Size,
    /// MCUs per row.
    pub mcu_cols: u32,
    /// MCU rows.
    pub mcu_rows: u32,
    /// Luma rows per MCU row — feed this many rows at a time from the
    /// resize engine to the encoder to avoid intermediate buffering.
    pub luma_rows_per_mcu: u32,
}

impl CodecLayout {
    /// Compute codec geometry from canvas dimensions and subsampling.
    ///
    /// Canvas should already be aligned (use [`Subsampling::mcu_align()`] with
    /// [`OutputLimits`]). If not aligned, dimensions are rounded up
    /// internally.
    pub fn new(canvas: Size, subsampling: Subsampling) -> Self {
        let (w, h) = (canvas.width, canvas.height);
        let (h_factor, v_factor) = subsampling.factors();
        let mcu = subsampling.mcu_size();

        // Extend to MCU boundary (should already be aligned if using mcu_align).
        let ext_w = w.div_ceil(mcu.width) * mcu.width;
        let ext_h = h.div_ceil(mcu.height) * mcu.height;

        let mcu_cols = ext_w / mcu.width;
        let mcu_rows = ext_h / mcu.height;

        let luma = PlaneLayout {
            content: Size::new(w, h),
            extended: Size::new(ext_w, ext_h),
            blocks_w: ext_w / 8,
            blocks_h: ext_h / 8,
        };

        let chroma_content_w = w.div_ceil(h_factor);
        let chroma_content_h = h.div_ceil(v_factor);
        let chroma_ext_w = ext_w / h_factor;
        let chroma_ext_h = ext_h / v_factor;

        let chroma = PlaneLayout {
            content: Size::new(chroma_content_w, chroma_content_h),
            extended: Size::new(chroma_ext_w, chroma_ext_h),
            blocks_w: chroma_ext_w / 8,
            blocks_h: chroma_ext_h / 8,
        };

        Self {
            luma,
            chroma,
            subsampling,
            mcu_size: mcu,
            mcu_cols,
            mcu_rows,
            luma_rows_per_mcu: mcu.height,
        }
    }
}

impl Default for CodecLayout {
    fn default() -> Self {
        Self::new(Size::default(), Subsampling::S444)
    }
}

/// Post-computation safety limits applied after all layout computation.
///
/// All limits target the **canvas** (the encoded output dimensions):
/// - `max`: prevents absurdly large outputs (security). Proportional downscale.
/// - `min`: prevents degenerate tiny outputs. Proportional upscale.
/// - `align`: snaps canvas to codec-required multiples.
///
/// If `max` and `min` conflict, `max` wins (security trumps aesthetics).
///
/// Applied to the [`Layout`] after constraint + padding computation, before
/// source crop is transformed back to source coordinates.
///
/// ```text
///     Layout from constraint
///           │
///           ▼
///     ┌─── max ───┐   Scale down proportionally if canvas > max
///     │            │
///     ▼            │
///     ┌─── min ───┐   Scale up proportionally if canvas < min
///     │            │   (re-applies max if min overshot -- max wins)
///     ▼            │
///     ┌── align ──┐   Snap to codec multiples (Crop/Extend/Distort)
///     │            │   NOTE: may slightly exceed max or drop below min
///     ▼
///     Final Layout
/// ```
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct OutputLimits {
    /// Maximum canvas dimensions. If exceeded, everything scales down proportionally.
    pub max: Option<Size>,
    /// Minimum canvas dimensions. If below, everything scales up proportionally.
    pub min: Option<Size>,
    /// Snap canvas to multiples. See [`Align`] for round-down vs extend modes.
    pub align: Option<Align>,
}

impl OutputLimits {
    /// Set maximum canvas dimensions.
    pub fn with_max(mut self, max: Size) -> Self {
        self.max = Some(max);
        self
    }

    /// Set minimum canvas dimensions.
    pub fn with_min(mut self, min: Size) -> Self {
        self.min = Some(min);
        self
    }

    /// Set alignment constraint.
    pub fn with_align(mut self, align: Align) -> Self {
        self.align = Some(align);
        self
    }

    /// Apply limits to a computed layout.
    ///
    /// Returns the modified layout and an optional content_size. If [`Align::Extend`]
    /// was used, `content_size` contains the original content dimensions — the
    /// renderer should replicate edge pixels into the extension area, and the
    /// encoder should record these as the real image dimensions.
    ///
    /// Order: max (cap canvas) → min (floor canvas) → align (snap canvas).
    /// Max wins if min conflicts.
    pub fn apply(&self, layout: Layout) -> (Layout, Option<Size>) {
        let mut layout = layout;

        // 1. Max: if canvas exceeds max, scale everything down proportionally.
        if let Some(max_sz) = self.max {
            let (max_w, max_h) = (max_sz.width, max_sz.height);
            if max_w > 0
                && max_h > 0
                && (layout.canvas.width > max_w || layout.canvas.height > max_h)
            {
                let scale = f64::min(
                    max_w as f64 / layout.canvas.width as f64,
                    max_h as f64 / layout.canvas.height as f64,
                );
                Self::scale_layout(&mut layout, scale);
            }
        }

        // 2. Min: if canvas is below min, scale everything up proportionally.
        if let Some(min_sz) = self.min {
            let (min_w, min_h) = (min_sz.width, min_sz.height);
            if min_w > 0
                && min_h > 0
                && (layout.canvas.width < min_w || layout.canvas.height < min_h)
            {
                let scale = f64::max(
                    min_w as f64 / layout.canvas.width as f64,
                    min_h as f64 / layout.canvas.height as f64,
                );
                Self::scale_layout(&mut layout, scale);

                // Re-apply max if min pushed us past it (max wins).
                if let Some(max_sz) = self.max {
                    let (max_w, max_h) = (max_sz.width, max_sz.height);
                    if max_w > 0
                        && max_h > 0
                        && (layout.canvas.width > max_w || layout.canvas.height > max_h)
                    {
                        let clamp = f64::min(
                            max_w as f64 / layout.canvas.width as f64,
                            max_h as f64 / layout.canvas.height as f64,
                        );
                        Self::scale_layout(&mut layout, clamp);
                    }
                }
            }
        }

        // 3. Align dimensions (per-axis).
        let content_size = match self.align {
            Some(Align::Crop(nx, ny)) if nx > 1 || ny > 1 => {
                let cw = if nx > 1 {
                    (layout.canvas.width / nx).max(1) * nx
                } else {
                    layout.canvas.width
                };
                let ch = if ny > 1 {
                    (layout.canvas.height / ny).max(1) * ny
                } else {
                    layout.canvas.height
                };
                layout.canvas = Size::new(cw, ch);

                // resize_to can't exceed canvas.
                layout.resize_to = Size::new(
                    layout.resize_to.width.min(cw),
                    layout.resize_to.height.min(ch),
                );

                // Clamp placement so image fits within canvas.
                layout.placement = (
                    layout
                        .placement
                        .0
                        .min((cw.saturating_sub(layout.resize_to.width)) as i32),
                    layout
                        .placement
                        .1
                        .min((ch.saturating_sub(layout.resize_to.height)) as i32),
                );
                None
            }
            Some(Align::Extend(nx, ny)) if nx > 1 || ny > 1 => {
                let (ow, oh) = (layout.canvas.width, layout.canvas.height);
                let cw = if nx > 1 { ow.div_ceil(nx) * nx } else { ow };
                let ch = if ny > 1 { oh.div_ceil(ny) * ny } else { oh };

                if cw != ow || ch != oh {
                    layout.placement = (0i32, 0i32);
                    layout.canvas = Size::new(cw, ch);
                    Some(Size::new(ow, oh))
                } else {
                    None // already aligned
                }
            }
            Some(Align::Distort(nx, ny)) if nx > 1 || ny > 1 => {
                let old_resize = layout.resize_to;
                let rw = if nx > 1 {
                    round_to_nearest(old_resize.width, nx)
                } else {
                    old_resize.width
                };
                let rh = if ny > 1 {
                    round_to_nearest(old_resize.height, ny)
                } else {
                    old_resize.height
                };
                layout.resize_to = Size::new(rw, rh);

                // Non-pad (canvas == old resize): canvas follows resize_to.
                // Pad (canvas > old resize): keep canvas, recenter image.
                if layout.canvas.width == old_resize.width {
                    layout.canvas.width = rw;
                    layout.placement.0 = 0;
                } else {
                    layout.placement.0 = (layout.canvas.width.saturating_sub(rw) / 2) as i32;
                }
                if layout.canvas.height == old_resize.height {
                    layout.canvas.height = rh;
                    layout.placement.1 = 0;
                } else {
                    layout.placement.1 = (layout.canvas.height.saturating_sub(rh) / 2) as i32;
                }
                None
            }
            _ => None,
        };

        (layout, content_size)
    }

    /// Scale all layout dimensions by a factor.
    fn scale_layout(layout: &mut Layout, scale: f64) {
        layout.resize_to = Size::new(
            (layout.resize_to.width as f64 * scale).round().max(1.0) as u32,
            (layout.resize_to.height as f64 * scale).round().max(1.0) as u32,
        );
        layout.canvas = Size::new(
            (layout.canvas.width as f64 * scale).round().max(1.0) as u32,
            (layout.canvas.height as f64 * scale).round().max(1.0) as u32,
        );
        layout.placement = (
            (layout.placement.0 as f64 * scale).round() as i32,
            (layout.placement.1 as f64 * scale).round() as i32,
        );
    }
}

/// Internal: unified source region (crop or viewport region share a slot).
#[derive(Clone, Debug)]
enum SourceRegion {
    Crop(SourceCrop),
    Region(Region),
}

/// Builder for image processing pipelines.
///
/// Provides a fluent API for specifying orientation, crop, constraint, and
/// padding operations. All operations are in post-orientation coordinates
/// (what the user sees after rotation).
///
/// **Last-setter-wins**: calling the same category of method twice replaces
/// the previous value (standard builder pattern). Orientation is the
/// exception — orientation commands always compose algebraically.
///
/// # Example
///
/// ```
/// use zenlayout::{Pipeline, DecoderOffer};
///
/// // EXIF-rotated JPEG, fit to 400×300
/// let (ideal, request) = Pipeline::new(4000, 3000)
///     .auto_orient(6)
///     .fit(400, 300)
///     .plan()
///     .unwrap();
///
/// // Decoder just decoded at full size
/// let plan = ideal.finalize(&request, &DecoderOffer::full_decode(4000, 3000));
/// assert!(!plan.resize_is_identity);
/// ```
#[derive(Clone, Debug)]
pub struct Pipeline {
    source_w: u32,
    source_h: u32,
    orientation: Orientation,
    source_region: Option<SourceRegion>,
    constraint: Option<Constraint>,
    padding: Option<Padding>,
    limits: Option<OutputLimits>,
}

impl Pipeline {
    /// Create a pipeline for a source image of the given dimensions.
    pub fn new(source_w: u32, source_h: u32) -> Self {
        Self {
            source_w,
            source_h,
            orientation: Orientation::Identity,
            source_region: None,
            constraint: None,
            padding: None,
            limits: None,
        }
    }

    /// Apply EXIF orientation correction (value 1-8). Invalid values are ignored.
    ///
    /// Orientation commands always compose algebraically into a single source
    /// transform, regardless of position in the pipeline. There is no
    /// "post-resize flip" — orientation is always applied to the source.
    pub fn auto_orient(mut self, exif: u8) -> Self {
        if let Some(o) = Orientation::from_exif(exif) {
            self.orientation = self.orientation.compose(o);
        }
        self
    }

    /// Rotate 90 degrees clockwise. Composes with all other orientation
    /// commands into a single source transform (see [`auto_orient`](Self::auto_orient)).
    pub fn rotate_90(mut self) -> Self {
        self.orientation = self.orientation.compose(Orientation::Rotate90);
        self
    }

    /// Rotate 180 degrees. Composes with all other orientation commands
    /// into a single source transform (see [`auto_orient`](Self::auto_orient)).
    pub fn rotate_180(mut self) -> Self {
        self.orientation = self.orientation.compose(Orientation::Rotate180);
        self
    }

    /// Rotate 270 degrees clockwise. Composes with all other orientation
    /// commands into a single source transform (see [`auto_orient`](Self::auto_orient)).
    pub fn rotate_270(mut self) -> Self {
        self.orientation = self.orientation.compose(Orientation::Rotate270);
        self
    }

    /// Flip horizontally. Composes with all other orientation commands
    /// into a single source transform (see [`auto_orient`](Self::auto_orient)).
    pub fn flip_h(mut self) -> Self {
        self.orientation = self.orientation.compose(Orientation::FlipH);
        self
    }

    /// Flip vertically. Composes with all other orientation commands
    /// into a single source transform (see [`auto_orient`](Self::auto_orient)).
    pub fn flip_v(mut self) -> Self {
        self.orientation = self.orientation.compose(Orientation::FlipV);
        self
    }

    /// Crop to pixel coordinates in post-orientation space.
    ///
    /// Uses origin + size: `(x, y)` is the top-left corner, `(width, height)`
    /// is the crop region size. Compare with [`region_viewport`](Self::region_viewport)
    /// which uses edge coordinates (left, top, right, bottom).
    ///
    /// Replaces any previous crop or region.
    pub fn crop_pixels(self, x: u32, y: u32, width: u32, height: u32) -> Self {
        self.crop(SourceCrop::pixels(x, y, width, height))
    }

    /// Crop using percentage coordinates (0.0–1.0) in post-orientation space.
    ///
    /// Replaces any previous crop or region.
    pub fn crop_percent(self, x: f32, y: f32, width: f32, height: f32) -> Self {
        self.crop(SourceCrop::percent(x, y, width, height))
    }

    /// Crop with a pre-built [`SourceCrop`].
    ///
    /// Replaces any previous crop or region.
    pub fn crop(mut self, source_crop: SourceCrop) -> Self {
        self.source_region = Some(SourceRegion::Crop(source_crop));
        self
    }

    /// Define a viewport region in source coordinates (post-orientation).
    ///
    /// Replaces any previous crop or region.
    pub fn region(mut self, region: Region) -> Self {
        self.source_region = Some(SourceRegion::Region(region));
        self
    }

    /// Define a viewport from pixel edge coordinates (left, top, right, bottom).
    ///
    /// Uses edge coordinates, not origin + size. A viewport from
    /// `(10, 10, 90, 90)` is 80x80 pixels. Compare with
    /// [`crop_pixels`](Self::crop_pixels) which uses origin + size:
    /// `(10, 10, 80, 80)` for the same region.
    pub fn region_viewport(
        self,
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        color: CanvasColor,
    ) -> Self {
        self.region(Region {
            left: RegionCoord::px(left),
            top: RegionCoord::px(top),
            right: RegionCoord::px(right),
            bottom: RegionCoord::px(bottom),
            color,
        })
    }

    /// Convenience: uniform padding via region.
    pub fn region_pad(self, amount: u32, color: CanvasColor) -> Self {
        self.region(Region::padded(amount, color))
    }

    /// Convenience: blank canvas (no source content).
    pub fn region_blank(self, width: u32, height: u32, color: CanvasColor) -> Self {
        self.region(Region::blank(width, height, color))
    }

    /// Fit within target dimensions, preserving aspect ratio.
    /// **Will upscale** small images to fill the target. Use
    /// [`within`](Self::within) to prevent upscaling.
    ///
    /// Replaces any previous constraint.
    pub fn fit(self, width: u32, height: u32) -> Self {
        self.constrain(Constraint::new(ConstraintMode::Fit, width, height))
    }

    /// Fit within target dimensions, never upscaling. Images smaller than
    /// the target stay at their original size.
    ///
    /// Replaces any previous constraint.
    pub fn within(self, width: u32, height: u32) -> Self {
        self.constrain(Constraint::new(ConstraintMode::Within, width, height))
    }

    /// Scale to fill target, cropping overflow. Preserves aspect ratio.
    /// Output is exactly `width × height`. May upscale.
    ///
    /// Replaces any previous constraint.
    pub fn fit_crop(self, width: u32, height: u32) -> Self {
        self.constrain(Constraint::new(ConstraintMode::FitCrop, width, height))
    }

    /// Like [`fit_crop`](Self::fit_crop), but never upscales.
    ///
    /// Replaces any previous constraint.
    pub fn within_crop(self, width: u32, height: u32) -> Self {
        self.constrain(Constraint::new(ConstraintMode::WithinCrop, width, height))
    }

    /// Fit within target, padding to exact target dimensions. May upscale.
    ///
    /// Replaces any previous constraint.
    pub fn fit_pad(self, width: u32, height: u32) -> Self {
        self.constrain(Constraint::new(ConstraintMode::FitPad, width, height))
    }

    /// Like [`fit_pad`](Self::fit_pad), but never upscales.
    ///
    /// Replaces any previous constraint.
    pub fn within_pad(self, width: u32, height: u32) -> Self {
        self.constrain(Constraint::new(ConstraintMode::WithinPad, width, height))
    }

    /// Scale to exact target dimensions, distorting aspect ratio.
    ///
    /// Replaces any previous constraint.
    pub fn distort(self, width: u32, height: u32) -> Self {
        self.constrain(Constraint::new(ConstraintMode::Distort, width, height))
    }

    /// Crop to target aspect ratio without scaling.
    ///
    /// Replaces any previous constraint.
    pub fn aspect_crop(self, width: u32, height: u32) -> Self {
        self.constrain(Constraint::new(ConstraintMode::AspectCrop, width, height))
    }

    /// Apply a pre-built [`Constraint`] for advanced cases (gravity, canvas color, single-axis).
    ///
    /// Replaces any previous constraint.
    ///
    /// If the [`Constraint`] has its own [`source_crop`](Constraint::source_crop),
    /// that crop composes with (nests inside) any pipeline-level crop or region.
    pub fn constrain(mut self, constraint: Constraint) -> Self {
        self.constraint = Some(constraint);
        self
    }

    /// Add padding with a pre-built [`Padding`].
    ///
    /// Replaces any previous padding.
    pub fn pad(mut self, padding: Padding) -> Self {
        self.padding = Some(padding);
        self
    }

    /// Add padding around the image. Values are absolute pixels; they
    /// never collapse or merge (unlike CSS margins).
    ///
    /// Replaces any previous padding.
    pub fn pad_sides(
        self,
        top: u32,
        right: u32,
        bottom: u32,
        left: u32,
        color: CanvasColor,
    ) -> Self {
        self.pad(Padding::new(top, right, bottom, left, color))
    }

    /// Add uniform padding on all sides.
    ///
    /// Replaces any previous padding.
    pub fn pad_uniform(self, amount: u32, color: CanvasColor) -> Self {
        self.pad(Padding::uniform(amount, color))
    }

    /// Apply safety limits after layout computation.
    ///
    /// See [`OutputLimits`] for details on max/min/align behavior.
    pub fn output_limits(mut self, limits: OutputLimits) -> Self {
        self.limits = Some(limits);
        self
    }

    /// Compute the ideal layout and decoder request.
    ///
    /// Processes the pipeline in fixed order: orient → crop/region → constrain → pad → limits.
    /// For sequential command evaluation, use [`compute_layout_sequential()`] directly.
    pub fn plan(self) -> Result<(IdealLayout, DecoderRequest), LayoutError> {
        let (crop, region) = match self.source_region {
            Some(SourceRegion::Crop(c)) => (Some(c), None),
            Some(SourceRegion::Region(r)) => (None, Some(r)),
            None => (None, None),
        };
        plan_from_parts(
            self.source_w,
            self.source_h,
            self.orientation,
            crop.as_ref(),
            region,
            self.constraint.as_ref(),
            self.padding,
            self.limits.as_ref(),
        )
    }
}

impl IdealLayout {
    /// Finalize layout after decoder reports what it actually did.
    ///
    /// Convenience method for the finalize step of two-phase layout.
    pub fn finalize(&self, request: &DecoderRequest, offer: &DecoderOffer) -> LayoutPlan {
        finalize(self, request, offer)
    }

    /// Derive an `(IdealLayout, DecoderRequest)` for a secondary plane that must
    /// stay spatially locked with the primary plane.
    ///
    /// Use this for gain maps, depth maps, alpha planes, or any auxiliary image
    /// that shares spatial extent with the primary image but lives at a different
    /// resolution and is decoded independently.
    ///
    /// The secondary plane goes through the same two-phase negotiation as the
    /// primary: send the `DecoderRequest` to the secondary decoder, get back a
    /// `DecoderOffer`, and call [`IdealLayout::finalize()`] to compute remaining work.
    /// Each decoder independently handles what it can; finalize compensates.
    ///
    /// # Arguments
    ///
    /// * `primary_source` — Source dimensions of the primary plane (before orientation).
    /// * `secondary_source` — Source dimensions of the secondary plane.
    /// * `secondary_target` — Desired output dimensions for the secondary plane.
    ///   Pass `None` to automatically maintain the source ratio
    ///   (e.g., if gain map is 1/4 of SDR source, output will be 1/4 of SDR output).
    ///
    /// # Coordinate scaling
    ///
    /// Source crop coordinates are scaled from primary to secondary space with
    /// **round-outward** logic: origin floors, extent ceils. This ensures the
    /// secondary plane always covers at least the full spatial extent of the
    /// primary crop. The renderer handles any sub-pixel offset.
    ///
    /// # Example
    ///
    /// ```
    /// use zenlayout::{Pipeline, DecoderOffer, Size};
    ///
    /// // SDR: 4000×3000, gain map: 1000×750 (1/4 scale)
    /// let (sdr_ideal, sdr_req) = Pipeline::new(4000, 3000)
    ///     .auto_orient(6)
    ///     .crop_pixels(100, 100, 2000, 2000)
    ///     .fit(800, 800)
    ///     .plan()
    ///     .unwrap();
    ///
    /// // Derive gain map plan from SDR plan
    /// let (gm_ideal, gm_req) = sdr_ideal.derive_secondary(
    ///     Size::new(4000, 3000),     // primary source
    ///     Size::new(1000, 750),      // gain map source
    ///     None,             // auto: 1/4 of SDR output
    /// );
    ///
    /// // Each decoder independently does its thing
    /// let sdr_plan = sdr_ideal.finalize(&sdr_req, &DecoderOffer::full_decode(4000, 3000));
    /// let gm_plan = gm_ideal.finalize(&gm_req, &DecoderOffer::full_decode(1000, 750));
    ///
    /// // Both plans produce spatially-locked results
    /// assert_eq!(sdr_plan.remaining_orientation, gm_plan.remaining_orientation);
    /// ```
    pub fn derive_secondary(
        &self,
        primary_source: Size,
        secondary_source: Size,
        secondary_target: Option<Size>,
    ) -> (IdealLayout, DecoderRequest) {
        let (ps_w, ps_h) = (primary_source.width, primary_source.height);
        let (ss_w, ss_h) = (secondary_source.width, secondary_source.height);

        // Scale ratios from primary source to secondary source.
        let scale_x = ss_w as f64 / ps_w as f64;
        let scale_y = ss_h as f64 / ps_h as f64;

        // Scale the source crop (in pre-orientation coords) with round-outward.
        let secondary_crop = self
            .source_crop
            .map(|crop| scale_rect_outward(crop, scale_x, scale_y, ss_w, ss_h));

        // Compute the oriented secondary source dimensions.
        let sec_o = self.orientation.transform_dimensions(ss_w, ss_h);
        let (sec_ow, sec_oh) = (sec_o.width, sec_o.height);

        // Scale the layout's source crop (in post-orientation coords) with round-outward.
        // Use the oriented scale factors.
        let orient_scale_x = sec_ow as f64 / self.layout.source.width as f64;
        let orient_scale_y = sec_oh as f64 / self.layout.source.height as f64;

        let secondary_layout_crop = self
            .layout
            .source_crop
            .map(|crop| scale_rect_outward(crop, orient_scale_x, orient_scale_y, sec_ow, sec_oh));

        // Effective source after crop in oriented space.
        let (eff_w, eff_h) = match &secondary_layout_crop {
            Some(r) => (r.width, r.height),
            None => (sec_ow, sec_oh),
        };

        // Compute target dimensions for the secondary plane.
        let (target_w, target_h) = match secondary_target {
            Some(t) => (t.width, t.height),
            None => {
                // Auto: maintain source ratio relative to primary output.
                let (pri_rw, pri_rh) = (self.layout.resize_to.width, self.layout.resize_to.height);
                let tw = (pri_rw as f64 * scale_x).round().max(1.0) as u32;
                let th = (pri_rh as f64 * scale_y).round().max(1.0) as u32;
                (tw, th)
            }
        };

        let sec_layout = Layout {
            source: Size::new(sec_ow, sec_oh),
            source_crop: secondary_layout_crop,
            resize_to: Size::new(target_w, target_h),
            canvas: Size::new(target_w, target_h),
            placement: (0, 0),
            canvas_color: CanvasColor::default(),
        };

        // Effective source is the crop region (or full secondary if no crop).
        // resize_is_identity will be computed by finalize().
        let _ = (eff_w, eff_h);

        let sec_ideal = IdealLayout {
            orientation: self.orientation,
            layout: sec_layout,
            source_crop: secondary_crop,
            padding: None, // secondary planes don't get padded
            content_size: None,
        };

        let sec_request = DecoderRequest {
            crop: secondary_crop,
            target_size: Size::new(target_w, target_h),
            orientation: self.orientation,
        };

        (sec_ideal, sec_request)
    }
}

/// Scale a rect from one coordinate space to another, rounding outward.
///
/// Round `v` to the nearest multiple of `n`. Ties round up.
fn round_to_nearest(v: u32, n: u32) -> u32 {
    ((v + n / 2) / n).max(1) * n
}

/// Origin (x, y) is floored, far edge (x+w, y+h) is ceiled, then clamped
/// to the target dimensions. This ensures the scaled rect always covers
/// at least the full spatial extent of the original.
fn scale_rect_outward(rect: Rect, scale_x: f64, scale_y: f64, max_w: u32, max_h: u32) -> Rect {
    let x0 = (rect.x as f64 * scale_x).floor() as u32;
    let y0 = (rect.y as f64 * scale_y).floor() as u32;
    let x1 = ((rect.x + rect.width) as f64 * scale_x)
        .ceil()
        .min(max_w as f64) as u32;
    let y1 = ((rect.y + rect.height) as f64 * scale_y)
        .ceil()
        .min(max_h as f64) as u32;
    Rect::new(x0, y0, (x1 - x0).max(1), (y1 - y0).max(1))
}

/// Compute ideal layout from commands and source image dimensions.
///
/// Orientation commands (AutoOrient, Rotate, Flip) are composed into a single
/// net orientation. Crop and Constrain operate in post-orientation coordinates
/// (what the user sees after rotation). The resulting source crop is transformed
/// back to pre-orientation source coordinates for the decoder.
///
/// First-wins: only the first `Crop`/`Region`, `Constrain`, and `Pad` are used;
/// later duplicates are ignored.
///
/// For a friendlier API, see [`Pipeline`].
pub fn compute_layout(
    commands: &[Command],
    source_w: u32,
    source_h: u32,
    limits: Option<&OutputLimits>,
) -> Result<(IdealLayout, DecoderRequest), LayoutError> {
    let mut orientation = Orientation::Identity;
    let mut crop: Option<&SourceCrop> = None;
    let mut region: Option<Region> = None;
    let mut constraint: Option<&Constraint> = None;
    let mut padding: Option<Padding> = None;

    for cmd in commands {
        match cmd {
            Command::AutoOrient(exif) => {
                if let Some(o) = Orientation::from_exif(*exif) {
                    orientation = orientation.compose(o);
                }
            }
            Command::Rotate(r) => {
                let o = match r {
                    Rotation::Rotate90 => Orientation::Rotate90,
                    Rotation::Rotate180 => Orientation::Rotate180,
                    Rotation::Rotate270 => Orientation::Rotate270,
                };
                orientation = orientation.compose(o);
            }
            Command::Flip(axis) => {
                let o = match axis {
                    FlipAxis::Horizontal => Orientation::FlipH,
                    FlipAxis::Vertical => Orientation::FlipV,
                };
                orientation = orientation.compose(o);
            }
            Command::Crop(c) => {
                if crop.is_none() && region.is_none() {
                    crop = Some(c);
                }
            }
            Command::Region(r) => {
                if crop.is_none() && region.is_none() {
                    region = Some(*r);
                }
            }
            Command::Constrain(c) => {
                if constraint.is_none() {
                    constraint = Some(c);
                }
            }
            Command::Pad(p) => {
                if padding.is_none() {
                    padding = Some(*p);
                }
            }
        }
    }

    plan_from_parts(
        source_w,
        source_h,
        orientation,
        crop,
        region,
        constraint,
        padding,
        limits,
    )
}

/// Compute layout from command sequence with sequential evaluation.
///
/// Unlike [`compute_layout()`] which uses fixed-pipeline semantics (first-wins),
/// this processes commands in order:
/// - Orientations always fuse (compose algebraically) regardless of position
/// - Crop/region commands before the first constraint compose sequentially
/// - Last constraint wins
/// - Post-constraint crop/region/pad adjusts the output canvas
/// - Limits are applied once at the end
///
/// For a friendlier builder API, see [`Pipeline`].
pub fn compute_layout_sequential(
    commands: &[Command],
    source_w: u32,
    source_h: u32,
    limits: Option<&OutputLimits>,
) -> Result<(IdealLayout, DecoderRequest), LayoutError> {
    // Phase 1: Partition commands into pre-constrain and post-constrain groups.
    let mut orientation = Orientation::Identity;
    let mut pre_regions: Vec<Region> = Vec::new();
    let mut constraint: Option<&Constraint> = None;
    let mut post_ops: Vec<&Command> = Vec::new();
    let mut saw_constrain = false;

    let mut post_orientation = Orientation::Identity;

    for cmd in commands {
        match cmd {
            Command::AutoOrient(exif) => {
                if let Some(o) = Orientation::from_exif(*exif) {
                    if saw_constrain {
                        post_orientation = post_orientation.compose(o);
                    } else {
                        orientation = orientation.compose(o);
                    }
                }
            }
            Command::Rotate(r) => {
                let o = match r {
                    Rotation::Rotate90 => Orientation::Rotate90,
                    Rotation::Rotate180 => Orientation::Rotate180,
                    Rotation::Rotate270 => Orientation::Rotate270,
                };
                if saw_constrain {
                    post_orientation = post_orientation.compose(o);
                } else {
                    orientation = orientation.compose(o);
                }
            }
            Command::Flip(axis) => {
                let o = match axis {
                    FlipAxis::Horizontal => Orientation::FlipH,
                    FlipAxis::Vertical => Orientation::FlipV,
                };
                if saw_constrain {
                    post_orientation = post_orientation.compose(o);
                } else {
                    orientation = orientation.compose(o);
                }
            }
            Command::Crop(sc) => {
                if saw_constrain {
                    post_ops.push(cmd);
                } else {
                    pre_regions.push(sc.to_region());
                }
            }
            Command::Region(r) => {
                if saw_constrain {
                    post_ops.push(cmd);
                } else {
                    pre_regions.push(*r);
                }
            }
            Command::Constrain(c) => {
                // Absorb any accumulated post_orientation into pre_orientation
                // since a new constrain resets the post-constrain context.
                orientation = orientation.compose(post_orientation);
                post_orientation = Orientation::Identity;
                constraint = Some(c); // last wins
                saw_constrain = true;
                post_ops.clear(); // reset post-ops on each new constrain
            }
            Command::Pad(p) => {
                if saw_constrain || !pre_regions.is_empty() {
                    post_ops.push(cmd);
                } else {
                    pre_regions.push(Region {
                        left: RegionCoord::px(-(p.left as i32)),
                        top: RegionCoord::px(-(p.top as i32)),
                        right: RegionCoord::pct_px(1.0, p.right as i32),
                        bottom: RegionCoord::pct_px(1.0, p.bottom as i32),
                        color: p.color,
                    });
                }
            }
        }
    }

    // If post_orientation swaps axes, swap the constraint's target dimensions
    // so the output matches: constrain→rotate90 = "resize then rotate."
    let swapped_constraint;
    if post_orientation.swaps_axes()
        && let Some(c) = constraint
    {
        swapped_constraint = Constraint {
            mode: c.mode,
            width: c.height,
            height: c.width,
            gravity: c.gravity,
            canvas_color: c.canvas_color,
            source_crop: c.source_crop,
        };
        constraint = Some(&swapped_constraint);
    }
    // Fuse post-orientation into pre-orientation (for source transform).
    orientation = orientation.compose(post_orientation);

    if source_w == 0 || source_h == 0 {
        return Err(LayoutError::ZeroSourceDimension);
    }

    // Phase 2: Compose pre-constrain regions into a single effective region.
    let oriented = orientation.transform_dimensions(source_w, source_h);
    let (ow, oh) = (oriented.width, oriented.height);

    let effective_region = if pre_regions.is_empty() {
        None
    } else {
        // Compose regions sequentially: each region operates on the effective
        // source from the previous step. The first region operates on the
        // full oriented source. Subsequent regions treat the previous region's
        // viewport as their source coordinate system.
        let mut composed = pre_regions[0];
        for next in &pre_regions[1..] {
            composed = compose_regions(composed, *next, ow, oh);
        }
        Some(composed)
    };

    // Phase 3: Compute layout from effective region + constraint.
    let layout = if let Some(reg) = effective_region {
        resolve_region(reg, ow, oh, constraint)?
    } else if let Some(c) = constraint {
        c.clone().compute(ow, oh)?
    } else {
        Layout {
            source: Size::new(ow, oh),
            source_crop: None,
            resize_to: Size::new(ow, oh),
            canvas: Size::new(ow, oh),
            placement: (0, 0),
            canvas_color: CanvasColor::default(),
        }
    };

    // Phase 4: Apply post-constrain ops to the canvas.
    let mut layout = layout;
    let mut pad_applied = false;
    for op in &post_ops {
        match op {
            Command::Crop(sc) => {
                // Post-constrain crop: trim the canvas.
                // Content shifts relative to new canvas origin.
                let canvas_crop = sc.resolve(layout.canvas.width, layout.canvas.height);
                layout.placement.0 -= canvas_crop.x as i32;
                layout.placement.1 -= canvas_crop.y as i32;
                layout.canvas = Size::new(canvas_crop.width, canvas_crop.height);
            }
            Command::Region(r) => {
                // Post-constrain region: redefine canvas viewport.
                let (left, top, right, bottom) =
                    r.resolve(layout.canvas.width, layout.canvas.height);
                let new_cw = right - left;
                let new_ch = bottom - top;
                if new_cw > 0 && new_ch > 0 {
                    layout.placement.0 -= left;
                    layout.placement.1 -= top;
                    layout.canvas = Size::new(new_cw as u32, new_ch as u32);
                    layout.canvas_color = r.color;
                }
            }
            Command::Pad(p) => {
                layout.canvas = Size::new(
                    layout.canvas.width + p.left + p.right,
                    layout.canvas.height + p.top + p.bottom,
                );
                layout.placement.0 += p.left as i32;
                layout.placement.1 += p.top as i32;
                layout.canvas_color = p.color;
                pad_applied = true;
            }
            _ => {} // orient commands already handled
        }
    }

    // Build padding record for IdealLayout
    let padding = if pad_applied {
        // Find the last pad command
        post_ops.iter().rev().find_map(|op| match op {
            Command::Pad(p) => Some(*p),
            _ => None,
        })
    } else {
        None
    };

    // Note: No separate post-orientation transform of the layout is needed.
    // The constraint target swap (for axis-swapping orientations) + fusion
    // into pre-orientation already produces the correct output geometry.
    // Non-axis-swapping orientations (flip, rot180) are handled entirely
    // by the pre-orientation source transform.

    // Phase 5: Apply limits.
    let (layout, content_size) = if let Some(mc) = limits {
        mc.apply(layout)
    } else {
        (layout, None)
    };

    // Phase 6: Transform source crop back to pre-orientation source coordinates.
    let source_crop_in_source = layout
        .source_crop
        .map(|r| orientation.transform_rect_to_source(r, source_w, source_h));

    let ideal = IdealLayout {
        orientation,
        layout: layout.clone(),
        source_crop: source_crop_in_source,
        padding,
        content_size,
    };

    let request = DecoderRequest {
        crop: source_crop_in_source,
        target_size: layout.resize_to,
        orientation,
    };

    Ok((ideal, request))
}

/// Compose two regions: `outer` defines a viewport, `inner` is relative to
/// that viewport's coordinate system. Result is in the original source coords.
fn compose_regions(outer: Region, inner: Region, source_w: u32, source_h: u32) -> Region {
    // Resolve outer to absolute coordinates
    let (ol, ot, or_, ob) = outer.resolve(source_w, source_h);
    let ow = (or_ - ol).max(1) as u32;
    let oh = (ob - ot).max(1) as u32;

    // Resolve inner relative to the outer viewport dimensions
    let (il, it, ir, ib) = inner.resolve(ow, oh);

    // Transform inner coords back to source space
    Region {
        left: RegionCoord::px(ol + il),
        top: RegionCoord::px(ot + it),
        right: RegionCoord::px(ol + ir),
        bottom: RegionCoord::px(ot + ib),
        color: inner.color,
    }
}

/// Core layout computation shared by [`compute_layout()`] and [`Pipeline::plan()`].
#[allow(clippy::too_many_arguments)]
fn plan_from_parts(
    source_w: u32,
    source_h: u32,
    orientation: Orientation,
    crop: Option<&SourceCrop>,
    region: Option<Region>,
    constraint: Option<&Constraint>,
    padding: Option<Padding>,
    limits: Option<&OutputLimits>,
) -> Result<(IdealLayout, DecoderRequest), LayoutError> {
    if source_w == 0 || source_h == 0 {
        return Err(LayoutError::ZeroSourceDimension);
    }

    // 1. Transform source dimensions to post-orientation space.
    let oriented = orientation.transform_dimensions(source_w, source_h);
    let (ow, oh) = (oriented.width, oriented.height);

    // Convert crop to region if present (region is the canonical form).
    let effective_region = if let Some(sc) = crop {
        Some(sc.to_region())
    } else {
        region
    };

    // 2. Compute layout in post-orientation space.
    let layout = if let Some(reg) = effective_region {
        resolve_region(reg, ow, oh, constraint)?
    } else if let Some(c) = constraint {
        c.clone().compute(ow, oh)?
    } else {
        Layout {
            source: Size::new(ow, oh),
            source_crop: None,
            resize_to: Size::new(ow, oh),
            canvas: Size::new(ow, oh),
            placement: (0, 0),
            canvas_color: CanvasColor::default(),
        }
    };

    // 3. Apply explicit padding if present (additive on existing canvas).
    let layout = if let Some(pad) = &padding {
        Layout {
            canvas: Size::new(
                layout.canvas.width + pad.left + pad.right,
                layout.canvas.height + pad.top + pad.bottom,
            ),
            placement: (
                layout.placement.0 + pad.left as i32,
                layout.placement.1 + pad.top as i32,
            ),
            canvas_color: pad.color,
            ..layout
        }
    } else {
        layout
    };

    // 4. Apply mandatory constraints (max/min/align).
    let (layout, content_size) = if let Some(mc) = limits {
        mc.apply(layout)
    } else {
        (layout, None)
    };

    // 5. Transform source crop back to pre-orientation source coordinates.
    let source_crop_in_source = layout
        .source_crop
        .map(|r| orientation.transform_rect_to_source(r, source_w, source_h));

    let ideal = IdealLayout {
        orientation,
        layout: layout.clone(),
        source_crop: source_crop_in_source,
        padding,
        content_size,
    };

    let request = DecoderRequest {
        crop: source_crop_in_source,
        target_size: layout.resize_to,
        orientation,
    };

    Ok((ideal, request))
}

/// Resolve a Region into a Layout.
///
/// The region defines a viewport in source coordinates. We compute:
/// 1. Viewport dimensions from resolved coordinates
/// 2. Overlap between viewport and source image
/// 3. Source crop (the overlap rect)
/// 4. Placement (where source content sits within the viewport)
/// 5. If a constraint is present, it operates on the overlap (effective source)
fn resolve_region(
    reg: Region,
    source_w: u32,
    source_h: u32,
    constraint: Option<&Constraint>,
) -> Result<Layout, LayoutError> {
    let (left, top, right, bottom) = reg.resolve(source_w, source_h);

    let vw = right - left;
    let vh = bottom - top;
    if vw <= 0 || vh <= 0 {
        return Err(LayoutError::ZeroRegionDimension);
    }
    let vw = vw as u32;
    let vh = vh as u32;

    // Compute overlap with source [0, source_w) × [0, source_h)
    let ol = left.max(0);
    let ot = top.max(0);
    let or_ = right.min(source_w as i32);
    let ob = bottom.min(source_h as i32);

    let has_overlap = ol < or_ && ot < ob;

    if !has_overlap {
        // Blank canvas — no source content visible
        let layout = Layout {
            source: Size::new(source_w, source_h),
            source_crop: None,
            resize_to: Size::new(vw, vh),
            canvas: Size::new(vw, vh),
            placement: (0, 0),
            canvas_color: reg.color,
        };
        return Ok(layout);
    }

    let overlap_x = ol as u32;
    let overlap_y = ot as u32;
    let overlap_w = (or_ - ol) as u32;
    let overlap_h = (ob - ot) as u32;

    // Where the overlap sits within the viewport
    let place_x = (ol - left) as u32;
    let place_y = (ot - top) as u32;

    // Is the overlap the full source? If so, no crop needed.
    let source_crop =
        if overlap_x == 0 && overlap_y == 0 && overlap_w == source_w && overlap_h == source_h {
            None
        } else {
            Some(Rect::new(overlap_x, overlap_y, overlap_w, overlap_h))
        };

    // Is the viewport exactly the overlap? (pure crop, no padding)
    let is_pure_crop = place_x == 0 && place_y == 0 && vw == overlap_w && vh == overlap_h;

    if let Some(c) = constraint {
        if is_pure_crop {
            // Pure crop: constraint operates on the overlap (cropped source).
            let mut builder = c.clone();
            if let Some(sc) = &source_crop {
                builder = builder.source_crop(SourceCrop::Pixels(*sc));
            }
            builder.compute(source_w, source_h)
        } else {
            // Viewport has padding: constraint targets the viewport dimensions
            // (the full padded area the user sees), then we back-derive content
            // dimensions from the scale factor. This matches immediate mode where
            // the user would resize the entire padded image.
            let viewport_layout = c.clone().compute(vw, vh)?;
            let scale_x = viewport_layout.resize_to.width as f64 / vw as f64;
            let scale_y = viewport_layout.resize_to.height as f64 / vh as f64;

            // Content dimensions scaled by the same factor as the viewport.
            let content_w = (overlap_w as f64 * scale_x).round().max(1.0) as u32;
            let content_h = (overlap_h as f64 * scale_y).round().max(1.0) as u32;
            let content_place_x = (place_x as f64 * scale_x).round() as i32;
            let content_place_y = (place_y as f64 * scale_y).round() as i32;

            // Use the constraint's canvas (which may include its own padding
            // for FitPad/WithinPad), adjusted for the viewport placement.
            let canvas = viewport_layout.canvas;
            let (vp_px, vp_py) = viewport_layout.placement;

            Ok(Layout {
                source: Size::new(source_w, source_h),
                source_crop,
                resize_to: Size::new(content_w, content_h),
                canvas,
                placement: (vp_px + content_place_x, vp_py + content_place_y),
                canvas_color: if reg.color != CanvasColor::Transparent {
                    reg.color
                } else {
                    viewport_layout.canvas_color
                },
            })
        }
    } else {
        Ok(Layout {
            source: Size::new(source_w, source_h),
            source_crop,
            resize_to: Size::new(overlap_w, overlap_h),
            canvas: Size::new(vw, vh),
            placement: (place_x as i32, place_y as i32),
            canvas_color: reg.color,
        })
    }
}

/// Finalize layout after decoder reports what it actually did.
///
/// Given the ideal layout from [`compute_layout()`] and the decoder's [`DecoderOffer`],
/// compute the remaining work: trim, resize, orientation, and canvas placement.
///
/// Prefer [`IdealLayout::finalize()`] which wraps this function.
pub(crate) fn finalize(
    ideal: &IdealLayout,
    request: &DecoderRequest,
    offer: &DecoderOffer,
) -> LayoutPlan {
    // 1. Remaining orientation = undo what decoder did, then apply full orientation.
    let remaining_orientation = offer
        .orientation_applied
        .inverse()
        .compose(ideal.orientation);

    // 2. Compute trim rect if decoder didn't crop exactly what we asked.
    let (decoder_w, decoder_h) = (offer.dimensions.width, offer.dimensions.height);
    let trim = compute_trim(&request.crop, &offer.crop_applied, decoder_w, decoder_h);

    // 3. Dimensions after trimming.
    let (after_trim_w, after_trim_h) = match &trim {
        Some(r) => (r.width, r.height),
        None => (decoder_w, decoder_h),
    };

    // 4. Dimensions after remaining orientation.
    let after_orient = remaining_orientation.transform_dimensions(after_trim_w, after_trim_h);
    let (after_orient_w, after_orient_h) = (after_orient.width, after_orient.height);

    // 5. Target resize dimensions from the ideal layout.
    let (target_w, target_h) = (ideal.layout.resize_to.width, ideal.layout.resize_to.height);

    // 6. Determine if resize is identity.
    let resize_is_identity = after_orient_w == target_w && after_orient_h == target_h;

    LayoutPlan {
        decoder_request: request.clone(),
        trim,
        resize_to: Size::new(target_w, target_h),
        remaining_orientation,
        canvas: ideal.layout.canvas,
        placement: ideal.layout.placement,
        canvas_color: ideal.layout.canvas_color,
        resize_is_identity,
        content_size: ideal.content_size,
    }
}

/// Compute trim rect when decoder crop doesn't exactly match request.
fn compute_trim(
    requested_crop: &Option<Rect>,
    applied_crop: &Option<Rect>,
    decoder_w: u32,
    decoder_h: u32,
) -> Option<Rect> {
    match (requested_crop, applied_crop) {
        // We asked for crop, decoder did nothing → trim the full decode to the requested region.
        (Some(req_crop), None) => Some(*req_crop),
        // We asked for crop, decoder cropped but not exactly → compute offset within decoder output.
        (Some(req_crop), Some(applied)) => {
            if req_crop == applied {
                // Exact match — no trim needed.
                None
            } else {
                // Decoder cropped a superset (e.g., block-aligned).
                // Trim within the decoder's output to get just the region we wanted.
                let dx = req_crop.x.saturating_sub(applied.x);
                let dy = req_crop.y.saturating_sub(applied.y);
                let tw = req_crop.width.min(decoder_w.saturating_sub(dx));
                let th = req_crop.height.min(decoder_h.saturating_sub(dy));
                if dx == 0 && dy == 0 && tw == decoder_w && th == decoder_h {
                    None
                } else {
                    Some(Rect::new(dx, dy, tw, th))
                }
            }
        }
        // No crop requested — no trim needed.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraint::Gravity;

    // ── No commands ──────────────────────────────────────────────────────

    #[test]
    fn empty_commands_passthrough() {
        let (ideal, req) = compute_layout(&[], 800, 600, None).unwrap();
        assert_eq!(ideal.orientation, Orientation::Identity);
        assert_eq!(ideal.layout.resize_to, Size::new(800, 600));
        assert_eq!(ideal.layout.canvas, Size::new(800, 600));
        assert!(ideal.source_crop.is_none());
        assert!(ideal.padding.is_none());
        assert!(req.crop.is_none());
        assert_eq!(req.target_size, Size::new(800, 600));
    }

    #[test]
    fn zero_source_rejected() {
        assert!(compute_layout(&[], 0, 600, None).is_err());
        assert!(compute_layout(&[], 800, 0, None).is_err());
    }

    // ── Orientation only ─────────────────────────────────────────────────

    #[test]
    fn auto_orient_90_swaps_dims() {
        let commands = [Command::AutoOrient(6)]; // EXIF 6 = Rotate90
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        assert_eq!(ideal.orientation, Orientation::Rotate90);
        // Post-orientation: 800×600 rotated 90° → 600×800
        assert_eq!(ideal.layout.resize_to, Size::new(600, 800));
        assert_eq!(ideal.layout.canvas, Size::new(600, 800));
        assert_eq!(req.orientation, Orientation::Rotate90);
    }

    #[test]
    fn auto_orient_180_preserves_dims() {
        let commands = [Command::AutoOrient(3)]; // EXIF 3 = Rotate180
        let (ideal, _) = compute_layout(&commands, 800, 600, None).unwrap();
        assert_eq!(ideal.orientation, Orientation::Rotate180);
        assert_eq!(ideal.layout.resize_to, Size::new(800, 600));
    }

    #[test]
    fn stacked_orientation() {
        // EXIF 6 (Rotate90) + manual Rotate90 = Rotate180
        let commands = [Command::AutoOrient(6), Command::Rotate(Rotation::Rotate90)];
        let (ideal, _) = compute_layout(&commands, 800, 600, None).unwrap();
        assert_eq!(ideal.orientation, Orientation::Rotate180);
        // 180° doesn't swap: still 800×600
        assert_eq!(ideal.layout.resize_to, Size::new(800, 600));
    }

    #[test]
    fn flip_horizontal() {
        let commands = [Command::Flip(FlipAxis::Horizontal)];
        let (ideal, _) = compute_layout(&commands, 800, 600, None).unwrap();
        assert_eq!(ideal.orientation, Orientation::FlipH);
        // FlipH doesn't change dimensions
        assert_eq!(ideal.layout.resize_to, Size::new(800, 600));
    }

    #[test]
    fn invalid_exif_ignored() {
        let commands = [Command::AutoOrient(0), Command::AutoOrient(9)];
        let (ideal, _) = compute_layout(&commands, 800, 600, None).unwrap();
        assert_eq!(ideal.orientation, Orientation::Identity);
    }

    // ── Crop in oriented space ───────────────────────────────────────────

    #[test]
    fn crop_in_oriented_space() {
        // Rotate 90°: 800×600 → oriented 600×800
        // Crop 100,100,400,600 in oriented space
        let commands = [
            Command::AutoOrient(6),
            Command::Crop(SourceCrop::pixels(100, 100, 400, 600)),
        ];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();

        // Layout crop is in oriented space
        let layout_crop = ideal.layout.source_crop.unwrap();
        assert_eq!(layout_crop, Rect::new(100, 100, 400, 600));

        // Source crop is transformed back to source coordinates
        let source_crop = ideal.source_crop.unwrap();
        assert_eq!(source_crop, req.crop.unwrap());
        // Verify dimensions make sense — rotated rect should have swapped w/h
        assert_eq!(source_crop.width, 600);
        assert_eq!(source_crop.height, 400);
    }

    #[test]
    fn crop_only_no_constraint() {
        let commands = [Command::Crop(SourceCrop::pixels(10, 20, 100, 200))];
        let (ideal, _) = compute_layout(&commands, 800, 600, None).unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(100, 200));
        assert_eq!(ideal.layout.canvas, Size::new(100, 200));
        let crop = ideal.source_crop.unwrap();
        assert_eq!(crop, Rect::new(10, 20, 100, 200));
    }

    // ── Constrain after orientation ──────────────────────────────────────

    #[test]
    fn constrain_after_rotate90() {
        // 800×600 rotated 90° → 600×800 oriented, then fit to 300×300
        let commands = [
            Command::AutoOrient(6),
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 300, 300)),
        ];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        // Fit 600×800 into 300×300 → 225×300
        assert_eq!(ideal.layout.resize_to, Size::new(225, 300));
        assert_eq!(req.target_size, Size::new(225, 300));
    }

    #[test]
    fn constrain_with_crop() {
        // Crop to 400×400, then fit to 200×200
        let commands = [
            Command::Crop(SourceCrop::pixels(100, 50, 400, 400)),
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 200, 200)),
        ];
        let (ideal, _) = compute_layout(&commands, 800, 600, None).unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(200, 200));
        // Source crop should be present (from the explicit crop)
        assert!(ideal.source_crop.is_some());
    }

    // ── Pad command ──────────────────────────────────────────────────────

    #[test]
    fn pad_expands_canvas() {
        let commands = [Command::Pad(Padding::new(
            10,
            20,
            10,
            20,
            CanvasColor::white(),
        ))];
        let (ideal, _) = compute_layout(&commands, 400, 300, None).unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(400, 300));
        assert_eq!(ideal.layout.canvas, Size::new(440, 320));
        assert_eq!(ideal.layout.placement, (20, 10));
        assert!(ideal.padding.is_some());
        let pad = ideal.padding.unwrap();
        assert_eq!(pad.top, 10);
        assert_eq!(pad.left, 20);
    }

    #[test]
    fn pad_after_constrain() {
        let commands = [
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 200, 200)),
            Command::Pad(Padding::uniform(5, CanvasColor::black())),
        ];
        let (ideal, _) = compute_layout(&commands, 800, 400, None).unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(200, 100));
        assert_eq!(ideal.layout.canvas, Size::new(210, 110));
        assert_eq!(ideal.layout.placement, (5, 5));
    }

    // ── finalize with full_decode ────────────────────────────────────────

    #[test]
    fn finalize_full_decode_no_orientation() {
        let commands = [Command::Constrain(Constraint::new(
            ConstraintMode::Fit,
            400,
            300,
        ))];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        let offer = DecoderOffer::full_decode(800, 600);
        let plan = finalize(&ideal, &req, &offer);

        assert!(plan.trim.is_none());
        assert_eq!(plan.resize_to, Size::new(400, 300));
        assert_eq!(plan.remaining_orientation, Orientation::Identity);
        assert_eq!(plan.canvas, Size::new(400, 300));
        assert!(!plan.resize_is_identity);
    }

    #[test]
    fn finalize_full_decode_with_orientation() {
        let commands = [
            Command::AutoOrient(6),
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 300, 300)),
        ];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        let offer = DecoderOffer::full_decode(800, 600);
        let plan = finalize(&ideal, &req, &offer);

        assert_eq!(plan.remaining_orientation, Orientation::Rotate90);
        assert!(plan.trim.is_none());
    }

    #[test]
    fn finalize_decoder_handles_orientation() {
        let commands = [
            Command::AutoOrient(6),
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 300, 300)),
        ];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        // Decoder applied the rotation itself
        let offer = DecoderOffer {
            dimensions: Size::new(600, 800),
            crop_applied: None,
            orientation_applied: Orientation::Rotate90,
        };
        let plan = finalize(&ideal, &req, &offer);

        assert_eq!(plan.remaining_orientation, Orientation::Identity);
    }

    #[test]
    fn finalize_decoder_partial_crop() {
        // Request crop of 100,100,200,200, decoder cropped wider (block-aligned)
        let commands = [Command::Crop(SourceCrop::pixels(100, 100, 200, 200))];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();

        assert_eq!(req.crop, Some(Rect::new(100, 100, 200, 200)));

        let offer = DecoderOffer {
            dimensions: Size::new(208, 208),
            crop_applied: Some(Rect::new(96, 96, 208, 208)),
            orientation_applied: Orientation::Identity,
        };
        let plan = finalize(&ideal, &req, &offer);

        // Should trim to get the exact region we wanted
        let trim = plan.trim.unwrap();
        assert_eq!(trim.x, 4); // 100 - 96
        assert_eq!(trim.y, 4); // 100 - 96
        assert_eq!(trim.width, 200);
        assert_eq!(trim.height, 200);
    }

    #[test]
    fn finalize_decoder_no_crop_when_requested() {
        // We asked for crop, decoder gave full image → trim = crop rect
        let commands = [Command::Crop(SourceCrop::pixels(100, 100, 200, 200))];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        let offer = DecoderOffer::full_decode(800, 600);
        let plan = finalize(&ideal, &req, &offer);

        let trim = plan.trim.unwrap();
        assert_eq!(trim, Rect::new(100, 100, 200, 200));
    }

    // ── resize_is_identity ───────────────────────────────────────────────

    #[test]
    fn resize_identity_crop_only() {
        let commands = [Command::Crop(SourceCrop::pixels(0, 0, 400, 300))];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        let offer = DecoderOffer {
            dimensions: Size::new(400, 300),
            crop_applied: Some(Rect::new(0, 0, 400, 300)),
            orientation_applied: Orientation::Identity,
        };
        let plan = finalize(&ideal, &req, &offer);
        assert!(plan.resize_is_identity);
    }

    #[test]
    fn resize_identity_rotate_only() {
        // Just rotate, no resize
        let commands = [Command::AutoOrient(6)];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        let offer = DecoderOffer::full_decode(800, 600);
        let plan = finalize(&ideal, &req, &offer);

        // After orientation (90°): 800×600 → 600×800
        // Target resize_to is (600, 800)
        // Decoder output is 800×600, remaining_orientation is Rotate90
        // After orient: 600×800 == target → identity
        assert!(plan.resize_is_identity);
    }

    #[test]
    fn resize_not_identity_when_scaling() {
        let commands = [Command::Constrain(Constraint::new(
            ConstraintMode::Fit,
            400,
            300,
        ))];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        let offer = DecoderOffer::full_decode(800, 600);
        let plan = finalize(&ideal, &req, &offer);
        assert!(!plan.resize_is_identity);
    }

    // ── Lossless scenario ────────────────────────────────────────────────

    #[test]
    fn lossless_rotate_and_crop() {
        // JPEG lossless scenario: rotate 90° + crop, decoder handles both
        let commands = [
            Command::AutoOrient(6),
            Command::Crop(SourceCrop::pixels(0, 0, 300, 400)),
        ];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        // oriented = 600×800, crop 0,0,300,400 in oriented space
        assert_eq!(ideal.layout.resize_to, Size::new(300, 400));

        // Decoder handles orientation and crop
        let offer = DecoderOffer {
            dimensions: Size::new(300, 400),
            crop_applied: req.crop,
            orientation_applied: Orientation::Rotate90,
        };
        let plan = finalize(&ideal, &req, &offer);
        assert!(plan.resize_is_identity);
        assert_eq!(plan.remaining_orientation, Orientation::Identity);
        assert!(plan.trim.is_none());
    }

    // ── Only first crop/constraint used ──────────────────────────────────

    #[test]
    fn duplicate_commands_use_first() {
        let commands = [
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 200, 200)),
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 100, 100)),
        ];
        let (ideal, _) = compute_layout(&commands, 800, 600, None).unwrap();
        // First constraint wins: Fit to 200×200
        assert_eq!(ideal.layout.resize_to, Size::new(200, 150));
    }

    // ════════════════════════════════════════════════════════════════════
    // Weird decoder behavior
    // ════════════════════════════════════════════════════════════════════

    /// Helper: plan + finalize in one step for concise tests.
    fn plan_finalize(
        commands: &[Command],
        source_w: u32,
        source_h: u32,
        offer: &DecoderOffer,
    ) -> (IdealLayout, LayoutPlan) {
        let (ideal, req) = compute_layout(commands, source_w, source_h, None).unwrap();
        let lp = finalize(&ideal, &req, offer);
        (ideal, lp)
    }

    // ── Decoder prescaling (JPEG 1/2, 1/4, 1/8) ─────────────────────

    #[test]
    fn decoder_prescale_half() {
        // Request: fit 4000×3000 to 500×500, decoder prescales to 2000×1500
        let commands = [Command::Constrain(Constraint::new(
            ConstraintMode::Fit,
            500,
            500,
        ))];
        let (ideal, req) = compute_layout(&commands, 4000, 3000, None).unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(500, 375));

        let offer = DecoderOffer {
            dimensions: Size::new(2000, 1500),
            crop_applied: None,
            orientation_applied: Orientation::Identity,
        };
        let lp = finalize(&ideal, &req, &offer);

        assert!(lp.trim.is_none());
        assert_eq!(lp.resize_to, Size::new(500, 375));
        // 2000×1500 → 500×375: still needs resize
        assert!(!lp.resize_is_identity);
    }

    #[test]
    fn decoder_prescale_to_exact_target() {
        // JPEG decoder prescales to exactly the target size — no resize needed
        let commands = [Command::Constrain(Constraint::new(
            ConstraintMode::Fit,
            500,
            375,
        ))];
        let (ideal, req) = compute_layout(&commands, 4000, 3000, None).unwrap();
        let offer = DecoderOffer {
            dimensions: Size::new(500, 375),
            crop_applied: None,
            orientation_applied: Orientation::Identity,
        };
        let lp = finalize(&ideal, &req, &offer);
        assert!(lp.resize_is_identity);
    }

    #[test]
    fn decoder_prescale_eighth() {
        // 1/8 prescale: 4000×3000 → 500×375, matches target exactly
        let commands = [Command::Constrain(Constraint::new(
            ConstraintMode::Fit,
            500,
            500,
        ))];
        let (_, req) = compute_layout(&commands, 4000, 3000, None).unwrap();
        // Decoder only managed 1/8 but dimensions don't match target
        let offer = DecoderOffer {
            dimensions: Size::new(500, 375),
            crop_applied: None,
            orientation_applied: Orientation::Identity,
        };
        let (_, lp) = plan_finalize(
            &[Command::Constrain(Constraint::new(
                ConstraintMode::Fit,
                500,
                500,
            ))],
            4000,
            3000,
            &offer,
        );
        // target is 500×375, decoder output is 500×375 → identity
        assert!(lp.resize_is_identity);
        assert_eq!(lp.resize_to, Size::new(500, 375));
        let _ = req; // used above
    }

    // ── Block-aligned crop overshoot ─────────────────────────────────

    #[test]
    fn decoder_crop_mcu_aligned_16x16() {
        // JPEG MCU is 16×16. Request crop at (103,47,200,200).
        // Decoder aligns to (96,32,224,224).
        let commands = [Command::Crop(SourceCrop::pixels(103, 47, 200, 200))];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        assert_eq!(req.crop.unwrap(), Rect::new(103, 47, 200, 200));

        let offer = DecoderOffer {
            dimensions: Size::new(224, 224),
            crop_applied: Some(Rect::new(96, 32, 224, 224)),
            orientation_applied: Orientation::Identity,
        };
        let lp = finalize(&ideal, &req, &offer);

        let trim = lp.trim.unwrap();
        assert_eq!(trim.x, 7); // 103 - 96
        assert_eq!(trim.y, 15); // 47 - 32
        assert_eq!(trim.width, 200);
        assert_eq!(trim.height, 200);
        assert!(lp.resize_is_identity); // crop-only = no resize
    }

    #[test]
    fn decoder_crop_mcu_aligned_8x8() {
        // 8×8 MCU alignment: request (50,50,100,100), decoder gives (48,48,104,104)
        let commands = [Command::Crop(SourceCrop::pixels(50, 50, 100, 100))];
        let (ideal, req) = compute_layout(&commands, 400, 300, None).unwrap();

        let offer = DecoderOffer {
            dimensions: Size::new(104, 104),
            crop_applied: Some(Rect::new(48, 48, 104, 104)),
            orientation_applied: Orientation::Identity,
        };
        let lp = finalize(&ideal, &req, &offer);

        let trim = lp.trim.unwrap();
        assert_eq!(trim, Rect::new(2, 2, 100, 100));
        assert!(lp.resize_is_identity);
    }

    #[test]
    fn decoder_crop_at_image_edge_truncated() {
        // Request crop near edge: (700,500,200,200) in 800×600.
        // Decoder crops (696,496,104,104) — truncated at image boundary.
        let commands = [Command::Crop(SourceCrop::pixels(700, 500, 100, 100))];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();

        let offer = DecoderOffer {
            dimensions: Size::new(104, 104),
            crop_applied: Some(Rect::new(696, 496, 104, 104)),
            orientation_applied: Orientation::Identity,
        };
        let lp = finalize(&ideal, &req, &offer);

        let trim = lp.trim.unwrap();
        assert_eq!(trim.x, 4); // 700 - 696
        assert_eq!(trim.y, 4); // 500 - 496
        assert_eq!(trim.width, 100);
        assert_eq!(trim.height, 100);
    }

    // ── Decoder applies wrong orientation ────────────────────────────

    #[test]
    fn decoder_applies_wrong_orientation() {
        // We want Rotate90 (EXIF 6), decoder applied Rotate180 instead
        let commands = [Command::AutoOrient(6)];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        assert_eq!(ideal.orientation, Orientation::Rotate90);

        let offer = DecoderOffer {
            dimensions: Size::new(800, 600), // 180° doesn't swap
            crop_applied: None,
            orientation_applied: Orientation::Rotate180,
        };
        let lp = finalize(&ideal, &req, &offer);

        // remaining = inverse(180°) ∘ 90° = 180° ∘ 90° = 270°
        assert_eq!(lp.remaining_orientation, Orientation::Rotate270);
        // After remaining 270° on 800×600 → 600×800
        // Target was 600×800 (from 90° of 800×600)
        assert_eq!(lp.resize_to, Size::new(600, 800));
        assert!(lp.resize_is_identity);
    }

    #[test]
    fn decoder_applies_flip_instead_of_rotate() {
        // We want Rotate90, decoder applied FlipH
        let commands = [Command::AutoOrient(6)];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();

        let offer = DecoderOffer {
            dimensions: Size::new(800, 600), // FlipH doesn't swap
            crop_applied: None,
            orientation_applied: Orientation::FlipH,
        };
        let lp = finalize(&ideal, &req, &offer);

        // remaining = inverse(FlipH) ∘ Rotate90 = FlipH ∘ Rotate90 = Transverse
        assert_eq!(lp.remaining_orientation, Orientation::Transverse);
        // Transpose swaps axes: 800×600 → 600×800 = target
        assert!(lp.resize_is_identity);
    }

    // ── Decoder applies partial orientation ──────────────────────────

    #[test]
    fn decoder_partial_orientation_flip_only() {
        // We want Transverse (EXIF 7 = rot270 + flip), decoder only flipped
        let commands = [Command::AutoOrient(7)];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        assert_eq!(ideal.orientation, Orientation::Transverse);

        let offer = DecoderOffer {
            dimensions: Size::new(800, 600),
            crop_applied: None,
            orientation_applied: Orientation::FlipH,
        };
        let lp = finalize(&ideal, &req, &offer);

        // remaining = inverse(FlipH) ∘ Transverse = FlipH ∘ Transverse
        let expected = Orientation::FlipH.compose(Orientation::Transverse);
        assert_eq!(lp.remaining_orientation, expected);
    }

    // ── Decoder crops AND orients simultaneously ─────────────────────

    #[test]
    fn decoder_crop_and_orient_simultaneously() {
        // Rotate90 + crop in oriented space → decoder handles both
        let commands = [
            Command::AutoOrient(6),
            Command::Crop(SourceCrop::pixels(50, 50, 200, 300)),
        ];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();

        // Decoder did everything: oriented + cropped
        let offer = DecoderOffer {
            dimensions: Size::new(200, 300),
            crop_applied: req.crop,
            orientation_applied: Orientation::Rotate90,
        };
        let lp = finalize(&ideal, &req, &offer);

        assert!(lp.trim.is_none());
        assert_eq!(lp.remaining_orientation, Orientation::Identity);
        assert!(lp.resize_is_identity);
        assert_eq!(lp.resize_to, Size::new(200, 300));
    }

    #[test]
    fn decoder_orients_but_not_crops() {
        // Rotate90 + crop. Decoder handles rotation but ignores crop.
        let commands = [
            Command::AutoOrient(6),
            Command::Crop(SourceCrop::pixels(50, 50, 200, 300)),
        ];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();

        // Decoder rotated (swapped dims) but didn't crop
        let offer = DecoderOffer {
            dimensions: Size::new(600, 800),
            crop_applied: None,
            orientation_applied: Orientation::Rotate90,
        };
        let lp = finalize(&ideal, &req, &offer);

        assert_eq!(lp.remaining_orientation, Orientation::Identity);
        // Should still have a trim for the requested crop (now in source coords)
        assert!(lp.trim.is_some());
        let trim = lp.trim.unwrap();
        let rc = req.crop.unwrap();
        assert_eq!((trim.width, trim.height), (rc.width, rc.height));
    }

    #[test]
    fn decoder_crops_but_not_orients() {
        // Rotate90 + crop. Decoder crops (in source coords) but doesn't rotate.
        let commands = [
            Command::AutoOrient(6),
            Command::Crop(SourceCrop::pixels(50, 50, 200, 300)),
        ];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        let source_crop = req.crop.unwrap();

        // Decoder cropped exactly in source coords but didn't orient
        let offer = DecoderOffer {
            dimensions: Size::new(source_crop.width, source_crop.height),
            crop_applied: Some(source_crop),
            orientation_applied: Orientation::Identity,
        };
        let lp = finalize(&ideal, &req, &offer);

        assert!(lp.trim.is_none()); // crop was exact
        assert_eq!(lp.remaining_orientation, Orientation::Rotate90);
        // After remaining 90° on cropped dims → should match target
        let after = lp
            .remaining_orientation
            .transform_dimensions(source_crop.width, source_crop.height);
        assert_eq!(after, lp.resize_to);
        assert!(lp.resize_is_identity);
    }

    // ── Decoder ignores everything ───────────────────────────────────

    #[test]
    fn decoder_ignores_everything_complex_pipeline() {
        // Full pipeline: EXIF 5 (Transpose) + crop + constrain + pad
        // Decoder does nothing.
        let commands = [
            Command::AutoOrient(5),
            Command::Crop(SourceCrop::pixels(10, 10, 200, 300)),
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 100, 100)),
            Command::Pad(Padding::uniform(5, CanvasColor::black())),
        ];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        let offer = DecoderOffer::full_decode(800, 600);
        let lp = finalize(&ideal, &req, &offer);

        // Full orientation remains
        assert_eq!(lp.remaining_orientation, Orientation::Transpose);
        // Decoder output is full 800×600, needs crop → trim present
        assert!(lp.trim.is_some());
        assert!(!lp.resize_is_identity);
        // Canvas includes padding
        assert!(lp.canvas.width > lp.resize_to.width);
        assert!(lp.canvas.height > lp.resize_to.height);
    }

    // ── All 8 EXIF orientations: decoder handles vs doesn't ──────────

    #[test]
    fn all_8_orientations_decoder_handles() {
        for exif in 1..=8u8 {
            let orientation = Orientation::from_exif(exif).unwrap();
            let commands = [
                Command::AutoOrient(exif),
                Command::Constrain(Constraint::new(ConstraintMode::Fit, 300, 300)),
            ];
            let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();

            // Decoder applied the orientation
            let dims = orientation.transform_dimensions(800, 600);
            let offer = DecoderOffer {
                dimensions: dims,
                crop_applied: None,
                orientation_applied: orientation,
            };
            let lp = finalize(&ideal, &req, &offer);

            assert_eq!(
                lp.remaining_orientation,
                Orientation::Identity,
                "EXIF {exif}: remaining should be identity when decoder handled it"
            );
            assert!(lp.trim.is_none());
        }
    }

    #[test]
    fn all_8_orientations_decoder_ignores() {
        for exif in 1..=8u8 {
            let orientation = Orientation::from_exif(exif).unwrap();
            let commands = [Command::AutoOrient(exif)];
            let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();

            // Decoder did nothing
            let offer = DecoderOffer::full_decode(800, 600);
            let lp = finalize(&ideal, &req, &offer);

            assert_eq!(
                lp.remaining_orientation, orientation,
                "EXIF {exif}: remaining should be the full orientation"
            );
            // For orient-only, after remaining orient the dims should match
            let after = lp.remaining_orientation.transform_dimensions(800, 600);
            assert_eq!(
                after, lp.resize_to,
                "EXIF {exif}: post-orient dims should match resize target"
            );
            assert!(
                lp.resize_is_identity,
                "EXIF {exif}: orient-only is identity"
            );
        }
    }

    // ── 1×1 pixel edge cases ─────────────────────────────────────────

    #[test]
    fn one_pixel_image_passthrough() {
        let (_, lp) = plan_finalize(&[], 1, 1, &DecoderOffer::full_decode(1, 1));
        assert!(lp.resize_is_identity);
        assert_eq!(lp.resize_to, Size::new(1, 1));
        assert_eq!(lp.canvas, Size::new(1, 1));
    }

    #[test]
    fn one_pixel_image_with_rotation() {
        let commands = [Command::AutoOrient(6)]; // Rotate90
        let (_, lp) = plan_finalize(&commands, 1, 1, &DecoderOffer::full_decode(1, 1));
        // 1×1 rotated is still 1×1
        assert!(lp.resize_is_identity);
        assert_eq!(lp.resize_to, Size::new(1, 1));
    }

    #[test]
    fn one_pixel_image_with_fit() {
        // Fit upscales: 1×1 → 100×100
        let commands = [Command::Constrain(Constraint::new(
            ConstraintMode::Fit,
            100,
            100,
        ))];
        let (_, lp) = plan_finalize(&commands, 1, 1, &DecoderOffer::full_decode(1, 1));
        assert_eq!(lp.resize_to, Size::new(100, 100));
        assert!(!lp.resize_is_identity);
    }

    #[test]
    fn one_pixel_image_with_within() {
        // Within never upscales: 1×1 stays 1×1
        let commands = [Command::Constrain(Constraint::new(
            ConstraintMode::Within,
            100,
            100,
        ))];
        let (_, lp) = plan_finalize(&commands, 1, 1, &DecoderOffer::full_decode(1, 1));
        assert_eq!(lp.resize_to, Size::new(1, 1));
        assert!(lp.resize_is_identity);
    }

    // ── Decoder prescales with orientation ────────────────────────────

    #[test]
    fn decoder_prescale_with_orientation_handled() {
        // 4000×3000, EXIF 6 (Rotate90), fit to 500×500
        // Decoder prescales 1/4 AND handles rotation → delivers 750×1000
        let commands = [
            Command::AutoOrient(6),
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 500, 500)),
        ];
        let (ideal, req) = compute_layout(&commands, 4000, 3000, None).unwrap();
        // Oriented: 3000×4000, fit to 500×500 → 375×500
        assert_eq!(ideal.layout.resize_to, Size::new(375, 500));

        let offer = DecoderOffer {
            dimensions: Size::new(750, 1000), // 1/4 prescale + rotation
            crop_applied: None,
            orientation_applied: Orientation::Rotate90,
        };
        let lp = finalize(&ideal, &req, &offer);

        assert_eq!(lp.remaining_orientation, Orientation::Identity);
        assert_eq!(lp.resize_to, Size::new(375, 500));
        assert!(!lp.resize_is_identity); // 750×1000 → 375×500
    }

    #[test]
    fn decoder_prescale_without_orientation() {
        // 4000×3000, EXIF 6 (Rotate90), fit to 500×500
        // Decoder prescales 1/4 but doesn't rotate → delivers 1000×750
        let commands = [
            Command::AutoOrient(6),
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 500, 500)),
        ];
        let (ideal, req) = compute_layout(&commands, 4000, 3000, None).unwrap();

        let offer = DecoderOffer {
            dimensions: Size::new(1000, 750), // 1/4 prescale, no rotation
            crop_applied: None,
            orientation_applied: Orientation::Identity,
        };
        let lp = finalize(&ideal, &req, &offer);

        assert_eq!(lp.remaining_orientation, Orientation::Rotate90);
        // After 90° on 1000×750 → 750×1000
        // Target is 375×500 → not identity
        assert!(!lp.resize_is_identity);
    }

    // ── Decoder crop + prescale combo ────────────────────────────────

    #[test]
    fn decoder_crop_then_prescale() {
        // Request crop 200×200, decoder crops to 208×208 (MCU) then prescales 1/2 → 104×104
        let commands = [
            Command::Crop(SourceCrop::pixels(100, 100, 200, 200)),
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 100, 100)),
        ];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(100, 100));

        let offer = DecoderOffer {
            dimensions: Size::new(104, 104), // MCU-aligned crop, then 1/2 prescale
            crop_applied: Some(Rect::new(96, 96, 208, 208)),
            orientation_applied: Orientation::Identity,
        };
        let lp = finalize(&ideal, &req, &offer);

        // Trim needed: within the 104×104 output, offset (4/2, 4/2) for 100×100?
        // Actually, the trim is computed from requested vs applied crop in source coords,
        // not accounting for prescale. The trim rect is in decoder-output coords.
        let trim = lp.trim.unwrap();
        assert_eq!(trim.x, 4); // 100 - 96 in source coords
        assert_eq!(trim.y, 4);
        // Width/height capped at decoder_w - dx
        assert_eq!(trim.width, 100); // min(200, 104-4) = 100
        assert_eq!(trim.height, 100);
    }

    // ── Canvas / placement preserved through finalize ────────────────

    #[test]
    fn finalize_preserves_canvas_from_fit_pad() {
        let commands = [Command::Constrain(
            Constraint::new(ConstraintMode::FitPad, 400, 400).canvas_color(CanvasColor::white()),
        )];
        let (ideal, req) = compute_layout(&commands, 1000, 500, None).unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(400, 400));
        assert_eq!(ideal.layout.resize_to, Size::new(400, 200));
        assert_eq!(ideal.layout.placement, (0, 100));

        let offer = DecoderOffer::full_decode(1000, 500);
        let lp = finalize(&ideal, &req, &offer);

        assert_eq!(lp.canvas, Size::new(400, 400));
        assert_eq!(lp.placement, (0, 100));
        assert_eq!(lp.canvas_color, CanvasColor::white());
    }

    #[test]
    fn finalize_preserves_canvas_from_fit_crop() {
        let commands = [Command::Constrain(Constraint::new(
            ConstraintMode::FitCrop,
            400,
            400,
        ))];
        let (ideal, req) = compute_layout(&commands, 1000, 500, None).unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(400, 400));
        assert_eq!(ideal.layout.resize_to, Size::new(400, 400));

        let offer = DecoderOffer::full_decode(1000, 500);
        let lp = finalize(&ideal, &req, &offer);

        assert_eq!(lp.canvas, Size::new(400, 400));
        assert_eq!(lp.resize_to, Size::new(400, 400));
    }

    // ── Decoder applies unrequested crop ─────────────────────────────

    #[test]
    fn decoder_crops_unrequested() {
        // No crop in commands, but decoder crops anyway (weird but possible)
        let commands = [Command::Constrain(Constraint::new(
            ConstraintMode::Fit,
            400,
            300,
        ))];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        assert!(req.crop.is_none());

        // Decoder randomly crops to 700×500
        let offer = DecoderOffer {
            dimensions: Size::new(700, 500),
            crop_applied: Some(Rect::new(50, 50, 700, 500)),
            orientation_applied: Orientation::Identity,
        };
        let lp = finalize(&ideal, &req, &offer);

        // No trim (we didn't request a crop, so no trim logic fires)
        assert!(lp.trim.is_none());
        // Resize target is still what the layout computed
        assert_eq!(lp.resize_to, Size::new(400, 300));
        // But resize_is_identity will be false (700×500 ≠ 400×300)
        assert!(!lp.resize_is_identity);
    }

    // ── Orientation composition edge cases with finalize ─────────────

    #[test]
    fn decoder_applies_inverse_of_requested() {
        // We want Rotate90, decoder applies Rotate270 (the inverse)
        let commands = [Command::AutoOrient(6)]; // Rotate90
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();

        let offer = DecoderOffer {
            dimensions: Size::new(600, 800), // 270° swaps
            crop_applied: None,
            orientation_applied: Orientation::Rotate270,
        };
        let lp = finalize(&ideal, &req, &offer);

        // remaining = inverse(270°) ∘ 90° = 90° ∘ 90° = 180°
        assert_eq!(lp.remaining_orientation, Orientation::Rotate180);
        // After 180° on 600×800 → 600×800 = target
        assert!(lp.resize_is_identity);
    }

    #[test]
    fn decoder_double_applies_orientation() {
        // We want Rotate90, decoder applies Rotate90 twice (=180°)
        // This is a weird edge case: decoder composed with itself
        let commands = [Command::AutoOrient(6)]; // Rotate90
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();

        let offer = DecoderOffer {
            dimensions: Size::new(800, 600), // 180° doesn't swap
            crop_applied: None,
            orientation_applied: Orientation::Rotate180,
        };
        let lp = finalize(&ideal, &req, &offer);

        // remaining = inverse(180°) ∘ 90° = 180° ∘ 90° = 270°
        assert_eq!(lp.remaining_orientation, Orientation::Rotate270);
        // 270° on 800×600 → 600×800 = target
        assert!(lp.resize_is_identity);
    }

    // ── Asymmetric images with orientation ────────────────────────────

    #[test]
    fn tall_image_rotate90_decoder_handles() {
        // 100×1000 (very tall), rotate 90° → 1000×100, fit to 500×500
        let commands = [
            Command::AutoOrient(6),
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 500, 500)),
        ];
        let (ideal, req) = compute_layout(&commands, 100, 1000, None).unwrap();
        // oriented: 1000×100, fit to 500×500 → 500×50
        assert_eq!(ideal.layout.resize_to, Size::new(500, 50));

        let offer = DecoderOffer {
            dimensions: Size::new(1000, 100),
            crop_applied: None,
            orientation_applied: Orientation::Rotate90,
        };
        let lp = finalize(&ideal, &req, &offer);

        assert_eq!(lp.remaining_orientation, Orientation::Identity);
        assert!(!lp.resize_is_identity);
        assert_eq!(lp.resize_to, Size::new(500, 50));
    }

    #[test]
    fn square_image_all_orientations_are_identity() {
        // Square image: all orientations produce same dimensions
        for exif in 1..=8u8 {
            let commands = [Command::AutoOrient(exif)];
            let (_, lp) = plan_finalize(&commands, 500, 500, &DecoderOffer::full_decode(500, 500));
            assert_eq!(lp.resize_to, Size::new(500, 500), "EXIF {exif}");
            assert!(lp.resize_is_identity, "EXIF {exif}");
        }
    }

    // ── Crop + constraint + orient + decoder partial ─────────────────

    #[test]
    fn full_pipeline_decoder_handles_only_orient() {
        // EXIF 8 (Rotate270) + crop + fit
        // 800×600 → oriented 600×800 → crop(50,50,400,600) → fit(200,200) → 150×200
        let commands = [
            Command::AutoOrient(8),
            Command::Crop(SourceCrop::pixels(50, 50, 400, 600)),
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 200, 200)),
        ];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();

        // Decoder handles rotation but not crop
        let offer = DecoderOffer {
            dimensions: Size::new(600, 800), // 270° swaps
            crop_applied: None,
            orientation_applied: Orientation::Rotate270,
        };
        let lp = finalize(&ideal, &req, &offer);

        assert_eq!(lp.remaining_orientation, Orientation::Identity);
        // Crop was in source coords; decoder didn't crop → trim = source crop
        assert!(lp.trim.is_some());
        assert!(!lp.resize_is_identity);
    }

    #[test]
    fn full_pipeline_decoder_handles_nothing() {
        // Same pipeline, decoder does absolutely nothing
        let commands = [
            Command::AutoOrient(8),
            Command::Crop(SourceCrop::pixels(50, 50, 400, 600)),
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 200, 200)),
        ];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        let offer = DecoderOffer::full_decode(800, 600);
        let lp = finalize(&ideal, &req, &offer);

        assert_eq!(lp.remaining_orientation, Orientation::Rotate270);
        assert!(lp.trim.is_some()); // crop not handled
        assert!(!lp.resize_is_identity);
    }

    #[test]
    fn full_pipeline_decoder_handles_everything() {
        // Decoder handles orient + crop + prescale to exact target
        let commands = [
            Command::AutoOrient(8),
            Command::Crop(SourceCrop::pixels(50, 50, 400, 600)),
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 200, 200)),
        ];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();
        let target = ideal.layout.resize_to;

        let offer = DecoderOffer {
            dimensions: target,
            crop_applied: req.crop,
            orientation_applied: Orientation::Rotate270,
        };
        let lp = finalize(&ideal, &req, &offer);

        assert_eq!(lp.remaining_orientation, Orientation::Identity);
        assert!(lp.trim.is_none());
        assert!(lp.resize_is_identity);
    }

    // ── Narrow / extreme aspect ratios ───────────────────────────────

    #[test]
    fn extreme_aspect_ratio_1x10000() {
        let commands = [Command::Constrain(Constraint::new(
            ConstraintMode::Fit,
            100,
            100,
        ))];
        let (ideal, req) = compute_layout(&commands, 1, 10000, None).unwrap();
        // Fit 1×10000 into 100×100 → 1×100
        assert_eq!(ideal.layout.resize_to, Size::new(1, 100));

        let offer = DecoderOffer::full_decode(1, 10000);
        let lp = finalize(&ideal, &req, &offer);
        assert!(!lp.resize_is_identity);
        assert_eq!(lp.resize_to, Size::new(1, 100));
    }

    #[test]
    fn extreme_aspect_ratio_10000x1() {
        let commands = [Command::Constrain(Constraint::new(
            ConstraintMode::Fit,
            100,
            100,
        ))];
        let (ideal, _) = compute_layout(&commands, 10000, 1, None).unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(100, 1));
    }

    // ── Exact match decoder behavior ─────────────────────────────────

    #[test]
    fn decoder_exact_crop_no_trim() {
        let commands = [Command::Crop(SourceCrop::pixels(100, 100, 200, 200))];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();

        let offer = DecoderOffer {
            dimensions: Size::new(200, 200),
            crop_applied: Some(Rect::new(100, 100, 200, 200)),
            orientation_applied: Orientation::Identity,
        };
        let lp = finalize(&ideal, &req, &offer);

        assert!(lp.trim.is_none());
        assert!(lp.resize_is_identity);
    }

    // ── Flips are self-inverse ───────────────────────────────────────

    #[test]
    fn decoder_applies_same_flip_twice_is_identity() {
        // User wants FlipH, decoder also applies FlipH → remaining = identity
        let commands = [Command::Flip(FlipAxis::Horizontal)];
        let (ideal, req) = compute_layout(&commands, 800, 600, None).unwrap();

        let offer = DecoderOffer {
            dimensions: Size::new(800, 600),
            crop_applied: None,
            orientation_applied: Orientation::FlipH,
        };
        let lp = finalize(&ideal, &req, &offer);
        assert_eq!(lp.remaining_orientation, Orientation::Identity);
    }

    // ── FitPad with decoder prescale ─────────────────────────────────

    #[test]
    fn fit_pad_with_prescaled_decoder() {
        let commands = [Command::Constrain(
            Constraint::new(ConstraintMode::FitPad, 400, 400).canvas_color(CanvasColor::white()),
        )];
        let (ideal, req) = compute_layout(&commands, 4000, 2000, None).unwrap();
        // Fit 4000×2000 into 400×400 → 400×200, canvas 400×400, placement (0,100)
        assert_eq!(ideal.layout.resize_to, Size::new(400, 200));
        assert_eq!(ideal.layout.canvas, Size::new(400, 400));
        assert_eq!(ideal.layout.placement, (0, 100));

        // Decoder prescales to 1000×500
        let offer = DecoderOffer {
            dimensions: Size::new(1000, 500),
            crop_applied: None,
            orientation_applied: Orientation::Identity,
        };
        let lp = finalize(&ideal, &req, &offer);

        assert_eq!(lp.resize_to, Size::new(400, 200));
        assert_eq!(lp.canvas, Size::new(400, 400));
        assert_eq!(lp.placement, (0, 100));
        assert!(!lp.resize_is_identity);
    }

    // ════════════════════════════════════════════════════════════════════
    // Pipeline builder API
    // ════════════════════════════════════════════════════════════════════

    #[test]
    fn pipeline_basic_fit() {
        let (ideal, _) = Pipeline::new(800, 600).fit(400, 300).plan().unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(400, 300));
    }

    #[test]
    fn pipeline_within() {
        let (ideal, _) = Pipeline::new(200, 100).within(400, 300).plan().unwrap();
        // Source smaller than target → no upscale
        assert_eq!(ideal.layout.resize_to, Size::new(200, 100));
    }

    #[test]
    fn pipeline_orient_then_fit() {
        let (ideal, _) = Pipeline::new(800, 600)
            .auto_orient(6) // Rotate90
            .fit(300, 300)
            .plan()
            .unwrap();
        // 800×600 → oriented 600×800 → fit 300×300 → 225×300
        assert_eq!(ideal.layout.resize_to, Size::new(225, 300));
    }

    #[test]
    fn pipeline_matches_command_api() {
        // Same operation via both APIs should produce identical results
        let commands = [
            Command::AutoOrient(6),
            Command::Crop(SourceCrop::pixels(50, 50, 400, 600)),
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 200, 200)),
        ];
        let (ideal_cmd, req_cmd) = compute_layout(&commands, 800, 600, None).unwrap();

        let (ideal_pipe, req_pipe) = Pipeline::new(800, 600)
            .auto_orient(6)
            .crop_pixels(50, 50, 400, 600)
            .fit(200, 200)
            .plan()
            .unwrap();

        assert_eq!(ideal_cmd.orientation, ideal_pipe.orientation);
        assert_eq!(ideal_cmd.layout, ideal_pipe.layout);
        assert_eq!(ideal_cmd.source_crop, ideal_pipe.source_crop);
        assert_eq!(req_cmd, req_pipe);
    }

    #[test]
    fn pipeline_stacked_rotations() {
        let (ideal, _) = Pipeline::new(800, 600)
            .auto_orient(6) // Rotate90
            .rotate_90() // +90 = 180 total
            .plan()
            .unwrap();
        assert_eq!(ideal.orientation, Orientation::Rotate180);
        assert_eq!(ideal.layout.resize_to, Size::new(800, 600));
    }

    #[test]
    fn pipeline_flip_h_and_v() {
        let (ideal, _) = Pipeline::new(800, 600).flip_h().flip_v().plan().unwrap();
        // FlipH then FlipV = Rotate180
        assert_eq!(ideal.orientation, Orientation::Rotate180);
    }

    #[test]
    fn pipeline_crop_percent() {
        let (ideal, _) = Pipeline::new(1000, 1000)
            .crop_percent(0.1, 0.1, 0.8, 0.8)
            .plan()
            .unwrap();
        let crop = ideal.layout.source_crop.unwrap();
        assert_eq!(crop, Rect::new(100, 100, 800, 800));
    }

    #[test]
    fn pipeline_fit_crop() {
        let (ideal, _) = Pipeline::new(1000, 500).fit_crop(400, 400).plan().unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(400, 400));
        assert!(ideal.layout.source_crop.is_some());
    }

    #[test]
    fn pipeline_fit_pad() {
        let (ideal, _) = Pipeline::new(1000, 500).fit_pad(400, 400).plan().unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(400, 200));
        assert_eq!(ideal.layout.canvas, Size::new(400, 400));
    }

    #[test]
    fn pipeline_distort() {
        let (ideal, _) = Pipeline::new(800, 600).distort(100, 100).plan().unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(100, 100));
    }

    #[test]
    fn pipeline_aspect_crop() {
        let (ideal, _) = Pipeline::new(1000, 500)
            .aspect_crop(400, 400)
            .plan()
            .unwrap();
        // Crop to 1:1 aspect, no scaling
        let crop = ideal.layout.source_crop.unwrap();
        assert_eq!(crop.width, crop.height);
        assert_eq!(ideal.layout.resize_to, Size::new(crop.width, crop.height));
    }

    #[test]
    fn pipeline_pad_uniform() {
        let (ideal, _) = Pipeline::new(400, 300)
            .pad_uniform(10, CanvasColor::white())
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(400, 300));
        assert_eq!(ideal.layout.canvas, Size::new(420, 320));
    }

    #[test]
    fn pipeline_pad_asymmetric() {
        let (ideal, _) = Pipeline::new(400, 300)
            .pad_sides(5, 10, 15, 20, CanvasColor::black())
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(430, 320)); // 400+10+20, 300+5+15
        assert_eq!(ideal.layout.placement, (20, 5));
    }

    #[test]
    fn pipeline_constrain_with_gravity() {
        let (ideal, _) = Pipeline::new(1000, 500)
            .constrain(
                Constraint::new(ConstraintMode::FitPad, 400, 400)
                    .gravity(Gravity::Percentage(0.0, 0.0))
                    .canvas_color(CanvasColor::white()),
            )
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(400, 200));
        assert_eq!(ideal.layout.canvas, Size::new(400, 400));
        assert_eq!(ideal.layout.placement, (0, 0)); // top-left gravity
    }

    #[test]
    fn pipeline_full_roundtrip() {
        // End-to-end: build pipeline, plan, finalize with full_decode
        let (ideal, req) = Pipeline::new(4000, 3000)
            .auto_orient(6)
            .crop_pixels(100, 100, 2000, 2500)
            .within(800, 800)
            .pad_uniform(5, CanvasColor::black())
            .plan()
            .unwrap();

        let lp = ideal.finalize(&req, &DecoderOffer::full_decode(4000, 3000));
        assert_eq!(lp.remaining_orientation, Orientation::Rotate90);
        assert!(lp.trim.is_some());
        assert!(!lp.resize_is_identity);
        // Canvas should be resize_to + 10 each dim
        assert_eq!(lp.canvas.width, lp.resize_to.width + 10);
        assert_eq!(lp.canvas.height, lp.resize_to.height + 10);
    }

    #[test]
    fn pipeline_zero_source_rejected() {
        assert!(Pipeline::new(0, 600).fit(100, 100).plan().is_err());
        assert!(Pipeline::new(800, 0).fit(100, 100).plan().is_err());
    }

    #[test]
    fn pipeline_last_constraint_wins() {
        let (ideal, _) = Pipeline::new(800, 600)
            .fit(200, 200)
            .within(100, 100) // replaces fit
            .plan()
            .unwrap();
        // Within 100×100 on 800×600: source is larger → downscale to 100×75
        assert_eq!(ideal.layout.resize_to, Size::new(100, 75));
    }

    #[test]
    fn pipeline_last_crop_wins() {
        let (ideal, _) = Pipeline::new(800, 600)
            .crop_pixels(0, 0, 100, 100)
            .crop_pixels(200, 200, 50, 50) // replaces first crop
            .plan()
            .unwrap();
        let crop = ideal.source_crop.unwrap();
        assert_eq!(crop, Rect::new(200, 200, 50, 50));
    }

    #[test]
    fn pipeline_within_crop() {
        let (ideal, _) = Pipeline::new(1000, 500)
            .within_crop(400, 400)
            .plan()
            .unwrap();
        // Source larger → crop to aspect + downscale
        assert_eq!(ideal.layout.resize_to, Size::new(400, 400));
        assert!(ideal.layout.source_crop.is_some());
    }

    #[test]
    fn pipeline_within_pad() {
        let (ideal, _) = Pipeline::new(200, 100).within_pad(400, 300).plan().unwrap();
        // Source fits within target → identity (imageflow behavior)
        assert_eq!(ideal.layout.resize_to, Size::new(200, 100));
        assert_eq!(ideal.layout.canvas, Size::new(200, 100));
    }

    #[test]
    fn pipeline_rotate_270() {
        let (ideal, _) = Pipeline::new(800, 600).rotate_270().plan().unwrap();
        assert_eq!(ideal.orientation, Orientation::Rotate270);
        assert_eq!(ideal.layout.resize_to, Size::new(600, 800));
    }

    #[test]
    fn pipeline_rotate_180() {
        let (ideal, _) = Pipeline::new(800, 600).rotate_180().plan().unwrap();
        assert_eq!(ideal.orientation, Orientation::Rotate180);
        assert_eq!(ideal.layout.resize_to, Size::new(800, 600));
    }

    // ════════════════════════════════════════════════════════════════════
    // Secondary plane / gain map tests
    // ════════════════════════════════════════════════════════════════════

    #[test]
    fn secondary_no_crop_quarter_scale() {
        // SDR: 4000×3000, gain map: 1000×750 (exactly 1/4)
        let (sdr, _) = Pipeline::new(4000, 3000).fit(800, 600).plan().unwrap();
        let (gm, gm_req) = sdr.derive_secondary(Size::new(4000, 3000), Size::new(1000, 750), None);

        assert_eq!(gm.orientation, sdr.orientation);
        assert!(gm.source_crop.is_none()); // no crop → no crop
        assert!(gm_req.crop.is_none());
        // Auto target: 800×600 * 0.25 = 200×150
        assert_eq!(gm.layout.resize_to, Size::new(200, 150));
    }

    #[test]
    fn secondary_no_crop_explicit_target() {
        let (sdr, _) = Pipeline::new(4000, 3000).fit(800, 600).plan().unwrap();
        let (gm, _) = sdr.derive_secondary(
            Size::new(4000, 3000),
            Size::new(1000, 750),
            Some(Size::new(800, 600)),
        );

        // Explicit target: gain map rendered at full SDR size
        assert_eq!(gm.layout.resize_to, Size::new(800, 600));
    }

    #[test]
    fn secondary_crop_scales_to_quarter() {
        // SDR crop (100,100,200,200) → gain map at 1/4 → (25,25,50,50)
        let (sdr, _) = Pipeline::new(4000, 3000)
            .crop_pixels(100, 100, 200, 200)
            .plan()
            .unwrap();

        let (gm, gm_req) = sdr.derive_secondary(Size::new(4000, 3000), Size::new(1000, 750), None);

        let crop = gm_req.crop.unwrap();
        assert_eq!(crop, Rect::new(25, 25, 50, 50));
        assert_eq!(gm.source_crop, gm_req.crop);
    }

    #[test]
    fn secondary_crop_rounds_outward() {
        // SDR crop (103,47,200,200). At 1/4:
        //   x: floor(103*0.25) = floor(25.75) = 25
        //   y: floor(47*0.25) = floor(11.75) = 11
        //   x1: ceil(303*0.25) = ceil(75.75) = 76 → w = 76-25 = 51
        //   y1: ceil(247*0.25) = ceil(61.75) = 62 → h = 62-11 = 51
        let (sdr, _) = Pipeline::new(4000, 3000)
            .crop_pixels(103, 47, 200, 200)
            .plan()
            .unwrap();

        let (_, gm_req) = sdr.derive_secondary(Size::new(4000, 3000), Size::new(1000, 750), None);
        let crop = gm_req.crop.unwrap();

        assert_eq!(crop.x, 25);
        assert_eq!(crop.y, 11);
        assert_eq!(crop.width, 51); // rounds outward
        assert_eq!(crop.height, 51);
    }

    #[test]
    fn secondary_orientation_preserved() {
        let (sdr, _) = Pipeline::new(4000, 3000)
            .auto_orient(6) // Rotate90
            .fit(800, 800)
            .plan()
            .unwrap();

        let (gm, gm_req) = sdr.derive_secondary(Size::new(4000, 3000), Size::new(1000, 750), None);

        assert_eq!(gm.orientation, Orientation::Rotate90);
        assert_eq!(gm_req.orientation, Orientation::Rotate90);
        // Oriented secondary: 750×1000 (rotated)
        assert_eq!(gm.layout.source, Size::new(750, 1000));
    }

    #[test]
    fn secondary_crop_with_orientation() {
        // SDR: 4000×3000, rotate 90° → oriented 3000×4000
        // Crop 100,100,2000,2000 in oriented space
        let (sdr, sdr_req) = Pipeline::new(4000, 3000)
            .auto_orient(6)
            .crop_pixels(100, 100, 2000, 2000)
            .fit(500, 500)
            .plan()
            .unwrap();

        let (_gm, gm_req) = sdr.derive_secondary(Size::new(4000, 3000), Size::new(1000, 750), None);

        // Both should have crops in source (pre-orient) space
        assert!(sdr_req.crop.is_some());
        assert!(gm_req.crop.is_some());

        // The gain map crop should be roughly 1/4 of the SDR crop
        let sdr_crop = sdr_req.crop.unwrap();
        let gm_crop = gm_req.crop.unwrap();

        // Spatial coverage should be at least as large (round-outward)
        let sdr_right = sdr_crop.x + sdr_crop.width;
        let sdr_bottom = sdr_crop.y + sdr_crop.height;
        let gm_right = gm_crop.x + gm_crop.width;
        let gm_bottom = gm_crop.y + gm_crop.height;

        // Gain map crop scaled back up should encompass SDR crop
        assert!(gm_crop.x as f64 * 4.0 <= sdr_crop.x as f64 + 0.01);
        assert!(gm_crop.y as f64 * 4.0 <= sdr_crop.y as f64 + 0.01);
        assert!(gm_right as f64 * 4.0 >= sdr_right as f64 - 0.01);
        assert!(gm_bottom as f64 * 4.0 >= sdr_bottom as f64 - 0.01);
    }

    #[test]
    fn secondary_finalize_both_full_decode() {
        // Both decoders do nothing — both finalize independently
        let (sdr, sdr_req) = Pipeline::new(4000, 3000)
            .auto_orient(6)
            .crop_pixels(100, 100, 2000, 2000)
            .fit(500, 500)
            .plan()
            .unwrap();

        let (gm, gm_req) = sdr.derive_secondary(Size::new(4000, 3000), Size::new(1000, 750), None);

        let sdr_plan = sdr.finalize(&sdr_req, &DecoderOffer::full_decode(4000, 3000));
        let gm_plan = gm.finalize(&gm_req, &DecoderOffer::full_decode(1000, 750));

        // Both should need the same remaining orientation
        assert_eq!(sdr_plan.remaining_orientation, Orientation::Rotate90);
        assert_eq!(gm_plan.remaining_orientation, Orientation::Rotate90);

        // Both need trim (decoder didn't crop)
        assert!(sdr_plan.trim.is_some());
        assert!(gm_plan.trim.is_some());

        // Neither is identity (both need resize)
        assert!(!sdr_plan.resize_is_identity);
        assert!(!gm_plan.resize_is_identity);
    }

    #[test]
    fn secondary_finalize_decoders_differ() {
        // SDR decoder: handles orientation + crop
        // Gain map decoder: does nothing (full decode)
        let (sdr, sdr_req) = Pipeline::new(4000, 3000)
            .auto_orient(6)
            .crop_pixels(100, 100, 2000, 2000)
            .fit(500, 500)
            .plan()
            .unwrap();

        let (gm, gm_req) = sdr.derive_secondary(Size::new(4000, 3000), Size::new(1000, 750), None);

        // SDR decoder handles everything
        let sdr_offer = DecoderOffer {
            dimensions: Size::new(2000, 2000),
            crop_applied: sdr_req.crop,
            orientation_applied: Orientation::Rotate90,
        };
        let sdr_plan = sdr.finalize(&sdr_req, &sdr_offer);

        // Gain map decoder does nothing
        let gm_offer = DecoderOffer::full_decode(1000, 750);
        let gm_plan = gm.finalize(&gm_req, &gm_offer);

        // SDR: decoder did everything → no trim, no remaining orient
        assert!(sdr_plan.trim.is_none());
        assert_eq!(sdr_plan.remaining_orientation, Orientation::Identity);

        // Gain map: decoder did nothing → trim + orient remain
        assert!(gm_plan.trim.is_some());
        assert_eq!(gm_plan.remaining_orientation, Orientation::Rotate90);

        // But both produce spatially-locked output (same orientation effect)
    }

    #[test]
    fn secondary_finalize_gain_map_has_own_mcu_grid() {
        // SDR crop (100,100,200,200), gain map at 1/4 scale
        // SDR decoder: MCU-aligns to (96,96,208,208) → output 208×208
        // GM decoder: MCU-aligns to (24,24,56,56) → output 56×56
        // (GM crop was (25,25,50,50) but decoder aligned differently)
        let (sdr, sdr_req) = Pipeline::new(800, 600)
            .crop_pixels(100, 100, 200, 200)
            .plan()
            .unwrap();

        let (gm, gm_req) = sdr.derive_secondary(Size::new(800, 600), Size::new(200, 150), None);
        let gm_crop_requested = gm_req.crop.unwrap();

        // SDR decoder: MCU-aligned
        let sdr_offer = DecoderOffer {
            dimensions: Size::new(208, 208),
            crop_applied: Some(Rect::new(96, 96, 208, 208)),
            orientation_applied: Orientation::Identity,
        };
        let sdr_plan = sdr.finalize(&sdr_req, &sdr_offer);

        // GM decoder: different MCU alignment
        let gm_offer = DecoderOffer {
            dimensions: Size::new(56, 56),
            crop_applied: Some(Rect::new(24, 24, 56, 56)),
            orientation_applied: Orientation::Identity,
        };
        let gm_plan = gm.finalize(&gm_req, &gm_offer);

        // SDR trim: offset within decoder output
        let sdr_trim = sdr_plan.trim.unwrap();
        assert_eq!(sdr_trim.x, 4); // 100 - 96
        assert_eq!(sdr_trim.y, 4);
        assert_eq!(sdr_trim.width, 200);
        assert_eq!(sdr_trim.height, 200);

        // GM trim: offset within its decoder output
        let gm_trim = gm_plan.trim.unwrap();
        let expected_dx = gm_crop_requested.x - 24; // requested.x - applied.x
        let expected_dy = gm_crop_requested.y - 24;
        assert_eq!(gm_trim.x, expected_dx);
        assert_eq!(gm_trim.y, expected_dy);
        assert_eq!(gm_trim.width, gm_crop_requested.width);
        assert_eq!(gm_trim.height, gm_crop_requested.height);

        // Both independently correct despite different MCU grids
        assert!(sdr_plan.resize_is_identity);
        assert!(gm_plan.resize_is_identity);
    }

    #[test]
    fn secondary_no_padding() {
        let (sdr, _) = Pipeline::new(800, 600)
            .fit_pad(400, 400)
            .pad_uniform(10, CanvasColor::white())
            .plan()
            .unwrap();

        // SDR has padding
        assert!(sdr.padding.is_some());
        assert_eq!(sdr.layout.canvas, Size::new(420, 420)); // 400+20 from explicit pad

        let (gm, _) = sdr.derive_secondary(Size::new(800, 600), Size::new(200, 150), None);

        // Gain map: no padding
        assert!(gm.padding.is_none());
        assert_eq!(gm.layout.canvas, gm.layout.resize_to);
    }

    #[test]
    fn secondary_non_integer_scale() {
        // SDR: 1920×1080, gain map: 480×270 (exactly 1/4)
        // Crop at (100,50,500,300): y0=floor(50*0.25)=12, y1=ceil(350*0.25)=88 → h=76
        // Round-outward because 50/4=12.5 doesn't divide cleanly
        let (sdr, _) = Pipeline::new(1920, 1080)
            .crop_pixels(100, 50, 500, 300)
            .plan()
            .unwrap();

        let (_, gm_req) = sdr.derive_secondary(Size::new(1920, 1080), Size::new(480, 270), None);
        let crop = gm_req.crop.unwrap();
        assert_eq!(crop, Rect::new(25, 12, 125, 76));
    }

    #[test]
    fn secondary_odd_ratio() {
        // SDR: 1000×1000, gain map: 333×333 (1/3.003 — not clean)
        // Crop (100,100,600,600) → scaled: floor(33.3)=33, ceil(233.1)=234 → w=201
        let (sdr, _) = Pipeline::new(1000, 1000)
            .crop_pixels(100, 100, 600, 600)
            .plan()
            .unwrap();

        let (_, gm_req) = sdr.derive_secondary(Size::new(1000, 1000), Size::new(333, 333), None);
        let crop = gm_req.crop.unwrap();

        // Round outward: origin floors, far edge ceils
        let scale: f64 = 333.0 / 1000.0;
        let x0 = (100.0_f64 * scale).floor() as u32;
        let y0 = (100.0_f64 * scale).floor() as u32;
        let x1 = (700.0_f64 * scale).ceil() as u32;
        let y1 = (700.0_f64 * scale).ceil() as u32;
        assert_eq!(crop.x, x0);
        assert_eq!(crop.y, y0);
        assert_eq!(crop.width, x1 - x0);
        assert_eq!(crop.height, y1 - y0);

        // Verify outward: scaled back up should encompass original
        assert!(crop.x as f64 / scale <= 100.0 + 0.01);
        assert!((crop.x + crop.width) as f64 / scale >= 700.0 - 0.01);
    }

    #[test]
    fn secondary_crop_at_edge_clamped() {
        // SDR crop near right edge: (3800,2800,200,200) in 4000×3000
        // GM at 1/4: (950,700,50,50) — right at the edge of 1000×750
        let (sdr, _) = Pipeline::new(4000, 3000)
            .crop_pixels(3800, 2800, 200, 200)
            .plan()
            .unwrap();

        let (_, gm_req) = sdr.derive_secondary(Size::new(4000, 3000), Size::new(1000, 750), None);
        let crop = gm_req.crop.unwrap();

        // Should be clamped to gain map bounds
        assert!(crop.x + crop.width <= 1000);
        assert!(crop.y + crop.height <= 750);
    }

    #[test]
    fn secondary_passthrough_no_commands() {
        let (sdr, _) = Pipeline::new(800, 600).plan().unwrap();
        let (gm, gm_req) = sdr.derive_secondary(Size::new(800, 600), Size::new(200, 150), None);

        assert!(gm.source_crop.is_none());
        assert!(gm_req.crop.is_none());
        assert_eq!(gm.orientation, Orientation::Identity);
        assert_eq!(gm.layout.resize_to, Size::new(200, 150));
        assert_eq!(gm.layout.source, Size::new(200, 150));
    }

    #[test]
    fn secondary_lossless_path() {
        // Both SDR and gain map do rotate+crop, both decoders handle it
        let (sdr, sdr_req) = Pipeline::new(4000, 3000)
            .auto_orient(6)
            .crop_pixels(0, 0, 1000, 1500)
            .plan()
            .unwrap();

        let (gm, gm_req) = sdr.derive_secondary(Size::new(4000, 3000), Size::new(1000, 750), None);

        // SDR decoder handles everything
        let sdr_offer = DecoderOffer {
            dimensions: Size::new(1000, 1500),
            crop_applied: sdr_req.crop,
            orientation_applied: Orientation::Rotate90,
        };
        let sdr_plan = sdr.finalize(&sdr_req, &sdr_offer);
        assert!(sdr_plan.resize_is_identity);
        assert_eq!(sdr_plan.remaining_orientation, Orientation::Identity);

        // GM decoder handles everything too
        let gm_offer = DecoderOffer {
            dimensions: Size::new(gm.layout.resize_to.width, gm.layout.resize_to.height),
            crop_applied: gm_req.crop,
            orientation_applied: Orientation::Rotate90,
        };
        let gm_plan = gm.finalize(&gm_req, &gm_offer);
        assert!(gm_plan.resize_is_identity);
        assert_eq!(gm_plan.remaining_orientation, Orientation::Identity);
    }

    #[test]
    fn secondary_all_8_orientations() {
        for exif in 1..=8u8 {
            let (sdr, _) = Pipeline::new(800, 600).auto_orient(exif).plan().unwrap();
            let (gm, gm_req) = sdr.derive_secondary(Size::new(800, 600), Size::new(200, 150), None);

            assert_eq!(
                gm.orientation,
                Orientation::from_exif(exif).unwrap(),
                "EXIF {exif}"
            );
            assert_eq!(gm_req.orientation, gm.orientation);

            // Oriented dims should match
            let expected = Orientation::from_exif(exif)
                .unwrap()
                .transform_dimensions(200, 150);
            assert_eq!(gm.layout.source, expected, "EXIF {exif} oriented dims");
        }
    }

    // ── OutputLimits ────────────────────────────────────────────

    #[test]
    fn limits_max_caps_canvas() {
        // Fit 100×100 into 2000×2000 → resize_to=2000×2000
        // Max 500×500 should cap to 500×500
        let (ideal, _) = Pipeline::new(100, 100)
            .fit(2000, 2000)
            .output_limits(OutputLimits {
                max: Some(Size::new(500, 500)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert!(ideal.layout.resize_to.width <= 500);
        assert!(ideal.layout.resize_to.height <= 500);
        assert!(ideal.layout.canvas.width <= 500);
        assert!(ideal.layout.canvas.height <= 500);
    }

    #[test]
    fn limits_max_preserves_aspect() {
        // 1000×500 fit to 2000×1000 → resize_to=2000×1000
        // Max 600×600 → should scale to 600×300 (2:1 aspect preserved)
        let (ideal, _) = Pipeline::new(1000, 500)
            .fit(2000, 1000)
            .output_limits(OutputLimits {
                max: Some(Size::new(600, 600)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(600, 300));
        assert_eq!(ideal.layout.canvas, Size::new(600, 300));
    }

    #[test]
    fn limits_max_scales_padded_canvas() {
        // FitPad(400, 400) on 800×600 → resize_to=(400,300), canvas=(400,400)
        // Max 200×200 → scale by 0.5 → resize_to=(200,150), canvas=(200,200)
        let (ideal, _) = Pipeline::new(800, 600)
            .fit_pad(400, 400)
            .output_limits(OutputLimits {
                max: Some(Size::new(200, 200)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert!(ideal.layout.canvas.width <= 200);
        assert!(ideal.layout.canvas.height <= 200);
        assert_eq!(ideal.layout.resize_to, Size::new(200, 150));
        assert_eq!(ideal.layout.canvas, Size::new(200, 200));
    }

    #[test]
    fn limits_max_noop_when_within() {
        // Already within max → no change
        let (ideal, _) = Pipeline::new(800, 600)
            .fit(400, 300)
            .output_limits(OutputLimits {
                max: Some(Size::new(1000, 1000)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(400, 300));
    }

    #[test]
    fn limits_min_scales_up() {
        // 1000×1000 within 50×50 → resize_to=50×50 (no upscale)
        // Wait, Within doesn't upscale. Use Within so we get small output.
        let (ideal, _) = Pipeline::new(100, 100)
            .within(50, 50)
            .output_limits(OutputLimits {
                min: Some(Size::new(200, 200)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        // Min should push resize_to up to at least 200 on the smaller axis
        assert!(ideal.layout.resize_to.width >= 200);
        assert!(ideal.layout.resize_to.height >= 200);
    }

    #[test]
    fn limits_min_preserves_aspect() {
        // 1000×500 within 100×50 → resize_to=100×50
        // Min (200, 200) → scale = max(200/100, 200/50) = 4 → 400×200
        let (ideal, _) = Pipeline::new(1000, 500)
            .within(100, 50)
            .output_limits(OutputLimits {
                min: Some(Size::new(200, 200)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(400, 200));
    }

    #[test]
    fn limits_max_wins_over_min() {
        // min=500×500, max=200×200 → max wins
        let (ideal, _) = Pipeline::new(1000, 1000)
            .within(100, 100)
            .output_limits(OutputLimits {
                max: Some(Size::new(200, 200)),
                min: Some(Size::new(500, 500)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert!(ideal.layout.resize_to.width <= 200);
        assert!(ideal.layout.resize_to.height <= 200);
        assert!(ideal.layout.canvas.width <= 200);
        assert!(ideal.layout.canvas.height <= 200);
    }

    #[test]
    fn limits_align_snaps_down() {
        // 1000×667 fit to 1000×667 → resize_to=1000×667
        // Align 16 → 992×656
        let (ideal, _) = Pipeline::new(1000, 667)
            .fit(1000, 667)
            .output_limits(OutputLimits {
                align: Some(Align::Crop(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to.width % 16, 0);
        assert_eq!(ideal.layout.resize_to.height % 16, 0);
        assert_eq!(ideal.layout.resize_to, Size::new(992, 656));
    }

    #[test]
    fn limits_align_mod2_for_video() {
        // 801×601 → align 2 → 800×600
        let (ideal, _) = Pipeline::new(801, 601)
            .fit(801, 601)
            .output_limits(OutputLimits {
                align: Some(Align::Crop(2, 2)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(800, 600));
    }

    #[test]
    fn limits_align_preserves_padded_canvas() {
        // FitPad(400, 400) on 800×600 → resize_to=(400,300), canvas=(400,400)
        // Align 16: canvas already 400×400 (mod 16). No change.
        let (ideal, _) = Pipeline::new(800, 600)
            .fit_pad(400, 400)
            .output_limits(OutputLimits {
                align: Some(Align::Crop(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas.width % 16, 0);
        assert_eq!(ideal.layout.canvas.height % 16, 0);
        assert_eq!(ideal.layout.canvas, Size::new(400, 400));
        assert_eq!(ideal.layout.resize_to, Size::new(400, 300)); // unchanged
    }

    #[test]
    fn limits_align_snaps_padded_canvas() {
        // FitPad(401, 401) on 800×600 → resize_to=(401,301), canvas=(401,401)
        // Align 16: canvas → 400×400, resize_to stays 401→clamped to 400, 301 stays
        let (ideal, _) = Pipeline::new(800, 600)
            .fit_pad(401, 401)
            .output_limits(OutputLimits {
                align: Some(Align::Crop(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas.width % 16, 0);
        assert_eq!(ideal.layout.canvas.height % 16, 0);
        // resize_to clamped to canvas
        assert!(ideal.layout.resize_to.width <= ideal.layout.canvas.width);
        assert!(ideal.layout.resize_to.height <= ideal.layout.canvas.height);
    }

    #[test]
    fn limits_align_1_is_noop() {
        let (ideal, _) = Pipeline::new(801, 601)
            .fit(801, 601)
            .output_limits(OutputLimits {
                align: Some(Align::Crop(1, 1)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(801, 601));
    }

    #[test]
    fn limits_all_three_combined() {
        // Start big: 100×100 fit to 10000×10000
        // Max 1920×1080, min 100×100, align 8
        let (ideal, _) = Pipeline::new(100, 100)
            .fit(10000, 10000)
            .output_limits(OutputLimits {
                max: Some(Size::new(1920, 1080)),
                min: Some(Size::new(100, 100)),
                align: Some(Align::Crop(8, 8)),
            })
            .plan()
            .unwrap();
        // Max caps to 1080×1080 (square, height constrains)
        assert!(ideal.layout.canvas.width <= 1920);
        assert!(ideal.layout.canvas.height <= 1080);
        // Align snaps canvas
        assert_eq!(ideal.layout.canvas.width % 8, 0);
        assert_eq!(ideal.layout.canvas.height % 8, 0);
        // Min satisfied (1080 > 100)
        assert!(ideal.layout.canvas.width >= 100);
    }

    #[test]
    fn limits_max_with_explicit_pad() {
        // Fit(200, 200) on 400×400 + pad 50 all → resize=200×200, canvas=300×300
        // Max 250×250 → scale by 250/300 ≈ 0.833
        let (ideal, _) = Pipeline::new(400, 400)
            .fit(200, 200)
            .pad_uniform(50, CanvasColor::white())
            .output_limits(OutputLimits {
                max: Some(Size::new(250, 250)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert!(ideal.layout.canvas.width <= 250);
        assert!(ideal.layout.canvas.height <= 250);
    }

    #[test]
    fn limits_default_is_noop() {
        // Default OutputLimits (all None) should be identity
        let (a, _) = Pipeline::new(800, 600).fit(400, 300).plan().unwrap();
        let (b, _) = Pipeline::new(800, 600)
            .fit(400, 300)
            .output_limits(OutputLimits::default())
            .plan()
            .unwrap();
        assert_eq!(a.layout, b.layout);
    }

    #[test]
    fn limits_tiny_image_align_doesnt_zero() {
        // 3×3 image, align 16 → canvas snaps to 16×16, resize_to stays 3×3
        let (ideal, _) = Pipeline::new(3, 3)
            .fit(3, 3)
            .output_limits(OutputLimits {
                align: Some(Align::Crop(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(16, 16));
        assert_eq!(ideal.layout.resize_to, Size::new(3, 3));
    }

    // ── CanvasColor::Linear ─────────────────────────────────────────────

    #[test]
    fn canvas_color_linear_equality() {
        let a = CanvasColor::Linear {
            r: 1.0,
            g: 0.5,
            b: 0.0,
            a: 1.0,
        };
        let b = CanvasColor::Linear {
            r: 1.0,
            g: 0.5,
            b: 0.0,
            a: 1.0,
        };
        assert_eq!(a, b);
    }

    #[test]
    fn canvas_color_linear_ne_srgb() {
        let linear = CanvasColor::Linear {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        };
        let srgb = CanvasColor::white();
        assert_ne!(linear, srgb);
    }

    #[test]
    fn canvas_color_linear_in_pipeline() {
        let (ideal, _) = Pipeline::new(800, 600)
            .fit_pad(400, 400)
            .pad_uniform(
                10,
                CanvasColor::Linear {
                    r: 0.5,
                    g: 0.5,
                    b: 0.5,
                    a: 1.0,
                },
            )
            .plan()
            .unwrap();
        assert!(matches!(
            ideal.layout.canvas_color,
            CanvasColor::Linear { .. }
        ));
    }

    // ── Align::Extend ───────────────────────────────────────────────────

    #[test]
    fn align_extend_rounds_up() {
        // 801×601, align extend 16 → canvas 816×608, content_size = (801, 601)
        let (ideal, _) = Pipeline::new(801, 601)
            .fit(801, 601)
            .output_limits(OutputLimits {
                align: Some(Align::Extend(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(816, 608));
        assert_eq!(ideal.content_size, Some(Size::new(801, 601)));
        assert_eq!(ideal.layout.placement, (0, 0));
        assert_eq!(ideal.layout.resize_to, Size::new(801, 601));
    }

    #[test]
    fn align_extend_already_aligned_noop() {
        // 800×640, already mod-16 → no extension
        let (ideal, _) = Pipeline::new(800, 640)
            .fit(800, 640)
            .output_limits(OutputLimits {
                align: Some(Align::Extend(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(800, 640));
        assert_eq!(ideal.content_size, None);
    }

    #[test]
    fn align_extend_mod2() {
        // 801×601, mod-2 → 802×602
        let (ideal, _) = Pipeline::new(801, 601)
            .fit(801, 601)
            .output_limits(OutputLimits {
                align: Some(Align::Extend(2, 2)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(802, 602));
        assert_eq!(ideal.content_size, Some(Size::new(801, 601)));
    }

    #[test]
    fn align_extend_mcu_8() {
        // 100×100, MCU-8 → 104×104
        let (ideal, _) = Pipeline::new(100, 100)
            .fit(100, 100)
            .output_limits(OutputLimits {
                align: Some(Align::Extend(8, 8)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(104, 104));
        assert_eq!(ideal.content_size, Some(Size::new(100, 100)));
        assert_eq!(ideal.layout.resize_to, Size::new(100, 100));
    }

    #[test]
    fn align_extend_with_pad() {
        // FitPad(400, 400) on 800×600 → canvas=400×400 (already mod-16)
        let (ideal, _) = Pipeline::new(800, 600)
            .fit_pad(400, 400)
            .output_limits(OutputLimits {
                align: Some(Align::Extend(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        // 400 is mod-16, so no extension
        assert_eq!(ideal.layout.canvas, Size::new(400, 400));
        assert_eq!(ideal.content_size, None);
    }

    #[test]
    fn align_extend_with_unaligned_pad() {
        // FitPad(401, 401) → canvas=401×401, extend to 416×416
        let (ideal, _) = Pipeline::new(800, 600)
            .fit_pad(401, 401)
            .output_limits(OutputLimits {
                align: Some(Align::Extend(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(416, 416));
        assert_eq!(ideal.content_size, Some(Size::new(401, 401)));
        assert_eq!(ideal.layout.placement, (0, 0)); // moved to origin
    }

    #[test]
    fn align_extend_finalize_carries_through() {
        // content_size passes through finalize
        let (ideal, req) = Pipeline::new(801, 601)
            .fit(801, 601)
            .output_limits(OutputLimits {
                align: Some(Align::Extend(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        let offer = DecoderOffer::full_decode(801, 601);
        let lp = ideal.finalize(&req, &offer);
        assert_eq!(lp.canvas, Size::new(816, 608));
        assert_eq!(lp.content_size, Some(Size::new(801, 601)));
    }

    #[test]
    fn align_extend_max_then_extend() {
        // 4000×3000 fit to 4000×3000, max 1920×1080 → 1440×1080
        // Then extend mod-16: 1440 is mod-16, 1080 is not → 1440×1088
        let (ideal, _) = Pipeline::new(4000, 3000)
            .fit(4000, 3000)
            .output_limits(OutputLimits {
                max: Some(Size::new(1920, 1080)),
                align: Some(Align::Extend(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert!(ideal.layout.canvas.width % 16 == 0);
        assert!(ideal.layout.canvas.height % 16 == 0);
        // Max applied first, then extend only adds a few pixels
        assert!(ideal.layout.canvas.width <= 1920 + 15);
        assert!(ideal.layout.canvas.height <= 1080 + 15);
        if let Some(Size {
            width: cw,
            height: ch,
        }) = ideal.content_size
        {
            assert!(cw <= 1920);
            assert!(ch <= 1080);
        }
    }

    // ── Per-axis alignment ──────────────────────────────────────────────

    #[test]
    fn align_per_axis_extend_422() {
        // 4:2:2: MCU is 16×8. 801×601 → extend to 816×608
        let (ideal, _) = Pipeline::new(801, 601)
            .fit(801, 601)
            .output_limits(OutputLimits {
                align: Some(Align::Extend(16, 8)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(816, 608));
        assert_eq!(ideal.content_size, Some(Size::new(801, 601)));
    }

    #[test]
    fn align_per_axis_extend_420() {
        // 4:2:0: MCU is 16×16. 801×601 → extend to 816×608 (y mod-16: 608)
        // Wait, 601 div_ceil 16 = 38, 38*16 = 608. So 816×608.
        // Hmm no: for 4:2:0, MCU is 16×16. 601/16 = 37.5625, ceil = 38, 38*16 = 608.
        let (ideal, _) = Pipeline::new(801, 601)
            .fit(801, 601)
            .output_limits(OutputLimits {
                align: Some(Subsampling::S420.mcu_align()),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(816, 608));
        assert_eq!(ideal.layout.canvas.width % 16, 0);
        assert_eq!(ideal.layout.canvas.height % 16, 0);
    }

    #[test]
    fn align_per_axis_rounddown_different() {
        // Round down: x mod-16, y mod-8. 801×601 → 800×600
        let (ideal, _) = Pipeline::new(801, 601)
            .fit(801, 601)
            .output_limits(OutputLimits {
                align: Some(Align::Crop(16, 8)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas.width % 16, 0);
        assert_eq!(ideal.layout.canvas.height % 8, 0);
        assert_eq!(ideal.layout.canvas, Size::new(800, 600));
    }

    #[test]
    fn subsampling_mcu_align_444() {
        // 4:4:4: MCU is 8×8
        let align = Subsampling::S444.mcu_align();
        assert_eq!(align, Align::Extend(8, 8));
    }

    #[test]
    fn subsampling_mcu_align_422() {
        let align = Subsampling::S422.mcu_align();
        assert_eq!(align, Align::Extend(16, 8));
    }

    #[test]
    fn subsampling_mcu_align_420() {
        let align = Subsampling::S420.mcu_align();
        assert_eq!(align, Align::Extend(16, 16));
    }

    // ── Align::Distort ─────────────────────────────────────────────────

    #[test]
    fn align_distort_rounds_to_nearest() {
        // 801×601, distort mod-16 → resize_to rounds to nearest:
        // 801 → 800 (801+8=809, 809/16=50, 50*16=800)
        // 601 → 608 (601+8=609, 609/16=38, 38*16=608)
        let (ideal, _) = Pipeline::new(801, 601)
            .fit(801, 601)
            .output_limits(OutputLimits {
                align: Some(Align::Distort(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(800, 608));
        assert_eq!(ideal.layout.canvas, Size::new(800, 608));
        assert_eq!(ideal.content_size, None); // no content_size for distort
    }

    #[test]
    fn align_distort_already_aligned_noop() {
        let (ideal, _) = Pipeline::new(800, 640)
            .fit(800, 640)
            .output_limits(OutputLimits {
                align: Some(Align::Distort(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(800, 640));
        assert_eq!(ideal.layout.canvas, Size::new(800, 640));
    }

    #[test]
    fn align_distort_mod2() {
        // 801×601, distort mod-2 → 802×602
        let (ideal, _) = Pipeline::new(801, 601)
            .fit(801, 601)
            .output_limits(OutputLimits {
                align: Some(Align::Distort(2, 2)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(802, 602));
        assert_eq!(ideal.layout.canvas, Size::new(802, 602));
    }

    #[test]
    fn align_distort_with_pad_recenters() {
        // FitPad(401, 401) on 800×600 → resize_to=(401,301), canvas=(401,401)
        // Distort mod-16: resize_to → round(401,16)=400, round(301,16)=304
        // Canvas 401 > 400 → placement recentered. 401 > 304 → placement recentered.
        let (ideal, _) = Pipeline::new(800, 600)
            .fit_pad(401, 401)
            .output_limits(OutputLimits {
                align: Some(Align::Distort(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to.width % 16, 0);
        assert_eq!(ideal.layout.resize_to.height % 16, 0);
        // Canvas stays >= resize_to in padded mode
        assert!(ideal.layout.canvas.width >= ideal.layout.resize_to.width);
        assert!(ideal.layout.canvas.height >= ideal.layout.resize_to.height);
    }

    #[test]
    fn align_distort_non_pad_canvas_tracks() {
        // Fit(801, 601) → resize=801×601, canvas=801×601 (no pad)
        // Distort mod-8: 801→800, 601→600
        // Canvas ≤ resize → canvas follows
        let (ideal, _) = Pipeline::new(801, 601)
            .fit(801, 601)
            .output_limits(OutputLimits {
                align: Some(Align::Distort(8, 8)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(800, 600));
        assert_eq!(ideal.layout.canvas, Size::new(800, 600));
        assert_eq!(ideal.layout.placement, (0, 0));
    }

    #[test]
    fn align_distort_per_axis() {
        // Distort: x mod-16, y mod-8. 801×601 → 800×600
        let (ideal, _) = Pipeline::new(801, 601)
            .fit(801, 601)
            .output_limits(OutputLimits {
                align: Some(Align::Distort(16, 8)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to.width % 16, 0);
        assert_eq!(ideal.layout.resize_to.height % 8, 0);
        assert_eq!(ideal.layout.resize_to, Size::new(800, 600));
    }

    // ── CodecLayout ─────────────────────────────────────────────────────

    #[test]
    fn codec_layout_420_aligned() {
        // 800×608, already MCU-aligned for 4:2:0
        let cl = CodecLayout::new(Size::new(800, 608), Subsampling::S420);
        assert_eq!(cl.mcu_size, Size::new(16, 16));
        assert_eq!(cl.luma.extended, Size::new(800, 608));
        assert_eq!(cl.luma.content, Size::new(800, 608));
        assert_eq!(cl.chroma.extended, Size::new(400, 304));
        assert_eq!(cl.chroma.content, Size::new(400, 304));
        assert_eq!(cl.mcu_cols, 50);
        assert_eq!(cl.mcu_rows, 38);
        assert_eq!(cl.luma_rows_per_mcu, 16);
        assert_eq!(cl.luma.blocks_w, 100);
        assert_eq!(cl.luma.blocks_h, 76);
        assert_eq!(cl.chroma.blocks_w, 50);
        assert_eq!(cl.chroma.blocks_h, 38);
    }

    #[test]
    fn codec_layout_422() {
        let cl = CodecLayout::new(Size::new(800, 600), Subsampling::S422);
        assert_eq!(cl.mcu_size, Size::new(16, 8));
        assert_eq!(cl.luma.extended, Size::new(800, 600));
        assert_eq!(cl.chroma.extended, Size::new(400, 600));
        assert_eq!(cl.luma_rows_per_mcu, 8);
        assert_eq!(cl.mcu_cols, 50);
        assert_eq!(cl.mcu_rows, 75);
    }

    #[test]
    fn codec_layout_444() {
        let cl = CodecLayout::new(Size::new(800, 600), Subsampling::S444);
        assert_eq!(cl.mcu_size, Size::new(8, 8));
        assert_eq!(cl.luma.extended, Size::new(800, 600));
        assert_eq!(cl.chroma.extended, Size::new(800, 600)); // same as luma
        assert_eq!(cl.luma_rows_per_mcu, 8);
    }

    #[test]
    fn codec_layout_unaligned_extends_internally() {
        // 801×601 with 4:2:0 — CodecLayout rounds up internally
        let cl = CodecLayout::new(Size::new(801, 601), Subsampling::S420);
        assert_eq!(cl.luma.content, Size::new(801, 601));
        assert_eq!(cl.luma.extended, Size::new(816, 608));
        assert_eq!(cl.chroma.content, Size::new(401, 301));
        assert_eq!(cl.chroma.extended, Size::new(408, 304));
    }

    #[test]
    fn codec_layout_pipeline_integration() {
        // Full pipeline: resize → align → codec layout
        let (ideal, _) = Pipeline::new(4000, 3000)
            .auto_orient(6) // 90° → 3000×4000
            .fit(600, 800)
            .output_limits(OutputLimits {
                align: Some(Subsampling::S420.mcu_align()),
                ..Default::default()
            })
            .plan()
            .unwrap();

        let cl = CodecLayout::new(ideal.layout.canvas, Subsampling::S420);

        // Canvas is MCU-aligned
        assert_eq!(ideal.layout.canvas.width % 16, 0);
        assert_eq!(ideal.layout.canvas.height % 16, 0);

        // CodecLayout agrees with canvas
        assert_eq!(cl.luma.extended, ideal.layout.canvas);

        // Chroma is exactly half
        assert_eq!(cl.chroma.extended.width, cl.luma.extended.width / 2);
        assert_eq!(cl.chroma.extended.height, cl.luma.extended.height / 2);

        // Streaming: feed 16 rows at a time
        assert_eq!(cl.luma_rows_per_mcu, 16);
    }

    #[test]
    fn codec_layout_1x1() {
        // Tiny image: 1×1 with 4:2:0 → MCU 16×16
        let cl = CodecLayout::new(Size::new(1, 1), Subsampling::S420);
        assert_eq!(cl.luma.extended, Size::new(16, 16));
        assert_eq!(cl.chroma.extended, Size::new(8, 8));
        assert_eq!(cl.mcu_cols, 1);
        assert_eq!(cl.mcu_rows, 1);
    }

    // ════════════════════════════════════════════════════════════════════
    // Constraint interaction tests (min/max/align combinations)
    // ════════════════════════════════════════════════════════════════════

    // ── min + align interactions ────────────────────────────────────────

    #[test]
    fn limits_min_then_extend() {
        // 100×50 within(100, 50) → resize=100×50, canvas=100×50
        // min(200, 200) → scale = max(200/100, 200/50) = 4.0 → 400×200
        // extend mod-16: 400 mod 16 = 0 (ok), 200 mod 16 = 8 → 400×208
        // content_size = (400, 200)
        let (ideal, _) = Pipeline::new(100, 50)
            .within(100, 50)
            .output_limits(OutputLimits {
                min: Some(Size::new(200, 200)),
                align: Some(Align::Extend(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(400, 208));
        assert_eq!(ideal.content_size, Some(Size::new(400, 200)));
        assert_eq!(ideal.layout.resize_to, Size::new(400, 200));
    }

    #[test]
    fn limits_min_then_crop_can_undo() {
        // 100×50 within(100, 50) → canvas=100×50
        // min(200, 200) → scale 4.0 → canvas=400×200
        // crop mod-16: 400/16=25*16=400, 200/16=12*16=192
        // Canvas drops to 400×192, which is below min_h=200.
        // This is by design: align is applied AFTER min, may slightly violate.
        let (ideal, _) = Pipeline::new(100, 50)
            .within(100, 50)
            .output_limits(OutputLimits {
                min: Some(Size::new(200, 200)),
                align: Some(Align::Crop(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(400, 192));
        assert!(ideal.layout.canvas.height < 200); // crop undid min on y axis
    }

    #[test]
    fn limits_min_then_distort_can_drop() {
        // 100×50 within(100, 50) → canvas=100×50
        // min(200, 200) → scale 4.0 → resize=400×200, canvas=400×200
        // distort mod-16: round_nearest(400,16)=400, round_nearest(200,16)=192 (200+8=208, 208/16=13, 13*16=208 — wait...)
        // round_to_nearest(v, n) = ((v + n/2) / n).max(1) * n
        // round_to_nearest(200, 16) = ((200 + 8) / 16).max(1) * 16 = (208/16)*16 = 13*16 = 208
        // So resize_to = 400×208, canvas = 400×208
        let (ideal, _) = Pipeline::new(100, 50)
            .within(100, 50)
            .output_limits(OutputLimits {
                min: Some(Size::new(200, 200)),
                align: Some(Align::Distort(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(400, 208));
        assert_eq!(ideal.layout.canvas, Size::new(400, 208));
    }

    // ── max + align interactions ────────────────────────────────────────

    #[test]
    fn limits_max_then_crop() {
        // 1000×1000 fit(1000, 1000) → canvas=1000×1000
        // max(500, 500) → scale 0.5 → 500×500
        // crop mod-16: 500/16=31*16=496, 500/16=31*16=496
        // Crop only reduces: 496 < 500 ✓
        let (ideal, _) = Pipeline::new(1000, 1000)
            .fit(1000, 1000)
            .output_limits(OutputLimits {
                max: Some(Size::new(500, 500)),
                align: Some(Align::Crop(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(496, 496));
        assert!(ideal.layout.canvas.width <= 500);
        assert!(ideal.layout.canvas.height <= 500);
    }

    #[test]
    fn limits_max_then_distort_can_exceed() {
        // 1000×1000 fit(1000, 1000) → canvas=1000×1000
        // max(500, 500) → 500×500
        // distort mod-16: round_nearest(500, 16) = ((500+8)/16)*16 = (508/16)*16 = 31*16 = 496
        // Actually, 508/16 = 31.75, truncated to 31, 31*16 = 496. So 496×496.
        let (ideal, _) = Pipeline::new(1000, 1000)
            .fit(1000, 1000)
            .output_limits(OutputLimits {
                max: Some(Size::new(500, 500)),
                align: Some(Align::Distort(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to.width % 16, 0);
        assert_eq!(ideal.layout.resize_to.height % 16, 0);
        // In this case distort rounds DOWN from 500 to 496
        assert_eq!(ideal.layout.resize_to, Size::new(496, 496));
    }

    // ── three-way interactions ──────────────────────────────────────────

    #[test]
    fn limits_min_max_extend() {
        // 100×50 within(100, 50) → canvas=100×50
        // max(500, 500), min(200, 200) → min scale=4.0 → 400×200. Both within max ✓
        // extend mod-16: 400 ok, 200→208. canvas=400×208
        // content_size=(400, 200). Canvas exceeds max? 400<500, 208<500 → no.
        let (ideal, _) = Pipeline::new(100, 50)
            .within(100, 50)
            .output_limits(OutputLimits {
                max: Some(Size::new(500, 500)),
                min: Some(Size::new(200, 200)),
                align: Some(Align::Extend(16, 16)),
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(400, 208));
        assert_eq!(ideal.content_size, Some(Size::new(400, 200)));
        // Canvas extended past 200 on height, but content_size is within max
        assert!(ideal.content_size.unwrap().width <= 500);
        assert!(ideal.content_size.unwrap().height <= 500);
    }

    #[test]
    fn limits_min_max_distort() {
        // 100×50 within(100, 50) → canvas=100×50
        // max(300, 300), min(200, 200) → min: scale=4.0→400×200
        //   max re-apply: 400>300, scale=300/400=0.75 → 300×150
        //   But 150<200 — max wins, min is violated.
        // distort mod-16: round_nearest(300,16)=304, round_nearest(150,16)=144
        //   ((300+8)/16)=19*16=304, ((150+8)/16)=9*16=144
        let (ideal, _) = Pipeline::new(100, 50)
            .within(100, 50)
            .output_limits(OutputLimits {
                max: Some(Size::new(300, 300)),
                min: Some(Size::new(200, 200)),
                align: Some(Align::Distort(16, 16)),
            })
            .plan()
            .unwrap();
        // Max won: ≤300. Distort may slightly exceed.
        assert_eq!(ideal.layout.resize_to, Size::new(304, 144));
        // 304 > 300: distort rounded UP past max (documented behavior)
        assert!(ideal.layout.resize_to.width > 300);
    }

    // ── max == min ──────────────────────────────────────────────────────

    #[test]
    fn limits_max_equals_min_matching() {
        // 100×100 within(100, 100) → canvas=100×100
        // max=min=(200, 200) → min scale=2.0 → 200×200. Exactly at max too.
        let (ideal, _) = Pipeline::new(100, 100)
            .within(100, 100)
            .output_limits(OutputLimits {
                max: Some(Size::new(200, 200)),
                min: Some(Size::new(200, 200)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(200, 200));
    }

    #[test]
    fn limits_max_equals_min_mismatch() {
        // 100×50 within(100, 50) → canvas=100×50
        // max=min=(200, 200):
        //   min: scale=max(200/100, 200/50)=4.0 → 400×200
        //   max re-apply: 400>200, scale=min(200/400, 200/200)=0.5 → 200×100
        //   Max wins: 200×100 (min violated on height)
        let (ideal, _) = Pipeline::new(100, 50)
            .within(100, 50)
            .output_limits(OutputLimits {
                max: Some(Size::new(200, 200)),
                min: Some(Size::new(200, 200)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(200, 100));
        // Max wins: canvas fits within max
        assert!(ideal.layout.canvas.width <= 200);
        assert!(ideal.layout.canvas.height <= 200);
    }

    // ── tiny image edge cases ──────────────────────────────────────────

    #[test]
    fn limits_1x1_distort_large_align() {
        // 1×1 fit(1, 1) → resize=1×1, canvas=1×1
        // distort mod-16: round_nearest(1, 16) = ((1+8)/16).max(1)*16 = 1*16 = 16
        let (ideal, _) = Pipeline::new(1, 1)
            .fit(1, 1)
            .output_limits(OutputLimits {
                align: Some(Align::Distort(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(16, 16));
        assert_eq!(ideal.layout.canvas, Size::new(16, 16));
    }

    #[test]
    fn limits_1x1_extend() {
        // 1×1 fit(1, 1) → resize=1×1, canvas=1×1
        // extend mod-16: canvas→16×16, content_size=(1, 1)
        let (ideal, _) = Pipeline::new(1, 1)
            .fit(1, 1)
            .output_limits(OutputLimits {
                align: Some(Align::Extend(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(16, 16));
        assert_eq!(ideal.content_size, Some(Size::new(1, 1)));
        assert_eq!(ideal.layout.resize_to, Size::new(1, 1));
    }

    // ── other edge cases ───────────────────────────────────────────────

    #[test]
    fn limits_distort_padded_axis_detection() {
        // FitPad where one axis is padded and one isn't.
        // 800×200 fit_pad(400, 400) → resize=(400,100), canvas=(400,400)
        // distort mod-16:
        //   resize_to: round_nearest(400,16)=400 (already aligned), round_nearest(100,16)=96
        //   Width: canvas.width==old_resize.width (400==400) → canvas follows → 400
        //   Height: canvas.height(400) != old_resize.height(100) → pad mode → recenter
        //     placement.1 = (400-96)/2 = 152
        let (ideal, _) = Pipeline::new(800, 200)
            .fit_pad(400, 400)
            .output_limits(OutputLimits {
                align: Some(Align::Distort(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(400, 96));
        assert_eq!(ideal.layout.canvas.width, 400); // width: non-pad, tracks resize
        assert_eq!(ideal.layout.canvas.height, 400); // height: pad mode, stays
        assert_eq!(ideal.layout.placement.1, 152); // recentered: (400-96)/2
    }

    #[test]
    fn limits_crop_align_larger_than_canvas() {
        // 3×3 fit(3, 3) → resize=3×3, canvas=3×3
        // crop mod-16: (3/16).max(1)*16 = 1*16 = 16
        // Crop with align > canvas rounds UP to 16 (by design, prevents zero)
        let (ideal, _) = Pipeline::new(3, 3)
            .fit(3, 3)
            .output_limits(OutputLimits {
                align: Some(Align::Crop(16, 16)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(16, 16));
        // resize_to clamped to canvas
        assert_eq!(ideal.layout.resize_to, Size::new(3, 3));
    }

    // ── RegionCoord ────────────────────────────────────────────────────

    #[test]
    fn region_coord_px() {
        let c = RegionCoord::px(100);
        assert_eq!(c.resolve(1000), 100);
    }

    #[test]
    fn region_coord_px_negative() {
        let c = RegionCoord::px(-50);
        assert_eq!(c.resolve(1000), -50);
    }

    #[test]
    fn region_coord_pct() {
        let c = RegionCoord::pct(0.5);
        assert_eq!(c.resolve(1000), 500);
    }

    #[test]
    fn region_coord_pct_px() {
        let c = RegionCoord::pct_px(1.0, 50);
        assert_eq!(c.resolve(1000), 1050);
    }

    #[test]
    fn region_coord_pct_px_negative() {
        let c = RegionCoord::pct_px(0.0, -100);
        assert_eq!(c.resolve(500), -100);
    }

    // ── Region: pure crop ──────────────────────────────────────────────

    #[test]
    fn region_pure_crop() {
        // Viewport inside source: pure crop
        let (ideal, _) = Pipeline::new(800, 600)
            .region(Region::crop(100, 50, 500, 350))
            .plan()
            .unwrap();
        // Viewport is 400×300
        assert_eq!(ideal.layout.canvas, Size::new(400, 300));
        assert_eq!(ideal.layout.resize_to, Size::new(400, 300));
        assert_eq!(ideal.layout.placement, (0, 0));
        // Source crop should be set
        assert!(ideal.layout.source_crop.is_some());
        let crop = ideal.layout.source_crop.unwrap();
        assert_eq!(crop, Rect::new(100, 50, 400, 300));
    }

    #[test]
    fn region_full_source() {
        // Viewport exactly matches source: no crop, no pad
        let (ideal, _) = Pipeline::new(800, 600)
            .region(Region::crop(0, 0, 800, 600))
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(800, 600));
        assert_eq!(ideal.layout.resize_to, Size::new(800, 600));
        assert!(ideal.layout.source_crop.is_none());
        assert_eq!(ideal.layout.placement, (0, 0));
    }

    // ── Region: pure padding ───────────────────────────────────────────

    #[test]
    fn region_pure_padding() {
        // Viewport extends beyond source on all sides
        let (ideal, _) = Pipeline::new(800, 600)
            .region(Region::padded(50, CanvasColor::white()))
            .plan()
            .unwrap();
        // Canvas = source + 2*50 on each side
        assert_eq!(ideal.layout.canvas, Size::new(900, 700));
        // Content is the full source
        assert_eq!(ideal.layout.resize_to, Size::new(800, 600));
        // Source content placed at (50, 50)
        assert_eq!(ideal.layout.placement, (50, 50));
        // No crop needed (full source visible)
        assert!(ideal.layout.source_crop.is_none());
    }

    // ── Region: mixed crop + pad ───────────────────────────────────────

    #[test]
    fn region_mixed_crop_pad() {
        // Viewport extends left and crops right:
        // left=-50, top=0, right=600, bottom=600
        // On 800×600 source:
        //   overlap: [0,800) ∩ [-50,600) = [0,600), [0,600) ∩ [0,600) = [0,600)
        //   So overlap x=[0..600), y=[0..600)
        //   viewport width = 650, height = 600
        //   place_x = 0 - (-50) = 50, place_y = 0
        let (ideal, _) = Pipeline::new(800, 600)
            .region_viewport(-50, 0, 600, 600, CanvasColor::black())
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(650, 600));
        assert_eq!(ideal.layout.resize_to, Size::new(600, 600));
        assert_eq!(ideal.layout.placement, (50, 0));
        let crop = ideal.layout.source_crop.unwrap();
        assert_eq!(crop, Rect::new(0, 0, 600, 600));
    }

    // ── Region: blank canvas ───────────────────────────────────────────

    #[test]
    fn region_blank_canvas() {
        let (ideal, _) = Pipeline::new(800, 600)
            .region_blank(400, 300, CanvasColor::white())
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(400, 300));
        assert_eq!(ideal.layout.resize_to, Size::new(400, 300));
        // No source crop (blank canvas, no overlap)
        assert!(ideal.layout.source_crop.is_none());
    }

    // ── Region + constraint ────────────────────────────────────────────

    #[test]
    fn region_crop_with_constraint() {
        // Region crops to 400×300, then Fit to 200×150
        let (ideal, _) = Pipeline::new(800, 600)
            .region(Region::crop(100, 50, 500, 350))
            .fit(200, 150)
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(200, 150));
        assert_eq!(ideal.layout.canvas, Size::new(200, 150));
    }

    #[test]
    fn region_pad_with_constraint() {
        // Region pads 50px around 800×600 source → viewport 900×700
        // Then Fit to 450×350: constraint targets viewport 900×700.
        // Fit 900×700 into 450×350 → 450×350 (scale = 450/900 = 0.5)
        // Wait: 900/700 = 1.286, 450/350 = 1.286. Same aspect → exact fit.
        // scale_x = 450/900 = 0.5, scale_y = 350/700 = 0.5
        // Content 800×600 resizes by 0.5 → 400×300.
        // Placement = 50 * 0.5 = 25px on each side.
        let (ideal, _) = Pipeline::new(800, 600)
            .region(Region::padded(50, CanvasColor::white()))
            .fit(450, 350)
            .plan()
            .unwrap();
        // Constraint targets viewport 900×700, fit to 450×350 → 450×350
        assert_eq!(ideal.layout.canvas, Size::new(450, 350));
        // Content = 800 * 0.5 × 600 * 0.5 = 400×300
        assert_eq!(ideal.layout.resize_to, Size::new(400, 300));
        // Placement = 50 * 0.5 = 25
        assert_eq!(ideal.layout.placement, (25, 25));
    }

    // ── Region with percentages ────────────────────────────────────────

    #[test]
    fn region_percentage_coords() {
        // 10% crop from each edge on 1000×500
        let reg = Region {
            left: RegionCoord::pct(0.1),
            top: RegionCoord::pct(0.1),
            right: RegionCoord::pct(0.9),
            bottom: RegionCoord::pct(0.9),
            color: CanvasColor::Transparent,
        };
        let (ideal, _) = Pipeline::new(1000, 500).region(reg).plan().unwrap();
        // 10% of 1000 = 100, 10% of 500 = 50
        // Viewport: 100..900 × 50..450 = 800×400
        assert_eq!(ideal.layout.canvas, Size::new(800, 400));
        assert_eq!(ideal.layout.resize_to, Size::new(800, 400));
        let crop = ideal.layout.source_crop.unwrap();
        assert_eq!(crop, Rect::new(100, 50, 800, 400));
    }

    // ── Region with pct_px ─────────────────────────────────────────────

    #[test]
    fn region_pct_px_coords() {
        // right = 100% + 20px, bottom = 100% + 20px (20px padding on right/bottom)
        let reg = Region {
            left: RegionCoord::px(0),
            top: RegionCoord::px(0),
            right: RegionCoord::pct_px(1.0, 20),
            bottom: RegionCoord::pct_px(1.0, 20),
            color: CanvasColor::black(),
        };
        let (ideal, _) = Pipeline::new(400, 300).region(reg).plan().unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(420, 320));
        assert_eq!(ideal.layout.resize_to, Size::new(400, 300));
        assert_eq!(ideal.layout.placement, (0, 0));
    }

    // ── Region + limits ────────────────────────────────────────────────

    #[test]
    fn region_with_max_limits() {
        // Region pads to 1000×800, limit max to 500×400
        let (ideal, _) = Pipeline::new(800, 600)
            .region(Region::padded(100, CanvasColor::white()))
            .output_limits(OutputLimits {
                max: Some(Size::new(500, 400)),
                ..Default::default()
            })
            .plan()
            .unwrap();
        // Canvas should be within 500×400
        assert!(ideal.layout.canvas.width <= 500);
        assert!(ideal.layout.canvas.height <= 400);
    }

    // ── Region mutually exclusive with Crop ─────────────────────────

    #[test]
    fn region_first_wins_over_crop() {
        // Region set first, crop ignored
        let commands = [
            Command::Region(Region::crop(0, 0, 400, 300)),
            Command::Crop(SourceCrop::pixels(100, 100, 200, 200)),
        ];
        let (ideal, _) = compute_layout(&commands, 800, 600, None).unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(400, 300));
    }

    #[test]
    fn crop_first_wins_over_region() {
        // Crop set first, region ignored
        let commands = [
            Command::Crop(SourceCrop::pixels(100, 100, 200, 200)),
            Command::Region(Region::crop(0, 0, 800, 600)),
        ];
        let (ideal, _) = compute_layout(&commands, 800, 600, None).unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(200, 200));
    }

    // ── SourceCrop::to_region equivalence ──────────────────────────────

    #[test]
    fn source_crop_to_region_pixels() {
        let crop = SourceCrop::pixels(100, 50, 400, 300);
        let region = crop.to_region();
        // Region from crop should produce same layout as crop directly
        let (crop_layout, _) = Pipeline::new(800, 600).crop(crop).plan().unwrap();
        let (region_layout, _) = Pipeline::new(800, 600).region(region).plan().unwrap();
        assert_eq!(crop_layout.layout.canvas, region_layout.layout.canvas);
        assert_eq!(crop_layout.layout.resize_to, region_layout.layout.resize_to);
        assert_eq!(
            crop_layout.layout.source_crop,
            region_layout.layout.source_crop
        );
    }

    #[test]
    fn source_crop_to_region_percent() {
        let crop = SourceCrop::percent(0.1, 0.1, 0.8, 0.8);
        let region = crop.to_region();
        let (crop_layout, _) = Pipeline::new(1000, 500).crop(crop).plan().unwrap();
        let (region_layout, _) = Pipeline::new(1000, 500).region(region).plan().unwrap();
        assert_eq!(crop_layout.layout.canvas, region_layout.layout.canvas);
        assert_eq!(crop_layout.layout.resize_to, region_layout.layout.resize_to);
    }

    // ── Region zero dimension rejected ─────────────────────────────────

    #[test]
    fn region_zero_width_rejected() {
        let result = Pipeline::new(800, 600)
            .region(Region::crop(100, 0, 100, 600))
            .plan();
        assert!(result.is_err());
    }

    #[test]
    fn region_negative_dimension_rejected() {
        let result = Pipeline::new(800, 600)
            .region(Region::crop(500, 0, 100, 600))
            .plan();
        assert!(result.is_err());
    }

    // ── Pipeline region convenience methods ─────────────────────────────

    #[test]
    fn pipeline_region_viewport() {
        let (ideal, _) = Pipeline::new(800, 600)
            .region_viewport(100, 100, 700, 500, CanvasColor::Transparent)
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(600, 400));
    }

    #[test]
    fn pipeline_region_pad() {
        let (ideal, _) = Pipeline::new(800, 600)
            .region_pad(30, CanvasColor::white())
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(860, 660));
        assert_eq!(ideal.layout.placement, (30, 30));
    }

    #[test]
    fn pipeline_region_blank() {
        let (ideal, _) = Pipeline::new(800, 600)
            .region_blank(200, 100, CanvasColor::black())
            .plan()
            .unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(200, 100));
        assert!(ideal.layout.source_crop.is_none());
    }

    // ── Sequential mode ────────────────────────────────────────────────

    #[test]
    fn sequential_orient_fuses() {
        // Orient commands compose regardless of position
        let commands = [
            Command::AutoOrient(6), // Rotate90
            Command::Crop(SourceCrop::pixels(0, 0, 600, 800)),
            Command::Rotate(Rotation::Rotate90), // + Rotate90 = Rotate180
        ];
        let (ideal, _) = compute_layout_sequential(&commands, 800, 600, None).unwrap();
        assert_eq!(ideal.orientation, Orientation::Rotate180);
    }

    #[test]
    fn sequential_matches_fixed_canonical_order() {
        // When commands are in canonical order with no duplicates,
        // sequential should match fixed mode
        let fixed = Pipeline::new(800, 600)
            .auto_orient(6)
            .crop_pixels(50, 50, 500, 700)
            .fit(300, 300)
            .plan()
            .unwrap();

        let commands = [
            Command::AutoOrient(6),
            Command::Crop(SourceCrop::pixels(50, 50, 500, 700)),
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 300, 300)),
        ];
        let sequential = compute_layout_sequential(&commands, 800, 600, None).unwrap();

        assert_eq!(fixed.0.layout.resize_to, sequential.0.layout.resize_to);
        assert_eq!(fixed.0.layout.canvas, sequential.0.layout.canvas);
        assert_eq!(fixed.0.orientation, sequential.0.orientation);
    }

    #[test]
    fn sequential_multiple_crops_compose() {
        // Second crop refines the first in sequential mode
        let commands = [
            Command::Crop(SourceCrop::pixels(100, 100, 600, 400)), // crop to 600×400
            Command::Crop(SourceCrop::pixels(50, 50, 500, 300)),   // refine: 50,50 within 600×400
        ];
        let (ideal, _) = compute_layout_sequential(&commands, 800, 600, None).unwrap();
        // Second crop is relative to first's viewport (600×400)
        // Position in source: first starts at (100,100), second at (50,50) within that
        // → source position (150, 150), size 500×300
        assert_eq!(ideal.layout.canvas, Size::new(500, 300));
        let crop = ideal.layout.source_crop.unwrap();
        assert_eq!(crop.x, 150);
        assert_eq!(crop.y, 150);
        assert_eq!(crop.width, 500);
        assert_eq!(crop.height, 300);
    }

    #[test]
    fn sequential_last_constrain_wins() {
        // In sequential mode, the last constraint wins
        let commands = [
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 100, 100)),
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 500, 500)),
        ];
        let (ideal, _) = compute_layout_sequential(&commands, 800, 600, None).unwrap();
        // Last constraint (500×500) wins: fit 800×600 into 500×500 → 500×375
        assert_eq!(ideal.layout.resize_to, Size::new(500, 375));
    }

    #[test]
    fn sequential_post_constrain_pad() {
        // Pad after constrain expands the canvas
        let commands = [
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 400, 300)),
            Command::Pad(Padding::uniform(10, CanvasColor::white())),
        ];
        let (ideal, _) = compute_layout_sequential(&commands, 800, 600, None).unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(400, 300));
        assert_eq!(ideal.layout.canvas, Size::new(420, 320));
        assert_eq!(ideal.layout.placement, (10, 10));
    }

    #[test]
    fn sequential_post_constrain_crop() {
        // Crop after constrain trims the canvas
        let commands = [
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 400, 300)),
            Command::Crop(SourceCrop::pixels(50, 50, 300, 200)),
        ];
        let (ideal, _) = compute_layout_sequential(&commands, 800, 600, None).unwrap();
        assert_eq!(ideal.layout.resize_to, Size::new(400, 300));
        assert_eq!(ideal.layout.canvas, Size::new(300, 200));
    }

    #[test]
    fn sequential_crop_constrain_pad_crop() {
        // crop → constrain → pad → crop (post-constrain trim)
        let commands = [
            Command::Crop(SourceCrop::pixels(0, 0, 400, 300)),
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 200, 150)),
            Command::Pad(Padding::uniform(20, CanvasColor::black())),
            Command::Crop(SourceCrop::pixels(10, 10, 220, 170)),
        ];
        let (ideal, _) = compute_layout_sequential(&commands, 800, 600, None).unwrap();
        // Crop to 400×300, fit to 200×150, pad 20 all sides → 240×190
        // Post-constrain crop: 10,10,220,170 → canvas=220×170
        assert_eq!(ideal.layout.canvas, Size::new(220, 170));
    }

    #[test]
    fn sequential_with_limits() {
        let commands = [Command::Constrain(Constraint::new(
            ConstraintMode::Fit,
            2000,
            2000,
        ))];
        let limits = OutputLimits {
            max: Some(Size::new(500, 500)),
            ..Default::default()
        };
        let (ideal, _) = compute_layout_sequential(&commands, 800, 600, Some(&limits)).unwrap();
        assert!(ideal.layout.canvas.width <= 500);
        assert!(ideal.layout.canvas.height <= 500);
    }

    #[test]
    fn sequential_empty_commands() {
        let (ideal, _) = compute_layout_sequential(&[], 800, 600, None).unwrap();
        assert_eq!(ideal.layout.canvas, Size::new(800, 600));
        assert_eq!(ideal.layout.resize_to, Size::new(800, 600));
        assert_eq!(ideal.orientation, Orientation::Identity);
    }

    #[test]
    fn sequential_via_free_function() {
        let commands = [
            Command::AutoOrient(6),
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 300, 300)),
        ];
        let (ideal, _) = compute_layout_sequential(&commands, 800, 600, None).unwrap();
        assert_eq!(ideal.orientation, Orientation::Rotate90);
        // 600×800 oriented, fit 300×300 → 225×300
        assert_eq!(ideal.layout.resize_to, Size::new(225, 300));
    }

    #[test]
    fn sequential_post_constrain_region() {
        // Region after constrain redefines the canvas viewport
        let commands = [
            Command::Constrain(Constraint::new(ConstraintMode::Fit, 400, 300)),
            Command::Region(Region {
                left: RegionCoord::px(-20),
                top: RegionCoord::px(-20),
                right: RegionCoord::pct_px(1.0, 20),
                bottom: RegionCoord::pct_px(1.0, 20),
                color: CanvasColor::white(),
            }),
        ];
        let (ideal, _) = compute_layout_sequential(&commands, 800, 600, None).unwrap();
        // Canvas should be expanded by 40 in each direction
        assert_eq!(ideal.layout.canvas, Size::new(440, 340));
    }
}
