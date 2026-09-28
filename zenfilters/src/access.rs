/// Selection of the filter pipeline's own four working planes.
///
/// Plane zero is Oklab lightness (red in an RGB working space), planes one
/// and two are Oklab chroma (green/blue in RGB), and plane three is alpha.
/// This mask carries no YUV layout, subsampling, or video sample semantics.
///
/// ```
/// use zenfilters::{ChannelAccess, PlaneMask};
/// let access = ChannelAccess::new(PlaneMask::ALL,
///     PlaneMask::LUMA.union(PlaneMask::ALPHA));
/// assert!(access.writes.intersection(PlaneMask::CHROMA).is_empty());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlaneMask(u8);

impl PlaneMask {
    /// All four working planes, including alpha.
    pub const ALL: Self = Self(0b1111);
    /// No working planes.
    pub const NONE: Self = Self(0);
    /// First color plane: Oklab lightness or RGB red.
    pub const LUMA: Self = Self(0b0001);
    /// Second and third color planes: Oklab chroma or RGB green/blue.
    pub const CHROMA: Self = Self(0b0110);
    /// Alpha coverage plane.
    pub const ALPHA: Self = Self(0b1000);

    /// Select planes present in either mask.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    /// Select planes present in both masks.
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
    /// Whether the selection is empty.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Declares which Oklab planes a filter reads and writes.
///
/// The pipeline uses this to skip unchanged planes and to determine
/// whether adjacent filters can share a planar layout without
/// intermediate scatter/gather.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChannelAccess {
    /// Planes this filter reads.
    pub reads: PlaneMask,
    /// Planes this filter writes.
    pub writes: PlaneMask,
}

impl ChannelAccess {
    /// Create a custom channel access descriptor.
    pub const fn new(reads: PlaneMask, writes: PlaneMask) -> Self {
        Self { reads, writes }
    }

    /// Filter reads and writes only the L (lightness) plane.
    pub const L_ONLY: Self = Self {
        reads: PlaneMask::LUMA,
        writes: PlaneMask::LUMA,
    };

    /// Filter reads and writes only the chroma (a, b) planes.
    pub const CHROMA_ONLY: Self = Self {
        reads: PlaneMask::CHROMA,
        writes: PlaneMask::CHROMA,
    };

    /// Filter reads and writes L, a, and b planes.
    pub const L_AND_CHROMA: Self = Self {
        reads: PlaneMask::LUMA.union(PlaneMask::CHROMA),
        writes: PlaneMask::LUMA.union(PlaneMask::CHROMA),
    };

    /// Filter reads and writes all planes including alpha.
    pub const ALL: Self = Self {
        reads: PlaneMask::ALL,
        writes: PlaneMask::ALL,
    };
}
