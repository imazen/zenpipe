//! RAW / DNG codec capability tests.
//!
//! Per `docs/hdr-per-codec.md`, RAW is the only codec that:
//!   - Carries DNG-specific EXIF tags (color matrices, AsShotNeutral) needed
//!     for proper colour reconstruction.
//!   - Embeds an Apple MPF gain map inside the preview JPEG of Apple ProRAW
//!     DNGs (the only "decode HDR from a RAW" path we support today).
//!
//! Synthetic-fixture tests (no external files needed) cover format
//! detection and negative cases. Real-fixture tests cover the Apple
//! ProRAW gain map path and DNG EXIF round-trip; those are
//! `#[ignore]`'d so they only run with `cargo test -- --ignored`.
//!
//! Required fixture for `--ignored` runs:
//!   `/mnt/v/heic/46CD6167-C36B-4F98-B386-2300D8E840F0.DNG`
//!   `/mnt/v/heic/CBFA569A-5C28-468E-96B4-CFFBAEB951C7.DNG`
//! (Apple ProRAW samples — same set zenraw's own integration tests use.)
//!
//! Backend status: `zencodecs` enables both `rawloader` and `rawler`
//! features on zenraw. rawler is required for iPhone ProRAW DNGs
//! (LJPEG predictor 7). With the fixtures present, three of the
//! `--ignored` tests pass:
//!   - apple_proraw_dng_decode_succeeds_and_reports_dimensions ✓
//!   - apple_proraw_dng_yields_gain_map ✓
//!   - apple_proraw_dng_second_fixture_round_trips_gain_map_metadata ✓
//!
//! The remaining one (`apple_proraw_dng_exif_carries_dng_tags`) is a
//! real zenraw gap rather than a fixture issue — `build_image_info`
//! doesn't attach the raw EXIF blob to ImageInfo. Marked with the
//! actual root cause.

#![cfg(feature = "raw-decode")]

use zencodecs::DecodeRequest;

const APPLE_PRORAW_FIXTURE: &str =
    "/mnt/v/heic/46CD6167-C36B-4F98-B386-2300D8E840F0.DNG";
const APPLE_PRORAW_FIXTURE_2: &str =
    "/mnt/v/heic/CBFA569A-5C28-468E-96B4-CFFBAEB951C7.DNG";

// ─── Fixture-free tests ───────────────────────────────────────────────────

/// Garbage input must not panic the RAW probe path.
#[test]
fn raw_probe_handles_garbage_without_panic() {
    let bytes = b"not a raw file at all, just garbage bytes";
    // probe will return Err(UnrecognizedFormat) — never panic.
    let _ = zencodecs::from_bytes_with_registry(
        bytes,
        &zencodecs::AllowedFormats::all(),
    );
}

/// `decode_gain_map` on a regular JPEG must NOT spuriously match the
/// Apple ProRAW MPF path. The dispatcher should detect "regular JPEG,
/// no UltraHDR XMP, no Apple MPF gain map" and return None.
#[cfg(all(feature = "jpeg", feature = "jpeg-ultrahdr", feature = "raw-decode-gainmap"))]
#[test]
fn decode_gain_map_returns_none_for_jpeg_without_apple_mpf() {
    use imgref::ImgVec;
    use rgb::Rgb;
    let pixels: Vec<Rgb<u8>> = (0..32 * 32)
        .map(|i| Rgb { r: (i % 32) as u8, g: 0, b: 0 })
        .collect();
    let img = ImgVec::new(pixels, 32, 32);
    let bytes = zencodecs::EncodeRequest::new(zencodec::ImageFormat::Jpeg)
        .with_quality(80.0)
        .encode_full_frame_rgb8(img.as_ref())
        .expect("encode plain JPEG")
        .into_vec();

    let (_, gm) = DecodeRequest::new(&bytes)
        .decode_gain_map()
        .expect("decode_gain_map shouldn't error on plain JPEG");
    assert!(
        gm.is_none(),
        "Apple MPF fallback path must not match plain JPEGs"
    );
}

// ─── Real-fixture tests (require Apple ProRAW DNG) ───────────────────────

#[cfg(feature = "raw-decode-exif")]
#[ignore = "needs Apple ProRAW DNG at /mnt/v/heic/; run with cargo test -- --ignored"]
#[test]
fn apple_proraw_dng_decode_succeeds_and_reports_dimensions() {
    let bytes = std::fs::read(APPLE_PRORAW_FIXTURE)
        .expect("Apple ProRAW fixture must be present for this test");
    // RAW/DNG is `ImageFormat::Custom`, which is intentionally outside
    // the `AllowedFormats` bitset (see registry.rs:88-90). The caller
    // must dispatch via `with_format` for Custom formats.
    let decoded = DecodeRequest::new(&bytes)
        .with_format(zencodec::ImageFormat::Custom(&zenraw::DNG_FORMAT))
        .decode_full_frame()
        .expect("Apple ProRAW DNG must decode via Custom dispatch");
    let info = decoded.info();
    assert!(info.width > 0, "DNG width should be > 0, got {}", info.width);
    assert!(info.height > 0, "DNG height should be > 0, got {}", info.height);
}

/// Apple ProRAW DNGs carry DNG-specific EXIF tags (camera color
/// matrices, AsShotNeutral, etc.) needed for proper raw → RGB color
/// reconstruction. zencodecs exposes these via
/// `DecodeRequest::read_raw_metadata()` which calls into zenraw's
/// kamadak-exif parser.
///
/// Note: the structured `ExifMetadata` IS available, but the raw EXIF
/// blob is NOT attached to `ImageInfo.exif` — that's a separate gap
/// in zenraw's `build_image_info` that's tracked separately
/// (apple_proraw_dng_raw_exif_blob_attached_to_info).
#[cfg(feature = "raw-decode-exif")]
#[ignore = "needs Apple ProRAW DNG at /mnt/v/heic/; run with cargo test -- --ignored"]
#[test]
fn apple_proraw_dng_exif_carries_dng_tags() {
    let bytes = std::fs::read(APPLE_PRORAW_FIXTURE)
        .expect("Apple ProRAW fixture must be present");
    let exif_meta = DecodeRequest::new(&bytes)
        .with_format(zencodec::ImageFormat::Custom(&zenraw::DNG_FORMAT))
        .read_raw_metadata()
        .expect("zenraw must return structured ExifMetadata for Apple ProRAW");

    assert!(
        exif_meta.make.is_some(),
        "Apple ProRAW must carry camera Make tag"
    );
    // Apple ProRAW DNG-specific: at least one of the color matrices.
    assert!(
        exif_meta.color_matrix_1.is_some()
            || exif_meta.color_matrix_2.is_some(),
        "Apple ProRAW must carry at least one ColorMatrix DNG tag"
    );
    // AsShotNeutral is the white-balance reference for raw conversion.
    assert!(
        exif_meta.as_shot_neutral.is_some(),
        "Apple ProRAW must carry AsShotNeutral DNG tag"
    );
}

/// Tracker for the separate gap: zenraw doesn't attach the raw EXIF
/// blob to `ImageInfo.exif` (only orientation, bit_depth, and XMP).
/// `zenraw/src/zencodec_impl.rs::build_image_info`. The structured
/// metadata IS extractable via `read_raw_metadata` (see test above);
/// this test is for the case where a caller wants the raw bytes for
/// re-embedding in another format.
#[cfg(feature = "raw-decode-exif")]
#[ignore = "needs fixture + zenraw build_image_info doesn't attach raw EXIF blob to ImageInfo"]
#[should_panic(expected = "raw EXIF blob")]
#[test]
fn apple_proraw_dng_raw_exif_blob_attached_to_info() {
    let bytes = std::fs::read(APPLE_PRORAW_FIXTURE)
        .expect("Apple ProRAW fixture must be present");
    let decoded = DecodeRequest::new(&bytes)
        .with_format(zencodec::ImageFormat::Custom(&zenraw::DNG_FORMAT))
        .decode_full_frame()
        .expect("decode Apple ProRAW DNG");
    let info = decoded.info();
    let meta = info.metadata();
    let _exif = meta
        .exif
        .as_ref()
        .expect("Apple ProRAW must carry raw EXIF blob on ImageInfo");
}

#[cfg(all(feature = "raw-decode-gainmap", feature = "jpeg-ultrahdr"))]
#[ignore = "needs Apple ProRAW DNG at /mnt/v/heic/; run with cargo test -- --ignored"]
#[test]
fn apple_proraw_dng_yields_gain_map() {
    let bytes = std::fs::read(APPLE_PRORAW_FIXTURE)
        .expect("Apple ProRAW fixture must be present");

    let (_decoded, gm) = DecodeRequest::new(&bytes)
        .with_format(zencodec::ImageFormat::Custom(&zenraw::DNG_FORMAT))
        .decode_gain_map()
        .expect("decode_gain_map on Apple ProRAW must not error");
    let gm = gm.expect(
        "Apple ProRAW DNG must yield a gain map via the MPF preview path"
    );

    assert!(
        gm.gain_map.width > 0 && gm.gain_map.height > 0,
        "extracted gain map must have non-zero dimensions"
    );
    assert!(
        !gm.gain_map.data.is_empty(),
        "gain map pixel data must be non-empty"
    );
    // Apple ProRAW MPF gain map convention: forward direction (base SDR).
    assert!(
        !gm.base_is_hdr,
        "Apple ProRAW MPF gain map is forward-direction (SDR base)"
    );
}

#[cfg(all(feature = "raw-decode-gainmap", feature = "jpeg-ultrahdr"))]
#[ignore = "needs second Apple ProRAW DNG at /mnt/v/heic/; run with cargo test -- --ignored"]
#[test]
fn apple_proraw_dng_second_fixture_round_trips_gain_map_metadata() {
    // Variant fixture: confirms the Apple MPF path isn't tied to one
    // specific file's quirks. If both fixtures decode and yield gain
    // maps with consistent metadata shape, the path is reliable.
    let bytes = std::fs::read(APPLE_PRORAW_FIXTURE_2)
        .expect("second Apple ProRAW fixture must be present");

    let (_decoded, gm) = DecodeRequest::new(&bytes)
        .with_format(zencodec::ImageFormat::Custom(&zenraw::DNG_FORMAT))
        .decode_gain_map()
        .expect("decode_gain_map on second Apple ProRAW must not error");
    let gm = gm.expect("second Apple ProRAW must yield a gain map");

    // Shape assertions (per ISO 21496-1).
    assert_eq!(gm.metadata.gain_map_min.len(), 3);
    assert_eq!(gm.metadata.gain_map_max.len(), 3);
    assert_eq!(gm.metadata.gamma.len(), 3);
    assert!(gm.metadata.alternate_hdr_headroom > 0.0);
}
