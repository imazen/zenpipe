//! Per-request format allowlist.
//!
//! [`AllowedFormats`] controls which image formats are permitted for a given
//! decode or encode operation. Compile-time features determine which codecs
//! are *available* (linked in), while `AllowedFormats` controls which are
//! *enabled* at runtime.
//!
//! For format-level capabilities (animation, lossless, alpha), use
//! [`ImageFormat::supports_animation()`](zencodec::ImageFormat::supports_animation) etc.
//! from zencodec. For codec-level capabilities (streaming, push_rows), use
//! `EncoderConfig::capabilities()` / `DecoderConfig::capabilities()`.

use crate::ImageFormat;
use crate::format_set::FormatSet;

// =========================================================================
// Compiled-in format sets (const, computed once at compile time)
// =========================================================================

/// Formats with both encode and decode support compiled in.
const fn compiled_both() -> FormatSet {
    let s = FormatSet::EMPTY;
    #[cfg(feature = "jpeg")]
    let s = s.with_const(ImageFormat::Jpeg);
    #[cfg(feature = "webp")]
    let s = s.with_const(ImageFormat::WebP);
    #[cfg(feature = "gif")]
    let s = s.with_const(ImageFormat::Gif);
    #[cfg(feature = "png")]
    let s = s.with_const(ImageFormat::Png);
    #[cfg(feature = "bitmaps")]
    let s = s
        .with_const(ImageFormat::Pnm)
        .with_const(ImageFormat::Farbfeld);
    #[cfg(feature = "bitmaps-bmp")]
    let s = s.with_const(ImageFormat::Bmp);
    #[cfg(feature = "tiff")]
    let s = s.with_const(ImageFormat::Tiff);
    s
}

/// All formats with decode support compiled in.
const COMPILED_DECODE: FormatSet = {
    let s = compiled_both();
    #[cfg(feature = "avif-decode")]
    let s = s.with_const(ImageFormat::Avif);
    #[cfg(feature = "jxl-decode")]
    let s = s.with_const(ImageFormat::Jxl);
    #[cfg(feature = "heic-decode")]
    let s = s.with_const(ImageFormat::Heic);
    s
};

/// All formats with encode support compiled in.
const COMPILED_ENCODE: FormatSet = {
    let s = compiled_both();
    #[cfg(feature = "avif-encode")]
    let s = s.with_const(ImageFormat::Avif);
    #[cfg(feature = "jxl-encode")]
    let s = s.with_const(ImageFormat::Jxl);
    s
};

// =========================================================================
// AllowedFormats
// =========================================================================

/// Per-request format allowlist.
///
/// Controls which image formats are permitted for decode and encode operations.
/// Compile-time features determine which codecs are *linked in*; this struct
/// controls which are *allowed* at runtime.
///
/// `Copy` — 4 bytes (two `u16` bitflag sets). Pass by value.
///
/// # Format capabilities
///
/// `AllowedFormats` only answers "is this format allowed?" — it does not
/// track what features a format supports. For that, use zencodec's APIs:
///
/// - [`ImageFormat::supports_animation()`](zencodec::ImageFormat::supports_animation)
/// - [`ImageFormat::supports_lossless()`](zencodec::ImageFormat::supports_lossless)
/// - [`ImageFormat::supports_alpha()`](zencodec::ImageFormat::supports_alpha)
/// - `EncoderConfig::capabilities()` / `DecoderConfig::capabilities()` for codec-level features
///
/// # Custom formats
///
/// Custom formats (e.g., RAW/DNG via `ImageFormat::Custom`) are not tracked
/// by the bitflag sets and are always considered disabled. Use format-specific
/// decode APIs for custom formats.
#[derive(Clone, Copy, Debug)]
pub struct AllowedFormats {
    decode: FormatSet,
    encode: FormatSet,
}

impl AllowedFormats {
    /// All compiled-in codecs enabled.
    pub fn all() -> Self {
        Self {
            decode: COMPILED_DECODE,
            encode: COMPILED_ENCODE,
        }
    }

    /// Nothing enabled — caller must opt in.
    pub fn none() -> Self {
        Self {
            decode: FormatSet::EMPTY,
            encode: FormatSet::EMPTY,
        }
    }

    /// Enable or disable decoding for a format.
    pub fn with_decode(mut self, format: ImageFormat, enabled: bool) -> Self {
        if enabled {
            self.decode.insert(format);
        } else {
            self.decode.remove(format);
        }
        self
    }

    /// Enable or disable encoding for a format.
    pub fn with_encode(mut self, format: ImageFormat, enabled: bool) -> Self {
        if enabled {
            self.encode.insert(format);
        } else {
            self.encode.remove(format);
        }
        self
    }

    /// Is this format compiled in AND enabled for decoding?
    pub fn can_decode(&self, format: ImageFormat) -> bool {
        self.decode.contains(format) && COMPILED_DECODE.contains(format)
    }

    /// Is this format compiled in AND enabled for encoding?
    pub fn can_encode(&self, format: ImageFormat) -> bool {
        self.encode.contains(format) && COMPILED_ENCODE.contains(format)
    }

    /// Formats that are both compiled in and enabled for decoding.
    pub fn decodable_formats(&self) -> impl Iterator<Item = ImageFormat> {
        self.decode.intersection(&COMPILED_DECODE).iter()
    }

    /// Formats that are both compiled in and enabled for encoding.
    pub fn encodable_formats(&self) -> impl Iterator<Item = ImageFormat> {
        self.encode.intersection(&COMPILED_ENCODE).iter()
    }

    /// The raw decode FormatSet (for intersection with policy sets etc.).
    pub fn decode_set(&self) -> FormatSet {
        self.decode.intersection(&COMPILED_DECODE)
    }

    /// The raw encode FormatSet.
    pub fn encode_set(&self) -> FormatSet {
        self.encode.intersection(&COMPILED_ENCODE)
    }
}

impl Default for AllowedFormats {
    fn default() -> Self {
        Self::all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_allows_compiled() {
        let af = AllowedFormats::all();

        #[cfg(feature = "jpeg")]
        assert!(af.can_decode(ImageFormat::Jpeg));
        #[cfg(feature = "webp")]
        assert!(af.can_decode(ImageFormat::WebP));
    }

    #[test]
    fn none_denies_all() {
        let af = AllowedFormats::none();
        assert!(!af.can_decode(ImageFormat::Jpeg));
        assert!(!af.can_encode(ImageFormat::Jpeg));
    }

    #[test]
    fn selective_enable() {
        let af = AllowedFormats::none()
            .with_decode(ImageFormat::Jpeg, true)
            .with_encode(ImageFormat::WebP, true);

        #[cfg(feature = "jpeg")]
        assert!(af.can_decode(ImageFormat::Jpeg));
        #[cfg(feature = "webp")]
        assert!(af.can_encode(ImageFormat::WebP));

        assert!(!af.can_decode(ImageFormat::Png));
        assert!(!af.can_encode(ImageFormat::Jpeg));
    }

    #[test]
    fn toggle_format() {
        let af = AllowedFormats::all().with_decode(ImageFormat::Jpeg, false);
        assert!(!af.can_decode(ImageFormat::Jpeg));
    }

    #[test]
    fn decodable_formats_iteration() {
        let af = AllowedFormats::all();
        let formats: alloc::vec::Vec<_> = af.decodable_formats().collect();
        #[cfg(feature = "jpeg")]
        assert!(formats.contains(&ImageFormat::Jpeg));
    }

    #[test]
    fn is_copy() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<AllowedFormats>();
    }

    #[test]
    fn compiled_sets_are_consistent() {
        // Every format in COMPILED_ENCODE should also be in COMPILED_DECODE
        for fmt in COMPILED_ENCODE.iter() {
            assert!(
                COMPILED_DECODE.contains(fmt),
                "{fmt:?} is in COMPILED_ENCODE but not COMPILED_DECODE"
            );
        }
    }

    #[test]
    fn enabling_non_compiled_format_still_returns_false() {
        let af = AllowedFormats::none().with_decode(ImageFormat::Avif, true);
        // AVIF is in the bitflag but can_decode checks COMPILED_DECODE too
        #[cfg(not(feature = "avif-decode"))]
        assert!(!af.can_decode(ImageFormat::Avif));
        let _ = af;
    }

    #[test]
    fn format_capabilities_from_zencodec() {
        // Animation/lossless/alpha capabilities come from zencodec, not from us
        assert!(!ImageFormat::Jpeg.supports_animation());
        assert!(ImageFormat::Gif.supports_animation());
        assert!(!ImageFormat::Jpeg.supports_lossless());
        assert!(ImageFormat::Png.supports_lossless());
        assert!(!ImageFormat::Jpeg.supports_alpha());
        assert!(ImageFormat::Png.supports_alpha());
    }

    // ═══════════════════════════════════════════════════════════════════
    // Regression: AVIF in AllowedFormats and FormatSet
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn avif_in_format_set_all() {
        // AVIF should always be present in FormatSet::all(), regardless
        // of whether AVIF codecs are compiled in.
        let all = FormatSet::all();
        assert!(
            all.contains(ImageFormat::Avif),
            "AVIF must be present in FormatSet::all()"
        );
    }

    #[test]
    fn avif_in_modern_web_format_set() {
        let modern = FormatSet::modern_web();
        assert!(
            modern.contains(ImageFormat::Avif),
            "AVIF must be present in FormatSet::modern_web()"
        );
    }

    #[test]
    #[cfg(feature = "avif-decode")]
    fn avif_decode_in_compiled_decode() {
        let af = AllowedFormats::all();
        assert!(
            af.can_decode(ImageFormat::Avif),
            "AVIF decoding should be available when avif-decode feature is enabled"
        );
    }

    #[test]
    #[cfg(feature = "avif-encode")]
    fn avif_encode_in_compiled_encode() {
        let af = AllowedFormats::all();
        assert!(
            af.can_encode(ImageFormat::Avif),
            "AVIF encoding should be available when avif-encode feature is enabled"
        );
    }

    #[test]
    #[cfg(all(feature = "avif-decode", feature = "avif-encode"))]
    fn avif_in_both_decodable_and_encodable() {
        let af = AllowedFormats::all();
        let decodable: alloc::vec::Vec<_> = af.decodable_formats().collect();
        let encodable: alloc::vec::Vec<_> = af.encodable_formats().collect();
        assert!(
            decodable.contains(&ImageFormat::Avif),
            "AVIF should be in decodable_formats when avif-decode is enabled"
        );
        assert!(
            encodable.contains(&ImageFormat::Avif),
            "AVIF should be in encodable_formats when avif-encode is enabled"
        );
    }

    #[test]
    fn allowed_formats_all_includes_expected_formats() {
        // AllowedFormats::all() should include all formats that are compiled in.
        // Verify a representative set of core formats.
        let af = AllowedFormats::all();

        #[cfg(feature = "jpeg")]
        {
            assert!(af.can_decode(ImageFormat::Jpeg));
            assert!(af.can_encode(ImageFormat::Jpeg));
        }
        #[cfg(feature = "png")]
        {
            assert!(af.can_decode(ImageFormat::Png));
            assert!(af.can_encode(ImageFormat::Png));
        }
        #[cfg(feature = "webp")]
        {
            assert!(af.can_decode(ImageFormat::WebP));
            assert!(af.can_encode(ImageFormat::WebP));
        }
        #[cfg(feature = "gif")]
        {
            assert!(af.can_decode(ImageFormat::Gif));
            assert!(af.can_encode(ImageFormat::Gif));
        }
    }
}
