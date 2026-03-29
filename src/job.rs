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

// ─── Job builder ───

/// A high-level image processing job.
///
/// Combines zencodecs (probe/decode/encode) with zenpipe (pipeline execution)
/// into a single bytes-in → bytes-out operation.
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

        // 5. Decode the source to a pixel stream.
        let source = self.decode_source(input_bytes, &image_info)?;

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
            has_gain_map: image_info.gain_map.is_present(),
            is_hdr: false, // handled by CMS
            exif_orientation: image_info.orientation.to_exif(),
            metadata: Some(image_info.metadata()),
        };

        // 9. Run the pipeline via orchestrate::stream().
        let config = crate::orchestrate::ProcessConfig {
            nodes: self.nodes,
            converters: self.converters,
            source_info: &source_info,
            hdr_mode: "sdr_only",
            trace_config: self.trace_config,
        };

        let output = crate::orchestrate::stream(source, &config, None)?;

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
                let source =
                    crate::sources::MaterializedSource::from_data(data, w, h, format);
                Ok(Box::new(source))
            }
        }
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
                let transform = crate::sources::TransformSource::new(source)
                    .push_boxed(Box::new(converter));
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

        // Try streaming encode.
        match encode_request.build_streaming_encoder(w, h) {
            Ok(streaming_enc) => {
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
            Err(_) => {
                // Fall back to full-frame encode.
                let materialized = crate::sources::MaterializedSource::from_source(source)?;
                let pixels = zenpixels::PixelSlice::new(
                    materialized.data(),
                    materialized.width(),
                    materialized.height(),
                    materialized.stride(),
                    src_format,
                )
                .map_err(|e| PipeError::Op(alloc::format!("PixelSlice failed: {e}")))?;

                let encode_output = zencodecs::EncodeRequest::new(target_format)
                    .with_quality(decision.quality.quality)
                    .with_registry(&self.registry)
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
        let transform = crate::sources::TransformSource::new(source).push_boxed(Box::new(converter));
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
