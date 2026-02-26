//! Runtime codec registry for enabling/disabling formats.

use crate::ImageFormat;

/// Set of image formats represented as bitflags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FormatSet(u8);

impl FormatSet {
    const EMPTY: Self = FormatSet(0);
    const JPEG: u8 = 1 << 0;
    const WEBP: u8 = 1 << 1;
    const GIF: u8 = 1 << 2;
    const PNG: u8 = 1 << 3;
    const AVIF: u8 = 1 << 4;
    const JXL: u8 = 1 << 5;
    const HEIC: u8 = 1 << 6;

    #[allow(unused_mut)]
    fn all_compiled() -> Self {
        let mut bits = 0u8;

        #[cfg(feature = "jpeg")]
        {
            bits |= Self::JPEG;
        }
        #[cfg(feature = "webp")]
        {
            bits |= Self::WEBP;
        }
        #[cfg(feature = "gif")]
        {
            bits |= Self::GIF;
        }
        #[cfg(feature = "png")]
        {
            bits |= Self::PNG;
        }

        FormatSet(bits)
    }

    #[allow(unused_mut)]
    fn all_compiled_decode() -> Self {
        let mut bits = Self::all_compiled().0;

        #[cfg(feature = "avif-decode")]
        {
            bits |= Self::AVIF;
        }
        #[cfg(feature = "jxl-decode")]
        {
            bits |= Self::JXL;
        }
        #[cfg(feature = "heic-decode")]
        {
            bits |= Self::HEIC;
        }

        FormatSet(bits)
    }

    #[allow(unused_mut)]
    fn all_compiled_encode() -> Self {
        let mut bits = Self::all_compiled().0;

        #[cfg(feature = "avif-encode")]
        {
            bits |= Self::AVIF;
        }
        #[cfg(feature = "jxl-encode")]
        {
            bits |= Self::JXL;
        }

        FormatSet(bits)
    }

    fn contains(self, format: ImageFormat) -> bool {
        let bit = match format {
            ImageFormat::Jpeg => Self::JPEG,
            ImageFormat::WebP => Self::WEBP,
            ImageFormat::Gif => Self::GIF,
            ImageFormat::Png => Self::PNG,
            ImageFormat::Avif => Self::AVIF,
            ImageFormat::Jxl => Self::JXL,
            ImageFormat::Heic => Self::HEIC,
            _ => return false,
        };
        (self.0 & bit) != 0
    }

    fn insert(&mut self, format: ImageFormat) {
        let bit = match format {
            ImageFormat::Jpeg => Self::JPEG,
            ImageFormat::WebP => Self::WEBP,
            ImageFormat::Gif => Self::GIF,
            ImageFormat::Png => Self::PNG,
            ImageFormat::Avif => Self::AVIF,
            ImageFormat::Jxl => Self::JXL,
            ImageFormat::Heic => Self::HEIC,
            _ => return,
        };
        self.0 |= bit;
    }

    fn remove(&mut self, format: ImageFormat) {
        let bit = match format {
            ImageFormat::Jpeg => Self::JPEG,
            ImageFormat::WebP => Self::WEBP,
            ImageFormat::Gif => Self::GIF,
            ImageFormat::Png => Self::PNG,
            ImageFormat::Avif => Self::AVIF,
            ImageFormat::Jxl => Self::JXL,
            ImageFormat::Heic => Self::HEIC,
            _ => return,
        };
        self.0 &= !bit;
    }

    fn iter(self) -> impl Iterator<Item = ImageFormat> {
        const ALL_FORMATS: [ImageFormat; 7] = [
            ImageFormat::Jpeg,
            ImageFormat::WebP,
            ImageFormat::Gif,
            ImageFormat::Png,
            ImageFormat::Avif,
            ImageFormat::Jxl,
            ImageFormat::Heic,
        ];

        ALL_FORMATS.into_iter().filter(move |&f| self.contains(f))
    }
}

/// Runtime codec registry.
///
/// Controls which codecs are enabled for a given operation. Compile-time features
/// determine which codecs are *available*, while the registry controls which are
/// *enabled* at runtime.
///
/// This lets image proxies restrict codecs per-request (e.g., disable AVIF for
/// clients that don't support it).
#[derive(Clone, Debug)]
pub struct CodecRegistry {
    decode_enabled: FormatSet,
    encode_enabled: FormatSet,
}

impl CodecRegistry {
    /// All compiled-in codecs enabled.
    pub fn all() -> Self {
        Self {
            decode_enabled: FormatSet::all_compiled_decode(),
            encode_enabled: FormatSet::all_compiled_encode(),
        }
    }

    /// Nothing enabled — caller must opt in.
    pub fn none() -> Self {
        Self {
            decode_enabled: FormatSet::EMPTY,
            encode_enabled: FormatSet::EMPTY,
        }
    }

    /// Enable or disable decoding for a format.
    pub fn with_decode(mut self, format: ImageFormat, enabled: bool) -> Self {
        if enabled {
            self.decode_enabled.insert(format);
        } else {
            self.decode_enabled.remove(format);
        }
        self
    }

    /// Enable or disable encoding for a format.
    pub fn with_encode(mut self, format: ImageFormat, enabled: bool) -> Self {
        if enabled {
            self.encode_enabled.insert(format);
        } else {
            self.encode_enabled.remove(format);
        }
        self
    }

    /// Is this format available (compiled in) AND enabled for decoding?
    pub fn can_decode(&self, format: ImageFormat) -> bool {
        // Check both runtime enablement and compile-time availability
        if !self.decode_enabled.contains(format) {
            return false;
        }

        match format {
            #[cfg(feature = "jpeg")]
            ImageFormat::Jpeg => true,
            #[cfg(not(feature = "jpeg"))]
            ImageFormat::Jpeg => false,

            #[cfg(feature = "webp")]
            ImageFormat::WebP => true,
            #[cfg(not(feature = "webp"))]
            ImageFormat::WebP => false,

            #[cfg(feature = "gif")]
            ImageFormat::Gif => true,
            #[cfg(not(feature = "gif"))]
            ImageFormat::Gif => false,

            #[cfg(feature = "png")]
            ImageFormat::Png => true,
            #[cfg(not(feature = "png"))]
            ImageFormat::Png => false,

            #[cfg(feature = "avif-decode")]
            ImageFormat::Avif => true,
            #[cfg(not(feature = "avif-decode"))]
            ImageFormat::Avif => false,

            #[cfg(feature = "jxl-decode")]
            ImageFormat::Jxl => true,
            #[cfg(not(feature = "jxl-decode"))]
            ImageFormat::Jxl => false,

            #[cfg(feature = "heic-decode")]
            ImageFormat::Heic => true,
            #[cfg(not(feature = "heic-decode"))]
            ImageFormat::Heic => false,

            _ => false,
        }
    }

    /// Is this format available (compiled in) AND enabled for encoding?
    pub fn can_encode(&self, format: ImageFormat) -> bool {
        // Check both runtime enablement and compile-time availability
        if !self.encode_enabled.contains(format) {
            return false;
        }

        match format {
            #[cfg(feature = "jpeg")]
            ImageFormat::Jpeg => true,
            #[cfg(not(feature = "jpeg"))]
            ImageFormat::Jpeg => false,

            #[cfg(feature = "webp")]
            ImageFormat::WebP => true,
            #[cfg(not(feature = "webp"))]
            ImageFormat::WebP => false,

            #[cfg(feature = "gif")]
            ImageFormat::Gif => true,
            #[cfg(not(feature = "gif"))]
            ImageFormat::Gif => false,

            #[cfg(feature = "png")]
            ImageFormat::Png => true,
            #[cfg(not(feature = "png"))]
            ImageFormat::Png => false,

            #[cfg(feature = "avif-encode")]
            ImageFormat::Avif => true,
            #[cfg(not(feature = "avif-encode"))]
            ImageFormat::Avif => false,

            #[cfg(feature = "jxl-encode")]
            ImageFormat::Jxl => true,
            #[cfg(not(feature = "jxl-encode"))]
            ImageFormat::Jxl => false,

            _ => false,
        }
    }

    /// Formats that are both compiled in and enabled for decoding.
    pub fn decodable_formats(&self) -> impl Iterator<Item = ImageFormat> {
        self.decode_enabled
            .iter()
            .filter(|&f| self.can_decode(f))
            .collect::<alloc::vec::Vec<_>>()
            .into_iter()
    }

    /// Formats that are both compiled in and enabled for encoding.
    pub fn encodable_formats(&self) -> impl Iterator<Item = ImageFormat> {
        self.encode_enabled
            .iter()
            .filter(|&f| self.can_encode(f))
            .collect::<alloc::vec::Vec<_>>()
            .into_iter()
    }
}

impl Default for CodecRegistry {
    fn default() -> Self {
        Self::all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_registry() {
        let registry = CodecRegistry::all();

        // All compiled formats should be enabled
        #[cfg(feature = "jpeg")]
        assert!(registry.can_decode(ImageFormat::Jpeg));
        #[cfg(feature = "webp")]
        assert!(registry.can_decode(ImageFormat::WebP));
    }

    #[test]
    fn none_registry() {
        let registry = CodecRegistry::none();

        // Nothing should be enabled
        assert!(!registry.can_decode(ImageFormat::Jpeg));
        assert!(!registry.can_encode(ImageFormat::Jpeg));
    }

    #[test]
    fn selective_enable() {
        let registry = CodecRegistry::none()
            .with_decode(ImageFormat::Jpeg, true)
            .with_encode(ImageFormat::WebP, true);

        // Only JPEG decode should work (if compiled in)
        #[cfg(feature = "jpeg")]
        assert!(registry.can_decode(ImageFormat::Jpeg));

        // Only WebP encode should work (if compiled in)
        #[cfg(feature = "webp")]
        assert!(registry.can_encode(ImageFormat::WebP));

        // Others should not work
        assert!(!registry.can_decode(ImageFormat::Png));
        assert!(!registry.can_encode(ImageFormat::Jpeg));
    }

    #[test]
    fn toggle_format() {
        let registry = CodecRegistry::all().with_decode(ImageFormat::Jpeg, false);

        // JPEG should be disabled even if compiled
        assert!(!registry.can_decode(ImageFormat::Jpeg));
    }
}
