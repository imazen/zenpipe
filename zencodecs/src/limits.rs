//! Resource limits and cooperative cancellation re-exports.
//!
//! [`Limits`] aliases [`zencodec::ResourceLimits`]. Construct with its builder
//! methods — `ResourceLimits` is `#[non_exhaustive]`, so struct-literal syntax
//! is unavailable; use `Limits::none().with_max_pixels(120_000_000)` etc.

/// Resource limits for decode/encode operations.
///
/// Alias for [`zencodec::ResourceLimits`]. Used to prevent DoS / resource
/// exhaustion; build with the `with_*` methods, e.g.
/// `Limits::none().with_max_width(16_384).with_max_memory(512 << 20)`.
pub use zencodec::ResourceLimits as Limits;

/// Re-export `Stop` for cooperative cancellation.
///
/// Codecs periodically check the request's pulse / stop token and surface
/// [`CodecError::Stopped`](crate::CodecError::Stopped) carrying the
/// [`enough::StopReason`] when the operation should stop. Use
/// `enough::Unstoppable` when you don't need cancellation (zero-cost).
pub use enough::Stop;

/// Adapt a [`Limits`] for a codec job.
#[inline]
pub(crate) fn to_resource_limits(limits: &Limits) -> zencodec::ResourceLimits {
    *limits
}
