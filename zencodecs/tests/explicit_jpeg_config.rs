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
