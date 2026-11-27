//! Minimal roundtrip example that works on wasm32-wasip1.
//!
//! Decodes an embedded JPEG, re-encodes to WebP/PNG/GIF, and prints sizes.
//!
//! Run natively: `cargo run --example wasm_roundtrip --features std`
//! Run on wasm:  `cargo build --example wasm_roundtrip --target wasm32-wasip1 --release --features std`
//!               `wasmtime target/wasm32-wasip1/release/examples/wasm_roundtrip.wasm`

use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat};

fn main() {
    // Embed a small test JPEG
    let jpeg_data = include_bytes!("../tests/images/ultrahdr_sample.jpg");

    let decoded = DecodeRequest::new(jpeg_data)
        .decode_full_frame()
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

    let rgb_buf = zenpixels_convert::convert_to_buf(
        &decoded.pixels(),
        zenpixels::PixelFormat::RGB8_SRGB,
        &zenpixels_convert::ConvertPolicy::permissive(),
    )
    .unwrap();
    let (w, h) = (rgb_buf.width() as usize, rgb_buf.height() as usize);
    let rgb8 = imgref::ImgVec::new(rgb_buf.into_vec().unwrap(), w, h);
    let img = rgb8.as_ref();

    for (name, format) in [
        ("JPEG", ImageFormat::Jpeg),
        ("WebP", ImageFormat::WebP),
        ("PNG", ImageFormat::Png),
        ("GIF", ImageFormat::Gif),
    ] {
        let encoded = EncodeRequest::new(format)
            .with_quality(80.0)
            .with_metadata(meta.clone())
            .encode_full_frame_rgb8(img)
            .expect("encode failed");

        println!("{name}: {} bytes", encoded.len());
    }

    println!("All roundtrips OK");
}
