//! Fuzz target: decode + re-encode through zenpipe's ImageJob API.
//!
//! Structured fuzzing: arbitrary image bytes + random output format + quality.
//! Tests the full decode -> encode path including pixel format conversion.

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use zenpipe::job::ImageJob;
use zenpipe::limits::Limits;

#[derive(Debug, Arbitrary)]
enum FuzzFormat {
    Jpeg,
    Png,
    WebP,
    Gif,
}

#[derive(Debug, Arbitrary)]
struct FuzzInput {
    image_data: Vec<u8>,
    output_format: FuzzFormat,
    quality_byte: u8,
}

fn fuzz_limits() -> Limits {
    Limits {
        max_pixels: Some(16_000_000),
        max_memory_bytes: Some(64 * 1024 * 1024),
        max_width: Some(4096),
        max_height: Some(4096),
        max_frames: Some(4),
        max_total_pixels: Some(16_000_000 * 4),
        max_duration: Some(core::time::Duration::from_secs(10)),
    }
}

fuzz_target!(|input: FuzzInput| {
    if input.image_data.len() < 8 {
        return;
    }

    use zencodecs::{CodecIntent, FormatChoice, ImageFormat};

    let format = match input.output_format {
        FuzzFormat::Jpeg => ImageFormat::Jpeg,
        FuzzFormat::Png => ImageFormat::Png,
        FuzzFormat::WebP => ImageFormat::WebP,
        FuzzFormat::Gif => ImageFormat::Gif,
    };

    let intent = CodecIntent {
        format: Some(FormatChoice::Specific(format)),
        quality_fallback: Some((input.quality_byte.min(100)) as f32),
        ..Default::default()
    };

    let job = ImageJob::new()
        .add_input_ref(0, &input.image_data)
        .add_output(1)
        .with_limits(fuzz_limits())
        .with_intent(intent);

    let _ = job.run();
});
