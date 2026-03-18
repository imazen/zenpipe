//! Runtime codec registry for enabling/disabling formats.

use crate::ImageFormat;
use crate::format_set::FormatSet;

/// Check at compile time whether a format has decode support compiled in.
fn compiled_decode(format: ImageFormat) -> bool {
    all_compiled_decode().contains(format)
}

/// Check at compile time whether a format has encode support compiled in.
fn compiled_encode(format: ImageFormat) -> bool {
    all_compiled_encode().contains(format)
}

/// Check whether a `Custom` format has decode support compiled in.
///
/// Custom formats are not tracked by `FormatSet` — they are checked by
/// matching the format definition's name against known custom codecs.
#[allow(unused_variables)]
fn compiled_decode_custom(format: ImageFormat) -> bool {
    match format {
        #[cfg(feature = "raw-decode")]
        ImageFormat::Custom(def) if def.name == "dng" || def.name == "raw" => true,
        _ => false,
    }
}

#[allow(unused_mut)]
fn all_compiled() -> FormatSet {
    let mut set = FormatSet::EMPTY;
    #[cfg(feature = "jpeg")]
    {
        set.insert(ImageFormat::Jpeg);
    }
    #[cfg(feature = "webp")]
    {
        set.insert(ImageFormat::WebP);
    }
    #[cfg(feature = "gif")]
    {
        set.insert(ImageFormat::Gif);
    }
    #[cfg(feature = "png")]
    {
        set.insert(ImageFormat::Png);
    }
    #[cfg(feature = "bitmaps")]
    {
        set.insert(ImageFormat::Pnm);
        set.insert(ImageFormat::Farbfeld);
    }
    #[cfg(feature = "bitmaps-bmp")]
    {
        set.insert(ImageFormat::Bmp);
    }
    set
}

#[allow(unused_mut)]
fn all_compiled_decode() -> FormatSet {
    let mut set = all_compiled();
    #[cfg(feature = "avif-decode")]
    {
        set.insert(ImageFormat::Avif);
    }
    #[cfg(feature = "jxl-decode")]
    {
        set.insert(ImageFormat::Jxl);
    }
    #[cfg(feature = "heic-decode")]
    {
        set.insert(ImageFormat::Heic);
    }
    set
}

#[allow(unused_mut)]
fn all_compiled_encode() -> FormatSet {
    let mut set = all_compiled();
    #[cfg(feature = "avif-encode")]
    {
        set.insert(ImageFormat::Avif);
    }
    #[cfg(feature = "jxl-encode")]
    {
        set.insert(ImageFormat::Jxl);
    }
    set
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
            decode_enabled: all_compiled_decode(),
            encode_enabled: all_compiled_encode(),
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
    ///
    /// Custom formats (e.g., RAW/DNG) are always considered enabled when their
    /// feature is compiled in, since they are not tracked by `FormatSet`.
    pub fn can_decode(&self, format: ImageFormat) -> bool {
        match format {
            ImageFormat::Custom(_) => compiled_decode_custom(format),
            _ => self.decode_enabled.contains(format) && compiled_decode(format),
        }
    }

    /// Is this format available (compiled in) AND enabled for encoding?
    pub fn can_encode(&self, format: ImageFormat) -> bool {
        self.encode_enabled.contains(format) && compiled_encode(format)
    }

    /// Formats that are both compiled in and enabled for decoding.
    pub fn decodable_formats(&self) -> impl Iterator<Item = ImageFormat> + '_ {
        all_compiled_decode()
            .iter()
            .filter(|&f| self.decode_enabled.contains(f))
    }

    /// Formats that are both compiled in and enabled for encoding.
    pub fn encodable_formats(&self) -> impl Iterator<Item = ImageFormat> + '_ {
        all_compiled_encode()
            .iter()
            .filter(|&f| self.encode_enabled.contains(f))
    }

    /// Whether true streaming (row-level) decode is available for a format.
    ///
    /// Returns true for codecs that implement native row-push or scanline
    /// streaming. Returns false for codecs that buffer the full image.
    pub fn streaming_decode_available(&self, format: ImageFormat) -> bool {
        if !self.can_decode(format) {
            return false;
        }
        matches!(format, ImageFormat::Jpeg | ImageFormat::Png)
    }

    /// Whether animation decode is available for a format.
    pub fn animation_decode_available(&self, format: ImageFormat) -> bool {
        if !self.can_decode(format) {
            return false;
        }
        matches!(
            format,
            ImageFormat::Gif
                | ImageFormat::WebP
                | ImageFormat::Png
                | ImageFormat::Avif
                | ImageFormat::Jxl
        )
    }

    /// Whether animation encode is available for a format.
    pub fn animation_encode_available(&self, format: ImageFormat) -> bool {
        if !self.can_encode(format) {
            return false;
        }
        matches!(
            format,
            ImageFormat::Gif | ImageFormat::WebP | ImageFormat::Png
        )
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
