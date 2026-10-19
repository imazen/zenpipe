//! Error propagation through the decoder-to-encoder seam.
#[path = "support/streaming_encoder.rs"]
mod streaming_encoder;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use streaming_encoder::{Failure, TestEncoder};
use zencodec::decode::DecodeRowSink;
use zencodecs::TranscodeSink;
use zenpixels::PixelDescriptor;

const RGB: &[PixelDescriptor] = &[PixelDescriptor::RGB8_SRGB];

struct OwnedTransport(Arc<AtomicBool>);

impl Drop for OwnedTransport {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[test]
fn failed_push_after_header_prevents_finish_encode() {
    let closed = Arc::new(AtomicBool::new(false));
    let transport = OwnedTransport(closed.clone());
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let out = bytes.clone();
    let mut sink = TranscodeSink::new(
        Box::new(TestEncoder {
            failure: Failure::Push,
            ..Default::default()
        }),
        RGB,
    )
    .with_writer(move |b| {
        let _transport = &transport;
        out.lock().unwrap().extend_from_slice(b);
        Ok(())
    });
    sink.begin(1, 1, RGB[0]).unwrap();
    assert_eq!(*bytes.lock().unwrap(), b"H");
    sink.provide_next_buffer(0, 1, 1, RGB[0])
        .unwrap()
        .row_mut(0)
        .copy_from_slice(&[1, 2, 3]);
    assert!(sink.finish().is_err());
    assert!(
        closed.load(Ordering::SeqCst),
        "release the owned transport immediately"
    );
    assert!(sink.finish_encode().is_err());
    assert_eq!(*bytes.lock().unwrap(), b"H");
}

#[test]
fn partially_failed_header_write_cannot_complete() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let out = bytes.clone();
    let mut sink = TranscodeSink::new(Box::<TestEncoder>::default(), RGB).with_writer(move |b| {
        out.lock().unwrap().push(b[0]);
        Err("injected partial write failure".into())
    });
    assert!(sink.begin(1, 1, RGB[0]).is_err());
    assert!(sink.provide_next_buffer(0, 1, 1, RGB[0]).is_err());
    assert!(sink.finish_encode().is_err());
    assert_eq!(*bytes.lock().unwrap(), b"H");
}

#[test]
fn finish_failure_leaves_only_an_incomplete_prefix() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let out = bytes.clone();
    let mut sink = TranscodeSink::new(
        Box::new(TestEncoder {
            failure: Failure::Finish,
            ..Default::default()
        }),
        RGB,
    )
    .with_writer(move |b| {
        out.lock().unwrap().extend_from_slice(b);
        Ok(())
    });
    sink.begin(1, 1, RGB[0]).unwrap();
    sink.provide_next_buffer(0, 1, 1, RGB[0])
        .unwrap()
        .row_mut(0)
        .copy_from_slice(&[1, 2, 3]);
    sink.finish().unwrap();
    assert!(sink.finish_encode().is_err());
    assert_eq!(*bytes.lock().unwrap(), &[b'H', 1, 2, 3]);
}

#[test]
fn second_begin_preserves_the_pending_decoded_strip() {
    let mut sink = TranscodeSink::new(Box::<TestEncoder>::default(), RGB);
    sink.begin(1, 1, RGB[0]).unwrap();
    sink.provide_next_buffer(0, 1, 1, RGB[0])
        .unwrap()
        .row_mut(0)
        .copy_from_slice(&[1, 2, 3]);
    assert!(sink.begin(2, 2, RGB[0]).is_err());
    sink.finish().unwrap();
    assert_eq!(sink.finish_encode().unwrap().data(), &[b'H', 1, 2, 3, b'F']);
}
