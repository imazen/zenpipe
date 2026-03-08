//! Runtime codec registry for enabling/disabling formats.

use crate::ImageFormat;

/// Set of image formats represented as bitflags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FormatSet(u16);

impl FormatSet {
    const EMPTY: Self = FormatSet(0);

    /// Map format to bit position. Returns None for unknown formats.
    const fn bit(format: ImageFormat) -> Option<u16> {
        match format {
            ImageFormat::Jpeg => Some(1 << 0),
            ImageFormat::WebP => Some(1 << 1),
            ImageFormat::Gif => Some(1 << 2),
            ImageFormat::Png => Some(1 << 3),
            ImageFormat::Avif => Some(1 << 4),
            ImageFormat::Jxl => Some(1 << 5),
            ImageFormat::Heic => Some(1 << 6),
            ImageFormat::Pnm => Some(1 << 7),
            ImageFormat::Bmp => Some(1 << 8),
            ImageFormat::Farbfeld => Some(1 << 9),
            _ => None,
        }
    }

    #[allow(unused_mut)]
    fn all_compiled() -> Self {
        let mut bits = 0u16;
        #[cfg(feature = "jpeg")]
        {
            bits |= 1 << 0;
        }
        #[cfg(feature = "webp")]
        {
            bits |= 1 << 1;
        }
        #[cfg(feature = "gif")]
        {
            bits |= 1 << 2;
        }
        #[cfg(feature = "png")]
        {
            bits |= 1 << 3;
        }
        #[cfg(feature = "bitmaps")]
        {
            bits |= 1 << 7; // Pnm
            bits |= 1 << 9; // Farbfeld
        }
        #[cfg(feature = "bitmaps-bmp")]
        {
            bits |= 1 << 8; // Bmp
        }
        FormatSet(bits)
    }

    #[allow(unused_mut)]
    fn all_compiled_decode() -> Self {
        let mut bits = Self::all_compiled().0;
        #[cfg(feature = "avif-decode")]
        {
            bits |= 1 << 4;
        }
        #[cfg(feature = "jxl-decode")]
        {
            bits |= 1 << 5;
        }
        #[cfg(feature = "heic-decode")]
        {
            bits |= 1 << 6;
        }
        FormatSet(bits)
    }

    #[allow(unused_mut)]
    fn all_compiled_encode() -> Self {
        let mut bits = Self::all_compiled().0;
        #[cfg(feature = "avif-encode")]
        {
            bits |= 1 << 4;
        }
        #[cfg(feature = "jxl-encode")]
        {
            bits |= 1 << 5;
        }
        FormatSet(bits)
    }

    fn contains(self, format: ImageFormat) -> bool {
        Self::bit(format).is_some_and(|b| (self.0 & b) != 0)
    }

    fn insert(&mut self, format: ImageFormat) {
        if let Some(b) = Self::bit(format) {
            self.0 |= b;
        }
    }

    fn remove(&mut self, format: ImageFormat) {
        if let Some(b) = Self::bit(format) {
            self.0 &= !b;
        }
    }

    fn iter(self) -> impl Iterator<Item = ImageFormat> {
        const ALL_FORMATS: [ImageFormat; 10] = [
            ImageFormat::Jpeg,
            ImageFormat::WebP,
            ImageFormat::Gif,
            ImageFormat::Png,
            ImageFormat::Avif,
            ImageFormat::Jxl,
            ImageFormat::Heic,
            ImageFormat::Pnm,
            ImageFormat::Bmp,
            ImageFormat::Farbfeld,
        ];
        ALL_FORMATS.into_iter().filter(move |&f| self.contains(f))
    }
}

/// Check at compile time whether a format has decode support compiled in.
fn compiled_decode(format: ImageFormat) -> bool {
    FormatSet::all_compiled_decode().contains(format)
}

/// Check at compile time whether a format has encode support compiled in.
fn compiled_encode(format: ImageFormat) -> bool {
    FormatSet::all_compiled_encode().contains(format)
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
        self.decode_enabled.contains(format) && compiled_decode(format)
    }

    /// Is this format available (compiled in) AND enabled for encoding?
    pub fn can_encode(&self, format: ImageFormat) -> bool {
        self.encode_enabled.contains(format) && compiled_encode(format)
    }

    /// Formats that are both compiled in and enabled for decoding.
    pub fn decodable_formats(&self) -> impl Iterator<Item = ImageFormat> + '_ {
        FormatSet::all_compiled_decode()
            .iter()
            .filter(|&f| self.decode_enabled.contains(f))
    }

    /// Formats that are both compiled in and enabled for encoding.
    pub fn encodable_formats(&self) -> impl Iterator<Item = ImageFormat> + '_ {
        FormatSet::all_compiled_encode()
            .iter()
            .filter(|&f| self.encode_enabled.contains(f))
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

        #[cfg(feature = "jpeg")]
        assert!(registry.can_decode(ImageFormat::Jpeg));
        #[cfg(feature = "webp")]
        assert!(registry.can_decode(ImageFormat::WebP));
    }

    #[test]
    fn none_registry() {
        let registry = CodecRegistry::none();
        assert!(!registry.can_decode(ImageFormat::Jpeg));
        assert!(!registry.can_encode(ImageFormat::Jpeg));
    }

    #[test]
    fn selective_enable() {
        let registry = CodecRegistry::none()
            .with_decode(ImageFormat::Jpeg, true)
            .with_encode(ImageFormat::WebP, true);

        #[cfg(feature = "jpeg")]
        assert!(registry.can_decode(ImageFormat::Jpeg));
        #[cfg(feature = "webp")]
        assert!(registry.can_encode(ImageFormat::WebP));

        assert!(!registry.can_decode(ImageFormat::Png));
        assert!(!registry.can_encode(ImageFormat::Jpeg));
    }

    #[test]
    fn toggle_format() {
        let registry = CodecRegistry::all().with_decode(ImageFormat::Jpeg, false);
        assert!(!registry.can_decode(ImageFormat::Jpeg));
    }

    #[test]
    fn decodable_formats_no_alloc() {
        let registry = CodecRegistry::all();
        let formats: alloc::vec::Vec<_> = registry.decodable_formats().collect();
        // Should contain at least the default features
        #[cfg(feature = "jpeg")]
        assert!(formats.contains(&ImageFormat::Jpeg));
    }
}
