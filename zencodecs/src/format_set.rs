//! Bitflag set of [`ImageFormat`] values.

use crate::ImageFormat;

/// Compact bitflag set of image formats.
///
/// Used by [`CodecPolicy`](crate::CodecPolicy) to restrict which output formats
/// are candidates for auto-selection, and internally by the registry to track
/// which formats are compiled in.
///
/// # Representable formats
///
/// Every **named** [`ImageFormat`] variant has a bit here. Two inputs are not
/// representable and are silently ignored by [`insert`](Self::insert) /
/// [`with`](Self::with) (and therefore never [`contains`](Self::contains)):
///
/// * [`ImageFormat::Unknown`] — not a format.
/// * [`ImageFormat::Custom`] — identified by a unique
///   `ImageFormatDefinition::name` (`&'static str`), which a bitflag cannot
///   hold. Policy over custom formats needs a name-keyed set instead; see
///   `AllowedFormats`' `custom_decode` side-channel for the existing pattern.
///
/// `ImageFormat` is `#[non_exhaustive]`, so a future variant added upstream
/// also lands in that bucket until [`bit`](Self::bit) and `ALL_FORMATS` learn
/// it. `all_named_formats_are_representable` is the regression gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatSet(u32);

impl FormatSet {
    /// Empty set — no formats.
    pub const EMPTY: Self = FormatSet(0);

    /// Map a format to its bit position.
    ///
    /// `None` for the non-representable inputs documented on the type
    /// ([`ImageFormat::Unknown`], [`ImageFormat::Custom`], and any future
    /// upstream variant).
    ///
    /// Bit positions 0-10 are historical and deliberately unchanged; new
    /// formats append from 11.
    pub(crate) const fn bit(format: ImageFormat) -> Option<u32> {
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
            ImageFormat::Tiff => Some(1 << 10),
            // Appended 2026-07-15 — ImageFormat's remaining named variants,
            // which the u16 table silently dropped: `with(Qoi)` returned an
            // EMPTY set and `all()` excluded them, so any explicit allowlist
            // denied them outright.
            //
            // The deny was LATENT when written, not live: `svg` and
            // `jp2-decode` are compile_error stubs (zenpipe#43) so their
            // adapters are unreachable, Ico/Exr/Dng have no adapter in
            // this crate at all, and Pdf/Raw are decode-only while
            // is_format_allowed gates encode-side selection. Fixed anyway
            // because the silent drop is a live API footgun regardless of
            // reachability, and because 20 named formats structurally do not
            // fit in u16 — wiring any of these later would trip it instantly.
            ImageFormat::Ico => Some(1 << 11),
            ImageFormat::Qoi => Some(1 << 12),
            ImageFormat::Pdf => Some(1 << 13),
            ImageFormat::Exr => Some(1 << 14),
            ImageFormat::Hdr => Some(1 << 15),
            ImageFormat::Tga => Some(1 << 16),
            ImageFormat::Dng => Some(1 << 17),
            ImageFormat::Raw => Some(1 << 18),
            ImageFormat::Svg => Some(1 << 19),
            // Not representable — see the type docs. `Unknown` and `Custom`
            // are spelled out so adding an upstream variant is a visible gap
            // in review rather than an invisible fall-through.
            ImageFormat::Unknown | ImageFormat::Custom(_) => None,
            _ => None,
        }
    }

    /// Every named format, in bit order. Keep in sync with [`bit`](Self::bit) —
    /// `all_named_formats_are_representable` fails if an entry has no bit.
    const ALL_FORMATS: [ImageFormat; 20] = [
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
        ImageFormat::Tiff,
        ImageFormat::Ico,
        ImageFormat::Qoi,
        ImageFormat::Pdf,
        ImageFormat::Exr,
        ImageFormat::Hdr,
        ImageFormat::Tga,
        ImageFormat::Dng,
        ImageFormat::Raw,
        ImageFormat::Svg,
    ];

    /// All known (named) formats.
    ///
    /// Derived from `ALL_FORMATS` rather than a hand-written bit mask — the
    /// hardcoded `(1 << 11) - 1` it replaces is exactly how nine wired formats
    /// went missing.
    pub fn all() -> Self {
        let mut set = FormatSet::EMPTY;
        let mut i = 0;
        while i < Self::ALL_FORMATS.len() {
            set = set.with_const(Self::ALL_FORMATS[i]);
            i += 1;
        }
        set
    }

    /// Web-safe formats only (JPEG, PNG, GIF).
    pub fn web_safe() -> Self {
        Self::EMPTY
            .with(ImageFormat::Jpeg)
            .with(ImageFormat::Png)
            .with(ImageFormat::Gif)
    }

    /// Modern web formats (JPEG, PNG, GIF, WebP, AVIF, JXL).
    pub fn modern_web() -> Self {
        Self::web_safe()
            .with(ImageFormat::WebP)
            .with(ImageFormat::Avif)
            .with(ImageFormat::Jxl)
    }

    /// Add a format to the set (builder style).
    pub fn with(mut self, format: ImageFormat) -> Self {
        self.insert(format);
        self
    }

    /// Remove a format from the set (builder style).
    pub fn without(mut self, format: ImageFormat) -> Self {
        self.remove(format);
        self
    }

    /// Const-compatible version of [`with`](Self::with).
    ///
    /// Use this in `const` or `static` contexts where `&mut self` isn't available.
    pub const fn with_const(self, format: ImageFormat) -> Self {
        match Self::bit(format) {
            Some(b) => Self(self.0 | b),
            None => self,
        }
    }

    /// Insert a format.
    pub fn insert(&mut self, format: ImageFormat) {
        if let Some(b) = Self::bit(format) {
            self.0 |= b;
        }
    }

    /// Remove a format.
    pub fn remove(&mut self, format: ImageFormat) {
        if let Some(b) = Self::bit(format) {
            self.0 &= !b;
        }
    }

    /// Check if a format is in the set.
    pub fn contains(&self, format: ImageFormat) -> bool {
        Self::bit(format).is_some_and(|b| (self.0 & b) != 0)
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// Number of formats in the set.
    pub fn len(&self) -> usize {
        self.0.count_ones() as usize
    }

    /// Iterate over formats in the set.
    pub fn iter(&self) -> impl Iterator<Item = ImageFormat> + use<> {
        let bits = self.0;
        Self::ALL_FORMATS
            .into_iter()
            .filter(move |&f| Self::bit(f).is_some_and(|b| (bits & b) != 0))
    }

    /// Intersection of two sets.
    pub fn intersection(&self, other: &Self) -> Self {
        FormatSet(self.0 & other.0)
    }

    /// Union of two sets.
    pub fn union(&self, other: &Self) -> Self {
        FormatSet(self.0 | other.0)
    }
}

impl Default for FormatSet {
    /// Default is all formats.
    fn default() -> Self {
        Self::all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_set() {
        let set = FormatSet::EMPTY;
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
        assert!(!set.contains(ImageFormat::Jpeg));
    }

    #[test]
    fn all_set() {
        let set = FormatSet::all();
        assert!(!set.is_empty());
        // Derived, not hardcoded: the old `assert_eq!(len, 11)` passed against
        // a table that was missing nine wired formats.
        assert_eq!(set.len(), FormatSet::ALL_FORMATS.len());
        assert!(set.contains(ImageFormat::Jpeg));
        assert!(set.contains(ImageFormat::Farbfeld));
        assert!(set.contains(ImageFormat::Tiff));
    }

    /// Every named format must round-trip through the set. This is the gate the
    /// `u16` table lacked: `bit()` fell through to `_ => None` for nine formats
    /// this crate actually wires, so `with()` silently no-op'd and `all()`
    /// excluded them.
    #[test]
    fn all_named_formats_are_representable() {
        for f in FormatSet::ALL_FORMATS {
            assert!(
                FormatSet::bit(f).is_some(),
                "{f:?} has no bit — add it to bit() and ALL_FORMATS"
            );
            assert!(
                FormatSet::EMPTY.with(f).contains(f),
                "{f:?} was silently dropped by with()/insert()"
            );
            assert!(FormatSet::all().contains(f), "all() excludes {f:?}");
            assert_eq!(FormatSet::EMPTY.with(f).len(), 1, "{f:?} bit collides");
        }
        // Bits must be distinct: 20 formats -> 20 set bits.
        assert_eq!(FormatSet::all().len(), 20);
    }

    /// The formats that regressed: wired codecs (`codecs/{qoi,tga,hdr}.rs`)
    /// that an explicit allowlist used to deny outright.
    #[test]
    fn previously_dropped_formats_survive_an_allowlist() {
        for f in [
            ImageFormat::Qoi,
            ImageFormat::Tga,
            ImageFormat::Hdr,
            ImageFormat::Ico,
            ImageFormat::Exr,
            ImageFormat::Pdf,
            ImageFormat::Dng,
            ImageFormat::Raw,
            ImageFormat::Svg,
        ] {
            let allowlist = FormatSet::EMPTY.with(f);
            assert!(!allowlist.is_empty(), "{f:?} produced an EMPTY allowlist");
            assert!(allowlist.contains(f));
            assert!(!allowlist.contains(ImageFormat::Jpeg));
            assert_eq!(allowlist.len(), 1);
            let mut it = allowlist.iter();
            assert_eq!(it.next(), Some(f));
            assert_eq!(it.next(), None);
        }
    }

    /// Documents the remaining known limitation (imazen/zencodec#121): a
    /// bitflag cannot key on `ImageFormatDefinition::name`, so custom formats
    /// stay unrepresentable. Asserted so the behavior is intentional, not a
    /// silent surprise — flip this test when a name-keyed set lands.
    #[test]
    fn custom_and_unknown_are_not_representable() {
        fn never(_: &[u8]) -> bool {
            false
        }
        static DEF: zencodec::ImageFormatDefinition = zencodec::ImageFormatDefinition::new(
            "test-custom",
            None,
            "Test Custom",
            "tc",
            &["tc"],
            "image/x-test-custom",
            &["image/x-test-custom"],
            false,
            false,
            true,
            false,
            0,
            never,
        );
        let custom = ImageFormat::Custom(&DEF);
        assert!(FormatSet::bit(custom).is_none());
        assert!(FormatSet::bit(ImageFormat::Unknown).is_none());
        assert!(!FormatSet::all().contains(custom));
        assert!(FormatSet::EMPTY.with(custom).is_empty());
    }

    #[test]
    fn web_safe() {
        let set = FormatSet::web_safe();
        assert!(set.contains(ImageFormat::Jpeg));
        assert!(set.contains(ImageFormat::Png));
        assert!(set.contains(ImageFormat::Gif));
        assert!(!set.contains(ImageFormat::WebP));
        assert!(!set.contains(ImageFormat::Avif));
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn modern_web() {
        let set = FormatSet::modern_web();
        assert!(set.contains(ImageFormat::WebP));
        assert!(set.contains(ImageFormat::Avif));
        assert!(set.contains(ImageFormat::Jxl));
        assert_eq!(set.len(), 6);
    }

    #[test]
    fn builder_with_without() {
        let set = FormatSet::EMPTY
            .with(ImageFormat::Jpeg)
            .with(ImageFormat::Png)
            .without(ImageFormat::Jpeg);
        assert!(!set.contains(ImageFormat::Jpeg));
        assert!(set.contains(ImageFormat::Png));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn set_operations() {
        let a = FormatSet::EMPTY
            .with(ImageFormat::Jpeg)
            .with(ImageFormat::Png);
        let b = FormatSet::EMPTY
            .with(ImageFormat::Png)
            .with(ImageFormat::WebP);
        assert_eq!(a.intersection(&b), FormatSet::EMPTY.with(ImageFormat::Png));
        assert_eq!(a.union(&b).len(), 3);
    }

    #[test]
    fn iter_order() {
        let set = FormatSet::EMPTY
            .with(ImageFormat::Png)
            .with(ImageFormat::Jpeg);
        let formats: alloc::vec::Vec<_> = set.iter().collect();
        // Iteration order is bit order (Jpeg=0, Png=3), not insertion order
        assert_eq!(formats, &[ImageFormat::Jpeg, ImageFormat::Png]);
    }
}
