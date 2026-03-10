//! Minimal roundtrip example that works on wasm32-wasip1.
//!
//! Decodes an embedded JPEG, re-encodes to WebP/PNG/GIF, and prints sizes.
//!
//! Run natively: `cargo run --example wasm_roundtrip --features std`
//! Run on wasm:  `cargo build --example wasm_roundtrip --target wasm32-wasip1 --release --features std`
//!               `wasmtime target/wasm32-wasip1/release/examples/wasm_roundtrip.wasm`

use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat, PixelBufferConvertTypedExt as _};

fn main() {
    // Embed a small test JPEG
    let jpeg_data = include_bytes!("../tests/images/ultrahdr_sample.jpg");

    let decoded = DecodeRequest::new(jpeg_data)
        .decode()
        .expect("decode failed");

    println!(
        "Decoded: {}x{} {:?}",
        decoded.width(),
        decoded.height(),
        decoded.format(),
    );

    let meta = decoded.metadata();
    println!(
        "Metadata: ICC={} EXIF={} XMP={}",
        meta.icc_profile.as_deref().map_or(0, |p| p.len()),
        meta.exif.as_deref().map_or(0, |p| p.len()),
        meta.xmp.as_deref().map_or(0, |p| p.len()),
    );

    let rgb8 = decoded.into_buffer().to_rgb8();
    let img = rgb8.as_imgref();

    for (name, format) in [
        ("JPEG", ImageFormat::Jpeg),
        ("WebP", ImageFormat::WebP),
        ("PNG", ImageFormat::Png),
        ("GIF", ImageFormat::Gif),
    ] {
        let encoded = EncodeRequest::new(format)
            .with_quality(80.0)
            .with_metadata(&meta)
            .encode_rgb8(img)
            .expect("encode failed");

        println!("{name}: {} bytes", encoded.len());
    }

    println!("All roundtrips OK");
}
