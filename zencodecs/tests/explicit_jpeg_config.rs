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
