//! Trace quality tests — verify that codec errors carry full `whereat` stack
//! traces across the zenpipe → zencodecs → <codec crate> boundary chain.
//!
//! Each test feeds a format-recognized-but-truncated blob through `ImageJob`,
//! then inspects the resulting `At<PipeError>` to verify:
//!
//! - `frame_count()` is in a plausible range (1..=20)
//! - `full_trace()` names the originating codec crate (for instrumented codecs)
//! - `full_trace()` names "zencodecs" and "zenpipe" boundary crossings
//!
//! This guards against regressions where `at_crate!()` + `map_err_at()` get
//! replaced by `map_err(|e| at!(...))`, which would discard the cross-crate trace
//! and flatten everything to a single frame at the conversion site.
//!
//! Run: `cargo test --features "job,nodes-all" --test trace_quality`

#![cfg(all(feature = "job", feature = "std"))]

use zenpipe::PipeError;
use zenpipe::job::ImageJob;

// ── Minimal corpus of corrupt-but-recognized images ──────────────────────────
//
// Each constant has the correct magic bytes for format detection but is
// truncated before any pixel / dimension data so the codec probe fails.

/// PNG magic + zero-length tEXt ancillary chunk (bit 5 of first byte set = lowercase 't').
/// First chunk is not IHDR → probe_png fires the instrumented `at!(PngError::Decode(...))` path.
const TRUNCATED_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG magic
    0x00, 0x00, 0x00, 0x00, // chunk length = 0
    0x74, 0x45, 0x58, 0x74, // chunk type "tEXt" (ancillary — lowercase first byte)
    0x00, 0x00, 0x00, 0x00, // CRC (wrong; ancillary chunks with bad CRC are silently skipped)
];

/// JPEG SOI + APP0 marker, truncated before segment length can be read.
const TRUNCATED_JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46];

/// GIF89a logical-screen descriptor, truncated before the colour table.
const TRUNCATED_GIF: &[u8] = b"GIF89a\x04\x00\x04\x00\x00\x00\x00";

/// RIFF/WEBP with a VP8 lossy chunk whose keyframe sync bytes are wrong.
/// Fires the instrumented `at!(DecodeError::Vp8MagicInvalid(...))` path in zenwebp.
const TRUNCATED_WEBP: &[u8] = &[
    b'R', b'I', b'F', b'F', 0x12, 0x00, 0x00,
    0x00, // RIFF size = 18 (WEBP + VP8 chunk header + 6-byte payload)
    b'W', b'E', b'B', b'P', b'V', b'P', b'8', b' ', // VP8 lossy chunk
    0x06, 0x00, 0x00, 0x00, // VP8 chunk size = 6
    0x00, 0x00, 0x00, // VP8 frame tag (bit 0 = 0 → keyframe)
    0x00, 0x00, 0x00, // WRONG sync code (should be 0x9D 0x01 0x2A)
];

/// Minimal AVIF ftyp box (major brand "avif"), nothing after it.
const TRUNCATED_AVIF: &[u8] = &[
    0x00, 0x00, 0x00, 0x18, b'f', b't', b'y', b'p', b'a', b'v', b'i', b'f', 0x00, 0x00, 0x00,
    0x00, // minor version
    b'a', b'v', b'i', b'f', b'm', b'i', b'f', b'1', // compat brands
];

/// JXL codestream magic (FF 0A) plus zeroed padding — truncated SizeHeader.
const TRUNCATED_JXL: &[u8] = &[0xFF, 0x0A, 0x00, 0x00, 0x00, 0x00];

/// Minimal HEIC ftyp box (major brand "heic"), nothing after it.
#[cfg(feature = "nodes-heic")]
const TRUNCATED_HEIC: &[u8] = &[
    0x00, 0x00, 0x00, 0x18, b'f', b't', b'y', b'p', b'h', b'e', b'i', b'c', 0x00, 0x00, 0x00, 0x00,
    b'h', b'e', b'i', b'c', b'm', b'i', b'f', b'1',
];

/// BMP magic + file-header stub, no DIB header.
const TRUNCATED_BMP: &[u8] = &[
    b'B', b'M', 0x1A, 0x00, 0x00, 0x00, // file size
    0x00, 0x00, 0x00, 0x00, // reserved
    0x36, 0x00, 0x00, 0x00, // pixel data offset = 54
];

// ── Helpers ───────────────────────────────────────────────────────────────────

fn run_job(data: &[u8]) -> zenpipe::PipeResult<zenpipe::job::JobResult> {
    ImageJob::new()
        .add_input(0, data.to_vec())
        .add_output(1)
        .run()
}

/// Assert trace quality on the `At<PipeError>` returned from a failed job.
///
/// - `frame_count()` in `1..=20`
/// - `full_trace()` text contains every entry in `expected_in_trace`
fn assert_trace(result: zenpipe::PipeResult<zenpipe::job::JobResult>, expected_in_trace: &[&str]) {
    let err = result.expect_err("expected error from truncated/corrupt data");

    // Inherent methods on At<PipeError> — no import needed.
    let trace_str = std::format!("{}", err.full_trace());
    let frames = err.frame_count();

    assert!(
        (1..=20).contains(&frames),
        "frame_count {frames} not in 1..=20\nfull trace:\n{trace_str}"
    );

    for &name in expected_in_trace {
        assert!(
            trace_str.contains(name),
            "expected '{name}' in trace\nfull trace:\n{trace_str}"
        );
    }
}

// ── Tests: instrumented codecs (return At<E>, so origin frames are preserved) ─

/// zenpng is instrumented — trace must include zenpng + both boundary crates.
#[test]
fn png_trace_includes_zenpng_and_boundaries() {
    assert_trace(run_job(TRUNCATED_PNG), &["zenpng", "zencodecs", "zenpipe"]);
}

/// zengif is instrumented.
#[test]
fn gif_trace_includes_zengif_and_boundaries() {
    assert_trace(run_job(TRUNCATED_GIF), &["zengif", "zencodecs", "zenpipe"]);
}

/// zenwebp is instrumented.
#[test]
fn webp_trace_includes_zenwebp_and_boundaries() {
    assert_trace(
        run_job(TRUNCATED_WEBP),
        &["zenwebp", "zencodecs", "zenpipe"],
    );
}

/// zenavif is instrumented.
#[test]
fn avif_trace_includes_zenavif_and_boundaries() {
    assert_trace(
        run_job(TRUNCATED_AVIF),
        &["zenavif", "zencodecs", "zenpipe"],
    );
}

/// zenjxl is instrumented.
#[test]
fn jxl_trace_includes_zenjxl_and_boundaries() {
    assert_trace(run_job(TRUNCATED_JXL), &["zenjxl", "zencodecs", "zenpipe"]);
}

/// heic is instrumented.
#[cfg(feature = "nodes-heic")]
#[test]
fn heic_trace_includes_heic_and_boundaries() {
    assert_trace(run_job(TRUNCATED_HEIC), &["heic", "zencodecs", "zenpipe"]);
}

// ── Tests: non-instrumented codecs (return plain E, no origin frames) ─────────
//
// For these codecs the trace starts at the zencodecs adapter that wraps the
// error with at!(...).  The two boundary crates are always present.

/// zenjpeg is NOT instrumented — bare errors get wrapped with `at!()` at the zencodecs
/// adapter, creating exactly 1 frame there. Crate boundary context is attached to that
/// frame rather than creating new frames, so frame_count is ≥ 1.
#[test]
fn jpeg_trace_has_at_least_one_frame_and_boundaries() {
    let err = run_job(TRUNCATED_JPEG).expect_err("truncated JPEG should fail");

    let trace_str = std::format!("{}", err.full_trace());
    let frames = err.frame_count();

    assert!(
        frames >= 1,
        "expected ≥1 frame for JPEG (zencodecs), got {frames}\nfull trace:\n{trace_str}"
    );
    assert!(
        frames <= 20,
        "unexpectedly deep trace: {frames}\nfull trace:\n{trace_str}"
    );

    for name in ["zencodecs", "zenpipe"] {
        assert!(
            trace_str.contains(name),
            "expected '{name}' in JPEG trace\nfull trace:\n{trace_str}"
        );
    }
}

/// zenbitmaps is NOT instrumented — bare errors get wrapped with `at!()` at the zencodecs
/// adapter, creating exactly 1 frame there. frame_count is ≥ 1.
#[test]
fn bmp_trace_has_at_least_one_frame_and_boundaries() {
    let err = run_job(TRUNCATED_BMP).expect_err("truncated BMP should fail");

    let trace_str = std::format!("{}", err.full_trace());
    let frames = err.frame_count();

    assert!(
        frames >= 1,
        "expected ≥1 frame for BMP (zencodecs), got {frames}\nfull trace:\n{trace_str}"
    );
    assert!(
        frames <= 20,
        "unexpectedly deep trace: {frames}\nfull trace:\n{trace_str}"
    );

    for name in ["zencodecs", "zenpipe"] {
        assert!(
            trace_str.contains(name),
            "expected '{name}' in BMP trace\nfull trace:\n{trace_str}"
        );
    }
}

// ── Sanity: At<PipeError> variant ─────────────────────────────────────────────

/// All codec errors should arrive as PipeError::Codec, not Op(string).
/// This confirms map_err_at(|e| PipeError::Codec(Box::new(e))) is wired up.
#[test]
fn codec_errors_use_codec_variant_not_op_string() {
    for (name, data) in [
        ("PNG", TRUNCATED_PNG.as_ref()),
        ("GIF", TRUNCATED_GIF.as_ref()),
        ("WebP", TRUNCATED_WEBP.as_ref()),
        ("AVIF", TRUNCATED_AVIF.as_ref()),
        ("JXL", TRUNCATED_JXL.as_ref()),
        ("JPEG", TRUNCATED_JPEG.as_ref()),
        ("BMP", TRUNCATED_BMP.as_ref()),
    ] {
        let err = run_job(data).expect_err(&std::format!("{name} should fail"));
        assert!(
            matches!(err.error(), PipeError::Codec(_)),
            "{name}: expected PipeError::Codec, got {:?}",
            err.error()
        );
    }

    #[cfg(feature = "nodes-heic")]
    {
        let err = run_job(TRUNCATED_HEIC).expect_err("HEIC should fail");
        assert!(
            matches!(err.error(), PipeError::Codec(_)),
            "HEIC: expected PipeError::Codec, got {:?}",
            err.error()
        );
    }
}
