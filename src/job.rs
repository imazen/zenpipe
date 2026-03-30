//! High-level bytes-in → bytes-out image processing job.
//!
//! [`ImageJob`] is the primary consumer-facing API for processing images.
//! It handles the full pipeline: **probe → decode → CMS → pipeline → encode**,
//! so consumers only need to provide input bytes, processing nodes, and
//! encoding intent.
//!
//! # Multi-IO support
//!
//! Jobs support multiple named I/O slots via integer `io_id` keys. Each slot
//! can be an input (image bytes) or an output (receives encoded bytes).
//! Non-image I/O slots (JSON metadata, watermark images, etc.) are also
//! supported.
//!
//! # Example
//!
//! ```ignore
//! use zenpipe::job::{ImageJob, IoSlot};
//! use zencodecs::CodecIntent;
//!
//! let result = ImageJob::new()
//!     .add_input(0, image_bytes)
//!     .add_output(1)
//!     .with_nodes(&nodes)
//!     .with_intent(CodecIntent::default())
//!     .run()?;
//!
//! let encoded = result.get_output(1).unwrap();
//! ```

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use hashbrown::HashMap;

use crate::Source;
use crate::bridge::NodeConverter;
use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::limits::Limits;

// ─── I/O slot model ───

/// An I/O slot in the job. Each slot is identified by an `io_id` (i32).
#[derive(Clone)]
pub enum IoSlot {
    /// Input bytes (image data, watermark, metadata, etc.).
    Input(Vec<u8>),
    /// Output placeholder — will be filled with encoded bytes after the job runs.
    Output,
}

/// Result of a single encode operation within a job.
#[derive(Clone, Debug)]
pub struct EncodeResult {
    /// The I/O ID this output was written to.
    pub io_id: i32,
    /// Encoded image bytes.
    pub bytes: Vec<u8>,
    /// Output width in pixels.
    pub width: u32,
    /// Output height in pixels.
    pub height: u32,
    /// MIME type of the encoded output (e.g., `"image/jpeg"`).
    pub mime_type: String,
    /// File extension (e.g., `"jpg"`).
    pub extension: String,
}

/// Information about a decoded input source.
#[derive(Clone, Debug)]
pub struct DecodeInfo {
    /// The I/O ID of the input.
    pub io_id: i32,
    /// Width of the source image.
    pub width: u32,
    /// Height of the source image.
    pub height: u32,
    /// Detected image format.
    pub format: zencodec::ImageFormat,
    /// Whether the source has meaningful alpha.
    pub has_alpha: bool,
    /// Whether the source is animated.
    pub has_animation: bool,
    /// Preferred MIME type.
    pub mime_type: String,
}

/// Result of a completed image processing job.
#[derive(Clone, Debug)]
pub struct JobResult {
    /// Encoded outputs, keyed by io_id.
    pub encode_results: Vec<EncodeResult>,
    /// Decode metadata for each input, keyed by io_id.
    pub decode_infos: Vec<DecodeInfo>,
}

// ─── CMS mode ───

/// Color management mode for the job.
///
/// Controls how ICC profiles are handled between decode and pipeline.
#[derive(Clone, Copy, Debug, Default)]
pub enum CmsMode {
    /// Convert to sRGB after decode using loose sRGB detection.
    /// Matches legacy imageflow v2 behavior.
    #[default]
    SrgbCompat,
    /// Convert to sRGB after decode using strict structural sRGB detection.
    SceneReferred,
    /// No CMS — preserve source color space.
    None,
}

// ─── Gain map mode ───

/// How to handle gain maps (UltraHDR / ISO 21496-1) during processing.
///
/// Gain maps enable HDR display from SDR base images. They're supported
/// by JPEG (UltraHDR/MPF), AVIF (tmap), and JXL (jhgm).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GainMapMode {
    /// Preserve the gain map through the pipeline (default).
    ///
    /// Extracts during decode, tracks geometry proportionally through
    /// resize/crop/orientation, re-embeds during encode. The gain map
    /// is automatically transcoded between formats (JPEG MPF → AVIF tmap → JXL jhgm).
    ///
    /// If the output format doesn't support gain maps (PNG, WebP, GIF),
    /// the gain map is silently dropped.
    #[default]
    Preserve,

    /// Discard the gain map. Output is SDR only.
    Discard,
}

// ─── Job builder ───

/// A high-level image processing job.
///
/// Combines zencodecs (probe/decode/encode) with zenpipe (pipeline execution)
/// into a single bytes-in → bytes-out operation.
///
/// Gain maps are preserved by default — no user action required. Set
/// `gain_map_mode(GainMapMode::Discard)` to strip them.
pub struct ImageJob<'a> {
    /// I/O slots keyed by io_id.
    io: HashMap<i32, IoSlot>,
    /// Processing nodes (pipeline definition).
    nodes: &'a [Box<dyn zennode::NodeInstance>],
    /// Extension converters for crate-specific nodes.
    converters: &'a [&'a dyn NodeConverter],
    /// Codec intent for format/quality selection.
    intent: zencodecs::CodecIntent,
    /// Color management mode.
    cms_mode: CmsMode,
    /// Gain map handling mode.
    gain_map_mode: GainMapMode,
    /// Resource limits.
    limits: Option<Limits>,
    /// Codec registry (which formats are enabled).
    registry: zencodecs::AllowedFormats,
    /// Codec config overrides.
    codec_config: Option<zencodecs::config::CodecConfig>,
    /// Pipeline trace config.
    trace_config: Option<&'a crate::trace::TraceConfig>,
    /// Primary decode io_id (which input to decode as the main image).
    decode_io_id: i32,
    /// Primary encode io_id (which output slot to write encoded result).
    encode_io_id: i32,
}

impl<'a> ImageJob<'a> {
    /// Create a new empty job with default settings.
    pub fn new() -> Self {
        Self {
            io: HashMap::new(),
            nodes: &[],
            converters: &[],
            intent: zencodecs::CodecIntent::default(),
            cms_mode: CmsMode::default(),
            gain_map_mode: GainMapMode::default(),
            limits: None,
            registry: zencodecs::AllowedFormats::all(),
            codec_config: None,
            trace_config: None,
            decode_io_id: 0,
            encode_io_id: 1,
        }
    }

    /// Add an input I/O slot.
    pub fn add_input(mut self, io_id: i32, data: Vec<u8>) -> Self {
        self.io.insert(io_id, IoSlot::Input(data));
        self
    }

    /// Add an input I/O slot by reference (clones the data).
    pub fn add_input_ref(mut self, io_id: i32, data: &[u8]) -> Self {
        self.io.insert(io_id, IoSlot::Input(data.to_vec()));
        self
    }

    /// Add an output I/O slot.
    pub fn add_output(mut self, io_id: i32) -> Self {
        self.io.insert(io_id, IoSlot::Output);
        self
    }

    /// Set the primary decode I/O id (default: 0).
    pub fn with_decode_io(mut self, io_id: i32) -> Self {
        self.decode_io_id = io_id;
        self
    }

    /// Set the primary encode I/O id (default: 1).
    pub fn with_encode_io(mut self, io_id: i32) -> Self {
        self.encode_io_id = io_id;
        self
    }

    /// Set the processing nodes.
    pub fn with_nodes(mut self, nodes: &'a [Box<dyn zennode::NodeInstance>]) -> Self {
        self.nodes = nodes;
        self
    }

    /// Set extension converters.
    pub fn with_converters(mut self, converters: &'a [&'a dyn NodeConverter]) -> Self {
        self.converters = converters;
        self
    }

    /// Set the codec intent (format/quality selection).
    pub fn with_intent(mut self, intent: zencodecs::CodecIntent) -> Self {
        self.intent = intent;
        self
    }

    /// Set the CMS mode.
    pub fn with_cms(mut self, mode: CmsMode) -> Self {
        self.cms_mode = mode;
        self
    }

    /// Set the gain map handling mode.
    ///
    /// Default: [`GainMapMode::Preserve`] — gain maps round-trip automatically.
    /// Use [`GainMapMode::Discard`] to strip gain maps from the output.
    pub fn with_gain_map_mode(mut self, mode: GainMapMode) -> Self {
        self.gain_map_mode = mode;
        self
    }

    /// Set resource limits.
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = Some(limits);
        self
    }

    /// Set the codec registry (enabled formats).
    pub fn with_registry(mut self, registry: zencodecs::AllowedFormats) -> Self {
        self.registry = registry;
        self
    }

    /// Set codec config overrides.
    pub fn with_codec_config(mut self, config: zencodecs::config::CodecConfig) -> Self {
        self.codec_config = Some(config);
        self
    }

    /// Set trace config for pipeline debugging.
    pub fn with_trace(mut self, config: &'a crate::trace::TraceConfig) -> Self {
        self.trace_config = Some(config);
        self
    }

    /// Execute the job: probe → decode → CMS → pipeline → encode.
    ///
    /// Returns a [`JobResult`] with encoded outputs and decode metadata.
    pub fn run(self) -> Result<JobResult, PipeError> {
        // 1. Get primary input bytes.
        let input_bytes = match self.io.get(&self.decode_io_id) {
            Some(IoSlot::Input(data)) => data,
            _ => {
                return Err(PipeError::Op(alloc::format!(
                    "no input data for io_id {}",
                    self.decode_io_id
                )));
            }
        };

        // 2. Probe the source.
        let image_info = zencodecs::from_bytes_with_registry(input_bytes, &self.registry)
            .map_err(|e| PipeError::Op(alloc::format!("probe failed: {e}")))?;

        let decode_info = DecodeInfo {
            io_id: self.decode_io_id,
            width: image_info.width,
            height: image_info.height,
            format: image_info.format,
            has_alpha: image_info.has_alpha,
            has_animation: image_info.is_animation(),
            mime_type: image_info.format.mime_type().to_string(),
        };

        // 3. Build image facts for format selection.
        let facts = zencodecs::ImageFacts::from_image_info(&image_info);

        // 4. Select output format.
        let decision = zencodecs::select_format_from_intent(
            &self.intent,
            &facts,
            &self.registry,
            &zencodecs::CodecPolicy::default(),
        )
        .map_err(|e| PipeError::Op(alloc::format!("format selection failed: {e}")))?;

        // 5. Decode the source to a pixel stream, optionally extracting gain map.
        //
        // Gain map detection at probe time is unreliable (JPEG returns Unknown
        // because MPF scanning requires full decode). So when preserving, we
        // attempt extraction for any format that COULD contain a gain map,
        // and let the decoder report whether one was actually found.
        let format_may_have_gainmap = matches!(
            image_info.format,
            zencodec::ImageFormat::Jpeg
                | zencodec::ImageFormat::Avif
                | zencodec::ImageFormat::Jxl
                | zencodec::ImageFormat::Heic
        );
        let try_extract_gainmap =
            self.gain_map_mode == GainMapMode::Preserve && format_may_have_gainmap;

        let (source, gain_map_sidecar) = if try_extract_gainmap {
            self.decode_source_with_gainmap(input_bytes)?
        } else {
            (self.decode_source(input_bytes, &image_info)?, None)
        };

        let has_gain_map = gain_map_sidecar.is_some();

        // 6. Apply CMS transform if needed.
        let source = self.apply_cms(source, &image_info, input_bytes)?;

        // 7. Ensure source is RGBA8 sRGB for pipeline compatibility.
        let source = ensure_srgb_rgba8(source)?;

        // 8. Build source info for orchestration.
        let source_info = crate::orchestrate::SourceImageInfo {
            width: source.width(),
            height: source.height(),
            format: source.format(),
            has_alpha: image_info.has_alpha,
            has_animation: image_info.is_animation(),
            has_gain_map,
            is_hdr: false, // handled by CMS
            exif_orientation: image_info.orientation.to_exif(),
            metadata: Some(image_info.metadata()),
        };

        // 9. Run the pipeline via orchestrate::stream().
        // When preserving gain maps, set hdr_mode to "preserve" so the sidecar
        // is tracked through geometry transforms (crop, resize, orientation).
        let hdr_mode = if has_gain_map { "preserve" } else { "sdr_only" };

        let config = crate::orchestrate::ProcessConfig {
            nodes: self.nodes,
            converters: self.converters,
            source_info: &source_info,
            hdr_mode,
            trace_config: self.trace_config,
        };

        let output = crate::orchestrate::stream(source, &config, gain_map_sidecar)?;

        // 10. Encode the output.
        let encode_result = self.stream_encode(output, &decision)?;

        Ok(JobResult {
            encode_results: vec![encode_result],
            decode_infos: vec![decode_info],
        })
    }

    /// Decode input bytes to a pixel [`Source`].
    fn decode_source(
        &self,
        data: &[u8],
        info: &zencodec::ImageInfo,
    ) -> Result<Box<dyn Source>, PipeError> {
        let _ = info; // may use for format negotiation later

        // Try streaming decode first, fall back to full-frame.
        let mut request = zencodecs::DecodeRequest::new(data).with_registry(&self.registry);

        if let Some(ref config) = self.codec_config {
            request = request.with_codec_config(config);
        }

        match request.build_streaming_decoder() {
            Ok(decoder) => {
                let decoder_source = crate::codec::DecoderSource::new(decoder)?;
                Ok(Box::new(decoder_source))
            }
            Err(_) => {
                // Fall back to full-frame decode.
                let decoded = zencodecs::DecodeRequest::new(data)
                    .with_registry(&self.registry)
                    .decode_full_frame()
                    .map_err(|e| PipeError::Op(alloc::format!("decode failed: {e}")))?;

                let pixels = decoded.pixels();
                let w = decoded.width();
                let h = decoded.height();
                let format: PixelFormat = pixels.descriptor();

                let data = pixels.as_strided_bytes().to_vec();
                let source = crate::sources::MaterializedSource::from_data(data, w, h, format);
                Ok(Box::new(source))
            }
        }
    }

    /// Decode source AND extract gain map in one call.
    ///
    /// Returns `(base_source, Option<SidecarStream>)`. The gain map is
    /// wrapped as a SidecarStream for the orchestration layer to track
    /// through geometry transforms.
    fn decode_source_with_gainmap(
        &self,
        data: &[u8],
    ) -> Result<(Box<dyn Source>, Option<crate::sidecar::SidecarStream>), PipeError> {
        let mut request = zencodecs::DecodeRequest::new(data)
            .with_registry(&self.registry)
            .with_gain_map_extraction(true);

        if let Some(ref config) = self.codec_config {
            request = request.with_codec_config(config);
        }

        let (decoded, gain_map) = request
            .decode_gain_map()
            .map_err(|e| PipeError::Op(alloc::format!("decode with gain map failed: {e}")))?;

        // Convert base image to Source.
        let pixels = decoded.pixels();
        let w = decoded.width();
        let h = decoded.height();
        let format: PixelFormat = pixels.descriptor();
        let pixel_data = pixels.as_strided_bytes().to_vec();
        let source = crate::sources::MaterializedSource::from_data(pixel_data, w, h, format);

        // Convert gain map to SidecarStream.
        let sidecar = gain_map.map(|gm| {
            let gm_w = gm.gain_map.width;
            let gm_h = gm.gain_map.height;
            let gm_channels = gm.gain_map.channels;
            let params = gm.params(); // extract before moving data

            // Determine pixel format from channel count.
            let gm_format = if gm_channels == 1 {
                crate::format::PixelFormat::new(
                    crate::ChannelType::U8,
                    crate::ChannelLayout::Gray,
                    None,
                    crate::TransferFunction::Srgb,
                )
            } else {
                crate::format::RGB8_SRGB
            };

            let gm_source = crate::sources::MaterializedSource::from_data(
                gm.gain_map.data, gm_w, gm_h, gm_format,
            );

            crate::sidecar::SidecarStream {
                source: Box::new(gm_source),
                width: gm_w,
                height: gm_h,
                kind: crate::sidecar::SidecarKind::GainMap { params },
            }
        });

        Ok((Box::new(source), sidecar))
    }

    /// Apply CMS transform (ICC → sRGB) if configured and needed.
    #[cfg(feature = "std")]
    fn apply_cms(
        &self,
        source: Box<dyn Source>,
        info: &zencodec::ImageInfo,
        raw_data: &[u8],
    ) -> Result<Box<dyn Source>, PipeError> {
        let cms_mode = match self.cms_mode {
            CmsMode::None => return Ok(source),
            CmsMode::SrgbCompat => zencodecs::CmsMode::Compat,
            CmsMode::SceneReferred => zencodecs::CmsMode::SceneReferred,
        };

        // Check if transform is needed via zencodecs CMS module.
        let transform_icc = if info.format == zencodec::ImageFormat::Png {
            // PNG: check gAMA/cHRM/cICP chunks in raw data.
            let from_icc = zencodecs::cms::srgb_transform_icc(&info.source_color, None, cms_mode);
            let from_png = zencodecs::cms::png_srgb_transform_icc(raw_data, cms_mode);
            // ICC profile takes precedence; PNG chunks are fallback.
            from_icc.or(from_png)
        } else {
            zencodecs::cms::srgb_transform_icc(&info.source_color, None, cms_mode)
        };

        let Some((src_icc, dst_icc)) = transform_icc else {
            return Ok(source); // Already sRGB
        };

        // Build and apply the transform.
        let src_descriptor = source.format();
        let pf = src_descriptor.pixel_format();

        use crate::ColorManagement as _;
        let transform = crate::MoxCms.build_transform_for_format(&src_icc, &dst_icc, pf, pf);

        match transform {
            Ok(row_transform) => {
                let dst_arc: std::sync::Arc<[u8]> = std::sync::Arc::from(dst_icc.as_slice());
                let transformed = crate::sources::IccTransformSource::from_transform(
                    source,
                    row_transform,
                    dst_arc,
                );
                Ok(Box::new(transformed))
            }
            Err(_) => Ok(source), // Transform not possible — pass through
        }
    }

    /// Encode the streaming pipeline output.
    fn stream_encode(
        &self,
        output: crate::orchestrate::StreamingOutput,
        decision: &zencodecs::FormatDecision,
    ) -> Result<EncodeResult, PipeError> {
        let mut source = output.source;
        let w = source.width();
        let h = source.height();
        let format = source.format();

        // Handle alpha removal for formats that don't support it (JPEG).
        let target_format = decision.format;
        let needs_alpha_removal = !target_format.supports_alpha() && format.has_alpha();

        if needs_alpha_removal {
            let matte = decision.matte.unwrap_or([255, 255, 255]);
            let remove_alpha_format = crate::format::RGB8_SRGB;
            // Use RemoveAlpha via TransformSource: composite onto matte, drop alpha
            if let Some(converter) = crate::ops::RowConverterOp::new(format, remove_alpha_format) {
                let transform =
                    crate::sources::TransformSource::new(source).push_boxed(Box::new(converter));
                source = Box::new(transform);
            }
            let _ = matte; // TODO: use matte color when RemoveAlpha supports it
        }

        let src_format = source.format();

        // Build streaming encoder via zencodecs.
        let mut encode_request = zencodecs::EncodeRequest::new(target_format)
            .with_quality(decision.quality.quality)
            .with_registry(&self.registry);

        if decision.lossless {
            encode_request = encode_request.with_lossless(true);
        }
        if let Some(effort) = decision.quality.effort {
            encode_request = encode_request.with_effort(effort);
        }
        if let Some(meta) = output.metadata {
            encode_request = encode_request.with_metadata(meta);
        }

        // Prepare gain map data for re-embedding (if sidecar was preserved).
        // These live outside the encode_request borrow scope.
        let gain_map_data = output.sidecar.and_then(|sidecar| {
            if let crate::sidecar::SidecarKind::GainMap { ref params } = sidecar.kind {
                let metadata = zencodecs::gainmap::params_to_metadata(params);
                let gain_map = zencodecs::GainMap {
                    data: sidecar.data().to_vec(),
                    width: sidecar.width(),
                    height: sidecar.height(),
                    channels: if sidecar.format().layout() == crate::ChannelLayout::Gray { 1 } else { 3 },
                };
                Some((gain_map, metadata))
            } else {
                None
            }
        });

        // Attach gain map to encoder if the target format supports it.
        // Formats without gain map support (PNG, WebP, GIF, BMP, etc.)
        // silently drop the gain map — no error.
        let target_supports_gainmap = matches!(
            target_format,
            zencodec::ImageFormat::Jpeg
                | zencodec::ImageFormat::Avif
                | zencodec::ImageFormat::Jxl
        );
        if target_supports_gainmap {
            if let Some((ref gm, ref meta)) = gain_map_data {
                encode_request = encode_request.with_gain_map(
                    zencodecs::GainMapSource::Precomputed {
                        gain_map: gm,
                        metadata: meta,
                    },
                );
            }
        }

        // When a gain map is present, use one-shot encode (gain map embedding
        // requires full-frame access for MPF/tmap/jhgm assembly).
        // Otherwise, try streaming encode with one-shot fallback.
        let has_gain_map_data = gain_map_data.is_some();
        let streaming_result = if has_gain_map_data {
            None // Force one-shot path
        } else {
            encode_request.build_streaming_encoder(w, h).ok()
        };
        match streaming_result {
            Some(streaming_enc) => {
                let mut sink = crate::codec::EncoderSink::new(streaming_enc.encoder, src_format);
                crate::execute(source.as_mut(), &mut sink)?;
                let encode_output = sink
                    .take_output()
                    .ok_or_else(|| PipeError::Op("encoder produced no output".to_string()))?;

                Ok(EncodeResult {
                    io_id: self.encode_io_id,
                    bytes: encode_output.data().to_vec(),
                    width: w,
                    height: h,
                    mime_type: target_format.mime_type().to_string(),
                    extension: target_format.extension().to_string(),
                })
            }
            None => {
                // One-shot encode (full-frame materialize).
                let materialized = crate::sources::MaterializedSource::from_source(source)?;
                let pixels = zenpixels::PixelSlice::new(
                    materialized.data(),
                    materialized.width(),
                    materialized.height(),
                    materialized.stride(),
                    src_format,
                )
                .map_err(|e| PipeError::Op(alloc::format!("PixelSlice failed: {e}")))?;

                let mut oneshot_request = zencodecs::EncodeRequest::new(target_format)
                    .with_quality(decision.quality.quality)
                    .with_registry(&self.registry);

                // Re-attach gain map for one-shot encode (only if format supports it).
                if target_supports_gainmap {
                    if let Some((ref gm, ref meta)) = gain_map_data {
                        oneshot_request = oneshot_request.with_gain_map(
                            zencodecs::GainMapSource::Precomputed {
                                gain_map: gm,
                                metadata: meta,
                            },
                        );
                    }
                }

                let encode_output = oneshot_request
                    .encode(pixels, format.has_alpha())
                    .map_err(|e| PipeError::Op(alloc::format!("encode failed: {e}")))?;

                Ok(EncodeResult {
                    io_id: self.encode_io_id,
                    bytes: encode_output.data().to_vec(),
                    width: materialized.width(),
                    height: materialized.height(),
                    mime_type: target_format.mime_type().to_string(),
                    extension: target_format.extension().to_string(),
                })
            }
        }
    }
}

/// Convert source to RGBA8 sRGB if it isn't already.
fn ensure_srgb_rgba8(source: Box<dyn Source>) -> Result<Box<dyn Source>, PipeError> {
    let src_format = source.format();
    let target = crate::format::RGBA8_SRGB;

    if src_format == target {
        return Ok(source);
    }
    if let Some(converter) = crate::ops::RowConverterOp::new(src_format, target) {
        let transform =
            crate::sources::TransformSource::new(source).push_boxed(Box::new(converter));
        Ok(Box::new(transform))
    } else {
        Ok(source)
    }
}

/// Get raw bytes for an io_id from the IO map.
pub fn get_io_bytes<'a>(io: &'a HashMap<i32, IoSlot>, io_id: i32) -> Option<&'a [u8]> {
    match io.get(&io_id) {
        Some(IoSlot::Input(data)) => Some(data.as_slice()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Builder pattern ──

    #[test]
    fn builder_chaining_compiles_and_returns_self() {
        // Every builder method should return Self, enabling method chaining.
        let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![];
        let converters: Vec<&dyn crate::bridge::NodeConverter> = vec![];
        let _job = ImageJob::new()
            .add_input(0, vec![0u8; 4])
            .add_input_ref(2, &[1, 2, 3])
            .add_output(1)
            .with_decode_io(0)
            .with_encode_io(1)
            .with_nodes(&nodes)
            .with_converters(&converters)
            .with_intent(zencodecs::CodecIntent::default())
            .with_cms(CmsMode::None)
            .with_limits(Limits::default())
            .with_registry(zencodecs::AllowedFormats::all());
        // If this compiles, chaining works.
    }

    #[test]
    fn builder_defaults() {
        let job = ImageJob::new();
        assert_eq!(job.decode_io_id, 0);
        assert_eq!(job.encode_io_id, 1);
        assert!(job.io.is_empty());
        assert!(job.nodes.is_empty());
        assert!(job.converters.is_empty());
        assert!(job.limits.is_none());
        assert!(job.codec_config.is_none());
        assert!(job.trace_config.is_none());
    }

    #[test]
    fn builder_add_input_stores_data() {
        let data = vec![10u8, 20, 30];
        let job = ImageJob::new().add_input(5, data.clone());
        assert!(job.io.contains_key(&5));
        match &job.io[&5] {
            IoSlot::Input(d) => assert_eq!(d, &data),
            IoSlot::Output => panic!("expected Input"),
        }
    }

    #[test]
    fn builder_add_input_ref_clones_data() {
        let data = [10u8, 20, 30];
        let job = ImageJob::new().add_input_ref(3, &data);
        match &job.io[&3] {
            IoSlot::Input(d) => assert_eq!(d.as_slice(), &data),
            IoSlot::Output => panic!("expected Input"),
        }
    }

    #[test]
    fn builder_add_output_stores_output_variant() {
        let job = ImageJob::new().add_output(7);
        assert!(job.io.contains_key(&7));
        assert!(matches!(job.io[&7], IoSlot::Output));
    }

    #[test]
    fn builder_overwrite_io_slot() {
        // Adding input then output to the same io_id should overwrite.
        let job = ImageJob::new().add_input(0, vec![1, 2, 3]).add_output(0);
        assert!(matches!(job.io[&0], IoSlot::Output));
    }

    // ── get_io_bytes ──

    #[test]
    fn get_io_bytes_returns_input_data() {
        let mut io = HashMap::new();
        io.insert(0, IoSlot::Input(vec![0xDE, 0xAD, 0xBE, 0xEF]));
        let bytes = get_io_bytes(&io, 0);
        assert_eq!(bytes, Some([0xDE, 0xAD, 0xBE, 0xEF].as_slice()));
    }

    #[test]
    fn get_io_bytes_returns_none_for_output_slot() {
        let mut io = HashMap::new();
        io.insert(1, IoSlot::Output);
        assert_eq!(get_io_bytes(&io, 1), None);
    }

    #[test]
    fn get_io_bytes_returns_none_for_missing_key() {
        let io: HashMap<i32, IoSlot> = HashMap::new();
        assert_eq!(get_io_bytes(&io, 42), None);
    }

    #[test]
    fn get_io_bytes_empty_input() {
        let mut io = HashMap::new();
        io.insert(0, IoSlot::Input(vec![]));
        assert_eq!(get_io_bytes(&io, 0), Some([].as_slice()));
    }

    // ── Error paths ──

    #[test]
    fn run_with_no_input_returns_error() {
        // No IO slots at all — run() should fail looking for decode_io_id=0.
        let result = ImageJob::new().add_output(1).run();
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = alloc::format!("{err}");
        assert!(
            msg.contains("no input data"),
            "expected 'no input data' error, got: {msg}"
        );
    }

    #[test]
    fn run_with_output_slot_as_decode_input_returns_error() {
        // decode_io_id=0 points to an Output slot, not Input.
        let result = ImageJob::new().add_output(0).add_output(1).run();
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = alloc::format!("{err}");
        assert!(
            msg.contains("no input data"),
            "expected 'no input data' error, got: {msg}"
        );
    }

    #[test]
    fn run_with_invalid_image_data_returns_probe_error() {
        // Valid Input slot but garbage data — probe should fail.
        let result = ImageJob::new()
            .add_input(0, vec![0, 0, 0, 0])
            .add_output(1)
            .run();
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = alloc::format!("{err}");
        assert!(
            msg.contains("probe failed"),
            "expected probe error, got: {msg}"
        );
    }

    #[test]
    fn run_with_custom_decode_io_id_missing_returns_error() {
        // Set decode_io_id=5 but only have input at io_id=0.
        let result = ImageJob::new()
            .add_input(0, vec![1, 2, 3])
            .add_output(1)
            .with_decode_io(5)
            .run();
        assert!(result.is_err());
        let msg = alloc::format!("{}", result.unwrap_err());
        assert!(msg.contains("no input data for io_id 5"), "got: {msg}");
    }

    // ── CmsMode default ──

    #[test]
    fn cms_mode_default_is_srgb_compat() {
        assert!(matches!(CmsMode::default(), CmsMode::SrgbCompat));
    }

    // ── IoSlot Clone ──

    #[test]
    fn io_slot_input_clone() {
        let original = IoSlot::Input(vec![1, 2, 3, 4]);
        let cloned = original.clone();
        match (&original, &cloned) {
            (IoSlot::Input(a), IoSlot::Input(b)) => assert_eq!(a, b),
            _ => panic!("clone changed variant"),
        }
    }

    #[test]
    fn io_slot_output_clone() {
        let original = IoSlot::Output;
        let cloned = original.clone();
        assert!(matches!(cloned, IoSlot::Output));
    }

    // ── Struct field access ──

    #[test]
    fn encode_result_fields() {
        let er = EncodeResult {
            io_id: 1,
            bytes: vec![0xFF, 0xD8],
            width: 100,
            height: 200,
            mime_type: String::from("image/jpeg"),
            extension: String::from("jpg"),
        };
        assert_eq!(er.io_id, 1);
        assert_eq!(er.bytes, vec![0xFF, 0xD8]);
        assert_eq!(er.width, 100);
        assert_eq!(er.height, 200);
        assert_eq!(er.mime_type, "image/jpeg");
        assert_eq!(er.extension, "jpg");
    }

    #[test]
    fn encode_result_clone_and_debug() {
        let er = EncodeResult {
            io_id: 0,
            bytes: vec![],
            width: 1,
            height: 1,
            mime_type: String::new(),
            extension: String::new(),
        };
        let cloned = er.clone();
        assert_eq!(cloned.io_id, er.io_id);
        // Debug should not panic.
        let _ = alloc::format!("{:?}", er);
    }

    #[test]
    fn decode_info_fields() {
        let di = DecodeInfo {
            io_id: 0,
            width: 640,
            height: 480,
            format: zencodec::ImageFormat::Jpeg,
            has_alpha: false,
            has_animation: false,
            mime_type: String::from("image/jpeg"),
        };
        assert_eq!(di.io_id, 0);
        assert_eq!(di.width, 640);
        assert_eq!(di.height, 480);
        assert_eq!(di.format, zencodec::ImageFormat::Jpeg);
        assert!(!di.has_alpha);
        assert!(!di.has_animation);
        assert_eq!(di.mime_type, "image/jpeg");
    }

    #[test]
    fn decode_info_clone_and_debug() {
        let di = DecodeInfo {
            io_id: 0,
            width: 1,
            height: 1,
            format: zencodec::ImageFormat::Png,
            has_alpha: true,
            has_animation: false,
            mime_type: String::from("image/png"),
        };
        let cloned = di.clone();
        assert_eq!(cloned.format, di.format);
        let _ = alloc::format!("{:?}", di);
    }

    #[test]
    fn job_result_fields() {
        let jr = JobResult {
            encode_results: vec![EncodeResult {
                io_id: 1,
                bytes: vec![1, 2, 3],
                width: 10,
                height: 10,
                mime_type: String::from("image/png"),
                extension: String::from("png"),
            }],
            decode_infos: vec![DecodeInfo {
                io_id: 0,
                width: 10,
                height: 10,
                format: zencodec::ImageFormat::Png,
                has_alpha: false,
                has_animation: false,
                mime_type: String::from("image/png"),
            }],
        };
        assert_eq!(jr.encode_results.len(), 1);
        assert_eq!(jr.decode_infos.len(), 1);
        assert_eq!(jr.encode_results[0].io_id, 1);
        assert_eq!(jr.decode_infos[0].io_id, 0);
    }

    #[test]
    fn job_result_clone_and_debug() {
        let jr = JobResult {
            encode_results: vec![],
            decode_infos: vec![],
        };
        let cloned = jr.clone();
        assert!(cloned.encode_results.is_empty());
        let _ = alloc::format!("{:?}", jr);
    }

    // ── CmsMode variants ──

    #[test]
    fn cms_mode_copy() {
        let mode = CmsMode::SceneReferred;
        let copied = mode;
        assert!(matches!(copied, CmsMode::SceneReferred));
    }

    #[test]
    fn cms_mode_debug() {
        let _ = alloc::format!("{:?}", CmsMode::SrgbCompat);
        let _ = alloc::format!("{:?}", CmsMode::SceneReferred);
        let _ = alloc::format!("{:?}", CmsMode::None);
    }

    // ── End-to-end with real JPEG ──

    /// End-to-end tests requiring a real JPEG codec.
    ///
    /// Run with: `cargo test --features job,nodes-jpeg,zencodecs/jpeg --lib -- job::tests::e2e_jpeg`
    ///
    /// The `nodes-jpeg` feature enables zenjpeg for zenpipe, while `zencodecs/jpeg`
    /// enables the JPEG codec inside zencodecs (used by `ImageJob::run()` for
    /// probing and format selection).
    #[cfg(feature = "nodes-jpeg")]
    mod e2e_jpeg {
        use super::*;

        /// Generate a small 8x8 JPEG using zenjpeg directly.
        fn make_test_jpeg() -> Vec<u8> {
            use zenjpeg::encoder::{ChromaSubsampling, EncoderConfig, PixelLayout};

            let w = 8u32;
            let h = 8u32;
            let bpp = 4usize; // RGBA
            let stride = w as usize * bpp;
            let mut pixels = vec![0u8; stride * h as usize];

            // Fill with a red gradient.
            for y in 0..h as usize {
                for x in 0..w as usize {
                    let i = y * stride + x * bpp;
                    pixels[i] = 200; // R
                    pixels[i + 1] = (x * 32) as u8; // G
                    pixels[i + 2] = (y * 32) as u8; // B
                    pixels[i + 3] = 255; // A
                }
            }

            let config = EncoderConfig::ycbcr(85.0, ChromaSubsampling::None)
                .progressive(false)
                .optimize_huffman(false);
            let mut enc = config
                .request()
                .encode_from_bytes(w, h, PixelLayout::Rgba8Srgb)
                .expect("encoder creation");

            enc.push(&pixels, h as usize, stride, enough::Unstoppable)
                .expect("push rows");
            enc.finish().expect("finish encode")
        }

        #[test]
        fn roundtrip_jpeg_no_nodes() {
            let jpeg_data = make_test_jpeg();

            // Verify it starts with JPEG SOI marker.
            assert_eq!(&jpeg_data[..2], &[0xFF, 0xD8], "not a valid JPEG");

            let result = ImageJob::new()
                .add_input(0, jpeg_data)
                .add_output(1)
                .with_cms(CmsMode::None)
                .run();

            let result = result.expect("ImageJob::run() failed");

            // Should have exactly one encode result and one decode info.
            assert_eq!(result.encode_results.len(), 1);
            assert_eq!(result.decode_infos.len(), 1);

            // Decode info should reflect the 8x8 JPEG input.
            let di = &result.decode_infos[0];
            assert_eq!(di.io_id, 0);
            assert_eq!(di.width, 8);
            assert_eq!(di.height, 8);
            assert_eq!(di.format, zencodec::ImageFormat::Jpeg);
            assert!(!di.has_alpha);
            assert!(!di.has_animation);
            assert_eq!(di.mime_type, "image/jpeg");

            // Encode result should contain valid output bytes.
            let er = &result.encode_results[0];
            assert_eq!(er.io_id, 1);
            assert!(!er.bytes.is_empty(), "output bytes should not be empty");
            assert_eq!(er.width, 8);
            assert_eq!(er.height, 8);

            // Output should be a valid JPEG (starts with SOI, ends with EOI).
            assert_eq!(
                &er.bytes[..2],
                &[0xFF, 0xD8],
                "output should start with JPEG SOI"
            );
            assert_eq!(
                &er.bytes[er.bytes.len() - 2..],
                &[0xFF, 0xD9],
                "output should end with JPEG EOI"
            );
        }

        #[test]
        fn roundtrip_jpeg_custom_io_ids() {
            let jpeg_data = make_test_jpeg();

            let result = ImageJob::new()
                .add_input(10, jpeg_data)
                .add_output(20)
                .with_decode_io(10)
                .with_encode_io(20)
                .with_cms(CmsMode::None)
                .run()
                .expect("ImageJob with custom io_ids failed");

            assert_eq!(result.decode_infos[0].io_id, 10);
            assert_eq!(result.encode_results[0].io_id, 20);
        }
    }
}
