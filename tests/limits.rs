//! Tests for resource limits and cooperative cancellation.

use std::sync::atomic::{AtomicU32, Ordering};

use zenpipe::format;
use zenpipe::sources::{CallbackSource, MaterializedSource, TeeSource};
use zenpipe::{Limits, PipeError, Source, Stop, Strip};

/// Create a solid RGBA8 source.
fn solid_source(width: u32, height: u32) -> Box<dyn Source> {
    let row_bytes = width as usize * 4;
    let mut rows = 0u32;
    Box::new(CallbackSource::new(
        width,
        height,
        format::RGBA8_SRGB,
        16,
        move |buf| {
            if rows >= height {
                return Ok(false);
            }
            buf[..row_bytes].fill(128);
            rows += 1;
            Ok(true)
        },
    ))
}

// =========================================================================
// Limits::check tests
// =========================================================================

#[test]
fn limits_check_ok() {
    let limits = Limits::default()
        .with_max_pixels(1000)
        .with_max_width(100)
        .with_max_height(100);
    assert!(limits.check(10, 10, format::RGBA8_SRGB).is_ok());
}

#[test]
fn limits_check_pixels_exceeded() {
    let limits = Limits::default().with_max_pixels(100);
    let result = limits.check(20, 20, format::RGBA8_SRGB);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err().into_inner(),
        PipeError::LimitExceeded(_)
    ));
}

#[test]
fn limits_check_width_exceeded() {
    let limits = Limits::default().with_max_width(10);
    assert!(limits.check(11, 1, format::RGBA8_SRGB).is_err());
    assert!(limits.check(10, 1, format::RGBA8_SRGB).is_ok());
}

#[test]
fn limits_check_height_exceeded() {
    let limits = Limits::default().with_max_height(10);
    assert!(limits.check(1, 11, format::RGBA8_SRGB).is_err());
    assert!(limits.check(1, 10, format::RGBA8_SRGB).is_ok());
}

#[test]
fn limits_check_memory_exceeded() {
    let limits = Limits::default().with_max_memory_bytes(100);
    // 10×10 RGBA8 = 400 bytes > 100
    assert!(limits.check(10, 10, format::RGBA8_SRGB).is_err());
    // 5×5 RGBA8 = 100 bytes = 100
    assert!(limits.check(5, 5, format::RGBA8_SRGB).is_ok());
}

#[test]
fn limits_none_allows_everything() {
    let limits = Limits::NONE;
    assert!(limits.check(100000, 100000, format::RGBA8_SRGB).is_ok());
}

// =========================================================================
// MaterializedSource with limits
// =========================================================================

#[test]
fn materialize_checked_within_limits() {
    let limits = Limits::default().with_max_pixels(1000);
    let src = solid_source(4, 4); // 16 pixels
    let mat = MaterializedSource::from_source_checked(src, &limits);
    assert!(mat.is_ok());
}

#[test]
fn materialize_checked_exceeds_limits() {
    let limits = Limits::default().with_max_pixels(10);
    let src = solid_source(4, 4); // 16 pixels > 10
    let result = MaterializedSource::from_source_checked(src, &limits);
    assert!(result.is_err_and(|e| matches!(e.error(), PipeError::LimitExceeded(_))));
}

// =========================================================================
// TeeSource with limits
// =========================================================================

#[test]
fn tee_checked_within_limits() {
    let limits = Limits::default().with_max_pixels(1000);
    let src = solid_source(4, 4);
    let tee = TeeSource::new_checked(src, &limits);
    assert!(tee.is_ok());
}

#[test]
fn tee_checked_exceeds_limits() {
    let limits = Limits::default().with_max_pixels(10);
    let src = solid_source(4, 4);
    let result = TeeSource::new_checked(src, &limits);
    assert!(result.is_err_and(|e| matches!(e.error(), PipeError::LimitExceeded(_))));
}

// =========================================================================
// Cancellation tests
// =========================================================================

/// A Stop implementation that fires after N checks.
struct CountdownStop {
    remaining: AtomicU32,
}

impl CountdownStop {
    fn new(n: u32) -> Self {
        Self {
            remaining: AtomicU32::new(n),
        }
    }
}

impl Stop for CountdownStop {
    fn check(&self) -> Result<(), enough::StopReason> {
        let prev = self.remaining.fetch_sub(1, Ordering::Relaxed);
        if prev == 0 {
            // Already fired — keep returning stopped.
            Err(enough::StopReason::Cancelled)
        } else if prev == 1 {
            // This is the Nth check — fire.
            Err(enough::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[test]
fn execute_with_stop_cancels() {
    let mut src = solid_source(4, 100); // 100 rows → many strips
    let stop = CountdownStop::new(2); // Cancel after 2 strip checks

    let mut sink = CollectSink::new();
    let result = zenpipe::execute_with_stop(&mut *src, &mut sink, &stop);

    assert!(result.is_err_and(|e| matches!(e.error(), PipeError::Cancelled)));
    // Should have processed some but not all data.
    assert!(sink.bytes < 4 * 100 * 4);
}

#[test]
fn execute_with_unstoppable_completes() {
    let mut src = solid_source(4, 4);
    let mut sink = CollectSink::new();
    let result = zenpipe::execute_with_stop(&mut *src, &mut sink, &zenpipe::Unstoppable);
    assert!(result.is_ok());
    assert_eq!(sink.bytes, 4 * 4 * 4);
}

struct CollectSink {
    bytes: usize,
}

impl CollectSink {
    fn new() -> Self {
        Self { bytes: 0 }
    }
}

impl zenpipe::Sink for CollectSink {
    fn consume(&mut self, strip: &Strip<'_>) -> zenpipe::PipeResult<()> {
        self.bytes += strip.as_strided_bytes().len();
        Ok(())
    }
    fn finish(&mut self) -> zenpipe::PipeResult<()> {
        Ok(())
    }
}

// =========================================================================
// Server preset
// =========================================================================

#[test]
fn server_limits_reject_huge_image() {
    let limits = Limits::SERVER;
    // 1000×1000 is well within all limits
    assert!(limits.check(1000, 1000, format::RGBA8_SRGB).is_ok());
    // 16385 width exceeds max_width
    assert!(limits.check(16385, 1, format::RGBA8_SRGB).is_err());
    // 16384×16384 RGBA8 = 1 TB memory, exceeds 4 GB max_memory
    assert!(limits.check(16384, 16384, format::RGBA8_SRGB).is_err());
}

#[test]
fn server_limits_have_frame_limit() {
    let limits = Limits::SERVER;
    assert_eq!(limits.max_frames, Some(10_000));
}

// =========================================================================
// Frame count checks
// =========================================================================

#[test]
fn check_frames_within_limit() {
    let limits = Limits::default().with_max_frames(100);
    assert!(limits.check_frames(50).is_ok());
    assert!(limits.check_frames(100).is_ok());
}

#[test]
fn check_frames_exceeds_limit() {
    let limits = Limits::default().with_max_frames(100);
    let err = limits.check_frames(101);
    assert!(err.is_err_and(|e| matches!(e.error(), PipeError::LimitExceeded(_))));
}

#[test]
fn check_frames_no_limit() {
    let limits = Limits::NONE;
    assert!(limits.check_frames(u32::MAX).is_ok());
}

// =========================================================================
// AllocationTracker
// =========================================================================

use std::sync::Arc;
use zenpipe::AllocationTracker;

#[test]
fn tracker_allocate_and_track() {
    let tracker = Arc::new(AllocationTracker::new(1024));

    let g1 = tracker.allocate(400).unwrap();
    assert_eq!(tracker.current_bytes(), 400);
    assert_eq!(tracker.peak_bytes(), 400);
    assert_eq!(tracker.allocation_count(), 1);

    let g2 = tracker.allocate(300).unwrap();
    assert_eq!(tracker.current_bytes(), 700);
    assert_eq!(tracker.peak_bytes(), 700);
    assert_eq!(tracker.allocation_count(), 2);

    drop(g1);
    assert_eq!(tracker.current_bytes(), 300);
    // Peak should remain at 700
    assert_eq!(tracker.peak_bytes(), 700);

    drop(g2);
    assert_eq!(tracker.current_bytes(), 0);
    assert_eq!(tracker.peak_bytes(), 700);
}

#[test]
fn tracker_exceeds_limit() {
    let tracker = Arc::new(AllocationTracker::new(1000));
    let _g = tracker.allocate(600).unwrap();

    // 600 + 500 = 1100 > 1000
    let err = tracker.allocate(500);
    assert!(err.is_err());
    assert!(matches!(
        err.unwrap_err().into_inner(),
        PipeError::LimitExceeded(_)
    ));

    // Current should be unchanged (allocation was rejected)
    assert_eq!(tracker.current_bytes(), 600);
}

#[test]
fn tracker_guard_drops_decrement() {
    let tracker = Arc::new(AllocationTracker::new(0)); // unlimited
    {
        let _g1 = tracker.allocate(100).unwrap();
        let _g2 = tracker.allocate(200).unwrap();
        assert_eq!(tracker.current_bytes(), 300);
    }
    // Both guards dropped
    assert_eq!(tracker.current_bytes(), 0);
    assert_eq!(tracker.peak_bytes(), 300);
}

#[test]
fn tracker_unlimited_allows_large() {
    let tracker = Arc::new(AllocationTracker::unlimited());
    let _g = tracker.allocate(u64::MAX / 2).unwrap();
    assert_eq!(tracker.current_bytes(), u64::MAX / 2);
}

#[test]
fn tracker_try_allocate_returns_none() {
    let tracker = Arc::new(AllocationTracker::new(100));
    let _g = tracker.allocate(80).unwrap();

    assert!(tracker.try_allocate(30).is_none());
    assert!(tracker.try_allocate(20).is_some());
}

#[test]
fn tracker_check_without_allocating() {
    let tracker = Arc::new(AllocationTracker::new(1000));
    let _g = tracker.allocate(800).unwrap();

    // check does not actually allocate
    assert!(tracker.check(100).is_ok());
    assert!(tracker.check(300).is_err());
    assert_eq!(tracker.current_bytes(), 800); // unchanged
}

#[test]
fn tracker_remaining_capacity() {
    let tracker = Arc::new(AllocationTracker::new(1000));
    assert_eq!(tracker.remaining(), Some(1000));

    let _g = tracker.allocate(400).unwrap();
    assert_eq!(tracker.remaining(), Some(600));

    let unlimited = Arc::new(AllocationTracker::unlimited());
    assert_eq!(unlimited.remaining(), None);
}

#[test]
fn tracker_from_limits() {
    let limits = Limits::default().with_max_memory_bytes(2048);
    let tracker = limits.to_allocation_tracker();
    assert_eq!(tracker.limit_bytes(), 2048);

    let no_limit = Limits::NONE.to_allocation_tracker();
    assert_eq!(no_limit.limit_bytes(), 0);
}

// =========================================================================
// Deadline
// =========================================================================

use std::time::Duration;
use zenpipe::Deadline;

#[test]
fn deadline_fresh_passes_check() {
    let deadline = Deadline::new(Duration::from_secs(60));
    assert!(deadline.check().is_ok());
    assert!(!deadline.is_expired());
    assert!(deadline.remaining() > Duration::ZERO);
}

#[test]
fn deadline_expired_fails_check() {
    let deadline = Deadline::new(Duration::from_nanos(1));
    // Burn through the nanosecond
    std::thread::sleep(Duration::from_millis(1));
    assert!(deadline.is_expired());
    assert!(deadline.check().is_err());
    assert_eq!(deadline.remaining(), Duration::ZERO);
}

#[test]
fn deadline_from_limits() {
    let limits = Limits::default().with_max_duration(Duration::from_secs(5));
    let deadline = limits.to_deadline();
    assert!(deadline.is_some());
    let d = deadline.unwrap();
    assert!(d.check().is_ok());

    let no_deadline = Limits::NONE.to_deadline();
    assert!(no_deadline.is_none());
}

#[test]
fn deadline_compose_with_execute() {
    // A deadline that has already expired should cancel execute_with_stop
    let deadline = Deadline::new(Duration::from_nanos(1));
    std::thread::sleep(Duration::from_millis(1));

    let mut src = solid_source(4, 100);
    let mut sink = CollectSink::new();
    let result = zenpipe::execute_with_stop(&mut *src, &mut sink, &deadline);
    assert!(result.is_err_and(|e| matches!(e.error(), PipeError::Cancelled)));
}
