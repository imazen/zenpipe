#![cfg(feature = "jp2-decode")]
#![allow(deprecated)] // Exercise the established static dispatch entry point.
use zencodecs::{DecodeRequest, ImageFormat};

#[test]
fn jp2_and_bare_codestream_decode_every_reference_sample() {
    for data in [
        &include_bytes!("fixtures/jp2/test.jp2")[..],
        &include_bytes!("fixtures/jp2/test.j2k")[..],
    ] {
        let decoded = DecodeRequest::new(data).decode_full_frame().unwrap();
        assert_eq!(decoded.info().format, ImageFormat::Jp2);
        assert_eq!((decoded.width(), decoded.height()), (16, 16));
        let buffer = decoded.into_buffer();
        let pixels = buffer.as_slice();
        assert_eq!(pixels.descriptor(), zenpixels::PixelDescriptor::RGB8_SRGB);
        for y in 0..16 {
            for x in 0..16 {
                assert_eq!(
                    &pixels.row(y)[x * 3..x * 3 + 3],
                    &[16 * x as u8, 16 * y as u8, 8 * (x as u8 + y as u8)]
                );
            }
        }
    }
}

#[test]
fn jp2_obeys_decode_allowlist_and_never_advertises_encode() {
    use zencodecs::AllowedFormats;
    let bytes = include_bytes!("fixtures/jp2/test.jp2");
    let denied = AllowedFormats::none();
    assert!(
        DecodeRequest::new(bytes)
            .with_registry(&denied)
            .decode_full_frame()
            .is_err()
    );
    let allowed = denied.with_decode(ImageFormat::Jp2, true);
    assert!(
        DecodeRequest::new(bytes)
            .with_registry(&allowed)
            .decode_full_frame()
            .is_ok()
    );
    assert!(
        !allowed
            .with_encode(ImageFormat::Jp2, true)
            .can_encode(ImageFormat::Jp2)
    );
}
