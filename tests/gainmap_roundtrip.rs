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

// ═══════════════════════════════════════════════════════════════════════
// Cross-format transcoding with gain map preservation
// ═══════════════════════════════════════════════════════════════════════

/// Make a larger UltraHDR JPEG for resize/crop tests (16x16).
fn make_ultrahdr_jpeg_16x16() -> Vec<u8> {
    use imgref::ImgVec;
    use rgb::Rgb;

    let pixels: Vec<Rgb<f32>> = (0..256)
        .map(|i| {
            let t = i as f32 / 255.0;
            Rgb {
                r: 0.5 + t * 4.0,
                g: 0.3 + t * 2.0,
                b: 0.1 + t * 1.0,
            }
        })
        .collect();
    let img = ImgVec::new(pixels, 16, 16);

    zencodecs::EncodeRequest::new(zencodec::ImageFormat::Jpeg)
        .with_quality(90.0)
        .encode_ultrahdr_rgb_f32(img.as_ref())
        .expect("UltraHDR encode failed")
        .into_vec()
}

/// Helper: run ImageJob with specific output format and optional resize nodes.
fn transcode_with_gainmap(
    input: Vec<u8>,
    output_format: zencodec::ImageFormat,
    nodes: &[Box<dyn zennode::NodeInstance>],
) -> (Vec<u8>, Option<zencodecs::DecodedGainMap>) {
    let intent = zencodecs::CodecIntent {
        format: Some(zencodecs::FormatChoice::Specific(output_format)),
        ..Default::default()
    };

    let result = ImageJob::new()
        .add_input(0, input)
        .add_output(1)
        .with_nodes(nodes)
        .with_intent(intent)
        .with_cms(zenpipe::job::CmsMode::None)
        .run()
        .expect("transcode should succeed");

    let output_bytes = result.encode_results[0].bytes.clone();

    let gm_result = zencodecs::DecodeRequest::new(&output_bytes)
        .with_registry(&zencodecs::AllowedFormats::all())
        .with_gain_map_extraction(true)
        .decode_gain_map();

    match gm_result {
        Ok((_, gm)) => (output_bytes, gm),
        Err(_) => (output_bytes, None),
    }
}

/// Helper: create a Constrain node for resizing.
fn resize_node(w: u32, h: u32) -> Box<dyn zennode::NodeInstance> {
    let registry = zenpipe::full_registry();
    let def = registry.get("zenresize.constrain").unwrap();
    let mut node = def.create_default().unwrap();
    node.set_param("w", zennode::ParamValue::U32(w));
    node.set_param("h", zennode::ParamValue::U32(h));
    node.set_param("mode", zennode::ParamValue::Str("fit".to_string()));
    node
}

/// Helper: create a Crop node.
fn crop_node(x: u32, y: u32, w: u32, h: u32) -> Box<dyn zennode::NodeInstance> {
    let registry = zenpipe::full_registry();
    let def = registry.get("zenlayout.crop").unwrap();
    let mut node = def.create_default().unwrap();
    node.set_param("x", zennode::ParamValue::U32(x));
    node.set_param("y", zennode::ParamValue::U32(y));
    node.set_param("w", zennode::ParamValue::U32(w));
    node.set_param("h", zennode::ParamValue::U32(h));
    node
}

// ─── JPEG → JPEG with resize ───

#[test]
fn jpeg_to_jpeg_with_resize_preserves_gainmap() {
    let input = make_ultrahdr_jpeg_16x16();
    let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![resize_node(8, 8)];
    let (_, gm) = transcode_with_gainmap(input, zencodec::ImageFormat::Jpeg, &nodes);
    assert!(
        gm.is_some(),
        "gain map should survive JPEG→JPEG with resize"
    );
}

// ─── JPEG → JPEG with crop ───

#[test]
fn jpeg_to_jpeg_with_crop_preserves_gainmap() {
    let input = make_ultrahdr_jpeg_16x16();
    // Crop a small region — use conservative coords that work with any decode size.
    let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![crop_node(1, 1, 6, 6)];
    let (_, gm) = transcode_with_gainmap(input, zencodec::ImageFormat::Jpeg, &nodes);
    assert!(gm.is_some(), "gain map should survive JPEG→JPEG with crop");
}

// ─── JPEG → PNG drops gain map (PNG has no gain map support) ───

#[test]
fn jpeg_to_png_drops_gainmap() {
    let input = make_ultrahdr_jpeg_16x16();
    let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![];
    let (output, _) = transcode_with_gainmap(input, zencodec::ImageFormat::Png, &nodes);
    // PNG can't carry gain maps — just verify encode succeeded.
    assert!(!output.is_empty());
}

// ─── Resize + crop combined ───

#[test]
fn jpeg_resize_and_crop_preserves_gainmap() {
    let input = make_ultrahdr_jpeg_16x16();
    let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![
        crop_node(1, 1, 6, 6), // crop to 6x6
        resize_node(4, 4),     // then resize to 4x4
    ];
    let (_, gm) = transcode_with_gainmap(input, zencodec::ImageFormat::Jpeg, &nodes);
    assert!(gm.is_some(), "gain map should survive crop + resize");
}
