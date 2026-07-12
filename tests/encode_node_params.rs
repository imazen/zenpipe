//! W10 (decision-level subset): explicit per-codec encode nodes must reach
//! the encoder — the bridge used to capture them into EncodeConfig and the
//! job dropped them, making `?jpeg.quality=` a silent no-op on ImageJob.

#![cfg(all(feature = "job", feature = "nodes-jpeg", feature = "nodes-png"))]

use zenpipe::job::ImageJob;

fn small_png() -> Vec<u8> {
    // Noisy-ish gradient so JPEG quality changes output size measurably.
    let mut pixels = Vec::with_capacity(32 * 32 * 4);
    for y in 0..32u32 {
        for x in 0..32u32 {
            pixels.extend_from_slice(&[(x * 8) as u8, (y * 8) as u8, ((x * y) % 251) as u8, 255]);
        }
    }
    let slice =
        zenpixels::PixelSlice::new(&pixels, 32, 32, 32 * 4, zenpipe::format::RGBA8_SRGB).unwrap();
    zencodecs::EncodeRequest::new(zencodec::ImageFormat::Png)
        .encode(slice, true)
        .expect("fixture png")
        .data()
        .to_vec()
}

fn run_qs(qs: &str) -> zenpipe::job::EncodeResult {
    let registry = zenpipe::full_registry();
    let nodes = registry.from_querystring(qs).instances;
    let mut result = ImageJob::new()
        .add_input(0, small_png())
        .add_output(1)
        .with_nodes(&nodes)
        .run()
        .expect("job");
    result.encode_results.remove(0)
}

#[test]
fn jpeg_encode_node_forces_format_and_quality_applies() {
    let low = run_qs("jpeg.quality=20");
    let high = run_qs("jpeg.quality=95");
    assert_eq!(low.mime_type, "image/jpeg", "encode node must force JPEG");
    assert_eq!(high.mime_type, "image/jpeg");
    assert!(
        low.bytes.len() < high.bytes.len(),
        "q20 ({}) must be smaller than q95 ({}) — quality key was dead",
        low.bytes.len(),
        high.bytes.len()
    );
}

#[test]
fn quality_intent_format_still_wins_over_encode_node_default() {
    // An explicit format= beats the encode node's implied format.
    let r = run_qs("format=png&jpeg.quality=50");
    assert_eq!(r.mime_type, "image/png");
}
