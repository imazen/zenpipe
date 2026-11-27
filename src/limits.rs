//! Resource limits for pipeline operations.
//!
//! [`Limits`] prevents memory-bomb attacks by rejecting images that exceed
//! configured dimension, pixel count, memory, frame, output-size, or duration
//! thresholds before allocating or during execution.

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
/// let limits = Limits::SERVER
///     .with_max_pixels(120_000_000)   // 120 megapixels
///     .with_max_width(16384)
///     .with_max_height(16384)
///     .with_max_frames(10000);
/// ```
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
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
    /// Maximum total pixel count across all animation frames
    /// (width × height × frame_count). Bounds the total decode work
    /// for animated images independently of per-frame dimensions.
    pub max_total_pixels: Option<u64>,
    /// Maximum encoded output size in bytes. Enforced by the codec layer
    /// during encoding (zencodec `ResourceLimits::max_output_bytes`).
    pub max_output_bytes: Option<u64>,
    /// Maximum job duration (enforced automatically by `ImageJob` via
    /// cooperative cancellation returning `PipeError::Stopped(StopReason::TimedOut)`).
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
        max_total_pixels: None,
        max_output_bytes: None,
        #[cfg(feature = "std")]
        max_duration: None,
    };

    /// Server-safe defaults: 120 megapixels, 16384 max dimension, 4 GB memory,
    /// 10 000 animation frames, 1 billion total pixels across all frames.
    ///
    /// 120 MP admits 108 MP phone-camera photos (a common real-world size)
    /// while still rejecting decode-bomb dimensions.
    pub const SERVER: Self = Self {
        max_pixels: Some(120_000_000),
        max_memory_bytes: Some(4 * 1024 * 1024 * 1024),
        max_width: Some(16384),
        max_height: Some(16384),
        max_frames: Some(10_000),
        max_total_pixels: Some(1_000_000_000),
        // No output-byte cap by default — hosts opt in per deployment.
        max_output_bytes: None,
        #[cfg(feature = "std")]
        max_duration: None,
    };

    pub fn with_max_pixels(mut self, max: u64) -> Self {
        self.max_pixels = Some(max);
        self
    }

    pub fn with_max_output_bytes(mut self, max: u64) -> Self {
        self.max_output_bytes = Some(max);
        self
    }

    /// Convert to the codec layer's [`zencodec::ResourceLimits`] so decoders
    /// and encoders enforce the same budget the pipeline does. Fields with
    /// no codec-side equivalent (`max_memory_bytes` is per-allocation here,
    /// a whole-operation budget there) map to the closest semantics.
    pub fn to_codec_limits(&self) -> zencodec::ResourceLimits {
        let mut rl = zencodec::ResourceLimits::none();
        if let Some(v) = self.max_pixels {
            rl = rl.with_max_pixels(v);
        }
        if let Some(v) = self.max_memory_bytes {
            rl = rl.with_max_memory(v);
        }
        if let Some(v) = self.max_width {
            rl = rl.with_max_width(v);
        }
        if let Some(v) = self.max_height {
            rl = rl.with_max_height(v);
        }
        if let Some(v) = self.max_frames {
            rl = rl.with_max_frames(v);
        }
        if let Some(v) = self.max_total_pixels {
            rl = rl.with_max_total_pixels(v);
        }
        if let Some(v) = self.max_output_bytes {
            rl = rl.with_max_output(v);
        }
        rl
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

    /// Set maximum total pixel count across all animation frames.
    pub fn with_max_total_pixels(mut self, max: u64) -> Self {
        self.max_total_pixels = Some(max);
        self
    }

    /// Set maximum job duration (requires `std`).
    #[cfg(feature = "std")]
    pub fn with_max_duration(mut self, max: core::time::Duration) -> Self {
        self.max_duration = Some(max);
        self
    }

    /// Check dimensions and pixel count only (no memory estimate).
    ///
    /// Use before decoding when the pixel format is not yet known.
    pub fn check_dimensions(&self, width: u32, height: u32) -> crate::PipeResult<()> {
        if let Some(max_w) = self.max_width
            && width > max_w
        {
            return Err(at!(PipeError::LimitExceeded(alloc::format!(
                "width {width} exceeds max {max_w}"
            ))));
        }
        if let Some(max_h) = self.max_height
            && height > max_h
        {
            return Err(at!(PipeError::LimitExceeded(alloc::format!(
                "height {height} exceeds max {max_h}"
            ))));
        }
        let pixels = width as u64 * height as u64;
        if let Some(max_px) = self.max_pixels
            && pixels > max_px
        {
            return Err(at!(PipeError::LimitExceeded(alloc::format!(
                "pixel count {pixels} exceeds max {max_px}"
            ))));
        }
        Ok(())
    }

    /// Check whether an image of the given dimensions and format is within limits.
    ///
    /// Returns `Ok(())` if within limits, `Err(at!(PipeError::LimitExceeded))` otherwise.
    pub fn check(&self, width: u32, height: u32, format: PixelFormat) -> crate::PipeResult<()> {
        if let Some(max_w) = self.max_width
            && width > max_w
        {
            return Err(at!(PipeError::LimitExceeded(alloc::format!(
                "width {width} exceeds max {max_w}"
            ))));
        }
        if let Some(max_h) = self.max_height
            && height > max_h
        {
            return Err(at!(PipeError::LimitExceeded(alloc::format!(
                "height {height} exceeds max {max_h}"
            ))));
        }

        let pixels = width as u64 * height as u64;
        if let Some(max_px) = self.max_pixels
            && pixels > max_px
        {
            return Err(at!(PipeError::LimitExceeded(alloc::format!(
                "pixel count {pixels} exceeds max {max_px}"
            ))));
        }

        if let Some(max_mem) = self.max_memory_bytes {
            let mem = pixels * format.layout.bytes_per_pixel() as u64;
            if mem > max_mem {
                return Err(at!(PipeError::LimitExceeded(alloc::format!(
                    "estimated memory {mem} bytes exceeds max {max_mem}"
                ))));
            }
        }

        Ok(())
    }

    /// Check whether total pixels across all frames is within limits.
    ///
    /// `total_pixels` is typically `width * height * frame_count`.
    pub fn check_total_pixels(&self, total_pixels: u64) -> crate::PipeResult<()> {
        if let Some(max) = self.max_total_pixels
            && total_pixels > max
        {
            return Err(at!(PipeError::LimitExceeded(alloc::format!(
                "total pixel count {total_pixels} exceeds max {max}"
            ))));
        }
        Ok(())
    }

    /// Check whether a frame count is within limits.
    ///
    /// Returns `Ok(())` if within limits or no frame limit is set.
    pub fn check_frames(&self, count: u32) -> crate::PipeResult<()> {
        if let Some(max) = self.max_frames
            && count > max
        {
            return Err(at!(PipeError::LimitExceeded(alloc::format!(
                "frame count {count} exceeds max {max}"
            ))));
        }
        Ok(())
    }

    /// Create a [`Deadline`] from `max_duration`, if set.
    ///
    /// The deadline starts from the moment this method is called.
    #[cfg(feature = "std")]
    #[allow(dead_code)] // Used by job/imageflow-compat features and unit tests.
    pub(crate) fn to_deadline(self) -> Option<Deadline> {
        self.max_duration.map(Deadline::new)
    }
}

/// Compute `width * height * bpp` as a `usize`, returning a
/// [`PipeError::LimitExceeded`] on overflow (32-bit, wasm32, i686 are
/// primary targets where this is reachable).
///
/// Use at every site that previously did `width as usize * height as usize
/// * bpp` followed by `vec![0u8; n]`.
pub fn checked_buffer_size(
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
) -> crate::PipeResult<usize> {
    let row = (width as usize)
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| {
            at!(PipeError::LimitExceeded(alloc::format!(
                "row size overflow: width {width} * bpp {bytes_per_pixel}"
            )))
        })?;
    row.checked_mul(height as usize).ok_or_else(|| {
        at!(PipeError::LimitExceeded(alloc::format!(
            "buffer size overflow: row {row} * height {height}"
        )))
    })
}

/// Compute `stride * height` as a `usize`, returning a
/// [`PipeError::LimitExceeded`] on overflow.
pub fn checked_stride_buffer(stride: usize, height: u32) -> crate::PipeResult<usize> {
    stride.checked_mul(height as usize).ok_or_else(|| {
        at!(PipeError::LimitExceeded(alloc::format!(
            "buffer size overflow: stride {stride} * height {height}"
        )))
    })
}

/// Compute `a + b` as `u32`, returning a [`PipeError::LimitExceeded`] on overflow.
///
/// Use for dimension addition (canvas geometry, crop offsets, etc.) where
/// silent wrap would produce a smaller dimension that bypasses limit checks.
pub fn checked_dim_add(a: u32, b: u32) -> crate::PipeResult<u32> {
    a.checked_add(b).ok_or_else(|| {
        at!(PipeError::LimitExceeded(alloc::format!(
            "dimension overflow: {a} + {b} > u32::MAX"
        )))
    })
}

// =========================================================================
// Deadline — Stop implementation with a wall-clock timeout (crate-internal)
// =========================================================================

/// A [`Stop`](enough::Stop) implementation that expires after a fixed duration.
///
/// Created from [`Limits::to_deadline()`] or directly via [`Deadline::new()`].
/// Composes with [`execute_with_pulse`](crate::execute_with_pulse) via
/// `zencodec::how_far::StopOnly`.
#[cfg(feature = "std")]
#[allow(dead_code)]
pub(crate) struct Deadline {
    start: std::time::Instant,
    max_duration: core::time::Duration,
}

#[cfg(feature = "std")]
#[allow(dead_code)]
impl Deadline {
    /// Create a deadline that expires `max_duration` from now.
    pub(crate) fn new(max_duration: core::time::Duration) -> Self {
        Self {
            start: std::time::Instant::now(),
            max_duration,
        }
    }

    /// Elapsed time since the deadline was created.
    pub(crate) fn elapsed(&self) -> core::time::Duration {
        self.start.elapsed()
    }

    /// Remaining time before the deadline expires, or zero if already expired.
    pub(crate) fn remaining(&self) -> core::time::Duration {
        self.max_duration.saturating_sub(self.start.elapsed())
    }

    /// Whether the deadline has expired.
    pub(crate) fn is_expired(&self) -> bool {
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
            Err(enough::StopReason::TimedOut)
        } else {
            Ok(())
        }
    }
}

#[cfg(all(test, feature = "std"))]
mod deadline_tests {
    use super::*;
    use core::time::Duration;
    use enough::Stop as _;

    #[test]
    fn deadline_fresh_not_expired() {
        let d = Deadline::new(Duration::from_secs(60));
        assert!(!d.is_expired());
        assert!(d.check().is_ok());
        assert!(d.remaining() > Duration::from_secs(50));
    }

    #[test]
    fn deadline_zero_expired_returns_timed_out() {
        let d = Deadline::new(Duration::ZERO);
        assert!(d.is_expired());
        assert_eq!(d.check(), Err(enough::StopReason::TimedOut));
        assert_eq!(d.remaining(), Duration::ZERO);
    }

    #[test]
    fn limits_to_deadline() {
        assert!(Limits::NONE.to_deadline().is_none());
        let d = Limits::NONE
            .with_max_duration(Duration::from_secs(10))
            .to_deadline()
            .expect("max_duration should yield Deadline");
        assert!(!d.is_expired());
    }
}
