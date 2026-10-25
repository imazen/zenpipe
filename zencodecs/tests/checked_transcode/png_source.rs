//! PNG source qualification through the same coupled operation as JPEG.
use super::*;
use zencodec::encode::{EncodePolicy, Fidelity};

pub(super) fn png_fixture(descriptor: PixelDescriptor, profile: bool) -> (Vec<u8>, Vec<u8>) {
    let (w, h) = (7, 5);
    let row = w as usize * descriptor.bytes_per_pixel();
    let padding = descriptor.bytes_per_pixel() * 3;
    let mut raw = vec![0xe7; (row + padding) * h as usize];
    for y in 0..h as usize {
        for x in 0..row {
            raw[y * (row + padding) + x] = (x * 13 + y * 29) as u8;
        }
    }
    let expected = raw
        .chunks_exact(row + padding)
        .flat_map(|r| r[..row].iter().copied())
        .collect::<Vec<_>>();
    let config = zenpng::PngEncoderConfig::new()
        .with_compression(zenpng::Compression::Effort(0))
        .with_downcast(zenpng::DowncastFlags::none())
        .with_preserve_transparent_rgb(true);
    let metadata = if profile {
        Metadata::none().with_icc(zenpixels_convert::icc_profiles::DISPLAY_P3_V4)
    } else {
        Metadata::none()
    };
    let bytes = config
        .job()
        .with_metadata_policy(metadata, MetadataPolicy::PreserveExact)
        .try_with_policy(EncodePolicy::none().with_color(zencodec::ColorEmitPolicy::Verbatim))
        .unwrap()
        .encoder()
        .unwrap()
        .encode(PixelSlice::new(&raw, w, h, row + padding, descriptor).unwrap())
        .unwrap()
        .data()
        .to_vec();
    (bytes, expected)
}
fn oracle(bytes: &[u8]) -> Vec<u8> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().unwrap();
    let mut raw = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut raw).unwrap();
    raw.truncate(info.buffer_size());
    if info.bit_depth == png::BitDepth::Sixteen {
        for sample in raw.as_chunks_mut::<2>().0 {
            *sample = u16::from_be_bytes(*sample).to_ne_bytes();
        }
    }
    raw
}
#[test]
fn png_source_retains_alpha_depth_samples_and_icc_through_lossless_transcode() {
    let codecs = codecs();
    for descriptor in [
        PixelDescriptor::RGB8_SRGB,
        PixelDescriptor::RGBA8_SRGB,
        PixelDescriptor::RGB16_SRGB,
        PixelDescriptor::RGBA16_SRGB,
        PixelDescriptor::GRAY16_SRGB,
        PixelDescriptor::GRAYA16_SRGB,
    ] {
        // RGB profiles apply only to color layouts, not to grayscale samples.
        let profile = matches!(
            descriptor,
            PixelDescriptor::RGB8_SRGB | PixelDescriptor::RGBA8_SRGB
        );
        let (input, expected) = png_fixture(descriptor, profile);
        assert_eq!(oracle(&input), expected);
        let budget = StorageBudget::new(512 * 1024, 16);
        let req = TranscodeRequest::new(ImageFormat::Png)
            .with_fidelity(Fidelity::Lossless)
            .with_storage(budget.clone());
        let output = req
            .transcode(&codecs, &input)
            .unwrap_or_else(|e| panic!("{descriptor:?}: {}", explain(&e)));
        assert_eq!(oracle(output.data()), expected, "{descriptor:?}");
        if profile {
            assert_eq!(
                zenpng::probe(output.data()).unwrap().icc_profile.as_deref(),
                Some(zenpixels_convert::icc_profiles::DISPLAY_P3_V4)
            );
        }
        let mut source = RangeSource {
            bytes: &input,
            reads: 0,
            fail_after: None,
        };
        let disk = req.transcode_from_read_at(&codecs, &mut source).unwrap();
        assert_eq!(disk.data(), output.data());
        assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
    }
}
#[test]
fn png_profile_conversion_matches_separate_cms_and_drops_reported_descriptions() {
    let (input, _) = png_fixture(PixelDescriptor::RGB8_SRGB, true);
    let codecs = codecs();
    let source = zencodec::operation::DecodeRequest::new()
        .decode(&codecs, &input)
        .unwrap();
    let mut cms = MoxCms
        .build_source_transform(
            ColorProfileSource::Icc(zenpixels_convert::icc_profiles::DISPLAY_P3_V4),
            ColorProfileSource::Cicp(Cicp::SRGB),
            PixelFormat::Rgb8,
            PixelFormat::Rgb8,
            &ConvertOptions::forbid_lossy(),
        )
        .unwrap()
        .unwrap();
    let mut expected = vec![0; 7 * 5 * 3];
    for y in 0..5 {
        cms.transform_row(
            source.pixels().row(y),
            &mut expected[y as usize * 21..(y as usize + 1) * 21],
            7,
        );
    }
    let budget = StorageBudget::new(512 * 1024, 16);
    let output = request(budget.clone()).transcode(&codecs, &input).unwrap();
    assert_eq!(oracle(output.data()), expected);
    let info = zenpng::probe(output.data()).unwrap();
    assert!(info.exif.is_none() && info.xmp.is_none());
    assert_ne!(
        info.icc_profile.as_deref(),
        Some(zenpixels_convert::icc_profiles::DISPLAY_P3_V4)
    );
    assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
}
