use zenpixels::PlaneMask;

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
