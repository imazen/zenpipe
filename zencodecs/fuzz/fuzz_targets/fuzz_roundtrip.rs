//! Fuzz target: encode→decode roundtrip consistency.
//!
//! Uses structured fuzzing to generate small valid images, encode them
//! through zencodecs, then decode the result and verify dimensions match.
#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use zencodecs::{AllowedFormats, DecodeRequest, EncodeRequest, ImageFormat, Limits};
use zenpixels::{PixelDescriptor, PixelSlice};

#[derive(Debug, Arbitrary)]
struct RoundtripInput {
    width: u8,
    height: u8,
    quality: u8,
    format_selector: u8,
    has_alpha: bool,
    pixels: Vec<u8>,
}

fuzz_target!(|input: RoundtripInput| {
    let width = (input.width as u32).max(1).min(256);
    let height = (input.height as u32).max(1).min(256);
    let quality = (input.quality as f32).max(1.0).min(100.0);

    let format = match input.format_selector % 4 {
        0 => ImageFormat::Jpeg,
        1 => ImageFormat::WebP,
        2 => ImageFormat::Png,
        _ => ImageFormat::Gif,
    };

    // Build pixel descriptor and data
    let (descriptor, bpp) = if input.has_alpha {
        (PixelDescriptor::RGBA8, 4usize)
    } else {
        (PixelDescriptor::RGB8, 3usize)
    };

    let stride = width as usize * bpp;
    let needed = stride * height as usize;
    let mut pixels = input.pixels;
    pixels.resize(needed, 128);

    // Create PixelSlice
    let slice = match PixelSlice::new(&pixels, width, height, stride, descriptor) {
        Ok(s) => s,
        Err(_) => return,
    };

    // Encode
    let encode_result = EncodeRequest::new(format)
        .with_quality(quality)
        .with_registry(&AllowedFormats::all())
        .encode(slice, input.has_alpha);

    let encoded = match encode_result {
        Ok(output) => output,
        Err(_) => return,
    };

    // Decode the encoded output
    let limits = Limits::none()
        .with_max_width(4096)
        .with_max_height(4096)
        .with_max_pixels(4_000_000)
        .with_max_memory_bytes(64 * 1024 * 1024)
        .with_max_frames(50);
    let decoded = match DecodeRequest::new(encoded.data())
        .with_limits(&limits)
        .with_registry(&AllowedFormats::all())
        .decode_full_frame()
    {
        Ok(d) => d,
        Err(_) => {
            // Encoding succeeded but decoding failed — this is a bug.
            if !encoded.data().is_empty() {
                panic!(
                    "Encoded {} bytes as {:?} but failed to decode",
                    encoded.data().len(),
                    format
                );
            }
            return;
        }
    };

    // Verify dimensions survived the roundtrip.
    debug_assert_eq!(decoded.width(), width, "width mismatch after roundtrip");
    debug_assert_eq!(decoded.height(), height, "height mismatch after roundtrip");
});
