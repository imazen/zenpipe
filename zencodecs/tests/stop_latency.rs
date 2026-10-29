//! Stop latency benchmarks with large images and timed cancellation.
//!
//! These tests measure how quickly each codec responds to a stop token fired
//! mid-operation. This reveals codecs with infrequent check() calls or large
//! non-cancellable critical sections.
//!
//! All tests are `#[ignore]` by default — run with:
//!   cargo test --features all,std --test stop_latency -- --ignored --nocapture

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use common::{encode_rgba_test_data, encode_test_data, rgb8_image, rgba8_image};
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat, SharedPulse};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Timed cancellation stop — cancelled from another thread after a delay.
struct DelayedStop {
    cancelled: Arc<AtomicBool>,
    cancel_time: Arc<std::sync::Mutex<Option<Instant>>>,
}

impl DelayedStop {
    fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            cancel_time: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Spawn a thread that cancels after `delay`.
    fn cancel_after(&self, delay: Duration) -> JoinHandle<()> {
        let flag = self.cancelled.clone();
        let time = self.cancel_time.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            let now = Instant::now();
            *time.lock().unwrap() = Some(now);
            flag.store(true, Ordering::Release);
        })
    }

    /// Get the exact instant the flag was set.
    #[allow(dead_code)] // diagnostic helper, kept for latency-debugging tests
    fn cancel_instant(&self) -> Option<Instant> {
        *self.cancel_time.lock().unwrap()
    }
}

impl enough::Stop for DelayedStop {
    fn check(&self) -> Result<(), enough::StopReason> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(enough::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// How an operation's error reports a stop: the reason of a codec error whose
/// category is `Stopped` (reached through `find_cause`, as `pulse_retention.rs`
/// does), or zencodecs' own `Cancelled`, which carries no reason. `None` for any
/// other error: that is a failure, not a stop, and has no stop latency.
fn stop_kind(error: &whereat::At<zencodecs::CodecError>) -> Option<String> {
    match error.error() {
        zencodecs::CodecError::Cancelled => Some("Cancelled".into()),
        zencodecs::CodecError::Codec { source, .. } => {
            zencodec::find_cause::<zencodec::CodecError>(&**source).and_then(|e| {
                match e.category() {
                    zencodec::ErrorCategory::Stopped(reason) => {
                        Some(format!("Stopped({reason:?})"))
                    }
                    _ => None,
                }
            })
        }
        _ => None,
    }
}

/// Measure stop latency for an operation.
///
/// Timestamps the exact moment the stop flag is set and the exact moment the
/// operation returns. The difference is the true stop latency — how long the
/// codec took to notice and bail out after the flag was raised. Only an error
/// that reports the stop counts; any other error is printed as a failure.
///
/// Prints results to stderr. Does not assert — this is observational.
fn measure_stop_latency(
    name: &str,
    cancel_delay: Duration,
    operation: impl FnOnce(SharedPulse) -> Result<(), whereat::At<zencodecs::CodecError>>,
) {
    let stop = DelayedStop::new();
    let cancel_time = stop.cancel_time.clone();
    let handle = stop.cancel_after(cancel_delay);
    let token = SharedPulse::new(zencodec::how_far::StopOnly::new(stop));
    let start = Instant::now();
    let result = operation(token);
    let return_time = Instant::now();
    handle.join().unwrap();

    let total = return_time.duration_since(start);
    let error = match result {
        Ok(()) => {
            eprintln!("{name}: COMPLETED BEFORE CANCEL total={total:?}");
            return;
        }
        Err(error) => error,
    };
    let Some(kind) = stop_kind(&error) else {
        eprintln!("{name}: FAILED (not a stop) total={total:?}: {error}");
        return;
    };
    if let Some(cancel_instant) = *cancel_time.lock().unwrap() {
        let latency = return_time.duration_since(cancel_instant);
        eprintln!(
            "{name}: CANCELLED ({kind}) total={total:?}, stop_latency={latency:?} (flag_set→return)"
        );
        if latency > Duration::from_millis(200) {
            eprintln!("  WARNING: stop latency > 200ms");
        }
    } else {
        // Operation failed before cancel thread even ran
        eprintln!("{name}: CANCELLED BEFORE FLAG ({kind}) total={total:?}");
    }
}

#[test]
fn stop_kind_tells_a_stop_from_another_error() {
    use enough::StopReason;
    use zencodec::ErrorCategory;
    let traced = |e: zencodecs::CodecError| whereat::At::from(e);
    let codec_stop = |reason| {
        zencodecs::CodecError::from_codec(
            ImageFormat::Jpeg,
            zencodec::CodecError::new(Some("probe"), ErrorCategory::Stopped(reason)),
        )
    };
    assert_eq!(
        stop_kind(&traced(codec_stop(StopReason::TimedOut))).as_deref(),
        Some("Stopped(TimedOut)")
    );
    assert_eq!(
        stop_kind(&traced(codec_stop(StopReason::Cancelled))).as_deref(),
        Some("Stopped(Cancelled)")
    );
    assert_eq!(
        stop_kind(&traced(zencodecs::CodecError::Cancelled)).as_deref(),
        Some("Cancelled")
    );
    // Not stops: each of these used to be reported as a cancellation with a
    // stop latency.
    assert_eq!(
        stop_kind(&traced(zencodecs::CodecError::UnsupportedFormat(
            ImageFormat::Avif
        ))),
        None
    );
    let limit = zencodec::CodecError::new(
        Some("probe"),
        ErrorCategory::Resource(zencodec::ResourceError::Limits(zencodec::LimitKind::Pixels)),
    );
    assert_eq!(
        stop_kind(&traced(zencodecs::CodecError::from_codec(
            ImageFormat::Png,
            limit
        ))),
        None
    );
    let native = std::io::Error::other("a codec's own error, without the shared envelope");
    assert_eq!(
        stop_kind(&traced(zencodecs::CodecError::from_codec(
            ImageFormat::Gif,
            native
        ))),
        None
    );
}

const CANCEL_DELAY: Duration = Duration::from_millis(20);

// ===========================================================================
// Decode latency tests
// ===========================================================================

#[test]
#[ignore]
fn stop_latency_jpeg_decode() {
    let data = encode_test_data(ImageFormat::Jpeg, 4096, 4096);
    measure_stop_latency("jpeg_decode", CANCEL_DELAY, |token| {
        DecodeRequest::new(&data)
            .with_pulse(token)
            .decode_full_frame()?;
        Ok(())
    });
}

#[test]
#[ignore]
fn stop_latency_webp_decode() {
    let data = encode_test_data(ImageFormat::WebP, 4096, 4096);
    measure_stop_latency("webp_decode", CANCEL_DELAY, |token| {
        DecodeRequest::new(&data)
            .with_pulse(token)
            .decode_full_frame()?;
        Ok(())
    });
}

#[test]
#[ignore]
fn stop_latency_png_decode() {
    let data = encode_test_data(ImageFormat::Png, 4096, 4096);
    measure_stop_latency("png_decode", CANCEL_DELAY, |token| {
        DecodeRequest::new(&data)
            .with_pulse(token)
            .decode_full_frame()?;
        Ok(())
    });
}

#[test]
#[ignore]
fn stop_latency_gif_decode() {
    let data = encode_rgba_test_data(ImageFormat::Gif, 2048, 2048);
    measure_stop_latency("gif_decode", CANCEL_DELAY, |token| {
        DecodeRequest::new(&data)
            .with_pulse(token)
            .decode_full_frame()?;
        Ok(())
    });
}

#[test]
#[ignore]
fn stop_latency_avif_decode() {
    let data = encode_test_data(ImageFormat::Avif, 1024, 1024);
    measure_stop_latency("avif_decode", CANCEL_DELAY, |token| {
        DecodeRequest::new(&data)
            .with_pulse(token)
            .decode_full_frame()?;
        Ok(())
    });
}

// ===========================================================================
// Encode latency tests
// ===========================================================================

#[test]
#[ignore]
fn stop_latency_jpeg_encode() {
    let img = rgb8_image(4096, 4096);
    measure_stop_latency("jpeg_encode", CANCEL_DELAY, |token| {
        EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality(50.0)
            .with_pulse(token)
            .encode_full_frame_rgb8(img.as_ref())?;
        Ok(())
    });
}

#[test]
#[ignore]
fn stop_latency_webp_encode() {
    let img = rgb8_image(4096, 4096);
    measure_stop_latency("webp_encode", CANCEL_DELAY, |token| {
        EncodeRequest::new(ImageFormat::WebP)
            .with_quality(50.0)
            .with_pulse(token)
            .encode_full_frame_rgb8(img.as_ref())?;
        Ok(())
    });
}

#[test]
#[ignore]
fn stop_latency_png_encode() {
    let img = rgb8_image(4096, 4096);
    measure_stop_latency("png_encode", CANCEL_DELAY, |token| {
        EncodeRequest::new(ImageFormat::Png)
            .with_pulse(token)
            .encode_full_frame_rgb8(img.as_ref())?;
        Ok(())
    });
}

#[test]
#[ignore]
fn stop_latency_gif_encode() {
    let img = rgba8_image(2048, 2048);
    measure_stop_latency("gif_encode", CANCEL_DELAY, |token| {
        EncodeRequest::new(ImageFormat::Gif)
            .with_pulse(token)
            .encode_full_frame_rgba8(img.as_ref())?;
        Ok(())
    });
}

#[test]
#[ignore]
fn stop_latency_avif_encode() {
    let img = rgb8_image(1024, 1024);
    measure_stop_latency("avif_encode", CANCEL_DELAY, |token| {
        EncodeRequest::new(ImageFormat::Avif)
            .with_quality(50.0)
            .with_pulse(token)
            .encode_full_frame_rgb8(img.as_ref())?;
        Ok(())
    });
}
