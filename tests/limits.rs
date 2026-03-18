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
    assert!(matches!(result.unwrap_err(), PipeError::LimitExceeded(_)));
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
    assert!(matches!(result, Err(PipeError::LimitExceeded(_))));
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
    assert!(matches!(result, Err(PipeError::LimitExceeded(_))));
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

    assert!(matches!(result, Err(PipeError::Cancelled)));
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
    fn consume(&mut self, strip: &Strip<'_>) -> Result<(), PipeError> {
        self.bytes += strip.as_strided_bytes().len();
        Ok(())
    }
    fn finish(&mut self) -> Result<(), PipeError> {
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
