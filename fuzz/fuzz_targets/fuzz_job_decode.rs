//! Fuzz target: decode arbitrary bytes through zenpipe's ImageJob API.
//!
//! Feeds raw bytes as image input. Tests format detection and all enabled
//! decoders for panics, OOB reads, and unbounded allocations.

#![no_main]

use libfuzzer_sys::fuzz_target;
use zenpipe::job::ImageJob;
use zenpipe::limits::Limits;

/// Tight limits for fuzzing: 4096x4096, ~64MB.
fn fuzz_limits() -> Limits {
    Limits {
        max_pixels: Some(16_000_000),
        max_memory_bytes: Some(64 * 1024 * 1024),
        max_width: Some(4096),
        max_height: Some(4096),
        max_frames: Some(4),
        max_total_pixels: Some(64_000_000),
        max_duration: Some(core::time::Duration::from_secs(10)),
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }

    let job = ImageJob::new()
        .add_input_ref(0, data)
        .add_output(1)
        .with_limits(fuzz_limits());

    // We only care that it doesn't panic or trigger UB.
    let _ = job.run();
});
