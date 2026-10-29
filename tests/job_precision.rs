//! Identity jobs must not discard precision before a lossless codec sees it.
#![cfg(all(feature = "job", feature = "nodes-png"))]

use zencodec::ImageFormat;
use zencodecs::{BoolKeep, CodecIntent, DecodeRequest, EncodeRequest, FormatChoice};
use zenpipe::job::{CmsMode, ImageJob};
use zenpixels::{PixelDescriptor, PixelSlice};

#[test]
fn png16_identity_job_preserves_every_sample() {
    // Include values that cannot survive an 8-bit round trip, and opaque alpha.
    // Padding must not become pixels or affect the encoded image.
    let (width, height, stride) = (3, 2, 32);
    let mut storage = vec![0xa5; stride * height];
    let values = [1u16, 257, 577, 65534, 32769, 234];
    for (i, value) in values.into_iter().enumerate() {
        let offset = (i / width) * stride + (i % width) * 8;
        for (channel, sample) in [value, value.wrapping_mul(17), 65535 - value, 65535]
            .into_iter()
            .enumerate()
        {
            storage[offset + channel * 2..offset + channel * 2 + 2]
                .copy_from_slice(&sample.to_ne_bytes());
        }
    }
    let pixels = PixelSlice::new(
        &storage,
        width as u32,
        height as u32,
        stride,
        PixelDescriptor::RGBA16_SRGB,
    )
    .unwrap();
    let input = EncodeRequest::new(ImageFormat::Png)
        .with_lossless(true)
        .encode(pixels.clone(), true)
        .expect("16-bit PNG fixture")
        .into_vec();
    let baseline = DecodeRequest::new(&input).decode_full_frame().unwrap();
    assert_eq!(
        baseline.pixels().descriptor().channel_type(),
        zenpixels::ChannelType::U16
    );
    for y in 0..height {
        assert_eq!(baseline.pixels().row(y as u32), pixels.row(y as u32));
    }
    let result = ImageJob::new()
        .add_input(0, input)
        .add_output(1)
        .with_cms(CmsMode::Preserve)
        .with_intent(CodecIntent {
            format: Some(FormatChoice::Specific(ImageFormat::Png)),
            lossless: Some(BoolKeep::True),
            ..Default::default()
        })
        .run()
        .expect("lossless identity job");
    let output = DecodeRequest::new(&result.encode_results[0].bytes)
        .decode_full_frame()
        .unwrap();
    assert_eq!(output.pixels().descriptor(), baseline.pixels().descriptor());
    for y in 0..height {
        assert_eq!(
            output.pixels().row(y as u32),
            baseline.pixels().row(y as u32)
        );
    }
}
