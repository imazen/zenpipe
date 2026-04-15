//! Fuzz target: depth map extraction and resize.
//!
//! Tests depth map decoding from JPEG MPF secondary images, plus the
//! bilinear resize and format conversion operations.
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
    let result = DecodeRequest::new(data)
        .with_limits(&limits)
        .with_registry(&AllowedFormats::all())
        .decode_depth_map();

    if let Ok((_output, Some(depth))) = result {
        // Exercise the resize path (had integer overflow bug).
        let _ = depth.resize(64, 64);
        let _ = depth.resize(1, 1);
        let _ = depth.to_normalized_f32();
        let _ = depth.to_meters();
    }
});
