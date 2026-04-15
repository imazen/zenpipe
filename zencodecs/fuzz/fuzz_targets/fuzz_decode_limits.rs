//! Fuzz target: limits enforcement with structured fuzzing.
//!
//! Generates both random image data and random limit configurations,
//! then asserts that limits are never violated on successful decode.
#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use zencodecs::{AllowedFormats, DecodeRequest, Limits};

#[derive(Debug, Arbitrary)]
struct FuzzInput {
    max_width: u16,
    max_height: u16,
    max_memory_kb: u16,
    max_frames: u8,
    data: Vec<u8>,
}

fuzz_target!(|input: FuzzInput| {
    let max_w = input.max_width.max(1);
    let max_h = input.max_height.max(1);
    let max_mem = (input.max_memory_kb.max(1) as u64) * 1024;
    let max_frames = input.max_frames.max(1) as u32;

    let limits = Limits::none()
        .with_max_width(max_w as u64)
        .with_max_height(max_h as u64)
        .with_max_pixels((max_w as u64) * (max_h as u64))
        .with_max_memory_bytes(max_mem)
        .with_max_frames(max_frames);

    let result = DecodeRequest::new(&input.data)
        .with_limits(&limits)
        .with_registry(&AllowedFormats::all())
        .decode_full_frame();

    if let Ok(output) = result {
        // Verify limits were actually enforced
        debug_assert!(
            output.width() <= max_w as u32,
            "width {} exceeds limit {}",
            output.width(),
            max_w
        );
        debug_assert!(
            output.height() <= max_h as u32,
            "height {} exceeds limit {}",
            output.height(),
            max_h
        );
    }
});
