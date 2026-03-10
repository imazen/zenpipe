//! Cross-format image pipeline: decode -> resize/orient -> encode.
//!
//! The pipeline combines codec dispatch, layout geometry, and pixel resampling
//! into a single builder. It handles pixel format normalization, EXIF orientation,
//! quality presets with per-format mapping, and metadata passthrough.
//!
//! # Example
//!
//! ```no_run
//! use zencodecs::pipeline::{Pipeline, QualityPreset};
//! use zencodecs::ImageFormat;
//!
//! let jpeg_bytes: &[u8] = &[]; // your image bytes
//! let output = Pipeline::from_bytes(jpeg_bytes)
//!     .output_format(ImageFormat::WebP)
//!     .fit(800, 600)
//!     .quality(QualityPreset::Balanced)
//!     .execute()?;
//!
//! println!("{}x{} {:?} ({} bytes)", output.width, output.height, output.format, output.bytes.len());
//! # Ok::<(), whereat::At<zencodecs::CodecError>>(())
//! ```

mod convert;
mod quality;

use alloc::vec::Vec;

use zencodec::ImageFormat;
use zenresize::{Filter, PixelDescriptor};

use crate::config::CodecConfig;
use crate::error::Result;
use crate::{CodecError, CodecRegistry, Limits, Stop};
use whereat::at;

pub use quality::QualityPreset;

/// How to handle metadata during transcoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum MetadataPolicy {
    /// Preserve all metadata the target format supports (ICC, EXIF, XMP).
    #[default]
    Preserve,
    /// Strip all metadata.
    Strip,
    /// Preserve only the ICC profile (important for color accuracy).
    PreserveIcc,
}

/// Layout constraint for the pipeline.
#[derive(Debug, Clone)]
enum LayoutConstraint {
    /// Scale to fit within the given dimensions (may upscale).
    Fit(u32, u32),
    /// Scale to fit within the given dimensions (never upscale).
    Within(u32, u32),
    /// Fill the given dimensions, cropping excess.
    FitCrop(u32, u32),
    /// Fit within the given dimensions, padding to fill.
    FitPad(u32, u32, zenresize::CanvasColor),
}

/// Output of a pipeline execution.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PipelineOutput {
    /// Encoded image bytes.
    pub bytes: Vec<u8>,
    /// Output image format.
    pub format: ImageFormat,
    /// Output width in pixels.
    pub width: u32,
    /// Output height in pixels.
    pub height: u32,
    /// Number of frames (1 for static images).
    pub frame_count: u32,
}

/// Cross-format image pipeline builder.
///
/// Decodes an image, optionally resizes and reorients it, then re-encodes
/// to a target format with quality presets.
pub struct Pipeline<'a> {
    input: &'a [u8],
    input_format: Option<ImageFormat>,
    output_format: Option<ImageFormat>,
    constraint: Option<LayoutConstraint>,
    auto_orient: bool,
    filter: Filter,
    quality: QualityPreset,
    metadata_policy: MetadataPolicy,
    limits: Option<&'a Limits>,
    stop: Option<&'a dyn Stop>,
    registry: Option<&'a CodecRegistry>,
    codec_config: Option<&'a CodecConfig>,
}

/// Owned copy of metadata bytes, allowing `DecodeOutput` to be consumed.
struct OwnedMetadata {
    icc_profile: Option<Vec<u8>>,
    exif: Option<Vec<u8>>,
    xmp: Option<Vec<u8>>,
    cicp: Option<zencodec::Cicp>,
    content_light_level: Option<zencodec::ContentLightLevel>,
    mastering_display: Option<zencodec::MasteringDisplay>,
    orientation: zencodec::Orientation,
}

impl OwnedMetadata {
    fn empty() -> Self {
        Self {
            icc_profile: None,
            exif: None,
            xmp: None,
            cicp: None,
            content_light_level: None,
            mastering_display: None,
            orientation: zencodec::Orientation::Normal,
        }
    }

    fn as_metadata(&self) -> zencodec::MetadataView<'_> {
        let mut m = zencodec::MetadataView::none();
        if let Some(ref icc) = self.icc_profile {
            m = m.with_icc(icc);
        }
        if let Some(ref exif) = self.exif {
            m = m.with_exif(exif);
        }
        if let Some(ref xmp) = self.xmp {
            m = m.with_xmp(xmp);
        }
        if let Some(cicp) = self.cicp {
            m = m.with_cicp(cicp);
        }
        if let Some(cll) = self.content_light_level {
            m = m.with_content_light_level(cll);
        }
        if let Some(md) = self.mastering_display {
            m = m.with_mastering_display(md);
        }
        m = m.with_orientation(self.orientation);
        m
    }
}

impl<'a> Pipeline<'a> {
    /// Create a pipeline from raw image bytes.
    ///
    /// Format will be auto-detected from magic bytes unless overridden
    /// with [`input_format`](Self::input_format).
    pub fn from_bytes(data: &'a [u8]) -> Self {
        Self {
            input: data,
            input_format: None,
            output_format: None,
            constraint: None,
            auto_orient: true,
            filter: Filter::Robidoux,
            quality: QualityPreset::Balanced,
            metadata_policy: MetadataPolicy::Preserve,
            limits: None,
            stop: None,
            registry: None,
            codec_config: None,
        }
    }

    /// Override input format detection.
    pub fn input_format(mut self, format: ImageFormat) -> Self {
        self.input_format = Some(format);
        self
    }

    /// Set the target output format.
    ///
    /// If not set, the format is auto-selected based on image properties
    /// and available encoders.
    pub fn output_format(mut self, format: ImageFormat) -> Self {
        self.output_format = Some(format);
        self
    }

    /// Scale to fit within the given dimensions (may upscale).
    pub fn fit(mut self, width: u32, height: u32) -> Self {
        self.constraint = Some(LayoutConstraint::Fit(width, height));
        self
    }

    /// Scale to fit within the given dimensions (never upscale).
    pub fn within(mut self, width: u32, height: u32) -> Self {
        self.constraint = Some(LayoutConstraint::Within(width, height));
        self
    }

    /// Fill the given dimensions, cropping excess.
    pub fn crop(mut self, width: u32, height: u32) -> Self {
        self.constraint = Some(LayoutConstraint::FitCrop(width, height));
        self
    }

    /// Fit within the given dimensions, padding with the given color.
    pub fn pad(mut self, width: u32, height: u32, color: zenresize::CanvasColor) -> Self {
        self.constraint = Some(LayoutConstraint::FitPad(width, height, color));
        self
    }

    /// Whether to apply EXIF orientation (default: true).
    pub fn auto_orient(mut self, enabled: bool) -> Self {
        self.auto_orient = enabled;
        self
    }

    /// Set the resize filter (default: Robidoux).
    pub fn filter(mut self, filter: Filter) -> Self {
        self.filter = filter;
        self
    }

    /// Set the quality preset (default: Balanced).
    pub fn quality(mut self, preset: QualityPreset) -> Self {
        self.quality = preset;
        self
    }

    /// Set the metadata handling policy (default: Preserve).
    pub fn metadata(mut self, policy: MetadataPolicy) -> Self {
        self.metadata_policy = policy;
        self
    }

    /// Set resource limits for decode/encode operations.
    pub fn with_limits(mut self, limits: &'a Limits) -> Self {
        self.limits = Some(limits);
        self
    }

    /// Set a cancellation token.
    pub fn with_stop(mut self, stop: &'a dyn Stop) -> Self {
        self.stop = Some(stop);
        self
    }

    /// Set a codec registry to control which formats are enabled.
    pub fn with_registry(mut self, registry: &'a CodecRegistry) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Set format-specific codec configuration.
    pub fn with_codec_config(mut self, config: &'a CodecConfig) -> Self {
        self.codec_config = Some(config);
        self
    }

    /// Execute the pipeline: decode -> resize/orient -> encode.
    pub fn execute(self) -> Result<PipelineOutput> {
        self.execute_static()
    }

    /// Execute the pipeline for a static (single-frame) image.
    fn execute_static(self) -> Result<PipelineOutput> {
        // 1. Decode
        let mut decode_req = crate::DecodeRequest::new(self.input);
        if let Some(fmt) = self.input_format {
            decode_req = decode_req.with_format(fmt);
        }
        if let Some(limits) = self.limits {
            decode_req = decode_req.with_limits(limits);
        }
        if let Some(stop) = self.stop {
            decode_req = decode_req.with_stop(stop);
        }
        if let Some(registry) = self.registry {
            decode_req = decode_req.with_registry(registry);
        }
        if let Some(config) = self.codec_config {
            decode_req = decode_req.with_codec_config(config);
        }
        let decoded = decode_req.decode()?;

        let source_format = decoded.format();
        let target_format = self.output_format.unwrap_or(source_format);
        let has_resize = self.constraint.is_some();

        // 2. Check for lossless passthrough: same format, no resize, lossless quality
        if !has_resize
            && source_format == target_format
            && matches!(self.quality, QualityPreset::Lossless)
            && (!self.auto_orient || decoded.info().orientation.is_identity())
        {
            return Ok(PipelineOutput {
                bytes: self.input.to_vec(),
                format: source_format,
                width: decoded.width(),
                height: decoded.height(),
                frame_count: 1,
            });
        }

        // 3. Get EXIF orientation
        let orientation = if self.auto_orient {
            decoded.info().orientation
        } else {
            zencodec::Orientation::Normal
        };

        // 4. Determine working pixel format based on source
        let has_alpha = decoded.has_alpha();
        let is_grayscale = decoded.descriptor().is_grayscale();

        // 5. Compute layout if needed
        let (output_width, output_height, layout_plan) =
            if let Some(ref constraint) = self.constraint {
                let source_w = decoded.width();
                let source_h = decoded.height();

                let mut pipeline = zenresize::Pipeline::new(source_w, source_h);

                if self.auto_orient && !orientation.is_identity() {
                    pipeline = pipeline.auto_orient(orientation.exif_value() as u8);
                }

                pipeline = match constraint {
                    LayoutConstraint::Fit(w, h) => pipeline.fit(*w, *h),
                    LayoutConstraint::Within(w, h) => pipeline.within(*w, *h),
                    LayoutConstraint::FitCrop(w, h) => pipeline.fit_crop(*w, *h),
                    LayoutConstraint::FitPad(w, h, color) => pipeline.constrain(
                        zenresize::Constraint::new(zenresize::ConstraintMode::FitPad, *w, *h)
                            .canvas_color(*color),
                    ),
                };

                let (ideal, decoder_request) = pipeline.plan().map_err(|e| {
                    at!(CodecError::InvalidInput(alloc::format!(
                        "layout error: {e}"
                    )))
                })?;

                // Full decode (no decoder-level crop/resize)
                let offer = zenresize::DecoderOffer::full_decode(source_w, source_h);
                let plan = ideal.finalize(&decoder_request, &offer);

                let out_w = plan.canvas.width;
                let out_h = plan.canvas.height;
                (out_w, out_h, Some(plan))
            } else if self.auto_orient && !orientation.is_identity() {
                // No resize but need orientation
                let source_w = decoded.width();
                let source_h = decoded.height();

                let mut pipeline = zenresize::Pipeline::new(source_w, source_h);
                pipeline = pipeline.auto_orient(orientation.exif_value() as u8);

                let (ideal, decoder_request) = pipeline.plan().map_err(|e| {
                    at!(CodecError::InvalidInput(alloc::format!(
                        "layout error: {e}"
                    )))
                })?;

                let offer = zenresize::DecoderOffer::full_decode(source_w, source_h);
                let plan = ideal.finalize(&decoder_request, &offer);

                let out_w = plan.canvas.width;
                let out_h = plan.canvas.height;
                (out_w, out_h, Some(plan))
            } else {
                (decoded.width(), decoded.height(), None)
            };

        // 6. Extract owned metadata before consuming decoded
        let owned_meta = self.extract_metadata(&decoded);

        if is_grayscale && !has_alpha {
            // Gray path
            let resized = if let Some(ref plan) = layout_plan {
                let source = {
                    use zenpixels_convert::PixelBufferConvertExt as _;
                    decoded.into_buffer().to_gray8()
                };
                let (buf, w, h) = source.as_imgref().to_contiguous_buf();
                // Zero-copy: Gray<u8> is repr(C) with a single u8, as_bytes is no-op
                let bytes: &[u8] = rgb::ComponentBytes::as_bytes(&*buf);
                let result = zenresize::execute_layout(
                    bytes,
                    w as u32,
                    h as u32,
                    plan,
                    PixelDescriptor::GRAY8_SRGB,
                    self.filter,
                );
                // Zero-copy: reinterpret Vec<u8> as Vec<Gray<u8>> (same layout)
                let gray_pixels: Vec<rgb::Gray<u8>> = bytemuck::allocation::cast_vec(result);
                zenpixels::PixelBuffer::from_pixels(gray_pixels, output_width, output_height)
                    .expect("resize output size mismatch")
            } else {
                {
                    use zenpixels_convert::PixelBufferConvertExt as _;
                    decoded.into_buffer().to_gray8()
                }
            };

            // Encode
            let metadata = owned_meta.as_metadata();
            let encode_output = self.encode_gray8(resized.as_imgref(), target_format, &metadata)?;

            Ok(PipelineOutput {
                bytes: encode_output.into_vec(),
                format: target_format,
                width: output_width,
                height: output_height,
                frame_count: 1,
            })
        } else if has_alpha {
            // RGBA path
            let resized = if let Some(ref plan) = layout_plan {
                let source = {
                    use zenpixels_convert::PixelBufferConvertExt as _;
                    decoded.into_buffer().to_rgba8()
                };
                let (buf, w, h) = source.as_imgref().to_contiguous_buf();
                // Zero-copy: Rgba<u8> is repr(C) [r,g,b,a], as_bytes is no-op
                let bytes: &[u8] = rgb::ComponentBytes::as_bytes(&*buf);
                let result = zenresize::execute_layout(
                    bytes,
                    w as u32,
                    h as u32,
                    plan,
                    PixelDescriptor::RGBA8_SRGB,
                    self.filter,
                );
                // Zero-copy: reinterpret Vec<u8> as Vec<Rgba<u8>> (same layout)
                let rgba_pixels: Vec<rgb::Rgba<u8>> = bytemuck::allocation::cast_vec(result);
                zenpixels::PixelBuffer::from_pixels(rgba_pixels, output_width, output_height)
                    .expect("resize output size mismatch")
            } else {
                {
                    use zenpixels_convert::PixelBufferConvertExt as _;
                    decoded.into_buffer().to_rgba8()
                }
            };

            // Encode
            let metadata = owned_meta.as_metadata();
            let encode_output = self.encode_rgba8(resized.as_imgref(), target_format, &metadata)?;

            Ok(PipelineOutput {
                bytes: encode_output.into_vec(),
                format: target_format,
                width: output_width,
                height: output_height,
                frame_count: 1,
            })
        } else {
            // RGB path
            let resized = if let Some(ref plan) = layout_plan {
                let source = {
                    use zenpixels_convert::PixelBufferConvertExt as _;
                    decoded.into_buffer().to_rgb8()
                };
                let (buf, w, h) = source.as_imgref().to_contiguous_buf();
                // Zero-copy: Rgb<u8> is repr(C) [r,g,b], as_bytes is no-op
                let bytes: &[u8] = rgb::ComponentBytes::as_bytes(&*buf);
                let result = zenresize::execute_layout(
                    bytes,
                    w as u32,
                    h as u32,
                    plan,
                    PixelDescriptor::RGB8_SRGB,
                    self.filter,
                );
                // Zero-copy: reinterpret Vec<u8> as Vec<Rgb<u8>> (same layout)
                let rgb_pixels: Vec<rgb::Rgb<u8>> = bytemuck::allocation::cast_vec(result);
                zenpixels::PixelBuffer::from_pixels(rgb_pixels, output_width, output_height)
                    .expect("resize output size mismatch")
            } else {
                {
                    use zenpixels_convert::PixelBufferConvertExt as _;
                    decoded.into_buffer().to_rgb8()
                }
            };

            // Encode
            let metadata = owned_meta.as_metadata();
            let encode_output = self.encode_rgb8(resized.as_imgref(), target_format, &metadata)?;

            Ok(PipelineOutput {
                bytes: encode_output.into_vec(),
                format: target_format,
                width: output_width,
                height: output_height,
                frame_count: 1,
            })
        }
    }

    /// Extract owned copies of metadata bytes, filtered by the metadata policy.
    ///
    /// This produces owned data so `decoded` can be consumed afterwards.
    fn extract_metadata(&self, decoded: &zencodec::decode::DecodeOutput) -> OwnedMetadata {
        let meta = decoded.metadata();
        match self.metadata_policy {
            MetadataPolicy::Preserve => OwnedMetadata {
                icc_profile: meta.icc_profile.map(|b| b.to_vec()),
                exif: meta.exif.map(|b| b.to_vec()),
                xmp: meta.xmp.map(|b| b.to_vec()),
                cicp: meta.cicp,
                content_light_level: meta.content_light_level,
                mastering_display: meta.mastering_display,
                orientation: meta.orientation,
            },
            MetadataPolicy::Strip => OwnedMetadata::empty(),
            MetadataPolicy::PreserveIcc => OwnedMetadata {
                icc_profile: meta.icc_profile.map(|b| b.to_vec()),
                ..OwnedMetadata::empty()
            },
        }
    }

    /// Build an EncodeRequest with quality settings applied.
    fn build_encode_request<'b>(
        &self,
        format: ImageFormat,
        metadata: &'b zencodec::MetadataView<'b>,
    ) -> crate::EncodeRequest<'b>
    where
        'a: 'b,
    {
        let (quality, lossless) = self.quality.for_format(format);
        let mut req = crate::EncodeRequest::new(format)
            .with_lossless(lossless)
            .with_metadata(metadata);

        if let Some(q) = quality {
            req = req.with_quality(q);
        }
        if let Some(limits) = self.limits {
            req = req.with_limits(limits);
        }
        if let Some(stop) = self.stop {
            req = req.with_stop(stop);
        }
        if let Some(registry) = self.registry {
            req = req.with_registry(registry);
        }
        if let Some(config) = self.codec_config {
            req = req.with_codec_config(config);
        }
        req
    }

    fn encode_rgb8(
        &self,
        img: imgref::ImgRef<rgb::Rgb<u8>>,
        format: ImageFormat,
        metadata: &zencodec::MetadataView<'_>,
    ) -> Result<zencodec::encode::EncodeOutput> {
        self.build_encode_request(format, metadata).encode_rgb8(img)
    }

    fn encode_rgba8(
        &self,
        img: imgref::ImgRef<rgb::Rgba<u8>>,
        format: ImageFormat,
        metadata: &zencodec::MetadataView<'_>,
    ) -> Result<zencodec::encode::EncodeOutput> {
        self.build_encode_request(format, metadata)
            .encode_rgba8(img)
    }

    fn encode_gray8(
        &self,
        img: imgref::ImgRef<rgb::Gray<u8>>,
        format: ImageFormat,
        metadata: &zencodec::MetadataView<'_>,
    ) -> Result<zencodec::encode::EncodeOutput> {
        self.build_encode_request(format, metadata)
            .encode_gray8(img)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_defaults() {
        let pipeline = Pipeline::from_bytes(&[]);
        assert!(pipeline.input_format.is_none());
        assert!(pipeline.output_format.is_none());
        assert!(pipeline.constraint.is_none());
        assert!(pipeline.auto_orient);
        assert!(matches!(pipeline.quality, QualityPreset::Balanced));
        assert_eq!(pipeline.metadata_policy, MetadataPolicy::Preserve);
    }

    #[test]
    fn builder_chain() {
        let pipeline = Pipeline::from_bytes(&[])
            .input_format(ImageFormat::Jpeg)
            .output_format(ImageFormat::WebP)
            .fit(800, 600)
            .auto_orient(false)
            .filter(Filter::Lanczos)
            .quality(QualityPreset::HighQuality)
            .metadata(MetadataPolicy::Strip);

        assert_eq!(pipeline.input_format, Some(ImageFormat::Jpeg));
        assert_eq!(pipeline.output_format, Some(ImageFormat::WebP));
        assert!(matches!(
            pipeline.constraint,
            Some(LayoutConstraint::Fit(800, 600))
        ));
        assert!(!pipeline.auto_orient);
        assert!(matches!(pipeline.quality, QualityPreset::HighQuality));
        assert_eq!(pipeline.metadata_policy, MetadataPolicy::Strip);
    }

    #[test]
    fn metadata_policy_default() {
        assert_eq!(MetadataPolicy::default(), MetadataPolicy::Preserve);
    }
}
