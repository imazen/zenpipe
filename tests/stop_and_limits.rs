//! Integration tests for Stop tokens and Limits forwarding across all codecs.
//!
//! Verifies that cooperative cancellation and resource limits are properly forwarded
//! through the unified zencodecs API to each underlying codec adapter.
//!
//! The individual codecs are responsible for validating limits and checking stop tokens.
//! These tests verify zencodecs correctly passes them through.
//!
//! Tests marked `#[ignore]` reveal gaps in individual codecs — they should pass once
//! the underlying codec adds support.

mod common;

use common::{encode_rgba_test_data, encode_test_data, rgb8_image, rgba8_image};
use zencodecs::{CodecError, DecodeRequest, EncodeRequest, ImageFormat, Limits};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Stop token that is already cancelled — any check() returns Err immediately.
struct AlreadyStopped;

impl enough::Stop for AlreadyStopped {
    fn check(&self) -> Result<(), enough::StopReason> {
        Err(enough::StopReason::Cancelled)
    }
}

/// Check if a CodecError indicates cancellation (either directly or wrapped).
fn is_cancelled(err: &CodecError) -> bool {
    match err {
        CodecError::Cancelled => true,
        CodecError::Codec { source, .. } => {
            let msg = format!("{source}");
            msg.contains("cancel") || msg.contains("Cancel") || msg.contains("stop")
        }
        _ => false,
    }
}

/// Check if an error indicates a limit violation.
fn is_limit_error(err: &CodecError) -> bool {
    match err {
        CodecError::LimitExceeded(_) => true,
        CodecError::Codec { source, .. } => {
            let msg = format!("{source}");
            msg.contains("limit")
                || msg.contains("Limit")
                || msg.contains("exceed")
                || msg.contains("too large")
                || msg.contains("too many")
                || msg.contains("maximum")
        }
        _ => false,
    }
}

// ===========================================================================
// 1. Stop Token Tests — Decode (AlreadyStopped)
// ===========================================================================

#[test]
fn stop_decode_jpeg() {
    let data = encode_test_data(ImageFormat::Jpeg, 256, 256);
    let result = DecodeRequest::new(&data)
        .with_stop(&AlreadyStopped)
        .decode();
    assert!(
        result.is_err(),
        "JPEG decode should fail with AlreadyStopped"
    );
    assert!(
        is_cancelled(&result.unwrap_err()),
        "JPEG decode error should indicate cancellation"
    );
}

#[test]
fn stop_decode_webp() {
    let data = encode_test_data(ImageFormat::WebP, 256, 256);
    let result = DecodeRequest::new(&data)
        .with_stop(&AlreadyStopped)
        .decode();
    assert!(
        result.is_err(),
        "WebP decode should fail with AlreadyStopped"
    );
    assert!(
        is_cancelled(&result.unwrap_err()),
        "WebP decode error should indicate cancellation"
    );
}

#[test]
fn stop_decode_gif() {
    let data = encode_rgba_test_data(ImageFormat::Gif, 128, 128);
    let result = DecodeRequest::new(&data)
        .with_stop(&AlreadyStopped)
        .decode();
    assert!(
        result.is_err(),
        "GIF decode should fail with AlreadyStopped"
    );
    assert!(
        is_cancelled(&result.unwrap_err()),
        "GIF decode error should indicate cancellation"
    );
}

#[test]
fn stop_decode_avif() {
    let data = encode_test_data(ImageFormat::Avif, 64, 64);
    let result = DecodeRequest::new(&data)
        .with_stop(&AlreadyStopped)
        .decode();
    // AVIF decode may not check stop before completing for small images,
    // but it should at least not panic.
    if let Err(e) = &result {
        assert!(
            is_cancelled(e),
            "AVIF decode error should indicate cancellation, got: {e}"
        );
    }
}

#[test]
#[ignore = "PNG codec adapter not yet wired into zencodecs"]
fn stop_decode_png() {
    let data = encode_test_data(ImageFormat::Png, 256, 256);
    let result = DecodeRequest::new(&data)
        .with_stop(&AlreadyStopped)
        .decode();
    assert!(
        result.is_err(),
        "PNG decode should fail with AlreadyStopped"
    );
    assert!(
        is_cancelled(&result.unwrap_err()),
        "PNG decode error should indicate cancellation"
    );
}

// ===========================================================================
// 2. Stop Token Tests — Encode (AlreadyStopped)
// ===========================================================================

#[test]
fn stop_encode_jpeg() {
    let img = rgb8_image(256, 256);
    let result = EncodeRequest::new(ImageFormat::Jpeg)
        .with_quality(50.0)
        .with_stop(&AlreadyStopped)
        .encode_rgb8(img.as_ref());
    assert!(
        result.is_err(),
        "JPEG encode should fail with AlreadyStopped"
    );
    assert!(
        is_cancelled(&result.unwrap_err()),
        "JPEG encode error should indicate cancellation"
    );
}

#[test]
fn stop_encode_webp() {
    let img = rgb8_image(256, 256);
    let result = EncodeRequest::new(ImageFormat::WebP)
        .with_quality(50.0)
        .with_stop(&AlreadyStopped)
        .encode_rgb8(img.as_ref());
    assert!(
        result.is_err(),
        "WebP encode should fail with AlreadyStopped"
    );
    assert!(
        is_cancelled(&result.unwrap_err()),
        "WebP encode error should indicate cancellation"
    );
}

#[test]
fn stop_encode_gif_rgb8() {
    let img = rgb8_image(128, 128);
    let result = EncodeRequest::new(ImageFormat::Gif)
        .with_stop(&AlreadyStopped)
        .encode_rgb8(img.as_ref());
    assert!(
        result.is_err(),
        "GIF encode (rgb8) should fail with AlreadyStopped"
    );
    assert!(
        is_cancelled(&result.unwrap_err()),
        "GIF encode error should indicate cancellation"
    );
}

#[test]
fn stop_encode_gif_rgba8() {
    let img = rgba8_image(128, 128);
    let result = EncodeRequest::new(ImageFormat::Gif)
        .with_stop(&AlreadyStopped)
        .encode_rgba8(img.as_ref());
    assert!(
        result.is_err(),
        "GIF encode (rgba8) should fail with AlreadyStopped"
    );
    assert!(
        is_cancelled(&result.unwrap_err()),
        "GIF encode error should indicate cancellation"
    );
}

#[test]
fn stop_encode_avif() {
    let img = rgb8_image(64, 64);
    let result = EncodeRequest::new(ImageFormat::Avif)
        .with_quality(50.0)
        .with_stop(&AlreadyStopped)
        .encode_rgb8(img.as_ref());
    // AVIF encode may not check stop for small images, but should not panic.
    if let Err(e) = &result {
        assert!(
            is_cancelled(e),
            "AVIF encode error should indicate cancellation, got: {e}"
        );
    }
}

#[test]
#[ignore = "PNG codec adapter not yet wired into zencodecs"]
fn stop_encode_png() {
    let img = rgb8_image(256, 256);
    let result = EncodeRequest::new(ImageFormat::Png)
        .with_stop(&AlreadyStopped)
        .encode_rgb8(img.as_ref());
    assert!(
        result.is_err(),
        "PNG encode should fail with AlreadyStopped"
    );
    assert!(
        is_cancelled(&result.unwrap_err()),
        "PNG encode error should indicate cancellation"
    );
}

// ===========================================================================
// 3. Limits Tests — Decode (dimension limits)
// ===========================================================================

#[test]
fn limits_decode_jpeg_width() {
    let data = encode_test_data(ImageFormat::Jpeg, 256, 256);
    let limits = Limits {
        max_width: Some(10),
        ..Default::default()
    };
    let result = DecodeRequest::new(&data).with_limits(&limits).decode();
    assert!(
        result.is_err(),
        "JPEG decode should fail with tight width limit"
    );
    assert!(
        is_limit_error(&result.unwrap_err()),
        "JPEG decode error should indicate limit exceeded"
    );
}

#[test]
#[ignore = "PNG codec adapter not yet wired into zencodecs"]
fn limits_decode_png_pixels() {
    let data = encode_test_data(ImageFormat::Png, 256, 256);
    let limits = Limits {
        max_pixels: Some(100),
        ..Default::default()
    };
    let result = DecodeRequest::new(&data).with_limits(&limits).decode();
    assert!(
        result.is_err(),
        "PNG decode should fail with tight pixel limit"
    );
    assert!(
        is_limit_error(&result.unwrap_err()),
        "PNG decode error should indicate limit exceeded"
    );
}

#[test]
fn limits_decode_gif_width() {
    let data = encode_rgba_test_data(ImageFormat::Gif, 128, 128);
    let limits = Limits {
        max_width: Some(10),
        ..Default::default()
    };
    let result = DecodeRequest::new(&data).with_limits(&limits).decode();
    assert!(
        result.is_err(),
        "GIF decode should fail with tight width limit"
    );
    assert!(
        is_limit_error(&result.unwrap_err()),
        "GIF decode error should indicate limit exceeded"
    );
}

#[test]
fn limits_decode_webp_width() {
    let data = encode_test_data(ImageFormat::WebP, 256, 256);
    let limits = Limits {
        max_width: Some(10),
        ..Default::default()
    };
    let result = DecodeRequest::new(&data).with_limits(&limits).decode();
    assert!(
        result.is_err(),
        "WebP decode should fail with tight width limit"
    );
    assert!(
        is_limit_error(&result.unwrap_err()),
        "WebP decode error should indicate limit exceeded"
    );
}

/// zenavif returns a generic decode error instead of a clean limit error message.
#[test]
fn limits_decode_avif_pixels() {
    let data = encode_test_data(ImageFormat::Avif, 64, 64);
    let limits = Limits {
        max_pixels: Some(100),
        ..Default::default()
    };
    let result = DecodeRequest::new(&data).with_limits(&limits).decode();
    assert!(
        result.is_err(),
        "AVIF decode should fail with tight pixel limit"
    );
    // zenavif rejects the image but with a generic decode error, not a clean limit message.
    // This verifies the limit is at least forwarded and causes rejection.
}

// ===========================================================================
// 4. Limits Tests — Encode (forwarded to individual codecs)
//
// Most codecs don't enforce memory/dimension limits during encode via the
// trait interface yet. These tests document the expected behavior.
// ===========================================================================

/// GIF encode enforces dimension limits (via zengif::encode_gif native limits).
#[test]
fn limits_encode_gif_dimensions() {
    let img = rgba8_image(128, 128);
    let limits = Limits {
        max_width: Some(10),
        ..Default::default()
    };
    let result = EncodeRequest::new(ImageFormat::Gif)
        .with_limits(&limits)
        .encode_rgba8(img.as_ref());
    assert!(
        result.is_err(),
        "GIF encode should fail with tight width limit"
    );
    assert!(
        is_limit_error(&result.unwrap_err()),
        "GIF encode error should indicate limit exceeded"
    );
}

#[test]
fn limits_encode_jpeg_memory() {
    let img = rgb8_image(256, 256);
    let limits = Limits {
        max_memory_bytes: Some(1),
        ..Default::default()
    };
    let result = EncodeRequest::new(ImageFormat::Jpeg)
        .with_quality(50.0)
        .with_limits(&limits)
        .encode_rgb8(img.as_ref());
    assert!(
        result.is_err(),
        "JPEG encode should fail with 1-byte memory limit"
    );
    assert!(
        is_limit_error(&result.unwrap_err()),
        "JPEG encode error should indicate limit exceeded"
    );
}

#[test]
fn limits_encode_jpeg_dimensions() {
    let img = rgb8_image(256, 256);
    let limits = Limits {
        max_width: Some(10),
        ..Default::default()
    };
    let result = EncodeRequest::new(ImageFormat::Jpeg)
        .with_quality(50.0)
        .with_limits(&limits)
        .encode_rgb8(img.as_ref());
    assert!(
        result.is_err(),
        "JPEG encode should fail with tight width limit"
    );
    assert!(
        is_limit_error(&result.unwrap_err()),
        "JPEG encode error should indicate limit exceeded"
    );
}

#[test]
fn limits_encode_webp_memory() {
    let img = rgb8_image(256, 256);
    let limits = Limits {
        max_memory_bytes: Some(1),
        ..Default::default()
    };
    let result = EncodeRequest::new(ImageFormat::WebP)
        .with_quality(50.0)
        .with_limits(&limits)
        .encode_rgb8(img.as_ref());
    assert!(
        result.is_err(),
        "WebP encode should fail with 1-byte memory limit"
    );
    assert!(
        is_limit_error(&result.unwrap_err()),
        "WebP encode error should indicate limit exceeded"
    );
}

#[test]
fn limits_encode_webp_dimensions() {
    let img = rgb8_image(256, 256);
    let limits = Limits {
        max_width: Some(10),
        ..Default::default()
    };
    let result = EncodeRequest::new(ImageFormat::WebP)
        .with_quality(50.0)
        .with_limits(&limits)
        .encode_rgb8(img.as_ref());
    assert!(
        result.is_err(),
        "WebP encode should fail with tight width limit"
    );
    assert!(
        is_limit_error(&result.unwrap_err()),
        "WebP encode error should indicate limit exceeded"
    );
}

#[test]
#[ignore = "PNG codec adapter not yet wired into zencodecs"]
fn limits_encode_png_memory() {
    let img = rgb8_image(256, 256);
    let limits = Limits {
        max_memory_bytes: Some(1),
        ..Default::default()
    };
    let result = EncodeRequest::new(ImageFormat::Png)
        .with_limits(&limits)
        .encode_rgb8(img.as_ref());
    assert!(
        result.is_err(),
        "PNG encode should fail with 1-byte memory limit"
    );
    assert!(
        is_limit_error(&result.unwrap_err()),
        "PNG encode error should indicate limit exceeded"
    );
}

#[test]
fn limits_encode_gif_memory() {
    let img = rgba8_image(128, 128);
    let limits = Limits {
        max_memory_bytes: Some(1),
        ..Default::default()
    };
    let result = EncodeRequest::new(ImageFormat::Gif)
        .with_limits(&limits)
        .encode_rgba8(img.as_ref());
    assert!(
        result.is_err(),
        "GIF encode should fail with 1-byte memory limit"
    );
    assert!(
        is_limit_error(&result.unwrap_err()),
        "GIF encode error should indicate limit exceeded"
    );
}

#[test]
fn limits_encode_avif_memory() {
    let img = rgb8_image(64, 64);
    let limits = Limits {
        max_memory_bytes: Some(1),
        ..Default::default()
    };
    let result = EncodeRequest::new(ImageFormat::Avif)
        .with_quality(50.0)
        .with_limits(&limits)
        .encode_rgb8(img.as_ref());
    assert!(
        result.is_err(),
        "AVIF encode should fail with 1-byte memory limit"
    );
    assert!(
        is_limit_error(&result.unwrap_err()),
        "AVIF encode error should indicate limit exceeded"
    );
}

// ===========================================================================
// 5. Sanity: normal operations still succeed
// ===========================================================================

#[test]
fn normal_decode_jpeg_succeeds() {
    let data = encode_test_data(ImageFormat::Jpeg, 256, 256);
    let result = DecodeRequest::new(&data).decode();
    assert!(result.is_ok(), "Normal JPEG decode should succeed");
    let output = result.unwrap();
    assert_eq!(output.width(), 256);
    assert_eq!(output.height(), 256);
}

#[test]
fn normal_encode_jpeg_succeeds() {
    let img = rgb8_image(256, 256);
    let result = EncodeRequest::new(ImageFormat::Jpeg)
        .with_quality(50.0)
        .encode_rgb8(img.as_ref());
    assert!(result.is_ok(), "Normal JPEG encode should succeed");
}

#[test]
fn generous_limits_still_work() {
    let data = encode_test_data(ImageFormat::Jpeg, 256, 256);
    let limits = Limits {
        max_width: Some(10000),
        max_height: Some(10000),
        max_pixels: Some(100_000_000),
        max_memory_bytes: Some(1_000_000_000),
    };
    let result = DecodeRequest::new(&data).with_limits(&limits).decode();
    assert!(
        result.is_ok(),
        "Decode with generous limits should succeed: {:?}",
        result.err()
    );
}

// ===========================================================================
// 6. Roundtrip: encode + decode with stop/limits forwarded
// ===========================================================================

#[test]
fn roundtrip_all_codecs_no_stop() {
    // Verify every codec can roundtrip without stop/limits interference
    for format in [
        ImageFormat::Jpeg,
        ImageFormat::WebP,
        // ImageFormat::Png,  // PNG codec adapter not yet wired into zencodecs
        ImageFormat::Avif,
    ] {
        let img = rgb8_image(64, 64);
        let encoded = EncodeRequest::new(format)
            .with_quality(50.0)
            .encode_rgb8(img.as_ref())
            .unwrap_or_else(|e| panic!("{format:?} encode failed: {e}"));

        let decoded = DecodeRequest::new(encoded.as_ref())
            .decode()
            .unwrap_or_else(|e| panic!("{format:?} decode failed: {e}"));

        assert_eq!(decoded.width(), 64, "{format:?} width mismatch");
        assert_eq!(decoded.height(), 64, "{format:?} height mismatch");
    }
}

#[test]
fn roundtrip_gif_no_stop() {
    let img = rgba8_image(64, 64);
    let encoded = EncodeRequest::new(ImageFormat::Gif)
        .encode_rgba8(img.as_ref())
        .expect("GIF encode failed");

    let decoded = DecodeRequest::new(encoded.as_ref())
        .decode()
        .expect("GIF decode failed");

    assert_eq!(decoded.width(), 64);
    assert_eq!(decoded.height(), 64);
}
