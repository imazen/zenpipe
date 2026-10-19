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

#[test]
fn rotated_fallback_applies_limits_to_the_output_canvas() {
    let limits = Limits::none().with_max_width(3).with_max_height(2);
    let make = || {
        EncodeRequest::new(ImageFormat::Bmp)
            .with_metadata(
                zencodec::Metadata::none().with_orientation(zencodec::Orientation::Rotate90),
            )
            .with_limits(&limits)
    };
    let data = [
        1, 2, 3, 4, 5, 6, 0xCD, 0xCD, 0xCD, 7, 8, 9, 10, 11, 12, 0xCD, 0xCD, 0xCD, 13, 14, 15, 16,
        17, 18,
    ];
    let pixels = PixelSlice::new(&data, 2, 3, 9, PixelDescriptor::RGB8_SRGB).unwrap();
    let whole = make().encode(pixels, false).unwrap();
    let mut encoder = make().build_streaming_encoder(2, 3).unwrap().encoder;
    assert_eq!(
        encoder.begin(2, 3, PixelDescriptor::RGB8_SRGB).unwrap(),
        zencodec::encode::OutputTiming::AT_FINISH
    );
    for y in 0..3 {
        let row = PixelSlice::new(&data[y * 9..], 2, 1, 9, PixelDescriptor::RGB8_SRGB).unwrap();
        encoder.push_rows(row).unwrap();
    }
    assert_eq!(encoder.finish().unwrap().data(), whole.data());

    let wrong_limits = Limits::none().with_max_width(2).with_max_height(3);
    let mut encoder = EncodeRequest::new(ImageFormat::Bmp)
        .with_metadata(zencodec::Metadata::none().with_orientation(zencodec::Orientation::Rotate90))
        .with_limits(&wrong_limits)
        .build_streaming_encoder(2, 3)
        .unwrap()
        .encoder;
    assert!(encoder.begin(2, 3, PixelDescriptor::RGB8_SRGB).is_err());
}
