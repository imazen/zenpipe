//! Fuzz target: format detection and header-only metadata parsing.
//!
//! Tests every format's header parser via the unified dispatch layer.
//! This is the fastest entry point and the first code path hit on every request.
#![no_main]

use libfuzzer_sys::fuzz_target;
use zencodecs::{AllowedFormats, probe};

fuzz_target!(|data: &[u8]| {
    let _ = probe(data, &AllowedFormats::all());
});
