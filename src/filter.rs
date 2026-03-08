use crate::access::ChannelAccess;
use crate::planes::OklabPlanes;

/// A photo filter that operates on planar Oklab f32 data.
///
/// Filters modify `OklabPlanes` in-place. The pipeline guarantees that
/// planes are in the correct format (f32 Oklab) before calling `apply`.
///
/// Filters are infallible — any validation (e.g., parameter clamping)
/// happens at construction time, not at apply time.
pub trait Filter: Send + Sync {
    /// Which planes this filter reads and writes.
    fn channel_access(&self) -> ChannelAccess;

    /// Whether this filter needs neighborhood access (reads adjacent pixels).
    ///
    /// Per-pixel filters return false. Neighborhood filters (clarity,
    /// brilliance, bilateral) return true.
    fn is_neighborhood(&self) -> bool {
        false
    }

    /// Apply the filter in-place to the given planes.
    fn apply(&self, planes: &mut OklabPlanes);
}
