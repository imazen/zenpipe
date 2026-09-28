//! Color conversion must change samples before dropping their source profile.
#![cfg(all(feature = "cms", feature = "png", feature = "gif"))]
#![allow(deprecated)] // The stable facade is the consumer under test.

use std::sync::Arc;
use zencodecs::{
    AllowedFormats, DecodeRequest, EncodeRequest, FormatDecision, ImageFormat, Metadata,
    TranscodeColor, TranscodeOptions, transcode,
};
use zenpixels::{
    Cicp, ColorContext, ColorPrimaries, ColorProfileSource, PixelBuffer, PixelDescriptor,
    PixelFormat, TransferFunction,
};
use zenpixels_convert::{
    cms::PluggableCms, cms_moxcms::MoxCms, icc_profiles::ADOBE_RGB, policy::ConvertOptions,
};

fn source_png(icc: &[u8]) -> Vec<u8> {
    let buffer = PixelBuffer::from_vec(
        [191, 97, 53, 255].repeat(16),
        4,
        4,
        PixelDescriptor::RGBA8_SRGB
            .with_primaries(ColorPrimaries::Unknown)
            .with_transfer(TransferFunction::Unknown),
    )
    .unwrap()
    .with_color_context(Arc::new(ColorContext::from_icc(icc)));
    EncodeRequest::new(ImageFormat::Png)
        .with_lossless(true)
        .with_metadata(Metadata::none().with_icc(icc.to_vec()))
        .encode(buffer.as_slice(), false)
        .unwrap()
        .into_vec()
}

#[test]
fn explicit_srgb_transcode_converts_adobe_rgb_before_png_and_gif() {
    let source = source_png(ADOBE_RGB);
    let all = AllowedFormats::all();
    let gif = FormatDecision::for_format(ImageFormat::Gif);
    assert!(transcode(&source, &gif, &TranscodeOptions::default(), &all).is_err());

    let mut transform = MoxCms
        .build_source_transform(
            ColorProfileSource::Icc(ADOBE_RGB),
            ColorProfileSource::Cicp(Cicp::SRGB),
            PixelFormat::Rgba8,
            PixelFormat::Rgba8,
            &ConvertOptions::permissive(),
        )
        .unwrap()
        .unwrap();
    let mut expected = [0; 4];
    transform.transform_row(&[191, 97, 53, 255], &mut expected, 1);
    assert_ne!(expected, [191, 97, 53, 255]);
    for format in [ImageFormat::Png, ImageFormat::Gif] {
        let mut decision = FormatDecision::for_format_quality(format, 100.0);
        decision.lossless = true;
        let opts = TranscodeOptions {
            color: TranscodeColor::Srgb8,
            ..Default::default()
        };
        let output = transcode(&source, &decision, &opts, &all).unwrap();
        let decoded = DecodeRequest::new(&output.data)
            .decode_full_frame()
            .unwrap();
        assert!(decoded.info().source_color.icc_profile.is_none());
        let pixels = decoded.into_buffer();
        let view = pixels.as_slice();
        assert_eq!(view.descriptor().transfer(), TransferFunction::Srgb);
        for y in 0..4 {
            for pixel in view
                .row(y)
                .chunks_exact(view.descriptor().bytes_per_pixel())
            {
                assert_eq!(&pixel[..3], &expected[..3], "{format:?}");
                if pixel.len() == 4 {
                    assert_eq!(pixel[3], 255);
                }
            }
        }
    }
}

#[test]
fn sdr_conversion_rejects_hdr_and_invalid_icc() {
    let options = TranscodeOptions {
        color: TranscodeColor::Srgb8,
        ..Default::default()
    };
    let target = FormatDecision::for_format(ImageFormat::Png);
    for transfer in [TransferFunction::Pq, TransferFunction::Hlg] {
        let pixels = PixelBuffer::from_vec(
            1000_u16.to_ne_bytes().repeat(3 * 4 * 4),
            4,
            4,
            PixelDescriptor::RGB16_SRGB
                .with_transfer(transfer)
                .with_primaries(ColorPrimaries::Bt2020),
        )
        .unwrap();
        let source = EncodeRequest::new(ImageFormat::Png)
            .with_lossless(true)
            .encode(pixels.as_slice(), false)
            .unwrap()
            .into_vec();
        let err = transcode(&source, &target, &options, &AllowedFormats::all()).unwrap_err();
        assert!(err.to_string().contains("tone-map policy"), "{err}");
    }
    let source = source_png(b"not an ICC profile");
    assert!(transcode(&source, &target, &options, &AllowedFormats::all()).is_err());
}
