//! Fuzz target: format auto-selection logic.
//!
//! Uses structured fuzzing to exercise the format selection engine with
//! arbitrary ImageFacts and QualityIntent combinations. Pure logic — no I/O.
#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use zencodecs::policy::CodecPolicy;
use zencodecs::quality::QualityIntent;
use zencodecs::select::ImageFacts;
use zencodecs::{AllowedFormats, ImageFormat};

#[derive(Debug, Arbitrary)]
struct SelectInput {
    has_alpha: bool,
    has_animation: bool,
    is_lossless_source: bool,
    pixel_count: u32,
    is_hdr: bool,
    quality: u8,
    lossless: bool,
    source_format: u8,
}

fuzz_target!(|input: SelectInput| {
    let source_format = match input.source_format % 7 {
        0 => Some(ImageFormat::Jpeg),
        1 => Some(ImageFormat::WebP),
        2 => Some(ImageFormat::Png),
        3 => Some(ImageFormat::Gif),
        4 => Some(ImageFormat::Avif),
        5 => Some(ImageFormat::Jxl),
        _ => None,
    };

    let facts = ImageFacts {
        has_alpha: input.has_alpha,
        has_animation: input.has_animation,
        is_lossless_source: input.is_lossless_source,
        pixel_count: input.pixel_count as u64,
        is_hdr: input.is_hdr,
        source_format,
    };

    let quality = (input.quality as f32).clamp(1.0, 100.0);
    let intent = QualityIntent::from_quality(quality).with_lossless(input.lossless);
    let policy = CodecPolicy::default();

    let _ = zencodecs::select::select_format(&facts, &intent, &AllowedFormats::all(), &policy);
});
