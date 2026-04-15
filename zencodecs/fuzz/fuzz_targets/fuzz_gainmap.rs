//! Fuzz target: gain map extraction.
//!
//! Tests UltraHDR MPF/XMP parsing, secondary image decode, and gain map
//! metadata extraction from JPEG, AVIF, and JXL inputs.
#![no_main]

use libfuzzer_sys::fuzz_target;
use zencodecs::{AllowedFormats, DecodeRequest, Limits};

fuzz_target!(|data: &[u8]| {
    let limits = Limits::none()
        .with_max_width(4096)
        .with_max_height(4096)
        .with_max_pixels(4_000_000)
        .with_max_memory_bytes(64 * 1024 * 1024)
        .with_max_frames(50);
    let _ = DecodeRequest::new(data)
        .with_limits(&limits)
        .with_registry(&AllowedFormats::all())
        .decode_gain_map();
});
