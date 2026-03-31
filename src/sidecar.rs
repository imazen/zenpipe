//! Sidecar auxiliary image tracking for gain maps and depth maps.
//!
//! A sidecar is a lower-resolution image that must stay spatially locked
//! with the primary image through geometry operations. When the primary
//! is cropped, resized, or oriented, the sidecar receives proportional
//! transforms computed via [`zenlayout::IdealLayout::derive_secondary()`].
//!
//! The sidecar runs as a separate mini-pipeline using the same
//! [`PipelineGraph`](crate::graph::PipelineGraph) infrastructure as the primary.

use alloc::boxed::Box;

use crate::Source;
#[allow(unused_imports)]
use whereat::at;

use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::graph::{EdgeKind, NodeOp, PipelineGraph};
use crate::sources::MaterializedSource;

use zencodec::GainMapParams;
use zenresize::{DecoderOffer, DecoderRequest, Filter, IdealLayout, LayoutPlan, Orientation, Size};

/// What kind of auxiliary data the sidecar carries.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum SidecarKind {
    /// ISO 21496-1 gain map for HDR/SDR adaptation.
    GainMap {
        /// Per-channel gain map parameters.
        params: GainMapParams,
    },
}

/// Pre-processing representation of a sidecar: source pixels + metadata,
/// before any geometry transforms have been applied.
pub struct SidecarStream {
    /// Pixel source for the sidecar image.
    pub source: Box<dyn Source>,
    /// Source image width.
    pub width: u32,
    /// Source image height.
    pub height: u32,
    /// What kind of sidecar this is.
    pub kind: SidecarKind,
}

/// A plan for transforming a sidecar to stay spatially locked with the primary.
///
/// Created by [`SidecarPlan::derive()`], then compiled with [`SidecarPlan::compile()`]
/// to produce an executable [`Source`] chain.
pub struct SidecarPlan {
    /// The derived layout for the sidecar (from `IdealLayout::derive_secondary()`).
    pub ideal: IdealLayout,
    /// What the sidecar decoder should do (crop, orientation, target size).
    pub request: DecoderRequest,
    /// Resampling filter to use for any resize step.
    pub filter: Filter,
    /// The finalized layout plan (computed from ideal + decoder offer).
    pub layout_plan: LayoutPlan,
}

impl SidecarPlan {
    /// Derive a sidecar plan from the primary image's ideal layout.
    ///
    /// # Arguments
    ///
    /// * `primary_ideal` — The primary image's computed ideal layout.
    /// * `primary_source` — Source dimensions of the primary image (pre-orientation).
    /// * `sidecar_source` — Source dimensions of the sidecar image.
    /// * `target` — Desired output size for the sidecar, or `None` to auto-scale
    ///   (maintaining the primary-to-sidecar source ratio).
    /// * `filter` — Resampling filter for any resize operation.
    pub fn derive(
        primary_ideal: &IdealLayout,
        primary_source: Size,
        sidecar_source: Size,
        target: Option<Size>,
        filter: Filter,
    ) -> Self {
        let (ideal, request) =
            primary_ideal.derive_secondary(primary_source, sidecar_source, target);

        // Build a pass-through decoder offer: the sidecar decoder applied nothing.
        let offer = DecoderOffer::full_decode(sidecar_source.width, sidecar_source.height);
        let layout_plan = ideal.finalize(&request, &offer);

        Self {
            ideal,
            request,
            filter,
            layout_plan,
        }
    }

    /// Returns `true` if no geometry transforms are needed — the sidecar
    /// source can be used as-is without any crop, resize, or orientation.
    pub fn is_identity(&self) -> bool {
        self.layout_plan.resize_is_identity
            && self.layout_plan.remaining_orientation == Orientation::Identity
            && self.layout_plan.trim.is_none()
    }

    /// Compile the sidecar plan into an executable [`Source`] chain.
    ///
    /// If no transforms are needed ([`is_identity()`](Self::is_identity) is true),
    /// returns the source unchanged.
    ///
    /// Otherwise, builds a mini [`PipelineGraph`] with
    /// `Source → Layout → Output` and compiles it.
    pub fn compile(self, sidecar_source: Box<dyn Source>) -> crate::PipeResult<Box<dyn Source>> {
        if self.is_identity() {
            return Ok(sidecar_source);
        }

        let mut graph = PipelineGraph::new();
        let src = graph.add_node(NodeOp::Source);
        let layout = graph.add_node(NodeOp::Layout {
            plan: self.layout_plan,
            filter: self.filter,
        });
        let out = graph.add_node(NodeOp::Output);

        graph.add_edge(src, layout, EdgeKind::Input);
        graph.add_edge(layout, out, EdgeKind::Input);

        let mut sources = hashbrown::HashMap::new();
        sources.insert(src, sidecar_source);

        graph.compile(sources)
    }
}

/// A fully materialized sidecar image with its associated metadata.
///
/// Created by executing a sidecar pipeline and draining the output
/// into a pixel buffer. The orchestration layer typically does:
///
/// ```ignore
/// let source = plan.compile(sidecar_source)?;
/// let mat = MaterializedSource::from_source(source)?;
/// let processed = ProcessedSidecar::new(mat, kind);
/// ```
pub struct ProcessedSidecar {
    /// The materialized pixel data.
    pub pixels: MaterializedSource,
    /// What kind of sidecar this is (gain map parameters, etc.).
    pub kind: SidecarKind,
}

impl ProcessedSidecar {
    /// Create a processed sidecar from materialized pixels and kind metadata.
    pub fn new(pixels: MaterializedSource, kind: SidecarKind) -> Self {
        Self { pixels, kind }
    }

    /// Width of the processed sidecar image.
    pub fn width(&self) -> u32 {
        self.pixels.width()
    }

    /// Height of the processed sidecar image.
    pub fn height(&self) -> u32 {
        self.pixels.height()
    }

    /// Pixel format of the processed sidecar.
    pub fn format(&self) -> PixelFormat {
        self.pixels.format()
    }

    /// Raw pixel data of the processed sidecar.
    pub fn data(&self) -> &[u8] {
        self.pixels.data()
    }

    /// Stride (bytes per row) of the processed sidecar.
    pub fn stride(&self) -> usize {
        self.pixels.stride()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::RGBA8_SRGB;
    use crate::strip::Strip;
    use zenresize::Pipeline;

    /// A test source that produces solid-color strips.
    struct SolidSource {
        width: u32,
        height: u32,
        format: PixelFormat,
        y: u32,
    }

    impl SolidSource {
        fn new(width: u32, height: u32) -> Self {
            Self {
                width,
                height,
                format: RGBA8_SRGB,
                y: 0,
            }
        }
    }

    impl Source for SolidSource {
        fn next(&mut self) -> crate::PipeResult<Option<Strip<'_>>> {
            use crate::strip::BufferResultExt as _;
            if self.y >= self.height {
                return Ok(None);
            }
            let rows = 16.min(self.height - self.y);
            let stride = self.format.aligned_stride(self.width);
            let data = alloc::vec![128u8; stride * rows as usize];
            self.y += rows;
            // Leak the data to get a 'static lifetime for testing.
            let leaked: &'static [u8] = alloc::vec::Vec::leak(data);
            Ok(Some(
                Strip::new(leaked, self.width, rows, stride, self.format).pipe_err()?,
            ))
        }

        fn width(&self) -> u32 {
            self.width
        }

        fn height(&self) -> u32 {
            self.height
        }

        fn format(&self) -> PixelFormat {
            self.format
        }
    }

    #[test]
    fn derive_no_commands_preserves_ratio() {
        // Primary: 4000x3000, sidecar: 1000x750 (1:4 ratio)
        // No commands -> sidecar should resize to maintain 1:4 of primary output.
        // Primary output with no commands = 4000x3000, so sidecar = 1000x750.
        let (primary_ideal, _) = Pipeline::new(4000, 3000).plan().unwrap();

        let plan = SidecarPlan::derive(
            &primary_ideal,
            Size::new(4000, 3000),
            Size::new(1000, 750),
            None,
            Filter::Robidoux,
        );

        // Auto target: primary output (4000x3000) * 0.25 = 1000x750
        // Sidecar source is 1000x750, target is 1000x750 -> identity
        assert!(plan.is_identity());
    }

    #[test]
    fn derive_with_resize_scales_proportionally() {
        // Primary: 4000x3000, fit to 800x600
        // Sidecar: 1000x750 (1:4 of primary source)
        let (primary_ideal, _) = Pipeline::new(4000, 3000).fit(800, 600).plan().unwrap();

        let plan = SidecarPlan::derive(
            &primary_ideal,
            Size::new(4000, 3000),
            Size::new(1000, 750),
            None,
            Filter::Robidoux,
        );

        // Auto target: 800 * 0.25 = 200, 600 * 0.25 = 150
        assert_eq!(plan.ideal.layout.resize_to, Size::new(200, 150));
        assert!(!plan.is_identity()); // needs resize from 1000x750 to 200x150
    }

    #[test]
    fn derive_with_crop_and_resize() {
        // Primary: 4000x3000, crop 100,100,2000,2000, fit 500x500
        let (primary_ideal, _) = Pipeline::new(4000, 3000)
            .crop_pixels(100, 100, 2000, 2000)
            .fit(500, 500)
            .plan()
            .unwrap();

        let plan = SidecarPlan::derive(
            &primary_ideal,
            Size::new(4000, 3000),
            Size::new(1000, 750),
            None,
            Filter::Robidoux,
        );

        // Should have a crop in the request (scaled from primary coords)
        assert!(plan.request.crop.is_some());
        let crop = plan.request.crop.unwrap();
        // 100/4 = 25, 100/4 = 25 (floors), 2000/4 = 500 (round outward)
        assert_eq!(crop.x, 25);
        assert_eq!(crop.y, 25); // 100 * (750/3000) = 25
    }

    #[test]
    fn derive_1_to_8_ratio() {
        // Primary: 8000x6000, fit to 800x600
        // Sidecar: 1000x750 (1:8 ratio)
        let (primary_ideal, _) = Pipeline::new(8000, 6000).fit(800, 600).plan().unwrap();

        let plan = SidecarPlan::derive(
            &primary_ideal,
            Size::new(8000, 6000),
            Size::new(1000, 750),
            None,
            Filter::Robidoux,
        );

        // Auto target: 800 * (1000/8000) = 100, 600 * (750/6000) = 75
        assert_eq!(plan.ideal.layout.resize_to, Size::new(100, 75));
        assert!(!plan.is_identity());
    }

    #[test]
    fn identity_detection_passthrough() {
        // When no transforms are needed, compile should return source unchanged.
        let (primary_ideal, _) = Pipeline::new(800, 600).plan().unwrap();

        let plan = SidecarPlan::derive(
            &primary_ideal,
            Size::new(800, 600),
            Size::new(200, 150),
            None,
            Filter::Robidoux,
        );

        assert!(plan.is_identity());

        let source = SolidSource::new(200, 150);
        let result = plan.compile(Box::new(source)).unwrap();
        assert_eq!(result.width(), 200);
        assert_eq!(result.height(), 150);
    }

    #[test]
    fn compile_with_resize_produces_correct_dims() {
        // Primary: 4000x3000, fit to 800x600
        // Sidecar: 1000x750 -> should resize to 200x150
        let (primary_ideal, _) = Pipeline::new(4000, 3000).fit(800, 600).plan().unwrap();

        let plan = SidecarPlan::derive(
            &primary_ideal,
            Size::new(4000, 3000),
            Size::new(1000, 750),
            None,
            Filter::Robidoux,
        );

        let source = SolidSource::new(1000, 750);
        let result = plan.compile(Box::new(source)).unwrap();
        assert_eq!(result.width(), 200);
        assert_eq!(result.height(), 150);
    }

    #[test]
    fn processed_sidecar_roundtrip() {
        // Derive + compile + materialize -> ProcessedSidecar
        let (primary_ideal, _) = Pipeline::new(4000, 3000).fit(800, 600).plan().unwrap();

        let plan = SidecarPlan::derive(
            &primary_ideal,
            Size::new(4000, 3000),
            Size::new(1000, 750),
            None,
            Filter::Robidoux,
        );

        let kind = SidecarKind::GainMap {
            params: GainMapParams::default(),
        };

        let source = SolidSource::new(1000, 750);
        let compiled = plan.compile(Box::new(source)).unwrap();
        let mat = MaterializedSource::from_source(compiled).unwrap();

        let processed = ProcessedSidecar::new(mat, kind);
        assert_eq!(processed.width(), 200);
        assert_eq!(processed.height(), 150);
        assert_eq!(processed.format(), RGBA8_SRGB);
        assert!(!processed.data().is_empty());
    }

    #[test]
    fn derive_with_orientation() {
        // Primary: 4000x3000, EXIF 6 (Rotate90), fit 800x800
        let (primary_ideal, _) = Pipeline::new(4000, 3000)
            .auto_orient(6)
            .fit(800, 800)
            .plan()
            .unwrap();

        let plan = SidecarPlan::derive(
            &primary_ideal,
            Size::new(4000, 3000),
            Size::new(1000, 750),
            None,
            Filter::Robidoux,
        );

        // Sidecar should also have the same orientation
        assert_eq!(plan.ideal.orientation, Orientation::Rotate90);
        assert_eq!(plan.request.orientation, Orientation::Rotate90);
    }

    #[test]
    fn derive_explicit_target_overrides_auto() {
        let (primary_ideal, _) = Pipeline::new(4000, 3000).fit(800, 600).plan().unwrap();

        let plan = SidecarPlan::derive(
            &primary_ideal,
            Size::new(4000, 3000),
            Size::new(1000, 750),
            Some(Size::new(400, 300)), // explicit target
            Filter::Robidoux,
        );

        // Explicit target: 400x300 (not auto 200x150)
        assert_eq!(plan.ideal.layout.resize_to, Size::new(400, 300));
    }
}
