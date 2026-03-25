//! Top-level orchestration for the zenode-to-pixel pipeline.
//!
//! Takes a decoded [`Source`] and a list of [`zennode::NodeInstance`] objects,
//! compiles them into a streaming pixel pipeline, executes it, and returns
//! a [`ProcessedImage`] with materialized pixels ready for encoding.
//!
//! This module handles the **pixel processing core** only. The caller is
//! responsible for:
//! - Probing the source image (to build [`SourceImageInfo`])
//! - Decoding the source into a [`Source`]
//! - Resolving the output format (via zencodecs `FormatDecision`)
//! - Building the actual encoder
//! - Passing the sidecar to the encoder
//!
//! # Example
//!
//! ```ignore
//! use zenpipe::orchestrate::{ProcessConfig, SourceImageInfo, process};
//! use zenpipe::format::RGBA8_SRGB;
//!
//! let config = ProcessConfig {
//!     nodes: &nodes,
//!     converters: &[],
//!     source_info: &SourceImageInfo {
//!         width: 4000,
//!         height: 3000,
//!         format: RGBA8_SRGB,
//!         has_alpha: false,
//!         has_animation: false,
//!         has_gain_map: false,
//!         is_hdr: false,
//!         exif_orientation: 1,
//!         metadata: None,
//!     },
//! };
//!
//! let result = process(decoded_source, &config)?;
//! // result.primary is a MaterializedSource ready for the encoder
//! // result.encode_config has quality/format settings
//! ```

use alloc::boxed::Box;

use crate::Source;
use crate::bridge::{self, CompileResult, DecodeConfig, EncodeConfig, NodeConverter};
use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::sidecar::{ProcessedSidecar, SidecarPlan, SidecarStream};
use crate::sources::MaterializedSource;

use zenresize::{Filter, Size};

// ─── Configuration ───

/// Configuration for image processing via the orchestration layer.
///
/// Groups the zenode node list, extension converters, and source metadata
/// needed to compile and execute a pixel pipeline.
pub struct ProcessConfig<'a> {
    /// Zenode node instances defining the processing pipeline.
    ///
    /// Only pixel-processing and encode-phase nodes. Decode is not a node —
    /// it's handled by the caller before invoking process().
    pub nodes: &'a [Box<dyn zennode::NodeInstance>],

    /// Extension converters for crate-specific nodes (zenfilters, etc.).
    pub converters: &'a [&'a dyn NodeConverter],

    /// HDR gain map handling mode. Set by the job/caller, not by a node.
    ///
    /// "sdr_only" — ignore gain map (default).
    /// "hdr_reconstruct" — apply gain map to produce HDR.
    /// "preserve" — track gain map as sidecar for re-embedding.
    pub hdr_mode: &'a str,

    /// Source image metadata from probing (dimensions, format, supplements).
    pub source_info: &'a SourceImageInfo,
}

/// Probed source image information.
///
/// Populated by the caller from decoder probing / header parsing before
/// calling [`process()`]. The orchestration layer uses this for:
/// - Sidecar derivation (gain map geometry)
/// - HDR mode decisions
/// - Metadata passthrough to the encoder
#[derive(Clone, Debug)]
pub struct SourceImageInfo {
    /// Source image width in pixels.
    pub width: u32,
    /// Source image height in pixels.
    pub height: u32,
    /// Pixel format of the decoded source.
    pub format: PixelFormat,
    /// Whether the source has an alpha channel.
    pub has_alpha: bool,
    /// Whether the source is an animated image (GIF, APNG, animated WebP).
    ///
    /// Animated sources should be processed via the `animation` module
    /// instead of this orchestration layer.
    pub has_animation: bool,
    /// Whether the source has a gain map (UltraHDR / ISO 21496-1).
    pub has_gain_map: bool,
    /// Whether the source is HDR content (PQ/HLG transfer function).
    pub is_hdr: bool,
    /// EXIF orientation tag (1-8). 1 = identity (no rotation).
    pub exif_orientation: u8,
    /// Metadata to pass through to the encoder (ICC, EXIF, XMP, CICP, HDR).
    ///
    /// `None` if no metadata preservation is needed.
    pub metadata: Option<zencodec::Metadata>,
}

// ─── Output ───

/// A processed image ready for encoding.
///
/// Contains the materialized primary image, optional sidecar (gain map),
/// extracted decode/encode configuration, and metadata for passthrough.
///
/// The caller uses `encode_config` to configure the encoder and passes
/// `metadata` through for ICC/EXIF/XMP preservation.
pub struct ProcessedImage {
    /// The processed primary image (materialized for random-access encoding).
    pub primary: MaterializedSource,

    /// Processed sidecar (gain map), if the source had one and HDR mode allows it.
    pub sidecar: Option<ProcessedSidecar>,

    /// Decode configuration extracted from nodes.
    ///
    /// Contains `hdr_mode`, `color_intent`, and `min_size` settings.
    /// The caller may have already used these to configure the decoder;
    /// included here for completeness.
    pub decode_config: DecodeConfig,

    /// Encode configuration extracted from nodes.
    ///
    /// Contains quality profile, format preference, DPR, lossless flag,
    /// and optional codec-specific params.
    pub encode_config: EncodeConfig,

    /// Metadata to pass through to the encoder.
    ///
    /// Cloned from [`SourceImageInfo::metadata`]. The caller should pass
    /// this to the encoder for ICC/EXIF/XMP/CICP preservation.
    pub metadata: Option<zencodec::Metadata>,
}

impl ProcessedImage {
    /// Width of the processed primary image.
    pub fn width(&self) -> u32 {
        self.primary.width()
    }

    /// Height of the processed primary image.
    pub fn height(&self) -> u32 {
        self.primary.height()
    }

    /// Pixel format of the processed primary image.
    pub fn format(&self) -> PixelFormat {
        self.primary.format()
    }
}

// ─── Public API ───

/// Process a decoded source through the zenode pipeline.
///
/// Compiles the node list into a streaming pixel pipeline, executes it,
/// and materializes the result. If the source has a gain map and HDR mode
/// permits, the sidecar is also processed in lockstep.
///
/// # Steps
///
/// 1. Compile nodes via [`bridge::compile_nodes()`] — separates decode/encode
///    nodes, coalesces fusable groups, builds [`PipelineGraph`].
/// 2. Wire the provided `source` into the graph's `Source` node.
/// 3. Compile the graph into an executable [`Source`] chain.
/// 4. Materialize the result (pull all strips into [`MaterializedSource`]).
/// 5. If sidecar is provided and `config.hdr_mode != "sdr_only"`,
///    derive proportional transforms, compile, and materialize the sidecar.
/// 6. Return [`ProcessedImage`] with primary + sidecar + configs + metadata.
///
/// # Arguments
///
/// * `source` — Decoded pixel source (the caller has already decoded the image).
/// * `config` — Processing configuration (nodes, converters, source info).
///
/// # Sidecar handling
///
/// Pass the sidecar source via [`process_with_sidecar()`] if you have one.
/// This function does not process sidecars.
///
/// # Errors
///
/// Returns [`PipeError`] if node compilation, graph compilation, or
/// pipeline execution fails.
pub fn process(
    source: Box<dyn Source>,
    config: &ProcessConfig<'_>,
) -> Result<ProcessedImage, PipeError> {
    process_with_sidecar(source, config, None)
}

/// Process a decoded source with an optional sidecar (gain map) stream.
///
/// Like [`process()`], but accepts a [`SidecarStream`] that will be
/// processed in lockstep with the primary image. The sidecar receives
/// proportional geometry transforms (crop, resize, orientation) derived
/// from the primary pipeline.
///
/// # Sidecar processing
///
/// When `sidecar` is `Some` and `config.hdr_mode != "sdr_only"`:
/// 1. The primary's ideal layout is computed from the compiled graph.
/// 2. [`SidecarPlan::derive()`] computes proportional transforms.
/// 3. The sidecar pipeline is compiled and materialized.
/// 4. The result is wrapped in [`ProcessedSidecar`].
///
/// When `sidecar` is `None` or `hdr_mode == "sdr_only"`, the sidecar
/// field in the result is `None`.
pub fn process_with_sidecar(
    source: Box<dyn Source>,
    config: &ProcessConfig<'_>,
    sidecar: Option<SidecarStream>,
) -> Result<ProcessedImage, PipeError> {
    // 1. Compile nodes: separate decode/encode, coalesce, build graph.
    let CompileResult {
        graph,
        decode_config,
        encode_config,
        ..
    } = bridge::compile_nodes(config.nodes, config.converters, config.source_info.width, config.source_info.height)?;

    // 2. Wire the source into the graph's Source node (always at index 0).
    let mut sources = hashbrown::HashMap::new();
    sources.insert(0, source);

    // 3. Compile the graph into an executable Source chain.
    let pipeline = graph.compile(sources)?;

    // 4. Materialize the primary image.
    let primary = MaterializedSource::from_source(pipeline)?;

    // 5. Process sidecar if present and HDR mode allows it.
    let processed_sidecar = if let Some(sidecar_stream) = sidecar {
        if config.hdr_mode != "sdr_only" {
            Some(process_sidecar(
                sidecar_stream,
                config.source_info,
                &primary,
            )?)
        } else {
            None
        }
    } else {
        None
    };

    // 6. Return the processed image.
    Ok(ProcessedImage {
        primary,
        sidecar: processed_sidecar,
        decode_config,
        encode_config,
        metadata: config.source_info.metadata.clone(),
    })
}

// ─── Sidecar processing ───

/// Process a sidecar stream to match the primary image's geometry.
///
/// Derives proportional transforms from the primary's output dimensions
/// relative to the source dimensions, then compiles and materializes
/// the sidecar pipeline.
fn process_sidecar(
    sidecar_stream: SidecarStream,
    source_info: &SourceImageInfo,
    primary: &MaterializedSource,
) -> Result<ProcessedSidecar, PipeError> {
    let primary_source = Size::new(source_info.width, source_info.height);
    let sidecar_source = Size::new(sidecar_stream.width, sidecar_stream.height);

    // Build a trivial ideal layout from source → primary output dimensions.
    // The primary has already been processed, so we derive the sidecar's
    // transforms from the ratio of source-to-output dimensions.
    let primary_output = Size::new(primary.width(), primary.height());

    let (primary_ideal, _request) = zenresize::Pipeline::new(source_info.width, source_info.height)
        .fit(primary_output.width, primary_output.height)
        .plan()
        .map_err(|e| PipeError::Op(alloc::format!("sidecar layout plan failed: {e}")))?;

    // Derive proportional transforms for the sidecar.
    let plan = SidecarPlan::derive(
        &primary_ideal,
        primary_source,
        sidecar_source,
        None, // auto-scale: maintain primary-to-sidecar source ratio
        Filter::Robidoux,
    );

    // Compile and materialize the sidecar pipeline.
    let compiled = plan.compile(sidecar_stream.source)?;
    let materialized = MaterializedSource::from_source(compiled)?;

    Ok(ProcessedSidecar::new(materialized, sidecar_stream.kind))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::RGBA8_SRGB;
    use crate::sidecar::{SidecarKind, SidecarStream};

    struct SolidSource { w: u32, h: u32, y: u32 }
    impl SolidSource {
        fn new(w: u32, h: u32) -> Self { Self { w, h, y: 0 } }
    }
    impl crate::Source for SolidSource {
        fn next(&mut self) -> Result<Option<crate::Strip<'_>>, PipeError> {
            if self.y >= self.h { return Ok(None); }
            let rows = 16.min(self.h - self.y);
            let stride = RGBA8_SRGB.aligned_stride(self.w);
            let data = alloc::vec![128u8; stride * rows as usize];
            self.y += rows;
            let leaked: &'static [u8] = alloc::vec::Vec::leak(data);
            Ok(Some(crate::strip::Strip::new(leaked, self.w, rows, stride, RGBA8_SRGB)?))
        }
        fn width(&self) -> u32 { self.w }
        fn height(&self) -> u32 { self.h }
        fn format(&self) -> PixelFormat { RGBA8_SRGB }
    }

    fn default_source_info(w: u32, h: u32) -> SourceImageInfo {
        SourceImageInfo {
            width: w, height: h, format: RGBA8_SRGB,
            has_alpha: false, has_animation: false, has_gain_map: false,
            is_hdr: false, exif_orientation: 1, metadata: None,
        }
    }

    #[test]
    fn passthrough_no_nodes() {
        let source = Box::new(SolidSource::new(200, 150));
        let info = default_source_info(200, 150);
        let config = ProcessConfig {
            nodes: &[], converters: &[], source_info: &info, hdr_mode: "sdr_only",
        };
        let result = process(source, &config).unwrap();
        assert_eq!(result.width(), 200);
        assert_eq!(result.height(), 150);
        assert!(result.sidecar.is_none());
    }

    #[test]
    fn passthrough_preserves_data() {
        let source = Box::new(SolidSource::new(64, 64));
        let info = default_source_info(64, 64);
        let config = ProcessConfig {
            nodes: &[], converters: &[], source_info: &info, hdr_mode: "sdr_only",
        };
        let result = process(source, &config).unwrap();
        assert!(!result.primary.data().is_empty());
    }

    #[test]
    fn default_encode_config() {
        let source = Box::new(SolidSource::new(100, 100));
        let info = default_source_info(100, 100);
        let config = ProcessConfig {
            nodes: &[], converters: &[], source_info: &info, hdr_mode: "sdr_only",
        };
        let result = process(source, &config).unwrap();
        assert!(result.encode_config.quality_profile.is_none());
        assert!(result.encode_config.format.is_none());
        assert_eq!(result.encode_config.dpr, 1.0);
    }

    #[test]
    fn metadata_passed_through() {
        let source = Box::new(SolidSource::new(100, 100));
        let info = SourceImageInfo {
            metadata: Some(zencodec::Metadata::default()),
            ..default_source_info(100, 100)
        };
        let config = ProcessConfig {
            nodes: &[], converters: &[], source_info: &info, hdr_mode: "sdr_only",
        };
        let result = process(source, &config).unwrap();
        assert!(result.metadata.is_some());
    }

    #[test]
    fn metadata_none_when_not_provided() {
        let source = Box::new(SolidSource::new(100, 100));
        let info = default_source_info(100, 100);
        let config = ProcessConfig {
            nodes: &[], converters: &[], source_info: &info, hdr_mode: "sdr_only",
        };
        let result = process(source, &config).unwrap();
        assert!(result.metadata.is_none());
    }

    #[test]
    fn sidecar_skipped_when_sdr_only() {
        let source = Box::new(SolidSource::new(400, 300));
        let info = SourceImageInfo {
            has_gain_map: true,
            ..default_source_info(400, 300)
        };
        let config = ProcessConfig {
            nodes: &[], converters: &[], source_info: &info, hdr_mode: "sdr_only",
        };
        let sidecar = SidecarStream {
            source: Box::new(SolidSource::new(100, 75)),
            width: 100, height: 75,
            kind: SidecarKind::GainMap { params: zencodec::GainMapParams::default() },
        };
        let result = process_with_sidecar(source, &config, Some(sidecar)).unwrap();
        assert!(result.sidecar.is_none()); // sdr_only skips sidecar
    }

    #[test]
    fn sidecar_processed_when_preserve() {
        let source = Box::new(SolidSource::new(400, 300));
        let info = SourceImageInfo {
            has_gain_map: true,
            ..default_source_info(400, 300)
        };
        let config = ProcessConfig {
            nodes: &[], converters: &[], source_info: &info, hdr_mode: "preserve",
        };
        let sidecar = SidecarStream {
            source: Box::new(SolidSource::new(100, 75)),
            width: 100, height: 75,
            kind: SidecarKind::GainMap { params: zencodec::GainMapParams::default() },
        };
        let result = process_with_sidecar(source, &config, Some(sidecar)).unwrap();
        assert!(result.sidecar.is_some());
        let sc = result.sidecar.unwrap();
        assert_eq!(sc.width(), 100);
        assert_eq!(sc.height(), 75);
    }

    #[test]
    fn sidecar_none_when_not_provided() {
        let source = Box::new(SolidSource::new(400, 300));
        let info = default_source_info(400, 300);
        let config = ProcessConfig {
            nodes: &[], converters: &[], source_info: &info, hdr_mode: "preserve",
        };
        let result = process_with_sidecar(source, &config, None).unwrap();
        assert!(result.sidecar.is_none());
    }

    #[test]
    fn processed_image_accessors() {
        let source = Box::new(SolidSource::new(320, 240));
        let info = default_source_info(320, 240);
        let config = ProcessConfig {
            nodes: &[], converters: &[], source_info: &info, hdr_mode: "sdr_only",
        };
        let result = process(source, &config).unwrap();
        assert_eq!(result.width(), 320);
        assert_eq!(result.height(), 240);
        assert_eq!(result.format(), RGBA8_SRGB);
    }
}
