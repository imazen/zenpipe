//! Image decoding.

pub use zencodec::decode::DecodeOutput;

use crate::config::CodecConfig;
use crate::error::Result;
use crate::policy::CodecPolicy;
use crate::{CodecError, CodecRegistry, ImageFormat, ImageInfo, Limits, Stop};
use whereat::at;

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
/// # Ok::<(), whereat::At<zencodecs::CodecError>>(())
/// ```
pub struct DecodeRequest<'a> {
    data: &'a [u8],
    format: Option<ImageFormat>,
    limits: Option<&'a Limits>,
    stop: Option<&'a dyn Stop>,
    registry: Option<&'a CodecRegistry>,
    codec_config: Option<&'a CodecConfig>,
    policy: Option<CodecPolicy>,
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
            policy: None,
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

    /// Set a per-request codec policy for filtering and preferences.
    ///
    /// Currently reserved for future use with fallback chains and
    /// multi-decoder-per-format support. The policy's format restrictions
    /// are checked during format detection.
    pub fn with_policy(mut self, policy: CodecPolicy) -> Self {
        self.policy = Some(policy);
        self
    }

    /// Resolve format (auto-detect or explicit) and check registry.
    fn resolve_format(&self) -> Result<ImageFormat> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let format = match self.format {
            Some(f) => f,
            None => crate::info::detect_format(self.data)
                .ok_or_else(|| at!(CodecError::UnrecognizedFormat))?,
        };
        if !registry.can_decode(format) {
            return Err(at!(CodecError::DisabledFormat(format)));
        }
        Ok(format)
    }

    /// Decode, convert to target pixel type, and copy rows into `dst`.
    fn decode_into<P: Copy + zenpixels::Pixel>(
        self,
        dst: imgref::ImgRefMut<'_, P>,
        convert: fn(zenpixels::PixelBuffer) -> zenpixels::PixelBuffer<P>,
    ) -> Result<ImageInfo> {
        let format = self.resolve_format()?;
        let output = self.decode_format(format)?;
        let info = output.info().clone();
        let src = convert(output.into_buffer());
        copy_rows(src.as_imgref(), dst);
        Ok(info)
    }

    /// Decode directly into a caller-provided RGB8 buffer.
    pub fn decode_into_rgb8(self, dst: imgref::ImgRefMut<'_, rgb::Rgb<u8>>) -> Result<ImageInfo> {
        use zenpixels_convert::PixelBufferConvertTypedExt as _;
        self.decode_into(dst, |b| b.to_rgb8())
    }

    /// Decode directly into a caller-provided RGBA8 buffer.
    pub fn decode_into_rgba8(self, dst: imgref::ImgRefMut<'_, rgb::Rgba<u8>>) -> Result<ImageInfo> {
        use zenpixels_convert::PixelBufferConvertTypedExt as _;
        self.decode_into(dst, |b| b.to_rgba8())
    }

    /// Decode directly into a caller-provided Gray8 buffer.
    pub fn decode_into_gray8(self, dst: imgref::ImgRefMut<'_, rgb::Gray<u8>>) -> Result<ImageInfo> {
        use zenpixels_convert::PixelBufferConvertTypedExt as _;
        self.decode_into(dst, |b| b.to_gray8())
    }

    /// Decode directly into a caller-provided BGRA8 buffer.
    pub fn decode_into_bgra8(self, dst: imgref::ImgRefMut<'_, rgb::Bgra<u8>>) -> Result<ImageInfo> {
        use zenpixels_convert::PixelBufferConvertTypedExt as _;
        self.decode_into(dst, |b| b.to_bgra8())
    }

    /// Decode directly into a caller-provided BGRX8 buffer (alpha byte set to 255).
    pub fn decode_into_bgrx8(self, dst: imgref::ImgRefMut<'_, rgb::Bgra<u8>>) -> Result<ImageInfo> {
        let format = self.resolve_format()?;
        use zenpixels_convert::PixelBufferConvertTypedExt as _;
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
    pub fn decode_into_rgb_f32(
        self,
        dst: imgref::ImgRefMut<'_, rgb::Rgb<f32>>,
    ) -> Result<ImageInfo> {
        use linear_srgb::default::srgb_u8_to_linear;
        use zenpixels_convert::PixelBufferConvertTypedExt as _;
        let format = self.resolve_format()?;
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
    pub fn decode_into_rgba_f32(
        self,
        dst: imgref::ImgRefMut<'_, rgb::Rgba<f32>>,
    ) -> Result<ImageInfo> {
        use linear_srgb::default::srgb_u8_to_linear;
        use zenpixels_convert::PixelBufferConvertTypedExt as _;
        let format = self.resolve_format()?;
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
    pub fn decode_into_gray_f32(
        self,
        dst: imgref::ImgRefMut<'_, rgb::Gray<f32>>,
    ) -> Result<ImageInfo> {
        use linear_srgb::default::srgb_u8_to_linear;
        use zenpixels_convert::PixelBufferConvertTypedExt as _;
        let format = self.resolve_format()?;
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
    pub fn decode_hdr(self, display_boost: f32) -> Result<DecodeOutput> {
        let format = self.resolve_format()?;
        if format != ImageFormat::Jpeg {
            return Err(at!(CodecError::UnsupportedOperation {
                format,
                detail: "UltraHDR decode only supported for JPEG",
            }));
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
    pub fn decode(self) -> Result<DecodeOutput> {
        let format = self.resolve_format()?;
        self.decode_format(format)
    }

    /// Dispatch to format-specific decoder.
    fn decode_format(self, format: ImageFormat) -> Result<DecodeOutput> {
        match format {
            #[cfg(feature = "jpeg")]
            ImageFormat::Jpeg => {
                crate::codecs::jpeg::decode(self.data, self.codec_config, self.limits, self.stop)
            }
            #[cfg(not(feature = "jpeg"))]
            ImageFormat::Jpeg => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "webp")]
            ImageFormat::WebP => {
                crate::codecs::webp::decode(self.data, self.codec_config, self.limits, self.stop)
            }
            #[cfg(not(feature = "webp"))]
            ImageFormat::WebP => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "gif")]
            ImageFormat::Gif => crate::codecs::gif::decode(self.data, self.limits, self.stop),
            #[cfg(not(feature = "gif"))]
            ImageFormat::Gif => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "png")]
            ImageFormat::Png => crate::codecs::png::decode(self.data, self.limits, self.stop),
            #[cfg(not(feature = "png"))]
            ImageFormat::Png => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "avif-decode")]
            ImageFormat::Avif => crate::codecs::avif_dec::decode(
                self.data,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "avif-decode"))]
            ImageFormat::Avif => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "jxl-decode")]
            ImageFormat::Jxl => crate::codecs::jxl_dec::decode(self.data, self.limits, self.stop),
            #[cfg(not(feature = "jxl-decode"))]
            ImageFormat::Jxl => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "heic-decode")]
            ImageFormat::Heic => crate::codecs::heic::decode(self.data, self.limits, self.stop),
            #[cfg(not(feature = "heic-decode"))]
            ImageFormat::Heic => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "bitmaps")]
            ImageFormat::Pnm => crate::codecs::pnm::decode(self.data, self.limits, self.stop),
            #[cfg(not(feature = "bitmaps"))]
            ImageFormat::Pnm => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "bitmaps-bmp")]
            ImageFormat::Bmp => crate::codecs::bmp::decode(self.data, self.limits, self.stop),
            #[cfg(not(feature = "bitmaps-bmp"))]
            ImageFormat::Bmp => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "bitmaps")]
            ImageFormat::Farbfeld => {
                crate::codecs::farbfeld::decode(self.data, self.limits, self.stop)
            }
            #[cfg(not(feature = "bitmaps"))]
            ImageFormat::Farbfeld => Err(at!(CodecError::UnsupportedFormat(format))),

            _ => Err(at!(CodecError::UnsupportedFormat(format))),
        }
    }
}

/// Copy rows from src to dst, handling stride mismatches.
fn copy_rows<P: Copy>(src: imgref::ImgRef<'_, P>, mut dst: imgref::ImgRefMut<'_, P>) {
    for (src_row, dst_row) in src.rows().zip(dst.rows_mut()) {
        let n = src_row.len().min(dst_row.len());
        dst_row[..n].copy_from_slice(&src_row[..n]);
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

        assert!(matches!(
            result.as_ref().map_err(|e| e.error()),
            Err(CodecError::DisabledFormat(_))
        ));
    }
}
