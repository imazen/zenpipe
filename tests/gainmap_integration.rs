//! Cross-format gain map integration tests.
//!
//! Tests that gain map metadata is correctly parsed across formats and that
//! the log2/linear domain conversion is correct.

#![cfg(all(feature = "jpeg-ultrahdr", feature = "avif-decode"))]

use zencodecs::DecodeRequest;

/// Decode a file from disk and extract gain map.
fn decode_with_gainmap(path: &str) -> (zencodecs::DecodeOutput, Option<zencodecs::DecodedGainMap>) {
    let data = std::fs::read(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    DecodeRequest::new(&data)
        .decode_gain_map()
        .unwrap_or_else(|e| panic!("decode_gain_map failed for {path}: {e}"))
}

// ─── JPEG UltraHDR ──────────────────────────────────────────────────────────

#[test]
fn jpeg_ultrahdr_seine_gainmap() {
    let path = "/mnt/v/input/gainmap-samples/jpeg/seine_sdr_gainmap_srgb.jpg";
    if !std::path::Path::new(path).exists() {
        eprintln!("SKIP: {path} not available");
        return;
    }

    let (_output, gainmap) = decode_with_gainmap(path);
    let gm = gainmap.expect("JPEG UltraHDR must have a gain map");

    assert!(!gm.base_is_hdr, "JPEG: base should be SDR");
    assert!(gm.gain_map.validate().is_ok());
    assert!(gm.gain_map.width > 0);
    assert!(gm.gain_map.height > 0);

    // Verify metadata is sane
    let meta = &gm.metadata;
    assert!(meta.max_content_boost[0] >= 1.0, "max_content_boost should be >= 1.0 (linear)");
    assert!(meta.hdr_capacity_max >= 1.0, "hdr_capacity_max should be >= 1.0 (linear)");

    // Verify canonical params are in log2 domain
    let params = gm.params();
    assert!(params.channels[0].max >= 0.0, "log2 max should be >= 0");
    assert!(params.validate().is_ok());
}

// ─── AVIF ────────────────────────────────────────────────────────────────────

#[test]
fn avif_seine_gainmap() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../zenavif/tests/vectors/libavif/seine_sdr_gainmap_srgb.avif"
    );
    if !std::path::Path::new(path).exists() {
        eprintln!("SKIP: {path} not available");
        return;
    }

    let (_output, gainmap) = decode_with_gainmap(path);
    let gm = gainmap.expect("AVIF seine must have a gain map");

    assert!(!gm.base_is_hdr, "AVIF: base should be SDR");
    assert!(gm.gain_map.validate().is_ok());

    // Verify metadata is in LINEAR domain (ultrahdr's convention)
    let meta = &gm.metadata;
    assert!(
        meta.max_content_boost[0] >= 1.0,
        "max_content_boost should be >= 1.0 (linear), got {}",
        meta.max_content_boost[0],
    );
    assert!(
        meta.hdr_capacity_max >= 1.0,
        "hdr_capacity_max should be >= 1.0 (linear), got {}",
        meta.hdr_capacity_max,
    );

    // Verify canonical params are in log2 domain
    let params = gm.params();
    assert!(params.validate().is_ok());
}

/// AVIF regression: headroom 13/10 in ISO 21496-1 is log2 1.3,
/// which must produce LINEAR 2^1.3 ≈ 2.462, NOT 1.3.
#[test]
fn avif_headroom_log2_linear_regression() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../zenavif/tests/vectors/libavif/seine_sdr_gainmap_srgb.avif"
    );
    if !std::path::Path::new(path).exists() {
        eprintln!("SKIP: {path} not available");
        return;
    }

    let (_output, gainmap) = decode_with_gainmap(path);
    let gm = gainmap.expect("AVIF seine must have a gain map");

    let meta = &gm.metadata;
    let params = gm.params();

    // The seine file has alternate_hdr_headroom = 13/10 = 1.3 (log2 domain)
    // This was confirmed from zenavif-parse tests.
    // In log2 domain (GainMapParams): alternate_hdr_headroom ≈ 1.3
    // In linear domain (GainMapMetadata): hdr_capacity_max ≈ 2^1.3 ≈ 2.462

    // The linear value must NOT be ~1.3 (that was the old bug)
    assert!(
        meta.hdr_capacity_max > 2.0,
        "hdr_capacity_max should be 2^1.3 ≈ 2.46, NOT the raw log2 value 1.3. Got: {}",
        meta.hdr_capacity_max,
    );

    // Verify the log2 value is approximately 1.3
    assert!(
        (params.alternate_hdr_headroom - 1.3).abs() < 0.01,
        "log2 alternate_hdr_headroom should be ~1.3, got {}",
        params.alternate_hdr_headroom,
    );

    // Verify the linear value is 2^1.3
    let expected_linear = 2.0f32.powf(1.3);
    assert!(
        (meta.hdr_capacity_max - expected_linear).abs() < 0.1,
        "linear hdr_capacity_max should be ~{expected_linear}, got {}",
        meta.hdr_capacity_max,
    );
}

/// Cross-format: JPEG and AVIF seine scene both have gain maps with valid params.
#[test]
fn cross_format_seine_both_have_gain_maps() {
    let jpeg_path = "/mnt/v/input/gainmap-samples/jpeg/seine_sdr_gainmap_srgb.jpg";
    let avif_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../zenavif/tests/vectors/libavif/seine_sdr_gainmap_srgb.avif"
    );

    if !std::path::Path::new(jpeg_path).exists() || !std::path::Path::new(avif_path).exists() {
        eprintln!("SKIP: seine test files not available");
        return;
    }

    let (_, jpeg_gm) = decode_with_gainmap(jpeg_path);
    let (_, avif_gm) = decode_with_gainmap(avif_path);

    let jpeg_params = jpeg_gm.expect("JPEG must have gain map").params();
    let avif_params = avif_gm.expect("AVIF must have gain map").params();

    // Both should validate
    assert!(jpeg_params.validate().is_ok(), "JPEG params should be valid");
    assert!(avif_params.validate().is_ok(), "AVIF params should be valid");

    // Both should have SDR base direction
    assert_eq!(
        jpeg_params.direction(),
        zencodecs::GainMapDirection::BaseIsSdr,
    );
    assert_eq!(
        avif_params.direction(),
        zencodecs::GainMapDirection::BaseIsSdr,
    );

    // AVIF should have non-trivial headroom (log2 > 0 means HDR alternate)
    assert!(
        avif_params.alternate_hdr_headroom > 0.0,
        "AVIF alternate headroom should be > 0 (HDR)"
    );
}
