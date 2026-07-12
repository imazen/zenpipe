//! Matte-color alpha flattening (Known Bug: JPEG flatten hardcoded white).
//!
//! `?matte=` (Constrain `matte_color`) must reach the encoder's alpha
//! flatten so transparent pixels composite onto the requested color.

#![cfg(all(feature = "job", feature = "nodes-jpeg", feature = "nodes-png"))]

use zenpipe::job::ImageJob;

/// Encode a fully-transparent 8x8 RGBA PNG.
fn transparent_png() -> Vec<u8> {
    let pixels = vec![0u8; 8 * 8 * 4]; // rgba all zero = transparent black
    let slice = zenpixels::PixelSlice::new(
        &pixels,
        8,
        8,
        8 * 4,
        zenpipe::format::RGBA8_SRGB,
    )
    .unwrap();
    zencodecs::EncodeRequest::new(zencodec::ImageFormat::Png)
        .encode(slice, true)
        .expect("encode fixture png")
        .data()
        .to_vec()
}

fn decode_first_pixel(jpeg: &[u8]) -> [u8; 3] {
    let decoded = zencodecs::DecodeRequest::new(jpeg)
        .decode_full_frame()
        .expect("decode output");
    let px = decoded.pixels();
    let b = px.as_strided_bytes();
    [b[0], b[1], b[2]]
}

#[test]
fn matte_key_colors_transparent_regions() {
    let png = transparent_png();
    let registry = zenpipe::full_registry();
    let nodes = registry.from_querystring("matte=ff0000").instances;
    assert!(!nodes.is_empty(), "matte key must produce a Constrain node");

    let result = ImageJob::new()
        .add_input(0, png)
        .add_output(1)
        .with_nodes(&nodes)
        .with_output_extension("jpg")
        .run()
        .expect("flatten job");

    let [r, g, b] = decode_first_pixel(&result.encode_results[0].bytes);
    assert!(
        r > 200 && g < 60 && b < 60,
        "transparent area must flatten to the red matte, got ({r},{g},{b})"
    );
}

#[test]
fn default_matte_is_white() {
    let png = transparent_png();
    let result = ImageJob::new()
        .add_input(0, png)
        .add_output(1)
        .with_output_extension("jpg")
        .run()
        .expect("flatten job");
    let [r, g, b] = decode_first_pixel(&result.encode_results[0].bytes);
    assert!(
        r > 200 && g > 200 && b > 200,
        "default matte stays white, got ({r},{g},{b})"
    );
}
