//! Fuzz target: full decode→encode transcode pipeline.
//!
//! Uses structured fuzzing to vary the target format and quality,
//! testing pixel format negotiation between decoder output and encoder input.
#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use zencodecs::quality::QualityIntent;
use zencodecs::transcode::{TranscodeOptions, transcode};
use zencodecs::{AllowedFormats, FormatDecision, ImageFormat};

#[derive(Debug, Arbitrary)]
struct TranscodeInput {
    format_selector: u8,
    quality: u8,
    data: Vec<u8>,
}

fuzz_target!(|input: TranscodeInput| {
    let target_format = match input.format_selector % 5 {
        0 => ImageFormat::Jpeg,
        1 => ImageFormat::WebP,
        2 => ImageFormat::Png,
        3 => ImageFormat::Gif,
        _ => ImageFormat::Jpeg,
    };
    let quality = (input.quality as f32).clamp(1.0, 100.0);

    let decision = FormatDecision {
        format: target_format,
        quality: QualityIntent::from_quality(quality),
        ..Default::default()
    };

    let _ = transcode(&input.data, &decision, &TranscodeOptions::default(), &AllowedFormats::all());
});
