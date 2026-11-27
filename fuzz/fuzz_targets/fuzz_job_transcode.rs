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
    Limits::NONE
        .with_max_pixels(16_000_000)
        .with_max_memory_bytes(64 * 1024 * 1024)
        .with_max_width(4096)
        .with_max_height(4096)
        .with_max_frames(4)
        .with_max_total_pixels(16_000_000 * 4)
        // Same ceiling as max_memory_bytes: an unbounded encode buffer is an
        // OOM the fuzzer would report as a crash, which is noise rather than a
        // finding. Added when `Limits` grew the field (9cb07998).
        .with_max_output_bytes(64 * 1024 * 1024)
        .with_max_duration(core::time::Duration::from_secs(10))
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

    let intent = CodecIntent::new()
        .with_format(FormatChoice::Specific(format))
        .with_quality_fallback((input.quality_byte.min(100)) as f32);

    let job = ImageJob::new()
        .add_input_ref(0, &input.image_data)
        .add_output(1)
        .with_limits(fuzz_limits())
        .with_intent(intent);

    let _ = job.run();
});
