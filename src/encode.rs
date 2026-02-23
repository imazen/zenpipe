//! Image encoding.

use crate::config::CodecConfig;
use crate::pixel::{Bgra, Gray, ImgRef, Rgb, Rgba};
use crate::{CodecError, CodecRegistry, ImageFormat, MetadataView, Limits, Stop};

pub use zencodec_types::EncodeOutput;

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
/// # Ok::<(), zencodecs::CodecError>(())
/// ```
pub struct EncodeRequest<'a> {
    format: Option<ImageFormat>,
    quality: Option<f32>,
    effort: Option<u32>,
    lossless: bool,
    limits: Option<&'a Limits>,
    stop: Option<&'a dyn Stop>,
    metadata: Option<&'a MetadataView<'a>>,
    registry: Option<&'a CodecRegistry>,
    codec_config: Option<&'a CodecConfig>,
}

impl<'a> EncodeRequest<'a> {
    /// Encode to a specific format.
    pub fn new(format: ImageFormat) -> Self {
        Self {
            format: Some(format),
            quality: None,
            effort: None,
            lossless: false,
            limits: None,
            stop: None,
            metadata: None,
            registry: None,
            codec_config: None,
        }
    }

    /// Auto-select best format based on image stats and allowed encoders.
    pub fn auto() -> Self {
        Self {
            format: None,
            quality: None,
            effort: None,
            lossless: false,
            limits: None,
            stop: None,
            metadata: None,
            registry: None,
            codec_config: None,
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
    pub fn with_metadata(mut self, metadata: &'a MetadataView<'a>) -> Self {
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

    /// Encode RGB8 pixels.
    pub fn encode_rgb8(self, img: ImgRef<Rgb<u8>>) -> Result<EncodeOutput, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let has_alpha = false;

        let format = match self.format {
            Some(f) => f,
            None => self.auto_select_format(has_alpha, registry)?,
        };

        self.validate_and_dispatch_rgb8(format, img, registry)
    }

    /// Encode RGBA8 pixels.
    pub fn encode_rgba8(self, img: ImgRef<Rgba<u8>>) -> Result<EncodeOutput, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);

        let has_alpha = img.pixels().any(|p| p.a < 255);

        let format = match self.format {
            Some(f) => f,
            None => self.auto_select_format(has_alpha, registry)?,
        };

        self.validate_and_dispatch_rgba8(format, img, registry)
    }

    /// Encode BGRA8 pixels (native byte order, zero-copy for codecs that support it).
    pub fn encode_bgra8(self, img: ImgRef<Bgra<u8>>) -> Result<EncodeOutput, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);

        let has_alpha = img.pixels().any(|p| p.a < 255);

        let format = match self.format {
            Some(f) => f,
            None => self.auto_select_format(has_alpha, registry)?,
        };

        self.validate_and_dispatch_bgra8(format, img, registry)
    }

    /// Encode BGRX8 pixels (opaque BGRA — padding byte is ignored).
    pub fn encode_bgrx8(self, img: ImgRef<Bgra<u8>>) -> Result<EncodeOutput, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let has_alpha = false;

        let format = match self.format {
            Some(f) => f,
            None => self.auto_select_format(has_alpha, registry)?,
        };

        self.validate_and_dispatch_bgrx8(format, img, registry)
    }

    /// Encode Gray8 pixels.
    pub fn encode_gray8(self, img: ImgRef<Gray<u8>>) -> Result<EncodeOutput, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let has_alpha = false;

        let format = match self.format {
            Some(f) => f,
            None => self.auto_select_format(has_alpha, registry)?,
        };

        self.validate_and_dispatch_gray8(format, img, registry)
    }

    /// Encode linear RGB f32 pixels.
    ///
    /// Input is expected in linear light (not sRGB gamma). Codecs that store
    /// sRGB will convert internally.
    pub fn encode_rgb_f32(self, img: ImgRef<Rgb<f32>>) -> Result<EncodeOutput, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let has_alpha = false;

        let format = match self.format {
            Some(f) => f,
            None => self.auto_select_format(has_alpha, registry)?,
        };

        self.validate_and_dispatch_rgb_f32(format, img, registry)
    }

    /// Encode linear RGBA f32 pixels.
    ///
    /// Input is expected in linear light (not sRGB gamma). Codecs that store
    /// sRGB will convert internally.
    pub fn encode_rgba_f32(self, img: ImgRef<Rgba<f32>>) -> Result<EncodeOutput, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);

        let has_alpha = img.pixels().any(|p| p.a < 1.0);

        let format = match self.format {
            Some(f) => f,
            None => self.auto_select_format(has_alpha, registry)?,
        };

        self.validate_and_dispatch_rgba_f32(format, img, registry)
    }

    /// Encode linear grayscale f32 pixels.
    ///
    /// Input is expected in linear light (not sRGB gamma). Codecs that store
    /// sRGB will convert internally.
    pub fn encode_gray_f32(self, img: ImgRef<Gray<f32>>) -> Result<EncodeOutput, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let has_alpha = false;

        let format = match self.format {
            Some(f) => f,
            None => self.auto_select_format(has_alpha, registry)?,
        };

        self.validate_and_dispatch_gray_f32(format, img, registry)
    }

    fn validate_and_dispatch_rgb8(
        self,
        format: ImageFormat,
        img: ImgRef<Rgb<u8>>,
        registry: &CodecRegistry,
    ) -> Result<EncodeOutput, CodecError> {
        if !registry.can_encode(format) {
            return Err(CodecError::DisabledFormat(format));
        }
        if self.lossless && !format.supports_lossless() {
            return Err(CodecError::UnsupportedOperation {
                format,
                detail: "lossless encoding not supported",
            });
        }
        self.encode_format_rgb8(format, img)
    }

    fn validate_and_dispatch_rgba8(
        self,
        format: ImageFormat,
        img: ImgRef<Rgba<u8>>,
        registry: &CodecRegistry,
    ) -> Result<EncodeOutput, CodecError> {
        if !registry.can_encode(format) {
            return Err(CodecError::DisabledFormat(format));
        }
        if self.lossless && !format.supports_lossless() {
            return Err(CodecError::UnsupportedOperation {
                format,
                detail: "lossless encoding not supported",
            });
        }
        self.encode_format_rgba8(format, img)
    }

    fn validate_and_dispatch_bgra8(
        self,
        format: ImageFormat,
        img: ImgRef<Bgra<u8>>,
        registry: &CodecRegistry,
    ) -> Result<EncodeOutput, CodecError> {
        if !registry.can_encode(format) {
            return Err(CodecError::DisabledFormat(format));
        }
        if self.lossless && !format.supports_lossless() {
            return Err(CodecError::UnsupportedOperation {
                format,
                detail: "lossless encoding not supported",
            });
        }
        self.encode_format_bgra8(format, img)
    }

    fn validate_and_dispatch_bgrx8(
        self,
        format: ImageFormat,
        img: ImgRef<Bgra<u8>>,
        registry: &CodecRegistry,
    ) -> Result<EncodeOutput, CodecError> {
        if !registry.can_encode(format) {
            return Err(CodecError::DisabledFormat(format));
        }
        if self.lossless && !format.supports_lossless() {
            return Err(CodecError::UnsupportedOperation {
                format,
                detail: "lossless encoding not supported",
            });
        }
        self.encode_format_bgrx8(format, img)
    }

    fn validate_and_dispatch_gray8(
        self,
        format: ImageFormat,
        img: ImgRef<Gray<u8>>,
        registry: &CodecRegistry,
    ) -> Result<EncodeOutput, CodecError> {
        if !registry.can_encode(format) {
            return Err(CodecError::DisabledFormat(format));
        }
        if self.lossless && !format.supports_lossless() {
            return Err(CodecError::UnsupportedOperation {
                format,
                detail: "lossless encoding not supported",
            });
        }
        self.encode_format_gray8(format, img)
    }

    fn validate_and_dispatch_rgb_f32(
        self,
        format: ImageFormat,
        img: ImgRef<Rgb<f32>>,
        registry: &CodecRegistry,
    ) -> Result<EncodeOutput, CodecError> {
        if !registry.can_encode(format) {
            return Err(CodecError::DisabledFormat(format));
        }
        if self.lossless && !format.supports_lossless() {
            return Err(CodecError::UnsupportedOperation {
                format,
                detail: "lossless encoding not supported",
            });
        }
        self.encode_format_rgb_f32(format, img)
    }

    fn validate_and_dispatch_rgba_f32(
        self,
        format: ImageFormat,
        img: ImgRef<Rgba<f32>>,
        registry: &CodecRegistry,
    ) -> Result<EncodeOutput, CodecError> {
        if !registry.can_encode(format) {
            return Err(CodecError::DisabledFormat(format));
        }
        if self.lossless && !format.supports_lossless() {
            return Err(CodecError::UnsupportedOperation {
                format,
                detail: "lossless encoding not supported",
            });
        }
        self.encode_format_rgba_f32(format, img)
    }

    fn validate_and_dispatch_gray_f32(
        self,
        format: ImageFormat,
        img: ImgRef<Gray<f32>>,
        registry: &CodecRegistry,
    ) -> Result<EncodeOutput, CodecError> {
        if !registry.can_encode(format) {
            return Err(CodecError::DisabledFormat(format));
        }
        if self.lossless && !format.supports_lossless() {
            return Err(CodecError::UnsupportedOperation {
                format,
                detail: "lossless encoding not supported",
            });
        }
        self.encode_format_gray_f32(format, img)
    }

    fn auto_select_format(
        &self,
        has_alpha: bool,
        registry: &CodecRegistry,
    ) -> Result<ImageFormat, CodecError> {
        if self.lossless {
            if registry.can_encode(ImageFormat::WebP) {
                return Ok(ImageFormat::WebP);
            }
            if registry.can_encode(ImageFormat::Png) {
                return Ok(ImageFormat::Png);
            }
        } else if has_alpha {
            if registry.can_encode(ImageFormat::WebP) {
                return Ok(ImageFormat::WebP);
            }
            if registry.can_encode(ImageFormat::Avif) {
                return Ok(ImageFormat::Avif);
            }
            if registry.can_encode(ImageFormat::Png) {
                return Ok(ImageFormat::Png);
            }
        } else {
            if registry.can_encode(ImageFormat::Jpeg) {
                return Ok(ImageFormat::Jpeg);
            }
            if registry.can_encode(ImageFormat::WebP) {
                return Ok(ImageFormat::WebP);
            }
            if registry.can_encode(ImageFormat::Avif) {
                return Ok(ImageFormat::Avif);
            }
        }

        Err(CodecError::NoSuitableEncoder)
    }

    fn encode_format_rgb8(
        self,
        format: ImageFormat,
        img: ImgRef<Rgb<u8>>,
    ) -> Result<EncodeOutput, CodecError> {
        match format {
            #[cfg(feature = "jpeg")]
            ImageFormat::Jpeg => crate::codecs::jpeg::encode_rgb8(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "jpeg"))]
            ImageFormat::Jpeg => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "webp")]
            ImageFormat::WebP => crate::codecs::webp::encode_rgb8(
                img,
                self.quality,
                self.lossless,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "webp"))]
            ImageFormat::WebP => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "gif")]
            ImageFormat::Gif => {
                crate::codecs::gif::encode_rgb8(img, self.codec_config, self.limits, self.stop)
            }
            #[cfg(not(feature = "gif"))]
            ImageFormat::Gif => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "png")]
            ImageFormat::Png => crate::codecs::png::encode_rgb8(
                img,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "png"))]
            ImageFormat::Png => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "avif-encode")]
            ImageFormat::Avif => crate::codecs::avif_enc::encode_rgb8(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "avif-encode"))]
            ImageFormat::Avif => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "jxl-encode")]
            ImageFormat::Jxl => crate::codecs::jxl_enc::encode_rgb8(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "jxl-encode"))]
            ImageFormat::Jxl => Err(CodecError::UnsupportedFormat(format)),

            _ => Err(CodecError::UnsupportedFormat(format)),
        }
    }

    fn encode_format_rgba8(
        self,
        format: ImageFormat,
        img: ImgRef<Rgba<u8>>,
    ) -> Result<EncodeOutput, CodecError> {
        match format {
            #[cfg(feature = "jpeg")]
            ImageFormat::Jpeg => crate::codecs::jpeg::encode_rgba8(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "jpeg"))]
            ImageFormat::Jpeg => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "webp")]
            ImageFormat::WebP => crate::codecs::webp::encode_rgba8(
                img,
                self.quality,
                self.lossless,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "webp"))]
            ImageFormat::WebP => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "gif")]
            ImageFormat::Gif => {
                crate::codecs::gif::encode_rgba8(img, self.codec_config, self.limits, self.stop)
            }
            #[cfg(not(feature = "gif"))]
            ImageFormat::Gif => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "png")]
            ImageFormat::Png => crate::codecs::png::encode_rgba8(
                img,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "png"))]
            ImageFormat::Png => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "avif-encode")]
            ImageFormat::Avif => crate::codecs::avif_enc::encode_rgba8(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "avif-encode"))]
            ImageFormat::Avif => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "jxl-encode")]
            ImageFormat::Jxl => crate::codecs::jxl_enc::encode_rgba8(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "jxl-encode"))]
            ImageFormat::Jxl => Err(CodecError::UnsupportedFormat(format)),

            _ => Err(CodecError::UnsupportedFormat(format)),
        }
    }

    fn encode_format_bgra8(
        self,
        format: ImageFormat,
        img: ImgRef<Bgra<u8>>,
    ) -> Result<EncodeOutput, CodecError> {
        match format {
            #[cfg(feature = "jpeg")]
            ImageFormat::Jpeg => crate::codecs::jpeg::encode_bgra8(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "jpeg"))]
            ImageFormat::Jpeg => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "webp")]
            ImageFormat::WebP => crate::codecs::webp::encode_bgra8(
                img,
                self.quality,
                self.lossless,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "webp"))]
            ImageFormat::WebP => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "jxl-encode")]
            ImageFormat::Jxl => crate::codecs::jxl_enc::encode_bgra8(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "jxl-encode"))]
            ImageFormat::Jxl => Err(CodecError::UnsupportedFormat(format)),

            // Codecs without native BGRA: swizzle to RGBA and delegate.
            _ => {
                let (buf, w, h) = img.to_contiguous_buf();
                let rgba: alloc::vec::Vec<Rgba<u8>> = buf
                    .iter()
                    .map(|p| Rgba {
                        r: p.r,
                        g: p.g,
                        b: p.b,
                        a: p.a,
                    })
                    .collect();
                let rgba_img = imgref::ImgVec::new(rgba, w, h);
                self.encode_format_rgba8(format, rgba_img.as_ref())
            }
        }
    }

    fn encode_format_bgrx8(
        self,
        format: ImageFormat,
        img: ImgRef<Bgra<u8>>,
    ) -> Result<EncodeOutput, CodecError> {
        match format {
            #[cfg(feature = "jpeg")]
            ImageFormat::Jpeg => crate::codecs::jpeg::encode_bgrx8(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "jpeg"))]
            ImageFormat::Jpeg => Err(CodecError::UnsupportedFormat(format)),

            // Codecs without native BGRX: swizzle to RGB and delegate.
            _ => {
                let (buf, w, h) = img.to_contiguous_buf();
                let rgb: alloc::vec::Vec<Rgb<u8>> = buf
                    .iter()
                    .map(|p| Rgb {
                        r: p.r,
                        g: p.g,
                        b: p.b,
                    })
                    .collect();
                let rgb_img = imgref::ImgVec::new(rgb, w, h);
                self.encode_format_rgb8(format, rgb_img.as_ref())
            }
        }
    }

    fn encode_format_gray8(
        self,
        format: ImageFormat,
        img: ImgRef<Gray<u8>>,
    ) -> Result<EncodeOutput, CodecError> {
        match format {
            #[cfg(feature = "jpeg")]
            ImageFormat::Jpeg => crate::codecs::jpeg::encode_gray8(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "jpeg"))]
            ImageFormat::Jpeg => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "webp")]
            ImageFormat::WebP => crate::codecs::webp::encode_gray8(
                img,
                self.quality,
                self.lossless,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "webp"))]
            ImageFormat::WebP => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "gif")]
            ImageFormat::Gif => {
                crate::codecs::gif::encode_gray8(img, self.codec_config, self.limits, self.stop)
            }
            #[cfg(not(feature = "gif"))]
            ImageFormat::Gif => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "png")]
            ImageFormat::Png => crate::codecs::png::encode_gray8(
                img,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "png"))]
            ImageFormat::Png => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "avif-encode")]
            ImageFormat::Avif => crate::codecs::avif_enc::encode_gray8(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "avif-encode"))]
            ImageFormat::Avif => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "jxl-encode")]
            ImageFormat::Jxl => crate::codecs::jxl_enc::encode_gray8(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "jxl-encode"))]
            ImageFormat::Jxl => Err(CodecError::UnsupportedFormat(format)),

            _ => Err(CodecError::UnsupportedFormat(format)),
        }
    }

    fn encode_format_rgb_f32(
        self,
        format: ImageFormat,
        img: ImgRef<Rgb<f32>>,
    ) -> Result<EncodeOutput, CodecError> {
        match format {
            #[cfg(feature = "jpeg")]
            ImageFormat::Jpeg => crate::codecs::jpeg::encode_rgb_f32(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "jpeg"))]
            ImageFormat::Jpeg => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "webp")]
            ImageFormat::WebP => crate::codecs::webp::encode_rgb_f32(
                img,
                self.quality,
                self.lossless,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "webp"))]
            ImageFormat::WebP => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "gif")]
            ImageFormat::Gif => {
                crate::codecs::gif::encode_rgb_f32(img, self.codec_config, self.limits, self.stop)
            }
            #[cfg(not(feature = "gif"))]
            ImageFormat::Gif => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "png")]
            ImageFormat::Png => crate::codecs::png::encode_rgb_f32(
                img,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "png"))]
            ImageFormat::Png => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "avif-encode")]
            ImageFormat::Avif => crate::codecs::avif_enc::encode_rgb_f32(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "avif-encode"))]
            ImageFormat::Avif => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "jxl-encode")]
            ImageFormat::Jxl => crate::codecs::jxl_enc::encode_rgb_f32(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "jxl-encode"))]
            ImageFormat::Jxl => Err(CodecError::UnsupportedFormat(format)),

            _ => Err(CodecError::UnsupportedFormat(format)),
        }
    }

    fn encode_format_rgba_f32(
        self,
        format: ImageFormat,
        img: ImgRef<Rgba<f32>>,
    ) -> Result<EncodeOutput, CodecError> {
        match format {
            #[cfg(feature = "jpeg")]
            ImageFormat::Jpeg => crate::codecs::jpeg::encode_rgba_f32(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "jpeg"))]
            ImageFormat::Jpeg => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "webp")]
            ImageFormat::WebP => crate::codecs::webp::encode_rgba_f32(
                img,
                self.quality,
                self.lossless,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "webp"))]
            ImageFormat::WebP => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "gif")]
            ImageFormat::Gif => {
                crate::codecs::gif::encode_rgba_f32(img, self.codec_config, self.limits, self.stop)
            }
            #[cfg(not(feature = "gif"))]
            ImageFormat::Gif => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "png")]
            ImageFormat::Png => crate::codecs::png::encode_rgba_f32(
                img,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "png"))]
            ImageFormat::Png => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "avif-encode")]
            ImageFormat::Avif => crate::codecs::avif_enc::encode_rgba_f32(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "avif-encode"))]
            ImageFormat::Avif => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "jxl-encode")]
            ImageFormat::Jxl => crate::codecs::jxl_enc::encode_rgba_f32(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "jxl-encode"))]
            ImageFormat::Jxl => Err(CodecError::UnsupportedFormat(format)),

            _ => Err(CodecError::UnsupportedFormat(format)),
        }
    }

    fn encode_format_gray_f32(
        self,
        format: ImageFormat,
        img: ImgRef<Gray<f32>>,
    ) -> Result<EncodeOutput, CodecError> {
        match format {
            #[cfg(feature = "jpeg")]
            ImageFormat::Jpeg => crate::codecs::jpeg::encode_gray_f32(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "jpeg"))]
            ImageFormat::Jpeg => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "webp")]
            ImageFormat::WebP => crate::codecs::webp::encode_gray_f32(
                img,
                self.quality,
                self.lossless,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "webp"))]
            ImageFormat::WebP => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "gif")]
            ImageFormat::Gif => {
                crate::codecs::gif::encode_gray_f32(img, self.codec_config, self.limits, self.stop)
            }
            #[cfg(not(feature = "gif"))]
            ImageFormat::Gif => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "png")]
            ImageFormat::Png => crate::codecs::png::encode_gray_f32(
                img,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "png"))]
            ImageFormat::Png => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "avif-encode")]
            ImageFormat::Avif => crate::codecs::avif_enc::encode_gray_f32(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "avif-encode"))]
            ImageFormat::Avif => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "jxl-encode")]
            ImageFormat::Jxl => crate::codecs::jxl_enc::encode_gray_f32(
                img,
                self.quality,
                self.metadata,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "jxl-encode"))]
            ImageFormat::Jxl => Err(CodecError::UnsupportedFormat(format)),

            _ => Err(CodecError::UnsupportedFormat(format)),
        }
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
            result,
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
        let meta = MetadataView::none()
            .with_icc(b"fake_icc")
            .with_exif(b"fake_exif")
            .with_xmp(b"fake_xmp");
        let _request = EncodeRequest::new(ImageFormat::Jpeg).with_metadata(&meta);
    }
}
