#![cfg(all(feature = "avif-encode", feature = "avif-decode"))]

use zencodec::{Orientation, encode::EncodeOrientation};
use zencodecs::{EncodeRequest, ImageFormat, Metadata};
use zenpixels::{PixelFormat, PixelView};

#[test]
fn application_can_choose_stored_or_baked_orientation() {
    use zencodec::decode::{Decode as _, DecodeJob as _, DecoderConfig as _};
    let pixels: Vec<u8> = (0..7 * 4).flat_map(|i| [i * 7, 200 - i * 5, 100]).collect();
    for mode in [EncodeOrientation::PreserveStored, EncodeOrientation::Bake] {
        let input = PixelView::new(&pixels, 7, 4, 21, &zenpixels::formats::RGB8_SRGB).unwrap();
        let bytes = EncodeRequest::new(ImageFormat::Avif)
            .with_metadata(Metadata::none().with_orientation(Orientation::Rotate90))
            .with_orientation_mode(mode)
            .encode(input, false)
            .unwrap()
            .into_vec();
        let decoded = zenavif::AvifDecoderConfig::new()
            .job()
            .decoder(
                std::borrow::Cow::Borrowed(&bytes),
                &[PixelFormat::RGB8_SRGB],
            )
            .unwrap()
            .decode()
            .unwrap();
        let expected = match mode {
            EncodeOrientation::PreserveStored => (7, 4, Orientation::Rotate90),
            EncodeOrientation::Bake => (4, 7, Orientation::Identity),
            _ => unreachable!(),
        };
        assert_eq!(
            (
                decoded.pixels().width(),
                decoded.pixels().height(),
                decoded.info().orientation
            ),
            expected
        );
    }
}
