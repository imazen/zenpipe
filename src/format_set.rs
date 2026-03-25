//! Bitflag set of [`ImageFormat`] values.

use crate::ImageFormat;

/// Compact bitflag set of image formats.
///
/// Used by [`CodecPolicy`](crate::CodecPolicy) to restrict which output formats
/// are candidates for auto-selection, and internally by the registry to track
/// which formats are compiled in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatSet(u16);

impl FormatSet {
    /// Empty set — no formats.
    pub const EMPTY: Self = FormatSet(0);

    /// Map a format to its bit position.
    pub(crate) const fn bit(format: ImageFormat) -> Option<u16> {
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
            _ => None,
        }
    }

    const ALL_FORMATS: [ImageFormat; 11] = [
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
    ];

    /// All known formats.
    pub fn all() -> Self {
        let mut bits = 0u16;
        // All 11 formats: bits 0-10
        bits |= (1 << 11) - 1;
        FormatSet(bits)
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

    /// Create from raw bits (for const contexts).
    pub(crate) const fn from_bits(bits: u16) -> Self {
        Self(bits)
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
        assert_eq!(set.len(), 11);
        assert!(set.contains(ImageFormat::Jpeg));
        assert!(set.contains(ImageFormat::Farbfeld));
        assert!(set.contains(ImageFormat::Tiff));
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
