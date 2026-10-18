//! Full-image fallback must obey the same declaration and limits contract.
#![cfg(feature = "bitmaps-bmp")]

use zencodecs::{EncodeRequest, ImageFormat, Limits};
use zenpixels::{PixelDescriptor, PixelSlice};

fn encoder(limits: &Limits) -> Box<dyn zencodec::encode::DynEncoder> {
    EncodeRequest::new(ImageFormat::Bmp)
        .with_limits(limits)
        .build_streaming_encoder(2, 2)
        .unwrap()
        .encoder
}

#[test]
fn buffered_fallback_refuses_one_shot_after_begin() {
    let mut enc = encoder(&Limits::none());
    enc.begin(2, 2, PixelDescriptor::RGB8_SRGB).unwrap();
    let px = [0; 12];
    let px = PixelSlice::new(&px, 2, 2, 6, PixelDescriptor::RGB8_SRGB).unwrap();
    assert!(enc.encode(px).is_err());
}

#[test]
fn buffered_fallback_refuses_rgba_one_shot_after_begin() {
    let mut enc = encoder(&Limits::none());
    enc.begin(2, 2, PixelDescriptor::RGBA8_SRGB).unwrap();
    assert!(enc.encode_srgba8(&mut [0; 16], false, 2, 2, 2).is_err());
}

#[test]
fn buffered_fallback_checks_memory_before_reserving_the_frame() {
    let mut enc = encoder(&Limits::none().with_max_memory(1));
    assert!(enc.begin(2, 2, PixelDescriptor::RGB8_SRGB).is_err());
}

#[test]
fn buffered_fallback_checks_dimensions_at_begin() {
    let mut enc = encoder(&Limits::none().with_max_width(1));
    assert!(enc.begin(2, 2, PixelDescriptor::RGB8_SRGB).is_err());
}

#[test]
fn failed_push_cannot_be_retried_as_a_successful_file() {
    let mut enc = encoder(&Limits::none());
    enc.begin(2, 2, PixelDescriptor::RGB8_SRGB).unwrap();
    let wrong = PixelSlice::new(&[0; 3], 1, 1, 3, PixelDescriptor::RGB8_SRGB).unwrap();
    assert!(enc.push_rows(wrong).is_err());
    let correct = PixelSlice::new(&[0; 12], 2, 2, 6, PixelDescriptor::RGB8_SRGB).unwrap();
    assert!(enc.push_rows(correct).is_err());
    assert!(enc.finish().is_err());
}
