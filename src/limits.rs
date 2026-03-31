//! Resource limits for pipeline operations.
//!
//! [`Limits`] prevents memory-bomb attacks by rejecting images that exceed
//! configured dimension or pixel count thresholds before allocating.
//!
//! # Runtime enforcement
//!
//! [`AllocationTracker`] provides runtime memory tracking with RAII guards.
//! Wrap each buffer allocation in [`allocate()`](AllocationTracker::allocate)
//! and hold the returned [`AllocationGuard`] for the buffer's lifetime.
//!
//! [`Deadline`] (requires `std`) implements [`enough::Stop`] so it composes
//! with the existing cancellation path in [`execute_with_stop`](crate::execute_with_stop).

use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};

#[allow(unused_imports)]
use whereat::at;

use crate::error::PipeError;
use crate::format::PixelFormat;

/// Resource limits for pipeline operations.
///
/// Check these before any allocation proportional to image dimensions
/// (materialize, tee, orient, resize output). All fields are optional —
/// `None` means no limit.
///
/// # Example
///
/// ```
/// use zenpipe::Limits;
///
/// let limits = Limits::default()
///     .with_max_pixels(100_000_000)   // 100 megapixels
///     .with_max_width(16384)
///     .with_max_height(16384)
///     .with_max_frames(10000);
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct Limits {
    /// Maximum total pixel count (width × height).
    pub max_pixels: Option<u64>,
    /// Maximum estimated memory in bytes for a single buffer allocation.
    pub max_memory_bytes: Option<u64>,
    /// Maximum image width in pixels.
    pub max_width: Option<u32>,
    /// Maximum image height in pixels.
    pub max_height: Option<u32>,
    /// Maximum animation frame count.
    pub max_frames: Option<u32>,
    /// Maximum job duration.
    ///
    /// Use [`to_deadline()`](Limits::to_deadline) to create a [`Deadline`]
    /// that implements [`enough::Stop`].
    #[cfg(feature = "std")]
    pub max_duration: Option<core::time::Duration>,
}

impl Limits {
    /// No limits — everything is allowed.
    pub const NONE: Self = Self {
        max_pixels: None,
        max_memory_bytes: None,
        max_width: None,
        max_height: None,
        max_frames: None,
        #[cfg(feature = "std")]
        max_duration: None,
    };

    /// Server-safe defaults: 100 megapixels, 16384 max dimension, 4 GB memory,
    /// 10 000 animation frames.
    pub const SERVER: Self = Self {
        max_pixels: Some(100_000_000),
        max_memory_bytes: Some(4 * 1024 * 1024 * 1024),
        max_width: Some(16384),
        max_height: Some(16384),
        max_frames: Some(10_000),
        #[cfg(feature = "std")]
        max_duration: None,
    };

    pub fn with_max_pixels(mut self, max: u64) -> Self {
        self.max_pixels = Some(max);
        self
    }

    pub fn with_max_memory_bytes(mut self, max: u64) -> Self {
        self.max_memory_bytes = Some(max);
        self
    }

    pub fn with_max_width(mut self, max: u32) -> Self {
        self.max_width = Some(max);
        self
    }

    pub fn with_max_height(mut self, max: u32) -> Self {
        self.max_height = Some(max);
        self
    }

    /// Set maximum animation frame count.
    pub fn with_max_frames(mut self, max: u32) -> Self {
        self.max_frames = Some(max);
        self
    }

    /// Set maximum job duration (requires `std`).
    #[cfg(feature = "std")]
    pub fn with_max_duration(mut self, max: core::time::Duration) -> Self {
        self.max_duration = Some(max);
        self
    }

    /// Check whether an image of the given dimensions and format is within limits.
    ///
    /// Returns `Ok(())` if within limits, `Err(at!(PipeError::LimitExceeded))` otherwise.
    pub fn check(&self, width: u32, height: u32, format: PixelFormat) -> crate::PipeResult<()> {
        if let Some(max_w) = self.max_width {
            if width > max_w {
                return Err(at!(PipeError::LimitExceeded(alloc::format!(
                    "width {width} exceeds max {max_w}"
                ))));
            }
        }
        if let Some(max_h) = self.max_height {
            if height > max_h {
                return Err(at!(PipeError::LimitExceeded(alloc::format!(
                    "height {height} exceeds max {max_h}"
                ))));
            }
        }

        let pixels = width as u64 * height as u64;
        if let Some(max_px) = self.max_pixels {
            if pixels > max_px {
                return Err(at!(PipeError::LimitExceeded(alloc::format!(
                    "pixel count {pixels} exceeds max {max_px}"
                ))));
            }
        }

        if let Some(max_mem) = self.max_memory_bytes {
            let mem = pixels * format.bytes_per_pixel() as u64;
            if mem > max_mem {
                return Err(at!(PipeError::LimitExceeded(alloc::format!(
                    "estimated memory {mem} bytes exceeds max {max_mem}"
                ))));
            }
        }

        Ok(())
    }

    /// Check whether a frame count is within limits.
    ///
    /// Returns `Ok(())` if within limits or no frame limit is set.
    pub fn check_frames(&self, count: u32) -> crate::PipeResult<()> {
        if let Some(max) = self.max_frames {
            if count > max {
                return Err(at!(PipeError::LimitExceeded(alloc::format!(
                    "frame count {count} exceeds max {max}"
                ))));
            }
        }
        Ok(())
    }

    /// Create a [`Deadline`] from `max_duration`, if set.
    ///
    /// The deadline starts from the moment this method is called.
    #[cfg(feature = "std")]
    pub fn to_deadline(&self) -> Option<Deadline> {
        self.max_duration.map(Deadline::new)
    }

    /// Create an [`AllocationTracker`] from `max_memory_bytes`, if set.
    ///
    /// Returns an unlimited tracker when no memory limit is configured.
    pub fn to_allocation_tracker(self) -> Arc<AllocationTracker> {
        Arc::new(match self.max_memory_bytes {
            Some(limit) => AllocationTracker::new(limit),
            None => AllocationTracker::unlimited(),
        })
    }
}

// =========================================================================
// AllocationTracker — runtime memory accounting
// =========================================================================

/// Thread-safe runtime memory tracker with RAII guards.
///
/// Tracks current and peak memory usage across all allocations.
/// Each [`allocate()`](Self::allocate) call returns an [`AllocationGuard`]
/// that decrements the counter on drop.
///
/// # `no_std` compatible
///
/// Uses only `core::sync::atomic` — works in `no_std + alloc` environments.
///
/// # Example
///
/// ```
/// use std::sync::Arc;
/// use zenpipe::limits::AllocationTracker;
///
/// let tracker = Arc::new(AllocationTracker::new(1024 * 1024)); // 1 MB limit
///
/// // Allocate 512 KB — succeeds
/// let guard = tracker.allocate(512 * 1024).unwrap();
/// assert_eq!(tracker.current_bytes(), 512 * 1024);
///
/// // Try another 512 KB + 1 — fails (would exceed 1 MB)
/// assert!(tracker.allocate(512 * 1024 + 1).is_err());
///
/// // Drop the first guard — frees the 512 KB
/// drop(guard);
/// assert_eq!(tracker.current_bytes(), 0);
/// ```
pub struct AllocationTracker {
    current_bytes: AtomicU64,
    peak_bytes: AtomicU64,
    allocation_count: AtomicU64,
    /// Memory limit in bytes. 0 = unlimited.
    limit_bytes: u64,
}

impl core::fmt::Debug for AllocationTracker {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AllocationTracker")
            .field("current_bytes", &self.current_bytes())
            .field("peak_bytes", &self.peak_bytes())
            .field("allocation_count", &self.allocation_count())
            .field("limit_bytes", &self.limit_bytes)
            .finish()
    }
}

impl AllocationTracker {
    /// Create a tracker with the given byte limit.
    ///
    /// Pass `0` for unlimited (equivalent to [`unlimited()`](Self::unlimited)).
    pub fn new(limit_bytes: u64) -> Self {
        Self {
            current_bytes: AtomicU64::new(0),
            peak_bytes: AtomicU64::new(0),
            allocation_count: AtomicU64::new(0),
            limit_bytes,
        }
    }

    /// Create a tracker with no memory limit.
    pub fn unlimited() -> Self {
        Self::new(0)
    }

    /// Record an allocation of `bytes` and return an RAII guard.
    ///
    /// Returns `Err(at!(PipeError::LimitExceeded))` if the allocation would
    /// push `current_bytes` past the configured limit.
    ///
    /// The returned [`AllocationGuard`] decrements `current_bytes` on drop.
    pub fn allocate(self: &Arc<Self>, bytes: u64) -> crate::PipeResult<AllocationGuard> {
        self.check(bytes)?;

        let new_total = self.current_bytes.fetch_add(bytes, Ordering::SeqCst) + bytes;
        self.peak_bytes.fetch_max(new_total, Ordering::SeqCst);
        self.allocation_count.fetch_add(1, Ordering::SeqCst);

        Ok(AllocationGuard {
            tracker: Arc::clone(self),
            bytes,
        })
    }

    /// Try to record an allocation, returning `None` on limit violation
    /// instead of an error.
    pub fn try_allocate(self: &Arc<Self>, bytes: u64) -> Option<AllocationGuard> {
        self.allocate(bytes).ok()
    }

    /// Check whether `requested` bytes can be allocated without exceeding
    /// the limit. Does **not** actually allocate.
    pub fn check(&self, requested: u64) -> crate::PipeResult<()> {
        if self.limit_bytes > 0 {
            let current = self.current_bytes.load(Ordering::SeqCst);
            if current + requested > self.limit_bytes {
                return Err(at!(PipeError::LimitExceeded(alloc::format!(
                    "memory: {} + {} = {} bytes exceeds limit {}",
                    current,
                    requested,
                    current + requested,
                    self.limit_bytes,
                ))));
            }
        }
        Ok(())
    }

    /// Current allocated bytes.
    pub fn current_bytes(&self) -> u64 {
        self.current_bytes.load(Ordering::SeqCst)
    }

    /// Peak allocated bytes (high-water mark).
    pub fn peak_bytes(&self) -> u64 {
        self.peak_bytes.load(Ordering::SeqCst)
    }

    /// Total number of allocations made (including released ones).
    pub fn allocation_count(&self) -> u64 {
        self.allocation_count.load(Ordering::SeqCst)
    }

    /// Configured memory limit (0 = unlimited).
    pub fn limit_bytes(&self) -> u64 {
        self.limit_bytes
    }

    /// Remaining capacity before hitting the limit, or `None` if unlimited.
    pub fn remaining(&self) -> Option<u64> {
        if self.limit_bytes == 0 {
            None
        } else {
            Some(
                self.limit_bytes
                    .saturating_sub(self.current_bytes.load(Ordering::SeqCst)),
            )
        }
    }
}

impl Default for AllocationTracker {
    fn default() -> Self {
        Self::unlimited()
    }
}

// =========================================================================
// AllocationGuard — RAII decrement on drop
// =========================================================================

/// RAII guard that decrements an [`AllocationTracker`] on drop.
///
/// Returned by [`AllocationTracker::allocate()`]. Hold this for the
/// lifetime of the associated buffer.
pub struct AllocationGuard {
    tracker: Arc<AllocationTracker>,
    bytes: u64,
}

impl AllocationGuard {
    /// Size of this allocation in bytes.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl Drop for AllocationGuard {
    fn drop(&mut self) {
        self.tracker
            .current_bytes
            .fetch_sub(self.bytes, Ordering::SeqCst);
    }
}

impl core::fmt::Debug for AllocationGuard {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AllocationGuard")
            .field("bytes", &self.bytes)
            .finish()
    }
}

// =========================================================================
// Deadline — Stop implementation with a wall-clock timeout
// =========================================================================

/// A [`Stop`](enough::Stop) implementation that expires after a fixed duration.
///
/// Created from [`Limits::to_deadline()`] or directly via [`Deadline::new()`].
/// Composes with [`execute_with_stop`](crate::execute_with_stop) — pass as
/// the `stop` parameter to enforce a wall-clock timeout on pipeline execution.
///
/// # Example
///
/// ```
/// use std::time::Duration;
/// use enough::Stop;
/// use zenpipe::limits::Deadline;
///
/// let deadline = Deadline::new(Duration::from_secs(30));
/// assert!(deadline.check().is_ok()); // fresh — hasn't expired yet
/// ```
///
/// # Requires `std`
///
/// Uses `std::time::Instant` for monotonic time.
#[cfg(feature = "std")]
pub struct Deadline {
    start: std::time::Instant,
    max_duration: core::time::Duration,
}

#[cfg(feature = "std")]
impl Deadline {
    /// Create a deadline that expires `max_duration` from now.
    pub fn new(max_duration: core::time::Duration) -> Self {
        Self {
            start: std::time::Instant::now(),
            max_duration,
        }
    }

    /// Elapsed time since the deadline was created.
    pub fn elapsed(&self) -> core::time::Duration {
        self.start.elapsed()
    }

    /// Remaining time before the deadline expires, or zero if already expired.
    pub fn remaining(&self) -> core::time::Duration {
        self.max_duration.saturating_sub(self.start.elapsed())
    }

    /// Whether the deadline has expired.
    pub fn is_expired(&self) -> bool {
        self.start.elapsed() >= self.max_duration
    }
}

#[cfg(feature = "std")]
impl core::fmt::Debug for Deadline {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Deadline")
            .field("elapsed", &self.elapsed())
            .field("max_duration", &self.max_duration)
            .field("expired", &self.is_expired())
            .finish()
    }
}

#[cfg(feature = "std")]
impl enough::Stop for Deadline {
    fn check(&self) -> Result<(), enough::StopReason> {
        if self.is_expired() {
            Err(enough::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}
