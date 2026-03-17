//! Image encoding.
//!
//! Uses [`dispatch::build_encoder`](crate::dispatch::build_encoder) for format dispatch.
//! Each codec's `Encoder` trait impl handles pixel format dispatch internally;
//! pixel format negotiation is handled by [`zenpixels::adapt::adapt_for_encode`].

use crate::config::CodecConfig;
use crate::dispatch::EncodeParams;
use crate::error::Result;
use crate::pixel::{Bgra, Gray, ImgRef, Rgb, Rgba};
use crate::policy::CodecPolicy;
use crate::quality::{QualityIntent, QualityProfile};
use crate::select::ImageFacts;
use crate::{CodecError, CodecRegistry, ImageFormat, Limits, Metadata, Stop};
use whereat::at;
use zenpixels::{AlphaMode, PixelDescriptor};

pub use zencodec::encode::EncodeOutput;

/// Image encode request builder.
///
/// # Example
///
/// ```no_run
/// use zencodecs::{EncodeRequest, ImageFormat};
/// use zencodecs::pixel::{ImgVec, Rgba};
///
/// let pixels = ImgVec::new(vec![Rgba { r: 0u8, g: 0, b: 0, a: 255 }; 100*100], 100, 100);
/// let output = EncodeRequest::new(ImageFormat::WebP)
///     .with_quality(85.0)
///     .encode_rgba8(pixels.as_ref())?;
/// # Ok::<(), whereat::At<zencodecs::CodecError>>(())
/// ```
pub struct EncodeRequest<'a> {
    format: Option<ImageFormat>,
    quality: Option<f32>,
    quality_profile: Option<QualityProfile>,
    dpr: Option<f32>,
    effort: Option<u32>,
    lossless: bool,
    limits: Option<&'a Limits>,
    stop: Option<&'a dyn Stop>,
    metadata: Option<&'a Metadata>,
    registry: Option<&'a CodecRegistry>,
    codec_config: Option<&'a CodecConfig>,
    policy: Option<CodecPolicy>,
    image_facts: Option<ImageFacts>,
    /// Quality for UltraHDR gain map JPEG (0-100). Only used by `encode_ultrahdr_*`.
    #[cfg(feature = "jpeg-ultrahdr")]
    gainmap_quality: Option<f32>,
    /// Gain map source for embedding in the encoded output.
    #[cfg(feature = "jpeg-ultrahdr")]
    gain_map_source: Option<crate::gainmap::GainMapSource<'a>>,
}

impl<'a> EncodeRequest<'a> {
    /// Encode to a specific format.
    pub fn new(format: ImageFormat) -> Self {
        Self {
            format: Some(format),
            quality: None,
            quality_profile: None,
            dpr: None,
            effort: None,
            lossless: false,
            limits: None,
            stop: None,
            metadata: None,
            registry: None,
            codec_config: None,
            policy: None,
            image_facts: None,
            #[cfg(feature = "jpeg-ultrahdr")]
            gainmap_quality: None,
            #[cfg(feature = "jpeg-ultrahdr")]
            gain_map_source: None,
        }
    }

    /// Auto-select best format based on image stats and allowed encoders.
    pub fn auto() -> Self {
        Self {
            format: None,
            quality: None,
            quality_profile: None,
            dpr: None,
            effort: None,
            lossless: false,
            limits: None,
            stop: None,
            metadata: None,
            registry: None,
            codec_config: None,
            policy: None,
            image_facts: None,
            #[cfg(feature = "jpeg-ultrahdr")]
            gainmap_quality: None,
            #[cfg(feature = "jpeg-ultrahdr")]
            gain_map_source: None,
        }
    }

    /// Set quality (0-100).
    pub fn with_quality(mut self, quality: f32) -> Self {
        self.quality = Some(quality);
        self
    }

    /// Set encoding effort (speed/quality tradeoff).
    pub fn with_effort(mut self, effort: u32) -> Self {
        self.effort = Some(effort);
        self
    }

    /// Request lossless encoding.
    pub fn with_lossless(mut self, lossless: bool) -> Self {
        self.lossless = lossless;
        self
    }

    /// Set resource limits.
    pub fn with_limits(mut self, limits: &'a Limits) -> Self {
        self.limits = Some(limits);
        self
    }

    /// Set a cancellation token.
    pub fn with_stop(mut self, stop: &'a dyn Stop) -> Self {
        self.stop = Some(stop);
        self
    }

    /// Set metadata to embed in the output (ICC profile, EXIF, XMP).
    ///
    /// Not all formats support all metadata types. Unsupported metadata
    /// is silently ignored — GIF ignores all metadata, AVIF encode only
    /// supports EXIF, etc.
    pub fn with_metadata(mut self, metadata: &'a Metadata) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Set a codec registry to control which formats are enabled.
    pub fn with_registry(mut self, registry: &'a CodecRegistry) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Set format-specific codec configuration.
    ///
    /// When set, the relevant codec's config overrides the generic
    /// quality/effort parameters for that format.
    pub fn with_codec_config(mut self, config: &'a CodecConfig) -> Self {
        self.codec_config = Some(config);
        self
    }

    /// Set a named quality profile instead of a raw quality value.
    ///
    /// Quality profiles map to per-codec calibrated settings via
    /// imageflow's perceptual tuning tables. When both `with_quality()`
    /// and `with_quality_profile()` are set, the profile takes precedence.
    pub fn with_quality_profile(mut self, profile: QualityProfile) -> Self {
        self.quality_profile = Some(profile);
        self
    }

    /// Set device pixel ratio for quality adjustment.
    ///
    /// At DPR 1.0, artifacts are magnified 3x → quality increases.
    /// At DPR 6.0, pixels are tiny → quality can decrease.
    /// Baseline is 3.0 (no adjustment). Only affects profile-based quality.
    pub fn with_dpr(mut self, dpr: f32) -> Self {
        self.dpr = Some(dpr);
        self
    }

    /// Set a per-request codec policy for filtering and preferences.
    ///
    /// The policy controls which codec implementations are available
    /// and which output formats are candidates for auto-selection.
    pub fn with_policy(mut self, policy: CodecPolicy) -> Self {
        self.policy = Some(policy);
        self
    }

    /// Set image facts for better format auto-selection.
    ///
    /// When using `auto()`, providing facts about the source image
    /// improves format selection (e.g., preferring AVIF for small images,
    /// JPEG for large opaque images).
    pub fn with_image_facts(mut self, facts: ImageFacts) -> Self {
        self.image_facts = Some(facts);
        self
    }

    /// Set the quality for the UltraHDR gain map JPEG (0-100).
    ///
    /// Only used by `encode_ultrahdr_rgb_f32` / `encode_ultrahdr_rgba_f32`.
    /// Defaults to 75.0 if not set.
    #[cfg(feature = "jpeg-ultrahdr")]
    pub fn with_gainmap_quality(mut self, quality: f32) -> Self {
        self.gainmap_quality = Some(quality);
        self
    }

    /// Attach a gain map to the encoded output.
    ///
    /// The gain map will be embedded in a format-appropriate way:
    /// - **JPEG**: Embedded as UltraHDR (MPF secondary image + XMP metadata)
    /// - **JXL**: Embedded as jhgm box (requires `jxl-encode` + `jxl-decode` features)
    /// - **AVIF**: Embedded as tmap item (future, not yet implemented)
    ///
    /// Currently only [`GainMapSource::Precomputed`] is supported.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zencodecs::{EncodeRequest, ImageFormat, GainMapSource};
    /// use zencodecs::gainmap::GainMapImage;
    /// use zencodecs::GainMapMetadata;
    ///
    /// # fn example(gain_map: &GainMapImage, metadata: &zencodecs::GainMapMetadata) {
    /// let request = EncodeRequest::new(ImageFormat::Jpeg)
    ///     .with_quality(85.0)
    ///     .with_gain_map(GainMapSource::Precomputed {
    ///         gain_map: &gain_map,
    ///         metadata: &metadata,
    ///     });
    /// # }
    /// ```
    #[cfg(feature = "jpeg-ultrahdr")]
    pub fn with_gain_map(mut self, source: crate::gainmap::GainMapSource<'a>) -> Self {
        self.gain_map_source = Some(source);
        self
    }

    // ═══════════════════════════════════════════════════════════════════
    // UltraHDR encode (JPEG-specific, bypasses dispatch)
    // ═══════════════════════════════════════════════════════════════════

    /// Encode linear f32 RGB pixels to UltraHDR JPEG.
    ///
    /// Takes HDR content in linear f32 RGB and produces a backward-compatible
    /// UltraHDR JPEG with embedded gain map. The `quality` setting controls
    /// the SDR base JPEG quality.
    #[cfg(feature = "jpeg-ultrahdr")]
    pub fn encode_ultrahdr_rgb_f32(self, img: ImgRef<Rgb<f32>>) -> Result<EncodeOutput> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);

        if !registry.can_encode(ImageFormat::Jpeg) {
            return Err(at!(CodecError::DisabledFormat(ImageFormat::Jpeg)));
        }

        crate::codecs::jpeg::encode_ultrahdr_rgb_f32(
            img,
            self.quality,
            self.gainmap_quality,
            self.metadata,
            self.codec_config,
            self.limits,
            self.stop,
        )
    }

    /// Encode linear f32 RGBA pixels to UltraHDR JPEG.
    ///
    /// Takes HDR content in linear f32 RGBA and produces a backward-compatible
    /// UltraHDR JPEG with embedded gain map. Alpha is discarded.
    /// The `quality` setting controls the SDR base JPEG quality.
    #[cfg(feature = "jpeg-ultrahdr")]
    pub fn encode_ultrahdr_rgba_f32(self, img: ImgRef<Rgba<f32>>) -> Result<EncodeOutput> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);

        if !registry.can_encode(ImageFormat::Jpeg) {
            return Err(at!(CodecError::DisabledFormat(ImageFormat::Jpeg)));
        }

        crate::codecs::jpeg::encode_ultrahdr_rgba_f32(
            img,
            self.quality,
            self.gainmap_quality,
            self.metadata,
            self.codec_config,
            self.limits,
            self.stop,
        )
    }

    // ═══════════════════════════════════════════════════════════════════
    // Animation encode
    // ═══════════════════════════════════════════════════════════════════

    /// Create a full-frame animation encoder.
    ///
    /// Push frames sequentially, then call `finish()` to get the encoded output.
    /// Supported formats: GIF, WebP, PNG (APNG).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zencodecs::{EncodeRequest, ImageFormat};
    /// use zenpixels::PixelSlice;
    ///
    /// let mut encoder = EncodeRequest::new(ImageFormat::Gif)
    ///     .with_quality(80.0)
    ///     .full_frame_encoder(320, 240)?;
    /// // encoder.push_frame(pixels, delay_ms, None)?;
    /// // let output = encoder.finish(None)?;
    /// # Ok::<(), whereat::At<zencodecs::CodecError>>(())
    /// ```
    pub fn full_frame_encoder(
        self,
        width: u32,
        height: u32,
    ) -> Result<alloc::boxed::Box<dyn zencodec::encode::DynFullFrameEncoder>> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);

        let format = match self.format {
            Some(f) => f,
            None => {
                return Err(at!(CodecError::InvalidInput(
                    "animation encode requires an explicit format (use new(), not auto())".into(),
                )));
            }
        };

        if !registry.can_encode(format) {
            return Err(at!(CodecError::DisabledFormat(format)));
        }

        let resolved_quality = self.resolve_quality();

        crate::dyn_dispatch::dyn_full_frame_encoder(
            format,
            crate::dyn_dispatch::AnimEncodeParams {
                quality: Some(resolved_quality),
                effort: self.effort,
                lossless: self.lossless,
                metadata: self.metadata,
                codec_config: self.codec_config,
                limits: self.limits,
                _stop: self.stop,
                canvas_width: width,
                canvas_height: height,
                loop_count: None,
            },
        )
    }

    // ═══════════════════════════════════════════════════════════════════
    // Build raw encoder for pipeline use
    // ═══════════════════════════════════════════════════════════════════

    // ═══════════════════════════════════════════════════════════════════
    // Typed encode methods — thin wrappers over dispatch
    // ═══════════════════════════════════════════════════════════════════

    /// Encode RGB8 pixels.
    pub fn encode_rgb8(self, img: ImgRef<Rgb<u8>>) -> Result<EncodeOutput> {
        let data: &[u8] = bytemuck::cast_slice(img.buf());
        let stride = img.stride() * core::mem::size_of::<Rgb<u8>>();
        self.encode_dispatch(
            data,
            PixelDescriptor::RGB8_SRGB,
            img.width() as u32,
            img.height() as u32,
            stride,
            false,
        )
    }

    /// Encode RGBA8 pixels.
    pub fn encode_rgba8(self, img: ImgRef<Rgba<u8>>) -> Result<EncodeOutput> {
        let has_alpha = img.pixels().any(|p| p.a < 255);
        let data: &[u8] = bytemuck::cast_slice(img.buf());
        let stride = img.stride() * core::mem::size_of::<Rgba<u8>>();
        self.encode_dispatch(
            data,
            PixelDescriptor::RGBA8_SRGB,
            img.width() as u32,
            img.height() as u32,
            stride,
            has_alpha,
        )
    }

    /// Encode sRGB RGBA8 pixels (straight alpha, not premultiplied).
    ///
    /// When `ignore_alpha` is true, the alpha channel is treated as padding —
    /// codecs may use RGB-only paths and skip alpha handling entirely.
    /// Use this when all alpha values are 255 or when alpha is irrelevant.
    ///
    /// When `ignore_alpha` is false, alpha is preserved as straight
    /// (unassociated) alpha. Codecs that don't support alpha (e.g. JPEG)
    /// will discard it during pixel format negotiation.
    ///
    /// Pixels must be straight (non-premultiplied) alpha. Premultiplied
    /// input will produce wrong output — unpremultiply first.
    pub fn encode_srgba8_imgref(
        self,
        img: ImgRef<Rgba<u8>>,
        ignore_alpha: bool,
    ) -> Result<EncodeOutput> {
        let descriptor = if ignore_alpha {
            PixelDescriptor::RGBA8_SRGB.with_alpha(Some(AlphaMode::Undefined))
        } else {
            PixelDescriptor::RGBA8_SRGB
        };
        let data: &[u8] = bytemuck::cast_slice(img.buf());
        let stride = img.stride() * core::mem::size_of::<Rgba<u8>>();
        self.encode_dispatch(
            data,
            descriptor,
            img.width() as u32,
            img.height() as u32,
            stride,
            !ignore_alpha,
        )
    }

    /// Encode BGRA8 pixels (native byte order, zero-copy for codecs that support it).
    pub fn encode_bgra8(self, img: ImgRef<Bgra<u8>>) -> Result<EncodeOutput> {
        let has_alpha = img.pixels().any(|p| p.a < 255);
        let data: &[u8] = bytemuck::cast_slice(img.buf());
        let stride = img.stride() * core::mem::size_of::<Bgra<u8>>();
        self.encode_dispatch(
            data,
            PixelDescriptor::BGRA8_SRGB,
            img.width() as u32,
            img.height() as u32,
            stride,
            has_alpha,
        )
    }

    /// Encode BGRX8 pixels (opaque BGRA — padding byte is ignored).
    pub fn encode_bgrx8(self, img: ImgRef<Bgra<u8>>) -> Result<EncodeOutput> {
        let data: &[u8] = bytemuck::cast_slice(img.buf());
        let stride = img.stride() * core::mem::size_of::<Bgra<u8>>();
        self.encode_dispatch(
            data,
            PixelDescriptor::BGRX8_SRGB,
            img.width() as u32,
            img.height() as u32,
            stride,
            false,
        )
    }

    /// Encode Gray8 pixels.
    pub fn encode_gray8(self, img: ImgRef<Gray<u8>>) -> Result<EncodeOutput> {
        let data: &[u8] = bytemuck::cast_slice(img.buf());
        let stride = img.stride() * core::mem::size_of::<Gray<u8>>();
        self.encode_dispatch(
            data,
            PixelDescriptor::GRAY8_SRGB,
            img.width() as u32,
            img.height() as u32,
            stride,
            false,
        )
    }

    /// Encode linear RGB f32 pixels.
    ///
    /// Input is expected in linear light (not sRGB gamma). Codecs that store
    /// sRGB will convert internally.
    pub fn encode_rgb_f32(self, img: ImgRef<Rgb<f32>>) -> Result<EncodeOutput> {
        let data: &[u8] = bytemuck::cast_slice(img.buf());
        let stride = img.stride() * core::mem::size_of::<Rgb<f32>>();
        self.encode_dispatch(
            data,
            PixelDescriptor::RGBF32_LINEAR,
            img.width() as u32,
            img.height() as u32,
            stride,
            false,
        )
    }

    /// Encode linear RGBA f32 pixels.
    ///
    /// Input is expected in linear light (not sRGB gamma). Codecs that store
    /// sRGB will convert internally.
    pub fn encode_rgba_f32(self, img: ImgRef<Rgba<f32>>) -> Result<EncodeOutput> {
        let has_alpha = img.pixels().any(|p| p.a < 1.0);
        let data: &[u8] = bytemuck::cast_slice(img.buf());
        let stride = img.stride() * core::mem::size_of::<Rgba<f32>>();
        self.encode_dispatch(
            data,
            PixelDescriptor::RGBAF32_LINEAR,
            img.width() as u32,
            img.height() as u32,
            stride,
            has_alpha,
        )
    }

    /// Encode linear grayscale f32 pixels.
    ///
    /// Input is expected in linear light (not sRGB gamma). Codecs that store
    /// sRGB will convert internally.
    pub fn encode_gray_f32(self, img: ImgRef<Gray<f32>>) -> Result<EncodeOutput> {
        let data: &[u8] = bytemuck::cast_slice(img.buf());
        let stride = img.stride() * core::mem::size_of::<Gray<f32>>();
        self.encode_dispatch(
            data,
            PixelDescriptor::GRAYF32_LINEAR,
            img.width() as u32,
            img.height() as u32,
            stride,
            false,
        )
    }

    // ═══════════════════════════════════════════════════════════════════
    // Core dispatch
    // ═══════════════════════════════════════════════════════════════════

    /// Resolve the effective quality value.
    ///
    /// Priority: quality_profile (with optional DPR) > raw quality > default (Good profile).
    fn resolve_quality(&self) -> f32 {
        if let Some(profile) = self.quality_profile {
            let intent = match self.dpr {
                Some(dpr) => profile.to_intent_with_dpr(dpr),
                None => profile.to_intent(),
            };
            intent.quality
        } else if let Some(q) = self.quality {
            q
        } else {
            QualityProfile::default().generic_quality()
        }
    }

    /// Build a [`QualityIntent`] from the request's quality settings.
    pub fn quality_intent(&self) -> QualityIntent {
        let mut intent = if let Some(profile) = self.quality_profile {
            match self.dpr {
                Some(dpr) => profile.to_intent_with_dpr(dpr),
                None => profile.to_intent(),
            }
        } else if let Some(q) = self.quality {
            QualityIntent::from_quality(q)
        } else {
            QualityIntent::default()
        };
        if let Some(e) = self.effort {
            intent = intent.with_effort(e);
        }
        if self.lossless {
            intent = intent.with_lossless(true);
        }
        intent
    }

    /// Common encode path: resolve format → validate → build encoder →
    /// negotiate pixel format via zenpixels → encode.
    fn encode_dispatch(
        self,
        data: &[u8],
        descriptor: PixelDescriptor,
        width: u32,
        height: u32,
        stride: usize,
        has_alpha: bool,
    ) -> Result<EncodeOutput> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let default_policy = CodecPolicy::new();
        let policy = self.policy.as_ref().unwrap_or(&default_policy);

        let format = match self.format {
            Some(f) => f,
            None => {
                // Use the new format selection engine
                let facts = self.image_facts.clone().unwrap_or(ImageFacts {
                    has_alpha,
                    pixel_count: width as u64 * height as u64,
                    ..Default::default()
                });
                let intent = self.quality_intent();
                crate::select::select_format(&facts, &intent, registry, policy)?.format
            }
        };

        if !registry.can_encode(format) {
            return Err(at!(CodecError::DisabledFormat(format)));
        }
        if self.lossless && !format.supports_lossless() {
            return Err(at!(CodecError::UnsupportedOperation {
                format,
                detail: "lossless encoding not supported",
            }));
        }

        let resolved_quality = self.resolve_quality();

        let params = EncodeParams {
            quality: Some(resolved_quality),
            effort: self.effort,
            lossless: self.lossless,
            metadata: self.metadata,
            codec_config: self.codec_config,
            limits: self.limits,
            stop: self.stop,
        };

        let built = crate::dispatch::build_encoder(format, params)?;

        // Use zenpixels to negotiate the cheapest pixel format conversion.
        // Returns Cow::Borrowed (zero-copy) when the input already matches
        // one of the encoder's supported formats.
        let adapted = zenpixels_convert::adapt::adapt_for_encode(
            data,
            descriptor,
            width,
            height,
            stride,
            built.supported,
        )
        .map_err(|e| {
            at!(CodecError::InvalidInput(alloc::format!(
                "pixel format negotiation: {e}"
            )))
        })?;

        // Adapted data is always packed (stride = width * bpp).
        let adapted_stride = adapted.width as usize * adapted.descriptor.bytes_per_pixel();

        let pixel_slice = zenpixels::PixelSlice::new(
            &adapted.data,
            adapted.width,
            adapted.rows,
            adapted_stride,
            adapted.descriptor,
        )
        .map_err(|e| at!(CodecError::InvalidInput(alloc::format!("pixel slice: {e}"))))?;

        // Check if we should embed a precomputed gain map
        #[cfg(feature = "jpeg-ultrahdr")]
        if let Some(crate::gainmap::GainMapSource::Precomputed {
            gain_map,
            metadata,
        }) = &self.gain_map_source
        {
            if format == ImageFormat::Jpeg {
                // For JPEG: use the specialized gain map encoder that produces
                // UltraHDR JPEG (base + gain map + XMP metadata)
                let channels = if adapted.descriptor.layout() == zenpixels::ChannelLayout::Rgba {
                    4u8
                } else {
                    3u8
                };
                return crate::codecs::jpeg::encode_with_precomputed_gainmap(
                    &adapted.data,
                    adapted.width,
                    adapted.rows,
                    channels,
                    Some(resolved_quality),
                    self.codec_config,
                    gain_map,
                    metadata,
                    self.stop,
                );
            }
            #[cfg(all(feature = "jxl-encode", feature = "jxl-decode"))]
            if format == ImageFormat::Jxl {
                return crate::codecs::jxl_enc::encode_with_precomputed_gainmap(
                    &adapted.data,
                    adapted.width,
                    adapted.rows,
                    adapted.descriptor,
                    Some(resolved_quality),
                    gain_map,
                    metadata,
                    self.stop,
                );
            }
            #[cfg(all(feature = "avif-encode", feature = "avif-decode"))]
            if format == ImageFormat::Avif {
                return crate::codecs::avif_enc::encode_with_precomputed_gainmap(
                    &adapted.data,
                    adapted.width,
                    adapted.rows,
                    adapted.descriptor,
                    Some(resolved_quality),
                    self.effort,
                    self.codec_config,
                    gain_map,
                    metadata,
                    self.limits,
                    self.stop,
                );
            }
            return Err(at!(CodecError::UnsupportedOperation {
                format,
                detail: "gain map embedding not supported for this format",
            }));
        }

        (built.encoder)(pixel_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn builder_pattern() {
        let request = EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality(85.0)
            .with_lossless(false);

        assert_eq!(request.format, Some(ImageFormat::Jpeg));
        assert_eq!(request.quality, Some(85.0));
        assert!(!request.lossless);
    }

    #[test]
    fn lossless_with_jpeg_error() {
        let img = imgref::ImgVec::new(
            vec![
                Rgb {
                    r: 255u8,
                    g: 255,
                    b: 255
                };
                100 * 100
            ],
            100,
            100,
        );
        let result = EncodeRequest::new(ImageFormat::Jpeg)
            .with_lossless(true)
            .encode_rgb8(img.as_ref());

        assert!(matches!(
            result.as_ref().map_err(|e| e.error()),
            Err(CodecError::UnsupportedOperation { .. })
        ));
    }

    #[test]
    fn codec_config_builder() {
        let config = CodecConfig::default();
        let _request = EncodeRequest::new(ImageFormat::Jpeg).with_codec_config(&config);
    }

    #[test]
    fn metadata_builder() {
        let meta = Metadata::none()
            .with_icc(b"fake_icc".as_slice())
            .with_exif(b"fake_exif".as_slice())
            .with_xmp(b"fake_xmp".as_slice());
        let _request = EncodeRequest::new(ImageFormat::Jpeg).with_metadata(&meta);
    }

    #[test]
    fn encode_srgba8_imgref_opaque() {
        let img = imgref::ImgVec::new(
            vec![
                Rgba {
                    r: 128u8,
                    g: 64,
                    b: 32,
                    a: 255
                };
                10 * 10
            ],
            10,
            10,
        );
        let output = EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality(75.0)
            .encode_srgba8_imgref(img.as_ref(), true)
            .unwrap();
        assert!(!output.data().is_empty());
    }

    #[test]
    #[cfg(feature = "webp")]
    fn encode_srgba8_imgref_straight() {
        let img = imgref::ImgVec::new(
            vec![
                Rgba {
                    r: 128u8,
                    g: 64,
                    b: 32,
                    a: 200
                };
                10 * 10
            ],
            10,
            10,
        );
        // WebP supports alpha
        let output = EncodeRequest::new(ImageFormat::WebP)
            .with_quality(75.0)
            .encode_srgba8_imgref(img.as_ref(), false)
            .unwrap();
        assert!(!output.data().is_empty());
    }

    /// Zero generics — `&dyn AnyEncoder` is fully codec-agnostic.
    #[test]
    #[cfg(all(feature = "jpeg", feature = "webp"))]
    fn any_encoder_dyn_dispatch() {
        use crate::AnyEncoder;
        use zencodec::encode::EncoderConfig as _;

        // This function has NO generic parameters
        fn encode_with(config: &dyn AnyEncoder, img: imgref::ImgRef<Rgba<u8>>) -> EncodeOutput {
            config.encode_srgba8_imgref(img, true).unwrap()
        }

        let img = imgref::ImgVec::new(
            vec![
                Rgba {
                    r: 128u8,
                    g: 64,
                    b: 32,
                    a: 255
                };
                10 * 10
            ],
            10,
            10,
        );

        let jpeg = zenjpeg::JpegEncoderConfig::new().with_generic_quality(75.0);
        let webp = zenwebp::WebpEncoderConfig::lossy();

        let jpeg_out = encode_with(&jpeg, img.as_ref());
        let webp_out = encode_with(&webp, img.as_ref());

        assert_eq!(jpeg_out.format(), ImageFormat::Jpeg);
        assert_eq!(webp_out.format(), ImageFormat::WebP);
        assert!(!jpeg_out.data().is_empty());
        assert!(!webp_out.data().is_empty());
    }

    #[test]
    fn quality_profile_encode() {
        let img = imgref::ImgVec::new(
            vec![
                Rgb {
                    r: 128u8,
                    g: 64,
                    b: 32,
                };
                10 * 10
            ],
            10,
            10,
        );
        let output = EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality_profile(QualityProfile::Good)
            .encode_rgb8(img.as_ref())
            .unwrap();
        assert!(!output.data().is_empty());
    }

    #[test]
    fn quality_profile_with_dpr_encode() {
        let img = imgref::ImgVec::new(
            vec![
                Rgb {
                    r: 128u8,
                    g: 64,
                    b: 32,
                };
                10 * 10
            ],
            10,
            10,
        );
        // DPR 1.0 should increase quality (larger output)
        let high = EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality_profile(QualityProfile::Good)
            .with_dpr(1.0)
            .encode_rgb8(img.as_ref())
            .unwrap();
        // DPR 6.0 should decrease quality (smaller output)
        let low = EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality_profile(QualityProfile::Good)
            .with_dpr(6.0)
            .encode_rgb8(img.as_ref())
            .unwrap();
        // Higher quality should generally produce more bytes
        assert!(
            high.data().len() >= low.data().len(),
            "DPR 1.0 ({} bytes) should be >= DPR 6.0 ({} bytes)",
            high.data().len(),
            low.data().len()
        );
    }

    #[test]
    fn quality_intent_accessor() {
        let req = EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality_profile(QualityProfile::High)
            .with_effort(7)
            .with_lossless(false);
        let intent = req.quality_intent();
        assert!((intent.quality - 91.0).abs() < 0.5);
        assert_eq!(intent.effort, Some(7));
        assert!(!intent.lossless);
    }

    #[test]
    fn auto_with_policy() {
        let img = imgref::ImgVec::new(
            vec![
                Rgb {
                    r: 128u8,
                    g: 64,
                    b: 32,
                };
                10 * 10
            ],
            10,
            10,
        );
        let output = EncodeRequest::auto()
            .with_quality(75.0)
            .with_policy(CodecPolicy::web_safe_output())
            .encode_rgb8(img.as_ref())
            .unwrap();
        // web_safe_output only allows JPEG, PNG, GIF — for opaque lossy, JPEG wins
        assert_eq!(output.format(), ImageFormat::Jpeg);
    }

    #[test]
    fn auto_with_image_facts() {
        let img = imgref::ImgVec::new(
            vec![
                Rgba {
                    r: 128u8,
                    g: 64,
                    b: 32,
                    a: 128,
                };
                10 * 10
            ],
            10,
            10,
        );
        let facts = ImageFacts {
            has_alpha: true,
            pixel_count: 100,
            ..Default::default()
        };
        let output = EncodeRequest::auto()
            .with_quality(75.0)
            .with_image_facts(facts)
            .encode_rgba8(img.as_ref())
            .unwrap();
        // With alpha, should pick a format that supports alpha (not JPEG)
        assert_ne!(output.format(), ImageFormat::Jpeg);
    }
}
