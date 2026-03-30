//! Gain map round-trip tests.
//!
//! Verifies that ImageJob preserves gain maps through the pipeline:
//! decode → process → encode, with automatic geometry tracking.

#![cfg(all(feature = "job", feature = "nodes-jpeg"))]

use zenpipe::job::{GainMapMode, ImageJob};

/// Create a UltraHDR JPEG test image (SDR base + gain map).
fn make_ultrahdr_jpeg() -> Vec<u8> {
    use imgref::ImgVec;
    use rgb::Rgb;

    // 8x8 orange-ish HDR image
    let pixels: Vec<Rgb<f32>> = (0..64)
        .map(|i| {
            let t = i as f32 / 63.0;
            Rgb {
                r: 1.0 + t * 3.0, // HDR range: 1.0-4.0
                g: 0.5 + t * 1.5,
                b: 0.2 + t * 0.5,
            }
        })
        .collect();
    let img = ImgVec::new(pixels, 8, 8);

    zencodecs::EncodeRequest::new(zencodec::ImageFormat::Jpeg)
        .with_quality(90.0)
        .encode_ultrahdr_rgb_f32(img.as_ref())
        .expect("UltraHDR encode failed")
        .into_vec()
}

#[test]
fn jpeg_ultrahdr_roundtrip_preserves_gainmap() {
    let input = make_ultrahdr_jpeg();

    // Verify the input has a gain map.
    let (_, gm) = zencodecs::DecodeRequest::new(&input)
        .with_registry(&zencodecs::AllowedFormats::all())
        .decode_gain_map()
        .expect("decode should succeed");
    assert!(gm.is_some(), "input should contain a gain map");

    // Process through ImageJob with default GainMapMode::Preserve.
    let result = ImageJob::new()
        .add_input(0, input)
        .add_output(1)
        .with_cms(zenpipe::job::CmsMode::None)
        .run()
        .expect("ImageJob should succeed");

    assert_eq!(result.encode_results.len(), 1);
    let output_bytes = &result.encode_results[0].bytes;
    assert!(!output_bytes.is_empty());

    // Decode the output and check for gain map.
    let (decoded, output_gm) = zencodecs::DecodeRequest::new(output_bytes)
        .with_registry(&zencodecs::AllowedFormats::all())
        .decode_gain_map()
        .expect("output decode should succeed");

    assert!(
        output_gm.is_some(),
        "output JPEG should still contain a gain map after round-trip"
    );

    let gm = output_gm.unwrap();
    assert!(gm.gain_map.width > 0);
    assert!(gm.gain_map.height > 0);
    assert!(!gm.gain_map.data.is_empty());

    // Verify base image decoded correctly.
    assert!(decoded.width() > 0);
    assert!(decoded.height() > 0);
}

#[test]
fn discard_mode_strips_gainmap() {
    let input = make_ultrahdr_jpeg();

    let result = ImageJob::new()
        .add_input(0, input)
        .add_output(1)
        .with_gain_map_mode(GainMapMode::Discard)
        .with_cms(zenpipe::job::CmsMode::None)
        .run()
        .expect("ImageJob should succeed");

    let output_bytes = &result.encode_results[0].bytes;

    // Decode the output — should NOT have a gain map.
    let (_, output_gm) = zencodecs::DecodeRequest::new(output_bytes)
        .with_registry(&zencodecs::AllowedFormats::all())
        .decode_gain_map()
        .expect("output decode should succeed");

    assert!(
        output_gm.is_none(),
        "output should NOT contain a gain map when Discard mode is used"
    );
}

#[test]
fn gainmap_default_mode_is_preserve() {
    assert_eq!(GainMapMode::default(), GainMapMode::Preserve);
}
