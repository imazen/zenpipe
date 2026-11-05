//! A job from HDR into a format with no HDR needs no option to look right.
#![cfg(all(feature = "job", feature = "nodes-png", feature = "nodes-jpeg"))]
use zencodec::{ImageFormat, Metadata};
use zencodecs::{BoolKeep, CodecIntent, DecodeRequest, EncodeRequest, FormatChoice};
use zenpipe::job::{CmsMode, ImageJob};
use zenpixels::{Cicp, ColorPrimaries, PixelBuffer, PixelDescriptor as D, TransferFunction};

/// PQ code for 203 cd/m².
const REFERENCE_WHITE: u16 = 38_000;
/// PQ code for 1000 cd/m², on row 0: tone mapping measures the peak, and this sets it.
const PEAK: u16 = 49_271;

fn pq_png() -> Vec<u8> {
    let d = D::RGBA16_SRGB
        .with_transfer(TransferFunction::Pq)
        .with_primaries(ColorPrimaries::Bt2020);
    let mut buffer = PixelBuffer::try_new(16, 16, d).unwrap();
    let mut pixels = buffer.as_slice_mut();
    for y in 0..16 {
        for px in pixels.row_mut(y).as_chunks_mut::<8>().0 {
            for (i, sample) in px.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let v = match (i, y) {
                    (3, _) => 65535,
                    (_, 0) => PEAK,
                    _ => REFERENCE_WHITE,
                };
                *sample = v.to_ne_bytes();
            }
        }
    }
    EncodeRequest::new(ImageFormat::Png)
        .with_lossless(true)
        .with_metadata(Metadata::none().with_cicp(Cicp::new(9, 16, 0, true)))
        .encode(buffer.as_slice(), false)
        .unwrap()
        .into_vec()
}

fn job(input: Vec<u8>, format: ImageFormat, lossless: bool) -> ImageJob<'static> {
    ImageJob::new()
        .add_input(0, input)
        .add_output(1)
        .with_cms(CmsMode::Preserve)
        .with_intent(CodecIntent {
            format: Some(FormatChoice::Specific(format)),
            lossless: lossless.then_some(BoolKeep::True),
            ..Default::default()
        })
}

fn run(format: ImageFormat, lossless: bool) -> Vec<u8> {
    job(pq_png(), format, lossless)
        .run()
        .unwrap()
        .encode_results
        .remove(0)
        .bytes
}

#[test]
fn jpeg_tone_maps_with_no_options() {
    let decoded = DecodeRequest::new(&run(ImageFormat::Jpeg, false))
        .decode_full_frame()
        .unwrap();
    let pixels = decoded.pixels();
    let bytes = pixels.descriptor().bytes_per_pixel();
    for px in pixels.row(8).chunks_exact(bytes) {
        // Written as code values, this PQ gray would show near 148.
        assert!((166..=176).contains(&px[0]), "{px:?}");
    }
}

#[test]
fn png_keeps_hdr() {
    let decoded = DecodeRequest::new(&run(ImageFormat::Png, true))
        .decode_full_frame()
        .unwrap();
    let pixels = decoded.pixels();
    assert_eq!(pixels.descriptor().transfer, TransferFunction::Pq);
    assert_eq!(
        pixels.descriptor().channel_type(),
        zenpixels::ChannelType::U16
    );
    assert_eq!(
        u16::from_ne_bytes([pixels.row(8)[0], pixels.row(8)[1]]),
        REFERENCE_WHITE
    );
}
