//! Image encoding.
//!
//! Uses [`dispatch::build_encoder`](crate::dispatch::build_encoder) for format dispatch.
//! Each codec's `Encoder` trait impl handles pixel format dispatch internally;
//! pixel format negotiation is handled by [`zenpixels::adapt::adapt_for_encode`].

use crate::config::CodecConfig;
use crate::dispatch::EncodeParams;
use crate::pixel::{Bgra, Gray, ImgRef, Rgb, Rgba};
use crate::{CodecError, CodecRegistry, ImageFormat, Limits, MetadataView, Stop};
use zenpixels::{AlphaMode, PixelDescriptor};

pub use zc::encode::EncodeOutput;

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
    /// Quality for UltraHDR gain map JPEG (0-100). Only used by `encode_ultrahdr_*`.
    #[cfg(feature = "jpeg-ultrahdr")]
    gainmap_quality: Option<f32>,
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
            #[cfg(feature = "jpeg-ultrahdr")]
            gainmap_quality: None,
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
            #[cfg(feature = "jpeg-ultrahdr")]
            gainmap_quality: None,
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

    /// Set the quality for the UltraHDR gain map JPEG (0-100).
    ///
    /// Only used by `encode_ultrahdr_rgb_f32` / `encode_ultrahdr_rgba_f32`.
    /// Defaults to 75.0 if not set.
    #[cfg(feature = "jpeg-ultrahdr")]
    pub fn with_gainmap_quality(mut self, quality: f32) -> Self {
        self.gainmap_quality = Some(quality);
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
    pub fn encode_ultrahdr_rgb_f32(
        self,
        img: ImgRef<Rgb<f32>>,
    ) -> Result<EncodeOutput, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);

        if !registry.can_encode(ImageFormat::Jpeg) {
            return Err(CodecError::DisabledFormat(ImageFormat::Jpeg));
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
    pub fn encode_ultrahdr_rgba_f32(
        self,
        img: ImgRef<Rgba<f32>>,
    ) -> Result<EncodeOutput, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);

        if !registry.can_encode(ImageFormat::Jpeg) {
            return Err(CodecError::DisabledFormat(ImageFormat::Jpeg));
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
    // Typed encode methods — thin wrappers over dispatch
    // ═══════════════════════════════════════════════════════════════════

    /// Encode RGB8 pixels.
    pub fn encode_rgb8(self, img: ImgRef<Rgb<u8>>) -> Result<EncodeOutput, CodecError> {
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
    pub fn encode_rgba8(self, img: ImgRef<Rgba<u8>>) -> Result<EncodeOutput, CodecError> {
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
    ) -> Result<EncodeOutput, CodecError> {
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
    pub fn encode_bgra8(self, img: ImgRef<Bgra<u8>>) -> Result<EncodeOutput, CodecError> {
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
    pub fn encode_bgrx8(self, img: ImgRef<Bgra<u8>>) -> Result<EncodeOutput, CodecError> {
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
    pub fn encode_gray8(self, img: ImgRef<Gray<u8>>) -> Result<EncodeOutput, CodecError> {
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
    pub fn encode_rgb_f32(self, img: ImgRef<Rgb<f32>>) -> Result<EncodeOutput, CodecError> {
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
    pub fn encode_rgba_f32(self, img: ImgRef<Rgba<f32>>) -> Result<EncodeOutput, CodecError> {
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
    pub fn encode_gray_f32(self, img: ImgRef<Gray<f32>>) -> Result<EncodeOutput, CodecError> {
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
    ) -> Result<EncodeOutput, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);

        let format = match self.format {
            Some(f) => f,
            None => self.auto_select_format(has_alpha, registry)?,
        };

        if !registry.can_encode(format) {
            return Err(CodecError::DisabledFormat(format));
        }
        if self.lossless && !format.supports_lossless() {
            return Err(CodecError::UnsupportedOperation {
                format,
                detail: "lossless encoding not supported",
            });
        }

        let params = EncodeParams {
            quality: self.quality,
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
        .map_err(|e| CodecError::InvalidInput(alloc::format!("pixel format negotiation: {e}")))?;

        // Adapted data is always packed (stride = width * bpp).
        let adapted_stride = adapted.width as usize * adapted.descriptor.bytes_per_pixel();

        let pixel_slice = zenpixels::PixelSlice::new(
            &adapted.data,
            adapted.width,
            adapted.rows,
            adapted_stride,
            adapted.descriptor,
        )
        .map_err(|e| CodecError::InvalidInput(alloc::format!("pixel slice: {e}")))?;

        (built.encoder)(pixel_slice)
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
        use zc::encode::EncoderConfig as _;

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
}
