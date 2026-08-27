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
// Custom-format identity tracking (decode-only)
// =========================================================================
//
// `ImageFormat::Custom(&'static ImageFormatDefinition)` is open-ended --
// downstream codec crates (zenraw, zenpdf, ...) define their own statics, so
// `FormatSet`'s fixed bitflag (keyed on the *named* `ImageFormat` variants)
// can't represent them. Custom-format identity is name-based (per
// `ImageFormatDefinition`'s own `PartialEq` impl: "two definitions with the
// same name are considered equal"), so track the small, closed set of
// Custom decode formats this crate actually wires up (RAW/DNG/PDF) by name
// in a side bitset. This is what both `decode.rs::resolve_format` and
// `info.rs::from_bytes_with_registry` gate on -- there is exactly one
// registry check for Custom formats, not a probe-only or decode-only one.
//
// Encode has no Custom format today, so there is no `custom_encode` side --
// a `Custom` format is always considered disabled for encode (matches the
// pre-existing "always disabled" behavior, now scoped to encode only).

/// Compare two `&str` for equality in a `const fn` (no `const PartialEq` on
/// stable yet -- manual byte comparison instead).
const fn str_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Map a Custom format's `ImageFormatDefinition::name` to its tracking bit.
/// Returns `None` for any name this crate doesn't wire up as a Custom
/// decode format -- such a format is never trackable (can't be enabled or
/// disabled) and `can_decode` treats it as unsupported.
const fn custom_decode_bit(name: &str) -> Option<u16> {
    if str_eq(name, "dng") {
        Some(1 << 0)
    } else if str_eq(name, "raw") {
        Some(1 << 1)
    } else if str_eq(name, "pdf") {
        Some(1 << 2)
    } else if str_eq(name, "svg") {
        Some(1 << 3)
    } else {
        None
    }
}

/// Custom decode formats compiled in, by the same name-keyed bits as
/// [`custom_decode_bit`].
///
/// Uses `cfg!(feature = ...)` (a `bool` at const-eval time) rather than
/// `#[cfg(feature = ...)]` attribute-stripping: with every custom-decode
/// feature off, `#[cfg]` would strip both bodies entirely, leaving `bits`
/// never mutated (a real `unused_mut` warning) -- `cfg!()` keeps the
/// mutations syntactically present so `mut` is always genuinely used, with
/// the dead branch folded away at const-eval time either way.
const COMPILED_CUSTOM_DECODE: u16 = {
    let mut bits = 0u16;
    // Both DNG and generic RAW come from the same `raw-decode` feature (zenraw).
    if cfg!(feature = "raw-decode") {
        if let Some(b) = custom_decode_bit("dng") {
            bits |= b;
        }
        if let Some(b) = custom_decode_bit("raw") {
            bits |= b;
        }
    }
    if cfg!(feature = "pdf-decode")
        && let Some(b) = custom_decode_bit("pdf")
    {
        bits |= b;
    }
    if cfg!(feature = "svg")
        && let Some(b) = custom_decode_bit("svg")
    {
        bits |= b;
    }
    bits
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
/// `Copy` — 6 bytes (two `u16` bitflag sets for the named formats, one `u16`
/// side-set for name-identified Custom decode formats). Pass by value.
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
/// `ImageFormat::Custom` decode formats (RAW/DNG via zenraw, PDF via zenpdf)
/// *are* tracked, by name (`ImageFormatDefinition::name`) rather than by the
/// bitflag sets — see [`can_decode`](Self::can_decode). A Custom format this
/// crate doesn't wire up (an unrecognized name) is never trackable and
/// [`can_decode`](Self::can_decode) always returns `false` for it. Custom
/// *encode* formats are always disabled — there are none today.
#[derive(Clone, Copy, Debug)]
pub struct AllowedFormats {
    decode: FormatSet,
    encode: FormatSet,
    /// Side bitset for name-identified `ImageFormat::Custom` decode formats
    /// (RAW/DNG/PDF) — see the module-level "Custom-format identity tracking"
    /// section above. `FormatSet` can't represent these: they're open-ended,
    /// defined by downstream codec crates, not fixed enum variants.
    custom_decode: u16,
}

impl AllowedFormats {
    /// All compiled-in codecs enabled.
    pub fn all() -> Self {
        Self {
            decode: COMPILED_DECODE,
            encode: COMPILED_ENCODE,
            custom_decode: COMPILED_CUSTOM_DECODE,
        }
    }

    /// Nothing enabled — caller must opt in.
    pub fn none() -> Self {
        Self {
            decode: FormatSet::EMPTY,
            encode: FormatSet::EMPTY,
            custom_decode: 0,
        }
    }

    /// Enable or disable decoding for a format.
    ///
    /// For `ImageFormat::Custom(def)`, toggles by `def.name` — only names
    /// this crate wires up (`"dng"`, `"raw"`, `"pdf"`, `"svg"`) are trackable; any
    /// other Custom name is a no-op (it can never be enabled).
    pub fn with_decode(mut self, format: ImageFormat, enabled: bool) -> Self {
        match format {
            ImageFormat::Custom(def) => {
                if let Some(bit) = custom_decode_bit(def.name) {
                    if enabled {
                        self.custom_decode |= bit;
                    } else {
                        self.custom_decode &= !bit;
                    }
                }
            }
            _ => {
                if enabled {
                    self.decode.insert(format);
                } else {
                    self.decode.remove(format);
                }
            }
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
    ///
    /// `ImageFormat::Custom(def)` is gated by `def.name` against the
    /// compiled-in + enabled Custom decode set (RAW/DNG/PDF) — same
    /// authority as any named format, no bypass. An unrecognized Custom
    /// name always returns `false`.
    pub fn can_decode(&self, format: ImageFormat) -> bool {
        match format {
            ImageFormat::Custom(def) => custom_decode_bit(def.name).is_some_and(|bit| {
                (self.custom_decode & bit) != 0 && Self::custom_decode_compiled(bit)
            }),
            _ => self.decode.contains(format) && COMPILED_DECODE.contains(format),
        }
    }

    /// Whether a `custom_decode_bit` value is in the compiled-in set.
    ///
    /// Split out so the `allow` below is scoped tightly: with both
    /// `raw-decode`, `pdf-decode` and `svg` off, `COMPILED_CUSTOM_DECODE` legitimately
    /// resolves to `0` and clippy's `bad_bit_mask` (correctly) notices the
    /// check can never be true in that configuration -- which is the right
    /// behavior (nothing Custom is compiled in, so nothing can match), not a bug.
    #[allow(clippy::bad_bit_mask)]
    const fn custom_decode_compiled(bit: u16) -> bool {
        (COMPILED_CUSTOM_DECODE & bit) != 0
    }

    /// Is this format compiled in AND enabled for encoding?
    pub fn can_encode(&self, format: ImageFormat) -> bool {
        self.encode.contains(format) && COMPILED_ENCODE.contains(format)
    }

    /// Formats that are both compiled in and enabled for decoding.
    ///
    /// Does not enumerate Custom decode formats (RAW/DNG/PDF/SVG) — those are
    /// name-identified, not values this crate can construct generically.
    /// Use [`can_decode`](Self::can_decode) to check a specific Custom format.
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

    // ═══════════════════════════════════════════════════════════════════
    // Custom-format identity tracking (RAW/DNG/PDF) — both directions
    // ═══════════════════════════════════════════════════════════════════
    //
    // Regression coverage for the two Custom-format registry bugs: `none()`
    // used to fail *open* (decode.rs bypassed the registry check entirely
    // for any `ImageFormat::Custom`), and `all()` used to fail *closed*
    // (`FormatSet::contains` can't represent `Custom` at all, so
    // `can_decode` always returned `false` even under `all()`).

    #[test]
    #[cfg(feature = "raw-decode")]
    fn all_allows_compiled_custom_raw_dng() {
        let af = AllowedFormats::all();
        assert!(
            af.can_decode(ImageFormat::Custom(&zenraw::DNG_FORMAT)),
            "AllowedFormats::all() must allow a compiled-in Custom format (DNG)"
        );
        assert!(
            af.can_decode(ImageFormat::Custom(&zenraw::RAW_FORMAT)),
            "AllowedFormats::all() must allow a compiled-in Custom format (RAW)"
        );
    }

    #[test]
    #[cfg(feature = "raw-decode")]
    fn none_denies_custom_raw_dng() {
        let af = AllowedFormats::none();
        assert!(
            !af.can_decode(ImageFormat::Custom(&zenraw::DNG_FORMAT)),
            "AllowedFormats::none() must deny Custom formats too (no fail-open bypass)"
        );
        assert!(
            !af.can_decode(ImageFormat::Custom(&zenraw::RAW_FORMAT)),
            "AllowedFormats::none() must deny Custom formats too (no fail-open bypass)"
        );
    }

    #[test]
    #[cfg(feature = "raw-decode")]
    fn selective_enable_custom_decode() {
        let af = AllowedFormats::none().with_decode(ImageFormat::Custom(&zenraw::DNG_FORMAT), true);
        assert!(af.can_decode(ImageFormat::Custom(&zenraw::DNG_FORMAT)));
        // RAW wasn't enabled — toggling DNG must not also enable RAW.
        assert!(!af.can_decode(ImageFormat::Custom(&zenraw::RAW_FORMAT)));
    }

    #[test]
    #[cfg(feature = "raw-decode")]
    fn toggle_custom_decode_off() {
        let af = AllowedFormats::all().with_decode(ImageFormat::Custom(&zenraw::DNG_FORMAT), false);
        assert!(!af.can_decode(ImageFormat::Custom(&zenraw::DNG_FORMAT)));
        // Disabling DNG must not also disable RAW.
        assert!(af.can_decode(ImageFormat::Custom(&zenraw::RAW_FORMAT)));
    }

    #[test]
    #[cfg(feature = "pdf-decode")]
    fn all_allows_compiled_custom_pdf() {
        let af = AllowedFormats::all();
        assert!(
            af.can_decode(ImageFormat::Custom(&zenpdf::PDF_FORMAT)),
            "AllowedFormats::all() must allow a compiled-in Custom format (PDF)"
        );
    }

    #[test]
    #[cfg(feature = "pdf-decode")]
    fn none_denies_custom_pdf() {
        let af = AllowedFormats::none();
        assert!(
            !af.can_decode(ImageFormat::Custom(&zenpdf::PDF_FORMAT)),
            "AllowedFormats::none() must deny Custom formats too (no fail-open bypass)"
        );
    }

    #[test]
    fn unrecognized_custom_name_never_trackable() {
        // A Custom format this crate doesn't wire up (any name outside
        // "dng"/"raw"/"pdf"/"svg") must never be allowed — not even under `all()`
        // — since it can't be enabled, and never bypassed.
        static UNKNOWN: zencodec::ImageFormatDefinition = zencodec::ImageFormatDefinition::new(
            "some-future-format",
            None,
            "Some Future Format",
            "fut",
            &["fut"],
            "application/x-future",
            &["application/x-future"],
            false,
            false,
            false,
            false,
            4,
            |_data| false,
        );
        let all = AllowedFormats::all();
        assert!(!all.can_decode(ImageFormat::Custom(&UNKNOWN)));
        let none = AllowedFormats::none();
        assert!(!none.can_decode(ImageFormat::Custom(&UNKNOWN)));
        // with_decode(..., true) is a documented no-op for an unrecognized name.
        let enabled_attempt =
            AllowedFormats::none().with_decode(ImageFormat::Custom(&UNKNOWN), true);
        assert!(!enabled_attempt.can_decode(ImageFormat::Custom(&UNKNOWN)));
    }
}
