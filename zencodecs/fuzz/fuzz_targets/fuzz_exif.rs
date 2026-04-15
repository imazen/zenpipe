//! Fuzz target: EXIF/TIFF IFD parser.
//!
//! Tests the custom EXIF parser on raw untrusted bytes. EXIF parsing
//! involves manual offset arithmetic and has historically been a rich
//! source of vulnerabilities in image libraries.
#![no_main]

use libfuzzer_sys::fuzz_target;
use zencodecs::exif::parse_exif;

fuzz_target!(|data: &[u8]| {
    // Attempt to parse as EXIF data (handles both JPEG-style with Exif\0\0 prefix
    // and raw TIFF bytes used by PNG/AVIF/HEIC).
    let _ = parse_exif(data);
});
