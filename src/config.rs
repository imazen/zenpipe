//! Format-specific codec configuration and re-exports.
//!
//! Each codec's configuration types are re-exported behind feature gates.
//! The [`CodecConfig`] struct bundles all format-specific overrides into
//! a single value that can be passed to encode/decode requests.

use alloc::boxed::Box;

// --- Re-exports ---

/// JPEG configuration types from zenjpeg.
#[cfg(feature = "jpeg")]
pub mod jpeg {
    pub use zenjpeg::decoder::{
        ChromaUpsampling, DecodeConfig, DecodedExtras, JpegInfo, MpfDirectory, MpfEntry,
        MpfImageType, OutputTarget, PreserveConfig, PreservedMpfImage, PreservedSegment,
        SegmentType, Strictness,
    };
    pub use zenjpeg::encoder::{
        ChromaSubsampling, ColorMode, EncodeRequest as JpegEncodeRequest, EncoderConfig, Exif,
        ExifFields, HuffmanStrategy, Orientation, PixelLayout as JpegPixelLayout, Quality,
    };
    pub use zenjpeg::lossless::LosslessTransform;

    /// UltraHDR types for HDR gain map encoding/decoding.
    #[cfg(feature = "jpeg-ultrahdr")]
    pub mod ultrahdr {
        pub use zenjpeg::ultrahdr::{
            // Core workflow functions
            UltraHdrExtras, create_hdr_reconstructor, encode_ultrahdr,
            encode_ultrahdr_with_tonemapper, encode_with_gainmap,
            // Gain map types
            GainMap, GainMapMetadata, GainMapConfig,
            // Color types
            UhdrColorGamut, UhdrColorTransfer, UhdrPixelFormat, UhdrRawImage,
            // Tonemapping
            AdaptiveTonemapper, ToneMapConfig,
            // Streaming
            RowDecoder, RowEncoder,
        };
    }
}

/// WebP configuration types from zenwebp.
#[cfg(feature = "webp")]
pub mod webp {
    pub use zenwebp::{
        DecodeConfig, LosslessConfig, LossyConfig, PixelLayout as WebpPixelLayout, Preset,
        UpsamplingMethod,
    };
}

/// GIF configuration types from zengif.
#[cfg(feature = "gif")]
pub mod gif {
    pub use zengif::EncoderConfig;
}

/// PNG configuration types from zenpng.
#[cfg(feature = "png")]
pub mod png_codec {
    pub use zenpng::{Compression, Filter};
}

/// AVIF decode configuration from zenavif.
#[cfg(feature = "avif-decode")]
pub mod avif_decode {
    pub use zenavif::DecoderConfig;
}

/// AVIF encode configuration from zenavif.
#[cfg(feature = "avif-encode")]
pub mod avif_encode {
    pub use zenavif::{EncodeAlphaMode, EncodeBitDepth, EncodeColorModel, EncoderConfig};
}

// --- Unified codec config ---

/// Format-specific configuration overrides.
///
/// Contains optional boxed configs for each codec. When a config is `Some`,
/// the codec adapter will use it instead of deriving settings from the
/// generic quality/effort/lossless parameters on `EncodeRequest`/`DecodeRequest`.
///
/// This gives callers full control over codec behavior without zencodecs
/// having to expose every knob in its own API.
///
/// # Example
///
/// ```ignore
/// use zencodecs::{EncodeRequest, ImageFormat};
/// use zencodecs::config::{jpeg::EncoderConfig, CodecConfig};
///
/// let jpeg_config = EncoderConfig::ycbcr(92, zencodecs::config::jpeg::ChromaSubsampling::Half);
/// let config = CodecConfig::default().with_jpeg_encoder(jpeg_config);
/// let request = EncodeRequest::new(ImageFormat::Jpeg).with_codec_config(&config);
/// ```
#[derive(Default)]
#[non_exhaustive]
pub struct CodecConfig {
    /// JPEG encoder configuration (overrides quality/effort).
    #[cfg(feature = "jpeg")]
    pub jpeg_encoder: Option<Box<jpeg::EncoderConfig>>,

    /// JPEG decoder configuration.
    #[cfg(feature = "jpeg")]
    pub jpeg_decoder: Option<Box<jpeg::DecodeConfig>>,

    /// WebP decoder configuration (upsampling method, limits).
    #[cfg(feature = "webp")]
    pub webp_decoder: Option<Box<webp::DecodeConfig>>,

    /// WebP lossy encoder configuration (overrides quality).
    #[cfg(feature = "webp")]
    pub webp_lossy: Option<Box<webp::LossyConfig>>,

    /// WebP lossless encoder configuration.
    #[cfg(feature = "webp")]
    pub webp_lossless: Option<Box<webp::LosslessConfig>>,

    /// GIF encoder configuration.
    #[cfg(feature = "gif")]
    pub gif_encoder: Option<Box<gif::EncoderConfig>>,

    /// PNG compression level.
    #[cfg(feature = "png")]
    pub png_compression: Option<zenpng::Compression>,

    /// PNG filter strategy.
    #[cfg(feature = "png")]
    pub png_filter: Option<zenpng::Filter>,

    /// AVIF decoder configuration.
    #[cfg(feature = "avif-decode")]
    pub avif_decoder: Option<Box<avif_decode::DecoderConfig>>,

    /// AVIF encode quality override (0-100).
    /// When set, overrides the generic quality on EncodeRequest.
    #[cfg(feature = "avif-encode")]
    pub avif_quality: Option<f32>,

    /// AVIF encode speed (1-10, lower = slower/better).
    #[cfg(feature = "avif-encode")]
    pub avif_speed: Option<u8>,

    /// AVIF alpha quality override.
    #[cfg(feature = "avif-encode")]
    pub avif_alpha_quality: Option<f32>,
}

impl CodecConfig {
    /// Set JPEG encoder configuration.
    #[cfg(feature = "jpeg")]
    pub fn with_jpeg_encoder(mut self, config: jpeg::EncoderConfig) -> Self {
        self.jpeg_encoder = Some(Box::new(config));
        self
    }

    /// Set JPEG decoder configuration.
    #[cfg(feature = "jpeg")]
    pub fn with_jpeg_decoder(mut self, config: jpeg::DecodeConfig) -> Self {
        self.jpeg_decoder = Some(Box::new(config));
        self
    }

    /// Set WebP decoder configuration (upsampling method, limits).
    #[cfg(feature = "webp")]
    pub fn with_webp_decoder(mut self, config: webp::DecodeConfig) -> Self {
        self.webp_decoder = Some(Box::new(config));
        self
    }

    /// Set WebP lossy encoder configuration.
    #[cfg(feature = "webp")]
    pub fn with_webp_lossy(mut self, config: webp::LossyConfig) -> Self {
        self.webp_lossy = Some(Box::new(config));
        self
    }

    /// Set WebP lossless encoder configuration.
    #[cfg(feature = "webp")]
    pub fn with_webp_lossless(mut self, config: webp::LosslessConfig) -> Self {
        self.webp_lossless = Some(Box::new(config));
        self
    }

    /// Set GIF encoder configuration.
    #[cfg(feature = "gif")]
    pub fn with_gif_encoder(mut self, config: gif::EncoderConfig) -> Self {
        self.gif_encoder = Some(Box::new(config));
        self
    }

    /// Set PNG compression level.
    #[cfg(feature = "png")]
    pub fn with_png_compression(mut self, compression: zenpng::Compression) -> Self {
        self.png_compression = Some(compression);
        self
    }

    /// Set PNG filter strategy.
    #[cfg(feature = "png")]
    pub fn with_png_filter(mut self, filter: zenpng::Filter) -> Self {
        self.png_filter = Some(filter);
        self
    }

    /// Set AVIF decoder configuration.
    #[cfg(feature = "avif-decode")]
    pub fn with_avif_decoder(mut self, config: avif_decode::DecoderConfig) -> Self {
        self.avif_decoder = Some(Box::new(config));
        self
    }

    /// Set AVIF encode quality (0-100).
    #[cfg(feature = "avif-encode")]
    pub fn with_avif_quality(mut self, quality: f32) -> Self {
        self.avif_quality = Some(quality);
        self
    }

    /// Set AVIF encode speed (1-10, lower = slower/better).
    #[cfg(feature = "avif-encode")]
    pub fn with_avif_speed(mut self, speed: u8) -> Self {
        self.avif_speed = Some(speed);
        self
    }

    /// Set AVIF alpha quality (0-100).
    #[cfg(feature = "avif-encode")]
    pub fn with_avif_alpha_quality(mut self, quality: f32) -> Self {
        self.avif_alpha_quality = Some(quality);
        self
    }
}

impl core::fmt::Debug for CodecConfig {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut d = f.debug_struct("CodecConfig");

        #[cfg(feature = "jpeg")]
        {
            d.field("jpeg_encoder", &self.jpeg_encoder.is_some());
            d.field("jpeg_decoder", &self.jpeg_decoder.is_some());
        }
        #[cfg(feature = "webp")]
        {
            d.field("webp_decoder", &self.webp_decoder.is_some());
            d.field("webp_lossy", &self.webp_lossy.is_some());
            d.field("webp_lossless", &self.webp_lossless.is_some());
        }
        #[cfg(feature = "gif")]
        d.field("gif_encoder", &self.gif_encoder.is_some());
        #[cfg(feature = "png")]
        {
            d.field("png_compression", &self.png_compression);
            d.field("png_filter", &self.png_filter);
        }
        #[cfg(feature = "avif-decode")]
        d.field("avif_decoder", &self.avif_decoder.is_some());
        #[cfg(feature = "avif-encode")]
        {
            d.field("avif_quality", &self.avif_quality);
            d.field("avif_speed", &self.avif_speed);
        }

        d.finish()
    }
}
