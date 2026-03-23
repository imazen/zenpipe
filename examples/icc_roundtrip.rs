//! ICC profile roundtrip: chain decode/encode across JPEG → WebP → PNG → JPEG.
//!
//! Verifies that ICC profile bytes are preserved exactly through each format
//! that supports ICC embedding.
//!
//! Run: `cargo run --example icc_roundtrip --features all,std`

use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat, PixelBufferConvertTypedExt as _};

fn main() {
    let jpeg_data = include_bytes!("../tests/images/ultrahdr_sample.jpg");

    // Step 1: Decode original JPEG
    let decoded = DecodeRequest::new(jpeg_data)
        .decode_full_frame()
        .expect("failed to decode JPEG");

    let original_icc: Vec<u8> = decoded
        .info()
        .source_color
        .icc_profile
        .as_deref()
        .expect("no ICC profile in test JPEG")
        .to_vec();

    println!("Original JPEG ICC: {} bytes", original_icc.len());
    println!(
        "  SHA-256 prefix: {:02x}{:02x}{:02x}{:02x}...",
        original_icc[0], original_icc[1], original_icc[2], original_icc[3]
    );

    let meta = decoded.metadata();
    let rgb8 = decoded.into_buffer().to_rgb8();
    let img = rgb8.as_imgref();

    // Chain: JPEG → WebP → PNG → JPEG
    let chain = [
        ("JPEG → WebP", ImageFormat::WebP),
        ("WebP → PNG", ImageFormat::Png),
        ("PNG → JPEG", ImageFormat::Jpeg),
    ];

    let mut current_data: Vec<u8>;
    let mut prev_icc = original_icc.to_vec();
    let first_encoded = EncodeRequest::new(chain[0].1)
        .with_quality(95.0)
        .with_metadata(&meta)
        .encode_full_frame_rgb8(img)
        .expect("encode failed");

    current_data = first_encoded.into_vec();

    // Verify first hop
    let step1 = DecodeRequest::new(&current_data)
        .decode_full_frame()
        .expect("decode failed");
    let step1_icc = step1
        .info()
        .source_color
        .icc_profile
        .as_deref()
        .unwrap_or(&[]);
    let match_str = if step1_icc == prev_icc {
        "MATCH"
    } else {
        "DIFFER"
    };
    println!(
        "\n{}: ICC {} bytes — {match_str}",
        chain[0].0,
        step1_icc.len()
    );
    prev_icc = step1_icc.to_vec();

    // Continue chain
    for &(label, format) in &chain[1..] {
        let step = DecodeRequest::new(&current_data)
            .decode_full_frame()
            .expect("decode failed");

        let step_meta = step.metadata();
        let step_rgb8 = step.into_buffer().to_rgb8();
        let step_img = step_rgb8.as_imgref();

        let encoded = EncodeRequest::new(format)
            .with_quality(95.0)
            .with_metadata(&step_meta)
            .encode_full_frame_rgb8(step_img)
            .expect("encode failed");

        current_data = encoded.into_vec();

        let re_decoded = DecodeRequest::new(&current_data)
            .decode_full_frame()
            .expect("decode failed");

        let new_icc = re_decoded
            .info()
            .source_color
            .icc_profile
            .as_deref()
            .unwrap_or(&[]);
        let match_str = if new_icc == prev_icc {
            "MATCH"
        } else {
            "DIFFER"
        };
        println!("{label}: ICC {} bytes — {match_str}", new_icc.len());
        prev_icc = new_icc.to_vec();
    }

    // Final check: does the last ICC match the original?
    let final_match = if prev_icc == original_icc {
        "EXACT MATCH with original"
    } else {
        "DIFFERS from original"
    };
    println!("\nFull chain result: {final_match}");
}
