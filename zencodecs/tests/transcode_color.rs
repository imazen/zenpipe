//! Color conversion must change samples before dropping their source profile.
#![cfg(all(feature = "cms", feature = "png", feature = "gif"))]
#![allow(deprecated)] // The stable facade is the consumer under test.

use std::sync::Arc;
use zencodecs::{
    AllowedFormats, DecodeRequest, EncodeRequest, FormatDecision, ImageFormat, Metadata,
    TranscodeColor, TranscodeOptions, transcode,
};
use zenpixels::{
    AlphaMode, ColorOrigin, ColorSpace, HlgMode, PixelBuf, PixelFormat, Primaries, RenderingIntent,
    SampleLayout, Transfer,
};
use zenpixels_convert::{
    cms::{CmsBackend, MoxCms},
    icc_profiles::ADOBE_RGB,
};

fn source_png(icc: &[u8]) -> Vec<u8> {
    let buffer = PixelBuf::from_bytes(
        [191, 97, 53, 255].repeat(16),
        4,
        4,
        PixelFormat::RGBA8_SRGB
            .with_primaries(Primaries::Unknown)
            .with_transfer(Transfer::Unknown),
    )
    .unwrap()
    .with_origin(ColorOrigin::from_icc(Arc::from(icc)));
    EncodeRequest::new(ImageFormat::Png)
        .with_lossless(true)
        .with_metadata(Metadata::none().with_icc(icc.to_vec()))
        .encode(buffer.as_view(), false)
        .unwrap()
        .into_vec()
}

#[test]
fn explicit_srgb_transcode_converts_adobe_rgb_before_png_and_gif() {
    let source = source_png(ADOBE_RGB);
    let all = AllowedFormats::all();
    let src_fmt = PixelFormat::new(
        SampleLayout::RGBA8,
        ColorSpace::Icc {
            profile: Arc::from(ADOBE_RGB),
            color_channels: 3,
            alpha: AlphaMode::Straight,
            intent: RenderingIntent::Perceptual,
        },
    );
    let transform = MoxCms
        .build_transform(&src_fmt, &zenpixels::formats::RGBA8_SRGB)
        .unwrap();
    let mut expected = [0; 4];
    transform
        .transform_row(&[191, 97, 53, 255], &mut expected, 1)
        .unwrap();
    assert_ne!(expected, [191, 97, 53, 255]);
    // Explicit sRGB conversion for both; GIF also by default, because a
    // format that cannot carry the profile has its pixels fitted to sRGB
    // (`color_fit`) rather than relabelled.
    for (format, color) in [
        (ImageFormat::Png, TranscodeColor::Srgb8),
        (ImageFormat::Gif, TranscodeColor::Srgb8),
        (ImageFormat::Gif, TranscodeOptions::default().color),
    ] {
        let mut decision = FormatDecision::for_format_quality(format, 100.0);
        decision.lossless = true;
        let opts = TranscodeOptions {
            color,
            ..Default::default()
        };
        let output = transcode(&source, &decision, &opts, &all).unwrap();
        let decoded = DecodeRequest::new(&output.data)
            .decode_full_frame()
            .unwrap();
        assert!(decoded.info().source_color.icc_profile.is_none());
        let pixels = decoded.into_buffer();
        let view = pixels.as_view();
        assert!(matches!(
            view.format().color,
            ColorSpace::Rgb {
                transfer: Transfer::Srgb,
                ..
            } | ColorSpace::Gray {
                transfer: Transfer::Srgb,
                ..
            }
        ));
        for y in 0..4 {
            for pixel in view
                .row(y)
                .chunks_exact(view.format().layout.bytes_per_pixel())
            {
                assert_eq!(&pixel[..3], &expected[..3], "{format:?}");
                if pixel.len() == 4 {
                    assert_eq!(pixel[3], 255);
                }
            }
        }
    }
}

/// Construct a valid 4x4 RGBA8 PNG carrying an arbitrary (possibly invalid) iCCP payload.
/// Uses the `png` crate directly so malformed ICC bytes can still be tested as decode inputs
/// even though `zenpng 0.2` validates ICC headers at encode time.
fn raw_png_with_iccp(icc: &[u8]) -> Vec<u8> {
    let mut zlib = Vec::with_capacity(icc.len() + 11);
    zlib.extend_from_slice(&[0x78, 0x01, 0x01]);
    let len = u16::try_from(icc.len()).expect("short icc payload");
    zlib.extend_from_slice(&len.to_le_bytes());
    zlib.extend_from_slice(&(!len).to_le_bytes());
    zlib.extend_from_slice(icc);
    let (mut s1, mut s2) = (1u32, 0u32);
    for &b in icc {
        s1 = (s1 + u32::from(b)) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    zlib.extend_from_slice(&((s2 << 16) | s1).to_be_bytes());

    let mut iccp = Vec::with_capacity(5 + zlib.len());
    iccp.extend_from_slice(b"bad\0\0");
    iccp.extend_from_slice(&zlib);

    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, 4, 4);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().unwrap();
    writer.write_chunk(png::chunk::iCCP, &iccp).unwrap();
    writer
        .write_image_data(&[191, 97, 53, 255].repeat(16))
        .unwrap();
    drop(writer);
    out
}

#[test]
fn sdr_conversion_rejects_hdr_and_invalid_icc() {
    let options = TranscodeOptions {
        color: TranscodeColor::Srgb8,
        ..Default::default()
    };
    let target = FormatDecision::for_format(ImageFormat::Png);
    for transfer in [Transfer::Pq, Transfer::Hlg(HlgMode::Scene)] {
        let pixels = PixelBuf::from_bytes(
            1000_u16.to_ne_bytes().repeat(3 * 4 * 4),
            4,
            4,
            PixelFormat::RGB16_SRGB
                .with_transfer(transfer)
                .with_primaries(Primaries::Bt2020),
        )
        .unwrap();
        let source = EncodeRequest::new(ImageFormat::Png)
            .with_lossless(true)
            .encode(pixels.as_view(), false)
            .unwrap()
            .into_vec();
        let err = transcode(&source, &target, &options, &AllowedFormats::all()).unwrap_err();
        assert!(err.to_string().contains("tone-map policy"), "{err}");
    }
    let source = raw_png_with_iccp(b"not an ICC profile");
    assert!(transcode(&source, &target, &options, &AllowedFormats::all()).is_err());
}
