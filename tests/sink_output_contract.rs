//! Writer failures and encoder failures must never allow successful completion.
whereat::define_at_crate_info!();
#[path = "support/streaming_encoder.rs"]
mod streaming_encoder;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use streaming_encoder::{Failure, TestEncoder};
use zenpipe::{Sink, codec::EncoderSink};
use zenpixels::{PixelDescriptor, PixelSlice};

struct OwnedTransport(Arc<AtomicBool>);

impl Drop for OwnedTransport {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn rows() -> PixelSlice<'static> {
    PixelSlice::new(&[1, 2, 3], 1, 1, 3, PixelDescriptor::RGB8_SRGB).unwrap()
}

#[test]
fn failed_push_after_header_cannot_be_finished() {
    let closed = Arc::new(AtomicBool::new(false));
    let transport = OwnedTransport(closed.clone());
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let out = bytes.clone();
    let mut sink = EncoderSink::new(
        Box::new(TestEncoder {
            failure: Failure::Push,
            ..Default::default()
        }),
        PixelDescriptor::RGB8_SRGB,
    )
    .with_writer(move |b| {
        let _transport = &transport;
        out.lock().unwrap().extend_from_slice(b);
        Ok(())
    });
    assert!(sink.begin(1, 1).unwrap().streams());
    assert_eq!(*bytes.lock().unwrap(), b"H");
    assert!(sink.consume(&rows()).is_err());
    assert!(
        closed.load(Ordering::SeqCst),
        "release the owned transport immediately"
    );
    assert!(
        sink.finish().is_err(),
        "a failed encode cannot complete its transport"
    );
    assert!(sink.take_output().is_none());
    assert_eq!(*bytes.lock().unwrap(), b"H");
}

#[test]
fn failed_writer_cannot_retry_or_finish() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let out = bytes.clone();
    let mut sink = EncoderSink::new(Box::<TestEncoder>::default(), PixelDescriptor::RGB8_SRGB)
        .with_writer(move |b| {
            out.lock().unwrap().push(b[0]);
            Err(whereat::at!(zenpipe::PipeError::Op(
                "injected partial write failure".into()
            )))
        });
    assert!(sink.begin(1, 1).is_err());
    assert!(sink.consume(&rows()).is_err());
    assert!(sink.finish().is_err());
    assert_eq!(
        *bytes.lock().unwrap(),
        b"H",
        "a partial failed write must not be retried"
    );
}

#[test]
fn failed_finish_never_emits_a_completion() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let out = bytes.clone();
    let mut sink = EncoderSink::new(
        Box::new(TestEncoder {
            failure: Failure::Finish,
            ..Default::default()
        }),
        PixelDescriptor::RGB8_SRGB,
    )
    .with_writer(move |b| {
        out.lock().unwrap().extend_from_slice(b);
        Ok(())
    });
    sink.begin(1, 1).unwrap();
    sink.consume(&rows()).unwrap();
    assert!(sink.finish().is_err());
    assert!(sink.take_output().is_none());
    assert_eq!(*bytes.lock().unwrap(), &[b'H', 1, 2, 3]);
}

#[test]
fn repeated_begin_is_refused_by_the_sink() {
    let mut sink = EncoderSink::new(Box::<TestEncoder>::default(), PixelDescriptor::RGB8_SRGB);
    let timing = sink.begin(1, 1).unwrap();
    assert!(sink.begin(2, 2).is_err());
    assert_eq!(sink.output_timing(), Some(timing));
    assert_eq!(sink.pending_output(), b"H");
}

#[test]
fn begin_after_rows_is_refused_by_the_sink() {
    let mut sink = EncoderSink::new(Box::<TestEncoder>::default(), PixelDescriptor::RGB8_SRGB);
    sink.consume(&rows()).unwrap();
    assert!(sink.begin(1, 1).is_err());
}

#[test]
fn uneven_drains_and_owned_handoffs_preserve_strided_rows() {
    use zencodec::encode::DynEncoder;
    let px = [1, 2, 3, 255, 255, 255, 4, 5, 6];
    let pixels = PixelSlice::new(&px, 1, 2, 6, PixelDescriptor::RGB8_SRGB).unwrap();
    let want = Box::<TestEncoder>::default()
        .encode(pixels.clone())
        .unwrap();
    for take in [false, true] {
        let mut sink = EncoderSink::new(Box::<TestEncoder>::default(), PixelDescriptor::RGB8_SRGB);
        sink.begin(1, 2).unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&sink.pending_output()[..1]);
        sink.consume_output(1);
        sink.consume(&pixels).unwrap();
        if take {
            sink.take_pending_output(&mut bytes);
        } else {
            bytes.extend_from_slice(&sink.pending_output()[..2]);
            sink.consume_output(2);
            bytes.extend_from_slice(sink.pending_output());
            sink.consume_output(usize::MAX);
        }
        sink.finish().unwrap();
        bytes.extend_from_slice(sink.take_output().unwrap().data());
        assert_eq!(bytes, want.data());
    }
}

#[test]
fn at_finish_encoder_exposes_no_prefix() {
    let mut sink = EncoderSink::new(
        Box::new(TestEncoder {
            buffered: true,
            ..Default::default()
        }),
        PixelDescriptor::RGB8_SRGB,
    );
    assert_eq!(
        sink.begin(1, 1).unwrap(),
        zencodec::encode::OutputTiming::AT_FINISH
    );
    sink.consume(&rows()).unwrap();
    assert!(sink.pending_output().is_empty());
    let mut taken = Vec::new();
    sink.take_pending_output(&mut taken);
    assert!(taken.is_empty());
    sink.finish().unwrap();
    assert_eq!(sink.take_output().unwrap().data(), &[b'H', 1, 2, 3, b'F']);
}
