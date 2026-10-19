//! Encoding into a format that cannot carry the source's colour description
//! converts the pixels to the closest colour it describes (sRGB) instead of
//! mislabelling them; a description that is sRGB already, or that describes
//! no colour space, is dropped with the pixels unchanged; a format that
//! carries the description keeps pixels and description as they are.
//!
//! PNM is the no-carrier target here: it is lossless, so converted pixels
//! can be compared exactly.

#![cfg(all(feature = "bitmaps", feature = "png"))]

use std::sync::Arc;

use zencodec::Metadata;
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat};
use zenpixels::{Cicp, ColorContext, ColorOrigin, PixelBuffer, PixelDescriptor, PixelSlice};
use zenpixels_convert::{OutputProfile, finalize_for_output_with};

const W: u32 = 9;
const H: u32 = 5;

/// A few in-gamut, saturated and gray colours, repeated.
fn pixels() -> Vec<u8> {
    let colours: [[u8; 3]; 5] = [
        [200, 100, 50],
        [30, 180, 90],
        [128, 128, 128],
        [250, 10, 10],
        [10, 40, 220],
    ];
    (0..W * H)
        .flat_map(|i| colours[i as usize % colours.len()])
        .collect()
}

fn slice(px: &[u8]) -> PixelSlice<'_> {
    PixelSlice::new(px, W, H, W as usize * 3, PixelDescriptor::RGB8_SRGB).unwrap()
}

fn encode(format: ImageFormat, px: &[u8], meta: Metadata) -> Vec<u8> {
    EncodeRequest::new(format)
        .with_metadata(meta)
        .encode(slice(px), false)
        .unwrap()
        .into_vec()
}

fn encode_streaming(format: ImageFormat, px: &[u8], meta: Metadata) -> Vec<u8> {
    let mut se = EncodeRequest::new(format)
        .with_metadata(meta)
        .build_streaming_encoder(W, H)
        .unwrap();
    se.encoder.begin(W, H, PixelDescriptor::RGB8_SRGB).unwrap();
    for y in 0..H {
        let row = &px[(y * W * 3) as usize..((y + 1) * W * 3) as usize];
        se.encoder
            .push_rows(PixelSlice::new(row, W, 1, row.len(), PixelDescriptor::RGB8_SRGB).unwrap())
            .unwrap();
    }
    let mut out = Vec::new();
    se.encoder.take_output(&mut out);
    out.extend_from_slice(se.encoder.finish().unwrap().data());
    out
}

fn decoded_rgb(bytes: &[u8]) -> Vec<u8> {
    let d = DecodeRequest::new(bytes).decode_full_frame().unwrap();
    assert_eq!(d.descriptor().layout(), zenpixels::ChannelLayout::Rgb);
    let px = d.pixels();
    (0..H)
        .flat_map(|y| px.row(y)[..(W * 3) as usize].to_vec())
        .collect()
}

/// What the pixels are in sRGB when they are actually Display P3.
fn p3_to_srgb(px: &[u8]) -> Vec<u8> {
    let desc = PixelDescriptor::RGB8_SRGB.with_primaries(zenpixels::ColorPrimaries::DisplayP3);
    let buf = PixelBuffer::from_vec(px.to_vec(), W, H, desc)
        .unwrap()
        .with_color_context(Arc::new(ColorContext::from_cicp(Cicp::DISPLAY_P3)));
    let ready = finalize_for_output_with(
        &buf,
        &ColorOrigin::from_cicp(Cicp::DISPLAY_P3),
        OutputProfile::Named(Cicp::SRGB),
        desc.pixel_format(),
        None,
    )
    .unwrap();
    let (out, _) = ready.into_parts();
    let s = out.as_slice();
    (0..H)
        .flat_map(|y| s.row(y)[..(W * 3) as usize].to_vec())
        .collect()
}

#[test]
fn display_p3_into_a_format_without_a_carrier_is_converted_to_srgb() {
    let px = pixels();
    let meta = Metadata::none().with_cicp(Cicp::DISPLAY_P3);
    let bytes = encode(ImageFormat::Pnm, &px, meta.clone());
    let got = decoded_rgb(&bytes);
    let want = p3_to_srgb(&px);
    assert_ne!(
        want, px,
        "the fixture must contain colours P3 and sRGB disagree on"
    );
    assert_eq!(got, want, "PNM pixels are the P3 source converted to sRGB");
    let info = DecodeRequest::new(&bytes).decode_full_frame().unwrap();
    assert!(info.info().source_color.icc_profile.is_none());

    // The streaming encode converts strip by strip and gives the same file.
    assert_eq!(encode_streaming(ImageFormat::Pnm, &px, meta), bytes);
}

#[test]
fn a_display_p3_icc_profile_converts_like_display_p3_cicp() {
    // A bundled matrix/TRC P3 profile is recognised without a CMS and
    // converted as its CICP.
    let px = pixels();
    let meta = Metadata::none().with_icc(zenpixels_convert::icc_profiles::DISPLAY_P3_V4.to_vec());
    let bytes = encode(ImageFormat::Pnm, &px, meta.clone());
    assert_eq!(decoded_rgb(&bytes), p3_to_srgb(&px));
    assert_eq!(encode_streaming(ImageFormat::Pnm, &px, meta), bytes);
}

#[test]
fn an_srgb_description_is_dropped_and_the_pixels_kept() {
    let px = pixels();
    #[allow(unused_mut)] // more cases with `cms`
    let mut metas = vec![Metadata::none().with_cicp(Cicp::SRGB)];
    #[cfg(feature = "cms")]
    metas.push(Metadata::none().with_icc(zencodecs::cms::srgb_icc_profile()));
    for meta in metas {
        let bytes = encode(ImageFormat::Pnm, &px, meta.clone());
        assert_eq!(decoded_rgb(&bytes), px);
        assert_eq!(encode_streaming(ImageFormat::Pnm, &px, meta), bytes);
    }
}

#[test]
fn a_profile_that_describes_no_colour_space_is_dropped_and_the_pixels_kept() {
    let px = pixels();
    let junk: Vec<u8> = (0..256u32).map(|i| (i * 31) as u8).collect();
    let bytes = encode(ImageFormat::Pnm, &px, Metadata::none().with_icc(junk));
    assert_eq!(decoded_rgb(&bytes), px);
}

#[test]
fn a_format_that_carries_the_description_keeps_pixels_and_description() {
    let px = pixels();
    let bytes = encode(
        ImageFormat::Png,
        &px,
        Metadata::none().with_cicp(Cicp::DISPLAY_P3),
    );
    assert_eq!(
        decoded_rgb(&bytes),
        px,
        "PNG is lossless and carries the colour"
    );
    let d = DecodeRequest::new(&bytes).decode_full_frame().unwrap();
    let color = &d.info().source_color;
    assert!(
        color.cicp == Some(Cicp::DISPLAY_P3) || color.icc_profile.is_some(),
        "PNG keeps a P3 description: {color:?}"
    );
}

fn strict() -> zencodec::encode::EncodePolicy {
    zencodec::encode::EncodePolicy::none().with_strict(true)
}

#[test]
fn strict_refuses_a_fit_that_loses_colour() {
    let px = pixels();
    let junk: Vec<u8> = (0..256u32).map(|i| (i * 31) as u8).collect();
    for meta in [
        Metadata::none().with_cicp(Cicp::DISPLAY_P3),
        Metadata::none().with_icc(junk),
    ] {
        let err = EncodeRequest::new(ImageFormat::Pnm)
            .with_metadata(meta.clone())
            .with_encode_policy(strict())
            .encode(slice(&px), false)
            .expect_err("strict must refuse");
        assert!(format!("{err}").contains("strict"), "{err}");
        let err = EncodeRequest::new(ImageFormat::Pnm)
            .with_metadata(meta)
            .with_encode_policy(strict())
            .build_streaming_encoder(W, H)
            .err()
            .expect("strict must refuse the streaming encoder too");
        assert!(format!("{err}").contains("strict"), "{err}");
    }
}

#[test]
fn strict_allows_fits_that_lose_nothing() {
    let px = pixels();
    // sRGB into a format without a carrier: dropping it loses nothing.
    let bytes = EncodeRequest::new(ImageFormat::Pnm)
        .with_metadata(Metadata::none().with_cicp(Cicp::SRGB))
        .with_encode_policy(strict())
        .encode(slice(&px), false)
        .unwrap()
        .into_vec();
    assert_eq!(decoded_rgb(&bytes), px);
    // P3 into a format that carries it.
    EncodeRequest::new(ImageFormat::Png)
        .with_metadata(Metadata::none().with_cicp(Cicp::DISPLAY_P3))
        .with_encode_policy(strict())
        .encode(slice(&px), false)
        .unwrap();
}

#[test]
fn strict_refuses_conversion_when_descriptor_matches_source_color() {
    let px = pixels();
    let descriptor =
        PixelDescriptor::RGB8_SRGB.with_primaries(zenpixels::ColorPrimaries::DisplayP3);
    let source = PixelSlice::new(&px, W, H, W as usize * 3, descriptor).unwrap();
    let make = || {
        EncodeRequest::new(ImageFormat::Pnm)
            .with_metadata(Metadata::none().with_cicp(Cicp::DISPLAY_P3))
            .with_encode_policy(strict())
    };
    let err = make()
        .encode(source, false)
        .expect_err("strict must refuse before pixel-format conversion");
    assert!(format!("{err}").contains("strict"), "{err}");
    let err = make()
        .build_streaming_encoder(W, H)
        .err()
        .expect("streaming must make the same strict decision");
    assert!(format!("{err}").contains("strict"), "{err}");
}
