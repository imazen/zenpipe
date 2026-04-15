//! Fuzz target: animation frame iteration.
//!
//! Tests animated format decoders (GIF, WebP, APNG) by iterating frames
//! with a frame count cap. Exercises frame compositing, disposal methods,
//! and loop handling.
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
    let decoder = DecodeRequest::new(data)
        .with_limits(&limits)
        .with_registry(&AllowedFormats::all())
        .animation_frame_decoder();

    if let Ok(mut dec) = decoder {
        for _ in 0..100 {
            match dec.render_next_frame_owned(None) {
                Ok(Some(_frame)) => {}
                Ok(None) | Err(_) => break,
            }
        }
    }
});
