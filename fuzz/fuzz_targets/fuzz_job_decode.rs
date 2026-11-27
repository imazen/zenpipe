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
    Limits::NONE
        .with_max_pixels(16_000_000)
        .with_max_memory_bytes(64 * 1024 * 1024)
        .with_max_width(4096)
        .with_max_height(4096)
        .with_max_frames(4)
        .with_max_total_pixels(64_000_000)
        // Same ceiling as max_memory_bytes: an unbounded encode buffer is an
        // OOM the fuzzer would report as a crash, which is noise rather than a
        // finding. Added when `Limits` grew the field (9cb07998).
        .with_max_output_bytes(64 * 1024 * 1024)
        .with_max_duration(core::time::Duration::from_secs(10))
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
