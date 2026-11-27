//! PNG source qualification through the same coupled operation as JPEG.
use super::*;
use zencodec::encode::{EncodePolicy, Fidelity};

pub(super) fn png_fixture(format: &PixelFormat, profile: bool) -> (Vec<u8>, Vec<u8>) {
    png_fixture_sized(format, profile, 7, 5)
}
fn png_fixture_sized(format: &PixelFormat, profile: bool, w: u32, h: u32) -> (Vec<u8>, Vec<u8>) {
    let bpp = format.layout.bytes_per_pixel();
    let row = w as usize * bpp;
    let padding = bpp * 3;
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
    let (metadata, pixel_format) = if profile {
        let icc = zenpixels_convert::icc_profiles::DISPLAY_P3_V4;
        (
            Metadata::none().with_icc(icc),
            PixelFormat::new(
                format.layout,
                ColorSpace::Icc {
                    profile: Arc::from(icc),
                    color_channels: 3,
                    alpha: format.color.alpha(),
                    intent: RenderingIntent::RelativeColorimetric,
                },
            ),
        )
    } else {
        (Metadata::none(), format.clone())
    };
    let bytes = config
        .job()
        .with_unfiltered_metadata(metadata.filtered(&MetadataPolicy::PreserveExact))
        .try_with_policy(EncodePolicy::none().with_color(zencodec::ColorEmitPolicy::Verbatim))
        .unwrap()
        .encoder()
        .unwrap()
        .encode(PixelView::new(&raw, w, h, row + padding, &pixel_format).unwrap())
        .unwrap()
        .data()
        .to_vec();
    (bytes, expected)
}

#[test]
fn native_png_batches_fit_independently_admitted_transcode_strips() {
    let codecs = codecs();
    for descriptor in [PixelFormat::RGB8_SRGB, PixelFormat::RGBA16_SRGB] {
        for height in [65, 257] {
            let (input, expected) = png_fixture_sized(&descriptor, false, 97, height);
            for strip in [1, 3, 64] {
                let budget = StorageBudget::new(2 * 1024 * 1024, 16);
                let output = TranscodeRequest::new(ImageFormat::Png)
                    .with_fidelity(Fidelity::Lossless)
                    .with_strip_rows(strip)
                    .with_storage(budget.clone())
                    .transcode(&codecs, &input)
                    .unwrap_or_else(|e| {
                        panic!(
                            "{descriptor:?}, height={height}, strip={strip}: {}",
                            explain(&e)
                        )
                    });
                assert_eq!(oracle(output.data()), expected);
                assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
            }
        }
    }
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
    let graya16_srgb = PixelFormat::new(
        SampleLayout::new(
            zenpixels::SampleType::U16,
            zenpixels::ChannelOrder::GrayAlpha,
        ),
        ColorSpace::GRAY_SRGB.with_alpha(AlphaMode::Straight),
    );
    for descriptor in [
        PixelFormat::RGB8_SRGB,
        PixelFormat::RGBA8_SRGB,
        PixelFormat::RGB16_SRGB,
        PixelFormat::RGBA16_SRGB,
        PixelFormat::GRAY16_SRGB,
        graya16_srgb,
    ] {
        // RGB profiles apply only to color layouts, not to grayscale samples.
        let profile = descriptor == PixelFormat::RGB8_SRGB || descriptor == PixelFormat::RGBA8_SRGB;
        let (input, expected) = png_fixture(&descriptor, profile);
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
    let (input, _) = png_fixture(&zenpixels::formats::RGB8_SRGB, true);
    let codecs = codecs();
    let source = zencodec::operation::DecodeOptions::new()
        .decode(&codecs, &input)
        .unwrap();
    let src_fmt = PixelFormat::new(
        SampleLayout::RGB8,
        ColorSpace::Icc {
            profile: Arc::from(zenpixels_convert::icc_profiles::DISPLAY_P3_V4),
            color_channels: 3,
            alpha: AlphaMode::None,
            intent: RenderingIntent::RelativeColorimetric,
        },
    );
    let cms = MoxCms
        .build_transform(&src_fmt, &zenpixels::formats::RGB8_SRGB)
        .unwrap();
    let mut expected = vec![0; 7 * 5 * 3];
    for y in 0..5 {
        cms.transform_row(
            source.pixels().row(y),
            &mut expected[y as usize * 21..(y as usize + 1) * 21],
            7,
        )
        .unwrap();
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
