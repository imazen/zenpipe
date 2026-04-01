//! Image encoding.
//!
//! Uses [`dispatch::build_encoder`](crate::dispatch::build_encoder) for format dispatch.
//! Each codec's `Encoder` trait impl handles pixel format dispatch internally;
//! pixel format negotiation is handled by [`zenpixels::adapt::adapt_for_encode`].

use crate::config::CodecConfig;
use crate::dispatch::EncodeParams;
use crate::error::Result;
#[cfg(feature = "jpeg-ultrahdr")]
use crate::pixel::{ImgRef, Rgb, Rgba};
use crate::policy::CodecPolicy;
use crate::quality::{QualityIntent, QualityProfile};
use crate::select::ImageFacts;
use crate::{AllowedFormats, CodecError, ImageFormat, Limits, Metadata, StopToken};
use whereat::at;
use zencodec::encode::EncodePolicy;
use zenpixels::PixelDescriptor;

pub use zencodec::encode::EncodeOutput;

/// Image encode request builder.
///
/// # Example
///
/// ```no_run
/// use zencodecs::{EncodeRequest, ImageFormat};
/// use zenpixels::{PixelBuffer, PixelDescriptor};
///
/// let buf = PixelBuffer::new_fill(100, 100, PixelDescriptor::RGBA8_SRGB, &[0, 0, 0, 255]).unwrap();
/// let output = EncodeRequest::new(ImageFormat::WebP)
///     .with_quality(85.0)
///     .encode(ps, false)?;
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
    stop: Option<StopToken>,
    metadata: Option<Metadata>,
    registry: Option<&'a AllowedFormats>,
    codec_config: Option<&'a CodecConfig>,
    policy: Option<CodecPolicy>,
    encode_policy: Option<EncodePolicy>,
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
            encode_policy: None,
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
            encode_policy: None,
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
    pub fn with_stop(mut self, stop: StopToken) -> Self {
        self.stop = Some(stop);
        self
    }

    /// Set metadata to embed in the output (ICC profile, EXIF, XMP).
    ///
    /// Not all formats support all metadata types. Unsupported metadata
    /// is silently ignored — GIF ignores all metadata, AVIF encode only
    /// supports EXIF, etc.
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Set a codec registry to control which formats are enabled.
    pub fn with_registry(mut self, registry: &'a AllowedFormats) -> Self {
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

    /// Set encode security policy.
    ///
    /// Controls which metadata the encoder embeds in the output.
    /// See [`EncodePolicy`] for details.
    ///
    /// # Example
    ///
    /// ```
    /// use zencodecs::{EncodeRequest, ImageFormat};
    /// use zencodec::encode::EncodePolicy;
    ///
    /// let request = EncodeRequest::new(ImageFormat::Jpeg)
    ///     .with_encode_policy(EncodePolicy::strip_all().with_embed_icc(true));
    /// ```
    pub fn with_encode_policy(mut self, policy: EncodePolicy) -> Self {
        self.encode_policy = Some(policy);
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
    /// use zencodecs::GainMap;
    /// use zencodecs::GainMapMetadata;
    ///
    /// # fn example(gain_map: &GainMap, metadata: &zencodecs::GainMapMetadata) {
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
        let default_registry = AllowedFormats::all();
        let registry = self.registry.unwrap_or(&default_registry);

        if !registry.can_encode(ImageFormat::Jpeg) {
            return Err(at!(CodecError::DisabledFormat(ImageFormat::Jpeg)));
        }

        crate::codecs::jpeg::encode_ultrahdr_rgb_f32(
            img,
            self.quality,
            self.gainmap_quality,
            self.metadata.as_ref(),
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
        let default_registry = AllowedFormats::all();
        let registry = self.registry.unwrap_or(&default_registry);

        if !registry.can_encode(ImageFormat::Jpeg) {
            return Err(at!(CodecError::DisabledFormat(ImageFormat::Jpeg)));
        }

        crate::codecs::jpeg::encode_ultrahdr_rgba_f32(
            img,
            self.quality,
            self.gainmap_quality,
            self.metadata.as_ref(),
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
    ///     .animation_frame_encoder(320, 240)?;
    /// // encoder.push_frame(pixels, delay_ms, None)?;
    /// // let output = encoder.finish(None)?;
    /// # Ok::<(), whereat::At<zencodecs::CodecError>>(())
    /// ```
    pub fn animation_frame_encoder(
        self,
        width: u32,
        height: u32,
    ) -> Result<alloc::boxed::Box<dyn zencodec::encode::DynAnimationFrameEncoder>> {
        let default_registry = AllowedFormats::all();
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

        crate::dyn_dispatch::dyn_animation_frame_encoder(
            format,
            crate::dyn_dispatch::AnimEncodeParams {
                quality: Some(resolved_quality),
                effort: self.effort,
                lossless: self.lossless,
                metadata: self.metadata,
                codec_config: self.codec_config,
                limits: self.limits,
                stop: self.stop,
                encode_policy: self.encode_policy,
                canvas_width: width,
                canvas_height: height,
                loop_count: None,
            },
        )
    }

    // ═══════════════════════════════════════════════════════════════════
    // Build streaming encoder for pipeline use
    // ═══════════════════════════════════════════════════════════════════

    /// Build a streaming encoder without encoding anything.
    ///
    /// Returns a [`StreamingEncoder`] containing:
    /// - A `DynEncoder` that accepts rows via `push_rows()` / `finish()`
    /// - The encoder's `supported` pixel descriptors (for `adapt_for_encode`)
    /// - The resolved output `format`
    ///
    /// The caller is responsible for pixel format conversion per-strip via
    /// [`adapt_for_encode`]. This avoids materializing the full image for
    /// format conversion — only a strip-sized buffer is needed.
    ///
    /// Codecs that require the full image (WebP, AVIF) buffer internally
    /// inside their `push_rows()` implementation. That's the codec's
    /// decision — the pipeline never buffers.
    ///
    /// [`adapt_for_encode`]: zenpixels_convert::adapt::adapt_for_encode
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use zencodecs::{EncodeRequest, ImageFormat};
    /// use zenpixels_convert::adapt::adapt_for_encode;
    ///
    /// let se = EncodeRequest::new(ImageFormat::Jpeg)
    ///     .with_quality(85.0)
    ///     .build_streaming_encoder(1920, 1080)?;
    ///
    /// // Per strip:
    /// let adapted = adapt_for_encode(
    ///     strip_bytes, descriptor, width, strip_rows, stride,
    ///     se.supported,
    /// )?;
    /// let ps = PixelSlice::new(&adapted.data, adapted.width, adapted.rows,
    ///     adapted.width as usize * adapted.descriptor.bytes_per_pixel(),
    ///     adapted.descriptor)?;
    /// se.encoder.push_rows(ps)?;
    ///
    /// // Finalize:
    /// let output = se.encoder.finish()?;
    /// ```
    pub fn build_streaming_encoder(
        self,
        width: u32,
        height: u32,
    ) -> Result<crate::dispatch::StreamingEncoder> {
        let default_registry = AllowedFormats::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let default_policy = CodecPolicy::new();
        let policy = self.policy.as_ref().unwrap_or(&default_policy);

        let format = match self.format {
            Some(f) => f,
            None => {
                let facts = self.image_facts.clone().unwrap_or(ImageFacts {
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

        let params = crate::dispatch::EncodeParams {
            quality: Some(resolved_quality),
            effort: self.effort,
            lossless: self.lossless,
            metadata: self.metadata,
            codec_config: self.codec_config,
            limits: self.limits,
            stop: self.stop,
            encode_policy: self.encode_policy,
        };

        crate::dispatch::build_streaming_encoder(format, params)
    }

    // ═══════════════════════════════════════════════════════════════════
    // One-shot encode
    // ═══════════════════════════════════════════════════════════════════

    /// Encode pixels (one-shot, full materialization).
    ///
    /// `pixels` is a type-erased pixel buffer with descriptor, dimensions,
    /// and stride. Construct via [`PixelSlice::new()`](zenpixels::PixelSlice::new).
    ///
    /// `has_meaningful_alpha` tells the auto-format selector whether the image
    /// has alpha that must be preserved. When `true`, JPEG (which doesn't support
    /// alpha) is excluded from auto-selection. When `false`, the alpha channel
    /// (if any) is treated as padding. This flag is ignored when `format` is
    /// explicitly set.
    ///
    /// For streaming encode, use [`build_streaming_encoder`](Self::build_streaming_encoder).
    pub fn encode(
        self,
        pixels: zenpixels::PixelSlice<'_>,
        has_meaningful_alpha: bool,
    ) -> Result<EncodeOutput> {
        self.encode_dispatch(
            pixels.as_strided_bytes(),
            pixels.descriptor(),
            pixels.width(),
            pixels.rows(),
            pixels.stride(),
            has_meaningful_alpha,
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

    // ═══════════════════════════════════════════════════════════════════
    // Core dispatch
    // ═══════════════════════════════════════════════════════════════════

    /// Internal: resolve format → validate → build encoder →
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
        let default_registry = AllowedFormats::all();
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
            stop: self.stop.clone(),
            encode_policy: self.encode_policy,
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
        if let Some(crate::gainmap::GainMapSource::Precomputed { gain_map, metadata }) =
            &self.gain_map_source
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
                    self.stop.as_ref(),
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
                    self.stop.as_ref(),
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
    use rgb::{Rgb, Rgba};

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
        let ps = zenpixels::PixelSlice::from(img.as_ref()).erase();
        let result = EncodeRequest::new(ImageFormat::Jpeg)
            .with_lossless(true)
            .encode(ps, false);

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
        let _request = EncodeRequest::new(ImageFormat::Jpeg).with_metadata(meta);
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
        let ps = zenpixels::PixelSlice::from(img.as_ref()).erase();
        let output = EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality(75.0)
            .encode(ps, false)
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
        let ps = zenpixels::PixelSlice::from(img.as_ref()).erase();
        let output = EncodeRequest::new(ImageFormat::WebP)
            .with_quality(75.0)
            .encode(ps, true)
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
        let webp = zenwebp::zencodec::WebpEncoderConfig::lossy();

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
        let ps = zenpixels::PixelSlice::from(img.as_ref()).erase();
        let output = EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality_profile(QualityProfile::Good)
            .encode(ps, false)
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
            .encode(zenpixels::PixelSlice::from(img.as_ref()).erase(), false)
            .unwrap();
        // DPR 6.0 should decrease quality (smaller output)
        let low = EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality_profile(QualityProfile::Good)
            .with_dpr(6.0)
            .encode(zenpixels::PixelSlice::from(img.as_ref()).erase(), false)
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
        let ps = zenpixels::PixelSlice::from(img.as_ref()).erase();
        let output = EncodeRequest::auto()
            .with_quality(75.0)
            .with_policy(CodecPolicy::web_safe_output())
            .encode(ps, false)
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
        let ps = zenpixels::PixelSlice::from(img.as_ref()).erase();
        let output = EncodeRequest::auto()
            .with_quality(75.0)
            .with_image_facts(facts)
            .encode(ps, true)
            .unwrap();
        // With alpha, should pick a format that supports alpha (not JPEG)
        assert_ne!(output.format(), ImageFormat::Jpeg);
    }
}
