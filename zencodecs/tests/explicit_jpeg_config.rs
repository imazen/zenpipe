//! Explicit native JPEG settings must survive the dispatch adapter.
#![cfg(feature = "jpeg")]

use zencodecs::{EncodeRequest, ImageFormat, config::CodecConfig};
use zenjpeg::encoder::{ChromaSubsampling, EncoderConfig, PixelLayout};
use zenpixels::{PixelDescriptor, PixelSlice};

#[test]
fn native_scan_and_entropy_settings_are_preserved() {
    const W: u32 = 67;
    const H: u32 = 93;
    let config = EncoderConfig::ycbcr(85.0, ChromaSubsampling::Quarter)
        .progressive(false)
        .huffman(false);
    let data: Vec<u8> = (0..W * H * 3)
        .map(|i| (i.wrapping_mul(73) ^ (i >> 6)) as u8)
        .collect();
    let stride = W as usize * 3;
    let mut native = config
        .request()
        .encode_from_bytes(W, H, PixelLayout::Rgb8Srgb)
        .unwrap();
    native
        .push(&data, H as usize, stride, zencodec::Unstoppable)
        .unwrap();
    let native = native.finish().unwrap();
    let overrides = CodecConfig::default().with_jpeg_encoder(config);
    let pixels = PixelSlice::new(&data, W, H, stride, PixelDescriptor::RGB8_SRGB).unwrap();
    let output = EncodeRequest::new(ImageFormat::Jpeg)
        .with_codec_config(&overrides)
        .encode(pixels, false)
        .unwrap();
    assert!(
        output.data() == native,
        "dispatch changed the caller's native JPEG scan or entropy configuration"
    );
}

#[test]
fn short_fixed_table_canvas_keeps_the_complete_file_until_finish() {
    for height in [1, 7] {
        let width = 17;
        let overrides = CodecConfig::default().with_jpeg_encoder(
            EncoderConfig::ycbcr(85.0, ChromaSubsampling::Quarter)
                .progressive(false)
                .huffman(false),
        );
        let make = || EncodeRequest::new(ImageFormat::Jpeg).with_codec_config(&overrides);
        let data: Vec<u8> = (0..width * height * 3).map(|i| (i * 37) as u8).collect();
        let pixels = PixelSlice::new(
            &data,
            width,
            height,
            width as usize * 3,
            PixelDescriptor::RGB8_SRGB,
        )
        .unwrap();
        let whole = make().encode(pixels.clone(), false).unwrap();
        let mut encoder = make()
            .build_streaming_encoder(width, height)
            .unwrap()
            .encoder;
        assert_eq!(
            encoder.begin(width, height, pixels.descriptor()).unwrap(),
            zencodec::encode::OutputTiming::AT_FINISH
        );
        assert!(encoder.pending_output().is_empty());
        encoder.push_rows(pixels).unwrap();
        assert!(encoder.pending_output().is_empty());
        assert_eq!(encoder.finish().unwrap().data(), whole.data());
    }
}

#[test]
fn explicit_jpeg_codec_config_keeps_baseline_and_matches_native() {
    use imgref::Img;
    use rgb::Rgb;

    let (w, h) = (64usize, 64usize);
    let mut rgb = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            rgb.push(Rgb::new(
                ((x * 13 + y * 29) & 0xFF) as u8,
                (((x * 37) ^ (y * 11)) & 0xFF) as u8,
                ((x + y) * 7) as u8,
            ));
        }
    }

    let jpeg = EncoderConfig::ycbcr(100.0, ChromaSubsampling::None)
        .force_baseline()
        .aq_enabled(false)
        .deringing(false);
    let cfg = CodecConfig::default().with_jpeg_encoder(jpeg.clone());
    let out = EncodeRequest::new(ImageFormat::Jpeg)
        .with_codec_config(&cfg)
        .encode_full_frame_rgb8(Img::new(&rgb[..], w, h))
        .unwrap()
        .into_vec();

    // Baseline SOF0 must be present and progressive SOF2 must not be emitted.
    assert!(out.windows(2).any(|m| m == [0xFF, 0xC0]));
    assert!(!out.windows(2).any(|m| m == [0xFF, 0xC2]));
    assert!(!zencodecs::from_bytes(&out).unwrap().is_progressive);

    let mut raw_bytes = Vec::with_capacity(w * h * 3);
    for px in &rgb {
        raw_bytes.extend_from_slice(&[px.r, px.g, px.b]);
    }
    let mut direct_enc = jpeg
        .encode_from_bytes(w as u32, h as u32, PixelLayout::Rgb8Srgb)
        .unwrap();
    direct_enc
        .push(&raw_bytes, h, w * 3, zencodec::Unstoppable)
        .unwrap();
    let direct = direct_enc.finish().unwrap();
    assert_eq!(out, direct);
}
