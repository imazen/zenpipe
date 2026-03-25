//! Codec implementation identifiers.
//!
//! Each format may have multiple implementations (e.g., zenjpeg vs a future
//! zune-jpeg adapter for JPEG decode). [`CodecId`] identifies a specific
//! implementation for policy targeting.

use crate::ImageFormat;

/// Identifies a specific codec implementation.
///
/// Used by [`CodecPolicy`](crate::CodecPolicy) to target killbits, allowlists,
/// and preference ordering at individual codec implementations rather than
/// entire formats.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CodecId {
    // JPEG
    /// zenjpeg decoder
    ZenjpegDecode,
    /// zenjpeg encoder
    ZenjpegEncode,

    // WebP
    /// zenwebp decoder
    ZenwebpDecode,
    /// zenwebp encoder
    ZenwebpEncode,

    // GIF
    /// zengif decoder
    ZengifDecode,
    /// zengif encoder
    ZengifEncode,

    // PNG
    /// png crate decoder
    PngDecode,
    /// png crate encoder
    PngEncode,

    // AVIF
    /// zenavif decoder
    ZenavifDecode,
    /// ravif encoder
    RavifEncode,

    // JXL
    /// zenjxl decoder
    ZenjxlDecode,
    /// jxl-encoder encoder
    JxlEncoderEncode,

    // HEIC
    /// heic-decoder decoder
    HeicDecode,

    // RAW/DNG
    /// zenraw decoder (RAW/DNG)
    ZenrawDecode,

    // Bitmaps
    /// zenbitmaps PNM decoder
    PnmDecode,
    /// zenbitmaps PNM encoder
    PnmEncode,
    /// zenbitmaps BMP decoder
    BmpDecode,
    /// zenbitmaps BMP encoder
    BmpEncode,
    /// zenbitmaps Farbfeld decoder
    FarbfeldDecode,
    /// zenbitmaps Farbfeld encoder
    FarbfeldEncode,

    // TIFF
    /// zentiff decoder
    TiffDecode,
    /// zentiff encoder
    TiffEncode,

    /// Third-party or dynamically registered codec.
    Custom(&'static str),
}

impl CodecId {
    /// The image format this codec handles.
    pub fn format(&self) -> ImageFormat {
        match self {
            Self::ZenjpegDecode | Self::ZenjpegEncode => ImageFormat::Jpeg,
            Self::ZenwebpDecode | Self::ZenwebpEncode => ImageFormat::WebP,
            Self::ZengifDecode | Self::ZengifEncode => ImageFormat::Gif,
            Self::PngDecode | Self::PngEncode => ImageFormat::Png,
            Self::ZenavifDecode | Self::RavifEncode => ImageFormat::Avif,
            Self::ZenjxlDecode | Self::JxlEncoderEncode => ImageFormat::Jxl,
            Self::HeicDecode => ImageFormat::Heic,
            // ZenrawDecode uses Custom format; return Unknown since there's no
            // built-in RAW variant — callers should use CodecId-level matching instead.
            Self::ZenrawDecode => ImageFormat::Unknown,
            Self::PnmDecode | Self::PnmEncode => ImageFormat::Pnm,
            Self::BmpDecode | Self::BmpEncode => ImageFormat::Bmp,
            Self::FarbfeldDecode | Self::FarbfeldEncode => ImageFormat::Farbfeld,
            Self::TiffDecode | Self::TiffEncode => ImageFormat::Tiff,
            // Custom codecs: caller is responsible for correct format association.
            // We return Jpeg as a fallback but this should never be relied upon.
            Self::Custom(_) => ImageFormat::Jpeg, // TODO: Custom needs format stored
        }
    }

    /// Whether this is a decoder.
    pub fn is_decoder(&self) -> bool {
        matches!(
            self,
            Self::ZenjpegDecode
                | Self::ZenwebpDecode
                | Self::ZengifDecode
                | Self::PngDecode
                | Self::ZenavifDecode
                | Self::ZenjxlDecode
                | Self::HeicDecode
                | Self::ZenrawDecode
                | Self::PnmDecode
                | Self::BmpDecode
                | Self::FarbfeldDecode
                | Self::TiffDecode
        )
    }

    /// Whether this is an encoder.
    pub fn is_encoder(&self) -> bool {
        matches!(
            self,
            Self::ZenjpegEncode
                | Self::ZenwebpEncode
                | Self::ZengifEncode
                | Self::PngEncode
                | Self::RavifEncode
                | Self::JxlEncoderEncode
                | Self::PnmEncode
                | Self::BmpEncode
                | Self::FarbfeldEncode
                | Self::TiffEncode
        )
    }

    /// Human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::ZenjpegDecode => "zenjpeg (decode)",
            Self::ZenjpegEncode => "zenjpeg (encode)",
            Self::ZenwebpDecode => "zenwebp (decode)",
            Self::ZenwebpEncode => "zenwebp (encode)",
            Self::ZengifDecode => "zengif (decode)",
            Self::ZengifEncode => "zengif (encode)",
            Self::PngDecode => "png (decode)",
            Self::PngEncode => "png (encode)",
            Self::ZenavifDecode => "zenavif (decode)",
            Self::RavifEncode => "ravif (encode)",
            Self::ZenjxlDecode => "zenjxl (decode)",
            Self::JxlEncoderEncode => "jxl-encoder (encode)",
            Self::HeicDecode => "heic-decoder (decode)",
            Self::ZenrawDecode => "zenraw (decode)",
            Self::PnmDecode => "zenbitmaps-pnm (decode)",
            Self::PnmEncode => "zenbitmaps-pnm (encode)",
            Self::BmpDecode => "zenbitmaps-bmp (decode)",
            Self::BmpEncode => "zenbitmaps-bmp (encode)",
            Self::FarbfeldDecode => "zenbitmaps-farbfeld (decode)",
            Self::FarbfeldEncode => "zenbitmaps-farbfeld (encode)",
            Self::TiffDecode => "zentiff (decode)",
            Self::TiffEncode => "zentiff (encode)",
            Self::Custom(name) => name,
        }
    }
}

impl core::fmt::Display for CodecId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_id_basics() {
        assert!(CodecId::ZenjpegDecode.is_decoder());
        assert!(!CodecId::ZenjpegDecode.is_encoder());
        assert!(CodecId::ZenjpegEncode.is_encoder());
        assert!(!CodecId::ZenjpegEncode.is_decoder());
        assert_eq!(CodecId::ZenjpegDecode.format(), ImageFormat::Jpeg);
        assert_eq!(CodecId::RavifEncode.format(), ImageFormat::Avif);
    }

    #[test]
    fn custom_codec_id() {
        let custom = CodecId::Custom("my-codec");
        assert_eq!(custom.name(), "my-codec");
        // Custom is neither decoder nor encoder by the built-in check
        assert!(!custom.is_decoder());
        assert!(!custom.is_encoder());
    }

    #[test]
    fn display_impl() {
        let id = CodecId::ZenwebpEncode;
        assert_eq!(alloc::format!("{id}"), "zenwebp (encode)");
    }
}
