//! Requests retain the caller's complete pulse and hand it to the codec job.
//! A full `Pulse` (not the `StopOnly` adapter) stops JPEG and PNG decode and
//! encode at every poll it receives, and each failure carries the deadline's
//! own reason through the codec's typed error, not only a "cancelled" message.
#![cfg(all(feature = "jpeg", feature = "png"))]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use zencodec::enough::{Stop, StopReason};
use zencodec::how_far::{Child, Execution, PhaseSpec, PlanError, Pulse, Report};
use zencodecs::{CodecError, DecodeRequest, EncodeRequest, ImageFormat, SharedPulse};
use zenpixels::{PixelDescriptor, PixelSlice};

/// A deadline that has passed once `polls` reaches `fail_at`, and stays passed.
struct Deadline {
    polls: Arc<AtomicUsize>,
    fail_at: usize,
}
impl Stop for Deadline {
    fn check(&self) -> Result<(), StopReason> {
        if self.polls.fetch_add(1, Ordering::Relaxed) >= self.fail_at {
            Err(StopReason::TimedOut)
        } else {
            Ok(())
        }
    }
}
impl Report for Deadline {
    fn advance(&self, _completed: u64) {}
}
impl Pulse for Deadline {
    fn split(
        &self,
        _execution: Execution,
        _parts: &[PhaseSpec<'_>],
    ) -> Result<Vec<Child<'_>>, PlanError> {
        Err(PlanError::Unsupported)
    }
}

/// The stop reason a failure carries in the codec's own typed error.
///
/// zenjpeg returns its `CodecError` inside a traced `At<CodecError>`; zenpng's
/// decoder returns the bare `CodecError`. `find_cause` reaches either envelope
/// (run 03 triage: every PNG decode failure printed `Stopped(TimedOut)` while
/// an `At`-only downcast saw nothing). The reason itself is still required.
fn stop_reason(error: &whereat::At<CodecError>) -> Option<StopReason> {
    match error.error() {
        CodecError::Codec { source, .. } => zencodec::find_cause::<zencodec::CodecError>(&**source)
            .and_then(|e| match e.category() {
                zencodec::ErrorCategory::Stopped(reason) => Some(reason),
                _ => None,
            }),
        _ => None,
    }
}

#[test]
fn transcode_sink_preserves_native_deadline_causes() {
    use zencodec::decode::DecodeRowSink;
    use zencodecs::TranscodeSink;

    for format in [ImageFormat::Jpeg, ImageFormat::Png] {
        for at_begin in [true, false] {
            let polls = Arc::new(AtomicUsize::new(0));
            let encoder = EncodeRequest::new(format)
                .with_pulse(SharedPulse::new(Deadline {
                    polls: polls.clone(),
                    fail_at: 1_000_000,
                }))
                .build_streaming_encoder(8, 8)
                .unwrap();
            let mut sink = TranscodeSink::new(encoder.encoder, encoder.supported);
            let error = if at_begin {
                polls.store(1_000_000, Ordering::Relaxed);
                sink.begin(8, 8, PixelDescriptor::RGB8_SRGB).unwrap_err()
            } else {
                sink.begin(8, 8, PixelDescriptor::RGB8_SRGB).unwrap();
                let mut rows = sink
                    .provide_next_buffer(0, 8, 8, PixelDescriptor::RGB8_SRGB)
                    .unwrap();
                for y in 0..8 {
                    rows.row_mut(y).fill(127);
                }
                drop(rows);
                polls.store(1_000_000, Ordering::Relaxed);
                sink.finish().unwrap_err()
            };
            let native = zencodec::find_cause::<zencodec::CodecError>(&*error)
                .unwrap_or_else(|| panic!("{format:?}, begin={at_begin}: lost cause: {error}"));
            assert_eq!(
                native.category(),
                zencodec::ErrorCategory::Stopped(StopReason::TimedOut),
                "{format:?}, begin={at_begin}"
            );
        }
    }
}

/// Every stop point of one operation. Failures are collected, not raised, so
/// one codec's defect does not hide another's result.
fn each_stop_point(
    what: &str,
    failures: &mut Vec<String>,
    mut run: impl FnMut(SharedPulse) -> Result<(), whereat::At<CodecError>>,
) {
    let polls = Arc::new(AtomicUsize::new(0));
    let pulse = |fail_at| {
        polls.store(0, Ordering::Relaxed);
        SharedPulse::new(Deadline {
            polls: polls.clone(),
            fail_at,
        })
    };
    if let Err(e) = run(pulse(usize::MAX)) {
        failures.push(format!("{what}: the unstopped run failed: {e}"));
        return;
    }
    let total = polls.load(Ordering::Relaxed);
    if total == 0 {
        failures.push(format!(
            "{what}: the codec never polled the request's pulse"
        ));
        return;
    }
    let before = failures.len();
    for fail_at in 0..total {
        match run(pulse(fail_at)) {
            Ok(()) => failures.push(format!(
                "{what} poll {fail_at}/{total}: completed after the deadline passed"
            )),
            Err(error) => {
                let reason = stop_reason(&error);
                if reason != Some(StopReason::TimedOut) {
                    failures.push(format!(
                        "{what} poll {fail_at}/{total}: reason {reason:?}, expected TimedOut: {error:?}"
                    ));
                }
            }
        }
    }
    eprintln!(
        "{what}: {total} stop points, {} wrong",
        failures.len() - before
    );
}

#[test]
fn jpeg_and_png_requests_hand_their_pulse_to_the_codec_job() {
    let (w, h) = (96u32, 64u32);
    let bytes: Vec<u8> = (0..w * h * 3).map(|i| (i * 7 % 251) as u8).collect();
    let pixels =
        || PixelSlice::new(&bytes, w, h, w as usize * 3, PixelDescriptor::RGB8_SRGB).unwrap();
    let mut failures = Vec::new();
    for format in [ImageFormat::Jpeg, ImageFormat::Png] {
        let encoded = EncodeRequest::new(format)
            .encode(pixels(), false)
            .unwrap()
            .into_vec();
        each_stop_point(&format!("{format:?} encode"), &mut failures, |pulse| {
            EncodeRequest::new(format)
                .with_pulse(pulse)
                .encode(pixels(), false)
                .map(drop)
        });
        each_stop_point(&format!("{format:?} decode"), &mut failures, |pulse| {
            DecodeRequest::new(&encoded)
                .with_pulse(pulse)
                .decode_full_frame()
                .map(drop)
        });
    }
    assert!(
        failures.is_empty(),
        "{} wrong stop points:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// `animation_frame_decoder` creates its codec job from the same request, so
/// it carries the same pulse: an animated GIF stops at every poll the decoder
/// makes, with the deadline's reason. (2026-10-28: the animation path built its
/// job without the request's pulse; only per-call pulses reached the codec.)
#[cfg(feature = "gif")]
#[test]
fn gif_animation_request_hands_its_pulse_to_the_codec_job() {
    let (w, h, frames) = (24u32, 16u32, 3u32);
    let stride = w as usize * 4;
    let mut encoder = EncodeRequest::new(ImageFormat::Gif)
        .animation_frame_encoder(w, h)
        .unwrap();
    for frame in 0..frames {
        let bytes: Vec<u8> = (0..w * h)
            .flat_map(|n| [(n * 7 + frame * 40) as u8, (frame * 60) as u8, 200, 255])
            .collect();
        let pixels = PixelSlice::new(&bytes, w, h, stride, PixelDescriptor::RGBA8_SRGB).unwrap();
        encoder.push_frame(pixels, 100, None).unwrap();
    }
    let encoded = encoder.finish(None).unwrap().into_vec();
    let mut failures = Vec::new();
    each_stop_point("Gif animation decode", &mut failures, |pulse| {
        let mut decoder = DecodeRequest::new(&encoded)
            .with_pulse(pulse)
            .animation_frame_decoder()?;
        let mut decoded = 0;
        while decoder
            .render_next_frame_owned(None)
            .map_err(|e| CodecError::from_codec_boxed(ImageFormat::Gif, e))?
            .is_some()
        {
            decoded += 1;
        }
        assert_eq!(decoded, frames, "an unstopped decode yields every frame");
        Ok(())
    });
    assert!(
        failures.is_empty(),
        "{} wrong stop points:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
