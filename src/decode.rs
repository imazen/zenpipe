//! Image decoding.

pub use zc::decode::DecodeOutput;

use crate::config::CodecConfig;
use crate::{CodecError, CodecRegistry, ImageFormat, ImageInfo, Limits, Stop};

/// Image decode request builder.
///
/// # Example
///
/// ```no_run
/// use zencodecs::DecodeRequest;
///
/// let data: &[u8] = &[]; // your image bytes
/// let output = DecodeRequest::new(data).decode()?;
/// println!("{}x{}", output.width(), output.height());
/// # Ok::<(), zencodecs::CodecError>(())
/// ```
pub struct DecodeRequest<'a> {
    data: &'a [u8],
    format: Option<ImageFormat>,
    limits: Option<&'a Limits>,
    stop: Option<&'a dyn Stop>,
    registry: Option<&'a CodecRegistry>,
    codec_config: Option<&'a CodecConfig>,
}

impl<'a> DecodeRequest<'a> {
    /// Create a new decode request.
    ///
    /// Format will be auto-detected from magic bytes.
    /// The decoder returns its native pixel format.
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            format: None,
            limits: None,
            stop: None,
            registry: None,
            codec_config: None,
        }
    }

    /// Override format auto-detection.
    pub fn with_format(mut self, format: ImageFormat) -> Self {
        self.format = Some(format);
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

    /// Decode directly into a caller-provided RGB8 buffer.
    ///
    /// Uses zero-copy path when the codec supports it (e.g. JPEG scanline reader).
    /// Falls back to decode + copy for other codecs.
    pub fn decode_into_rgb8(
        self,
        dst: imgref::ImgRefMut<'_, rgb::Rgb<u8>>,
    ) -> Result<ImageInfo, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let format = match self.format {
            Some(f) => f,
            None => crate::info::detect_format(self.data).ok_or(CodecError::UnrecognizedFormat)?,
        };
        if !registry.can_decode(format) {
            return Err(CodecError::DisabledFormat(format));
        }
        self.decode_into_rgb8_format(format, dst)
    }

    fn decode_into_rgb8_format(
        self,
        format: ImageFormat,
        dst: imgref::ImgRefMut<'_, rgb::Rgb<u8>>,
    ) -> Result<ImageInfo, CodecError> {
        use zenpixels_convert::PixelBufferConvertExt as _;
        let output = self.decode_format(format)?;
        let info = output.info().clone();
        let src = output.into_buffer().to_rgb8();
        let mut dst = dst;
        for (src_row, dst_row) in src.as_imgref().rows().zip(dst.rows_mut()) {
            let n = src_row.len().min(dst_row.len());
            dst_row[..n].copy_from_slice(&src_row[..n]);
        }
        Ok(info)
    }

    /// Decode directly into a caller-provided RGBA8 buffer.
    ///
    /// Uses zero-copy path when the codec supports it (e.g. JPEG, WebP).
    /// Falls back to decode + copy for other codecs.
    pub fn decode_into_rgba8(
        self,
        dst: imgref::ImgRefMut<'_, rgb::Rgba<u8>>,
    ) -> Result<ImageInfo, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let format = match self.format {
            Some(f) => f,
            None => crate::info::detect_format(self.data).ok_or(CodecError::UnrecognizedFormat)?,
        };
        if !registry.can_decode(format) {
            return Err(CodecError::DisabledFormat(format));
        }
        self.decode_into_rgba8_format(format, dst)
    }

    fn decode_into_rgba8_format(
        self,
        format: ImageFormat,
        dst: imgref::ImgRefMut<'_, rgb::Rgba<u8>>,
    ) -> Result<ImageInfo, CodecError> {
        use zenpixels_convert::PixelBufferConvertExt as _;
        let output = self.decode_format(format)?;
        let info = output.info().clone();
        let src = output.into_buffer().to_rgba8();
        let mut dst = dst;
        for (src_row, dst_row) in src.as_imgref().rows().zip(dst.rows_mut()) {
            let n = src_row.len().min(dst_row.len());
            dst_row[..n].copy_from_slice(&src_row[..n]);
        }
        Ok(info)
    }

    /// Decode directly into a caller-provided Gray8 buffer.
    ///
    /// Uses zero-copy path when the codec supports it (e.g. JPEG).
    /// Falls back to decode + copy for other codecs.
    pub fn decode_into_gray8(
        self,
        dst: imgref::ImgRefMut<'_, rgb::Gray<u8>>,
    ) -> Result<ImageInfo, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let format = match self.format {
            Some(f) => f,
            None => crate::info::detect_format(self.data).ok_or(CodecError::UnrecognizedFormat)?,
        };
        if !registry.can_decode(format) {
            return Err(CodecError::DisabledFormat(format));
        }
        self.decode_into_gray8_format(format, dst)
    }

    fn decode_into_gray8_format(
        self,
        format: ImageFormat,
        dst: imgref::ImgRefMut<'_, rgb::Gray<u8>>,
    ) -> Result<ImageInfo, CodecError> {
        use zenpixels_convert::PixelBufferConvertExt as _;
        let output = self.decode_format(format)?;
        let info = output.info().clone();
        let src = output.into_buffer().to_gray8();
        let mut dst = dst;
        for (src_row, dst_row) in src.as_imgref().rows().zip(dst.rows_mut()) {
            let n = src_row.len().min(dst_row.len());
            dst_row[..n].copy_from_slice(&src_row[..n]);
        }
        Ok(info)
    }

    /// Decode directly into a caller-provided BGRA8 buffer.
    ///
    /// Falls back to decode + swizzle + copy for codecs without native BGRA support.
    pub fn decode_into_bgra8(
        self,
        dst: imgref::ImgRefMut<'_, rgb::Bgra<u8>>,
    ) -> Result<ImageInfo, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let format = match self.format {
            Some(f) => f,
            None => crate::info::detect_format(self.data).ok_or(CodecError::UnrecognizedFormat)?,
        };
        if !registry.can_decode(format) {
            return Err(CodecError::DisabledFormat(format));
        }
        self.decode_into_bgra8_format(format, dst)
    }

    fn decode_into_bgra8_format(
        self,
        format: ImageFormat,
        dst: imgref::ImgRefMut<'_, rgb::Bgra<u8>>,
    ) -> Result<ImageInfo, CodecError> {
        use zenpixels_convert::PixelBufferConvertExt as _;
        let output = self.decode_format(format)?;
        let info = output.info().clone();
        let src = output.into_buffer().to_bgra8();
        let mut dst = dst;
        for (src_row, dst_row) in src.as_imgref().rows().zip(dst.rows_mut()) {
            let n = src_row.len().min(dst_row.len());
            dst_row[..n].copy_from_slice(&src_row[..n]);
        }
        Ok(info)
    }

    /// Decode directly into a caller-provided BGRX8 buffer (alpha byte set to 255).
    ///
    /// Falls back to decode + swizzle + copy for codecs without native BGRX support.
    pub fn decode_into_bgrx8(
        self,
        dst: imgref::ImgRefMut<'_, rgb::Bgra<u8>>,
    ) -> Result<ImageInfo, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let format = match self.format {
            Some(f) => f,
            None => crate::info::detect_format(self.data).ok_or(CodecError::UnrecognizedFormat)?,
        };
        if !registry.can_decode(format) {
            return Err(CodecError::DisabledFormat(format));
        }
        self.decode_into_bgrx8_format(format, dst)
    }

    fn decode_into_bgrx8_format(
        self,
        format: ImageFormat,
        dst: imgref::ImgRefMut<'_, rgb::Bgra<u8>>,
    ) -> Result<ImageInfo, CodecError> {
        use zenpixels_convert::PixelBufferConvertExt as _;
        let output = self.decode_format(format)?;
        let info = output.info().clone();
        let src = output.into_buffer().to_rgb8();
        let mut dst = dst;
        for (src_row, dst_row) in src.as_imgref().rows().zip(dst.rows_mut()) {
            for (s, d) in src_row.iter().zip(dst_row.iter_mut()) {
                *d = rgb::Bgra {
                    b: s.b,
                    g: s.g,
                    r: s.r,
                    a: 255,
                };
            }
        }
        Ok(info)
    }

    /// Decode directly into a caller-provided linear RGB f32 buffer.
    ///
    /// Output is in linear light (not sRGB gamma). Falls back to decode + sRGB
    /// linearization + copy.
    pub fn decode_into_rgb_f32(
        self,
        dst: imgref::ImgRefMut<'_, rgb::Rgb<f32>>,
    ) -> Result<ImageInfo, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let format = match self.format {
            Some(f) => f,
            None => crate::info::detect_format(self.data).ok_or(CodecError::UnrecognizedFormat)?,
        };
        if !registry.can_decode(format) {
            return Err(CodecError::DisabledFormat(format));
        }
        self.decode_into_rgb_f32_format(format, dst)
    }

    fn decode_into_rgb_f32_format(
        self,
        format: ImageFormat,
        dst: imgref::ImgRefMut<'_, rgb::Rgb<f32>>,
    ) -> Result<ImageInfo, CodecError> {
        use linear_srgb::default::srgb_u8_to_linear;
        use zenpixels_convert::PixelBufferConvertExt as _;
        let output = self.decode_format(format)?;
        let info = output.info().clone();
        let src = output.into_buffer().to_rgb8();
        let mut dst = dst;
        for (src_row, dst_row) in src.as_imgref().rows().zip(dst.rows_mut()) {
            for (s, d) in src_row.iter().zip(dst_row.iter_mut()) {
                *d = rgb::Rgb {
                    r: srgb_u8_to_linear(s.r),
                    g: srgb_u8_to_linear(s.g),
                    b: srgb_u8_to_linear(s.b),
                };
            }
        }
        Ok(info)
    }

    /// Decode directly into a caller-provided linear RGBA f32 buffer.
    ///
    /// Output is in linear light (not sRGB gamma). Alpha is linear (not gamma-encoded).
    /// Falls back to decode + sRGB linearization + copy.
    pub fn decode_into_rgba_f32(
        self,
        dst: imgref::ImgRefMut<'_, rgb::Rgba<f32>>,
    ) -> Result<ImageInfo, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let format = match self.format {
            Some(f) => f,
            None => crate::info::detect_format(self.data).ok_or(CodecError::UnrecognizedFormat)?,
        };
        if !registry.can_decode(format) {
            return Err(CodecError::DisabledFormat(format));
        }
        self.decode_into_rgba_f32_format(format, dst)
    }

    fn decode_into_rgba_f32_format(
        self,
        format: ImageFormat,
        dst: imgref::ImgRefMut<'_, rgb::Rgba<f32>>,
    ) -> Result<ImageInfo, CodecError> {
        use linear_srgb::default::srgb_u8_to_linear;
        use zenpixels_convert::PixelBufferConvertExt as _;
        let output = self.decode_format(format)?;
        let info = output.info().clone();
        let src = output.into_buffer().to_rgba8();
        let mut dst = dst;
        for (src_row, dst_row) in src.as_imgref().rows().zip(dst.rows_mut()) {
            for (s, d) in src_row.iter().zip(dst_row.iter_mut()) {
                *d = rgb::Rgba {
                    r: srgb_u8_to_linear(s.r),
                    g: srgb_u8_to_linear(s.g),
                    b: srgb_u8_to_linear(s.b),
                    a: s.a as f32 / 255.0,
                };
            }
        }
        Ok(info)
    }

    /// Decode directly into a caller-provided linear grayscale f32 buffer.
    ///
    /// Output is in linear light (not sRGB gamma). Falls back to decode + sRGB
    /// linearization + copy.
    pub fn decode_into_gray_f32(
        self,
        dst: imgref::ImgRefMut<'_, rgb::Gray<f32>>,
    ) -> Result<ImageInfo, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let format = match self.format {
            Some(f) => f,
            None => crate::info::detect_format(self.data).ok_or(CodecError::UnrecognizedFormat)?,
        };
        if !registry.can_decode(format) {
            return Err(CodecError::DisabledFormat(format));
        }
        self.decode_into_gray_f32_format(format, dst)
    }

    fn decode_into_gray_f32_format(
        self,
        format: ImageFormat,
        dst: imgref::ImgRefMut<'_, rgb::Gray<f32>>,
    ) -> Result<ImageInfo, CodecError> {
        use linear_srgb::default::srgb_u8_to_linear;
        use zenpixels_convert::PixelBufferConvertExt as _;
        let output = self.decode_format(format)?;
        let info = output.info().clone();
        let src = output.into_buffer().to_gray8();
        let mut dst = dst;
        for (src_row, dst_row) in src.as_imgref().rows().zip(dst.rows_mut()) {
            for (s, d) in src_row.iter().zip(dst_row.iter_mut()) {
                *d = rgb::Gray::new(srgb_u8_to_linear(s.value()));
            }
        }
        Ok(info)
    }

    /// Decode UltraHDR JPEG to linear f32 RGBA HDR pixels.
    ///
    /// Extracts the gain map from an UltraHDR JPEG and reconstructs HDR content.
    /// Returns linear f32 RGBA pixels. Fails if the image is not an UltraHDR JPEG.
    ///
    /// `display_boost` controls the HDR headroom: 1.0 = SDR, 4.0 = typical HDR display.
    #[cfg(feature = "jpeg-ultrahdr")]
    pub fn decode_hdr(self, display_boost: f32) -> Result<DecodeOutput, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);

        let format = match self.format {
            Some(f) => f,
            None => crate::info::detect_format(self.data).ok_or(CodecError::UnrecognizedFormat)?,
        };

        if format != ImageFormat::Jpeg {
            return Err(CodecError::UnsupportedOperation {
                format,
                detail: "UltraHDR decode only supported for JPEG",
            });
        }

        if !registry.can_decode(format) {
            return Err(CodecError::DisabledFormat(format));
        }

        crate::codecs::jpeg::decode_hdr(
            self.data,
            display_boost,
            self.codec_config,
            self.limits,
            self.stop,
        )
    }

    /// Decode the image to pixels.
    pub fn decode(self) -> Result<DecodeOutput, CodecError> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);

        let format = match self.format {
            Some(f) => f,
            None => crate::info::detect_format(self.data).ok_or(CodecError::UnrecognizedFormat)?,
        };

        if !registry.can_decode(format) {
            return Err(CodecError::DisabledFormat(format));
        }

        self.decode_format(format)
    }

    /// Dispatch to format-specific decoder.
    fn decode_format(self, format: ImageFormat) -> Result<DecodeOutput, CodecError> {
        match format {
            #[cfg(feature = "jpeg")]
            ImageFormat::Jpeg => self.decode_jpeg(),
            #[cfg(not(feature = "jpeg"))]
            ImageFormat::Jpeg => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "webp")]
            ImageFormat::WebP => self.decode_webp(),
            #[cfg(not(feature = "webp"))]
            ImageFormat::WebP => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "gif")]
            ImageFormat::Gif => self.decode_gif(),
            #[cfg(not(feature = "gif"))]
            ImageFormat::Gif => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "png")]
            ImageFormat::Png => self.decode_png(),
            #[cfg(not(feature = "png"))]
            ImageFormat::Png => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "avif-decode")]
            ImageFormat::Avif => self.decode_avif(),
            #[cfg(not(feature = "avif-decode"))]
            ImageFormat::Avif => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "jxl-decode")]
            ImageFormat::Jxl => self.decode_jxl(),
            #[cfg(not(feature = "jxl-decode"))]
            ImageFormat::Jxl => Err(CodecError::UnsupportedFormat(format)),

            #[cfg(feature = "heic-decode")]
            ImageFormat::Heic => self.decode_heic(),
            #[cfg(not(feature = "heic-decode"))]
            ImageFormat::Heic => Err(CodecError::UnsupportedFormat(format)),

            _ => Err(CodecError::UnsupportedFormat(format)),
        }
    }

    #[cfg(feature = "jpeg")]
    fn decode_jpeg(self) -> Result<DecodeOutput, CodecError> {
        crate::codecs::jpeg::decode(self.data, self.codec_config, self.limits, self.stop)
    }

    #[cfg(feature = "webp")]
    fn decode_webp(self) -> Result<DecodeOutput, CodecError> {
        crate::codecs::webp::decode(self.data, self.codec_config, self.limits, self.stop)
    }

    #[cfg(feature = "gif")]
    fn decode_gif(self) -> Result<DecodeOutput, CodecError> {
        crate::codecs::gif::decode(self.data, self.limits, self.stop)
    }

    #[cfg(feature = "png")]
    fn decode_png(self) -> Result<DecodeOutput, CodecError> {
        crate::codecs::png::decode(self.data, self.limits, self.stop)
    }

    #[cfg(feature = "avif-decode")]
    fn decode_avif(self) -> Result<DecodeOutput, CodecError> {
        crate::codecs::avif_dec::decode(self.data, self.codec_config, self.limits, self.stop)
    }

    #[cfg(feature = "jxl-decode")]
    fn decode_jxl(self) -> Result<DecodeOutput, CodecError> {
        crate::codecs::jxl_dec::decode(self.data, self.limits, self.stop)
    }

    #[cfg(feature = "heic-decode")]
    fn decode_heic(self) -> Result<DecodeOutput, CodecError> {
        crate::codecs::heic::decode(self.data, self.limits, self.stop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_pattern() {
        let data = b"test";
        let request = DecodeRequest::new(data).with_format(ImageFormat::Jpeg);
        assert_eq!(request.format, Some(ImageFormat::Jpeg));
    }

    #[test]
    fn disabled_format_error() {
        let jpeg_data = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        let registry = CodecRegistry::none();

        let result = DecodeRequest::new(&jpeg_data)
            .with_registry(&registry)
            .decode();

        assert!(matches!(result, Err(CodecError::DisabledFormat(_))));
    }
}
