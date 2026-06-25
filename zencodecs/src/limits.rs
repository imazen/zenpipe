//! Resource limits — a thin lean on zencodec's [`ResourceLimits`].
//!
//! zencodecs used to carry its own `Limits` struct: a field-for-field duplicate
//! of [`zencodec::ResourceLimits`] plus a manual `to_resource_limits` converter
//! and its tests. The crate now aliases the upstream type directly, so the
//! converter is an identity and the duplicate builders/validators are gone.
//!
//! `zencodecs::Limits` references keep working. Construct with the builder
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
/// Codecs periodically call `stop.check()` and return `CodecError::Cancelled`
/// if the operation should be cancelled. Use `enough::Unstoppable` when you
/// don't need cancellation (zero-cost).
pub use enough::Stop;

/// Get a `&dyn Stop` reference, defaulting to `Unstoppable` if `None`.
#[cfg(feature = "jpeg-ultrahdr")]
pub(crate) fn stop_or_default(stop: &Option<zencodec::StopToken>) -> &dyn Stop {
    match stop {
        Some(s) => s,
        None => &enough::Unstoppable,
    }
}

/// Adapt a [`Limits`] for a codec job.
///
/// Identity now that `Limits` *is* [`zencodec::ResourceLimits`] (`Copy`); kept as
/// a one-liner so the ~45 codec call sites stay terse without each importing the
/// upstream type. Inline it in a later sweep if the indirection ever grates.
#[inline]
pub(crate) fn to_resource_limits(limits: &Limits) -> zencodec::ResourceLimits {
    *limits
}
