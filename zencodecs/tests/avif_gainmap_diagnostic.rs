//! Diagnostic for the AVIF gain map embed gap.
//!
//! Bypasses `zencodecs::EncodeRequest` entirely and goes straight through
//! `zenavif::EncoderConfig::with_gain_map(...)` → `encode_rgb8(...)` →
//! `zenavif_parse::AvifParser`. This isolates the question:
//!
//!   "Does the zenavif/ravif/zenavif-serialize encode stack actually
//!    write a tmap auxiliary item into the AVIF bytes?"
//!
//! If this test passes (a gain map round-trips through the bare zenavif
//! API), the gap is in zencodecs's `encode_with_precomputed_gainmap`.
//! If this test fails, the gap is deeper in the encoder stack.

#![cfg(all(feature = "avif-encode", feature = "avif-decode"))]

use rgb::Rgb;

/// Trait-path zenavif test. Exactly what zencodecs's
/// `build_from_config` does internally, but without zencodecs in the
/// loop. Builds AvifEncoderConfig → job() → with_metadata(meta) →
/// encoder() → encode(pixel_slice). Inspects the AVIF for an Exif item.
///
/// Three outcomes possible:
///  - Bare encode_rgb8 + EncoderConfig::exif works
///  - This trait-path test works  → zencodecs wrapping has a different bug
///  - This trait-path test fails  → bug is in zenavif's job-encoder-encode chain
#[test]
fn zenavif_trait_path_with_metadata_writes_exif_item() {
    use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};

    let base_pixels: Vec<Rgb<u8>> = (0..16 * 16)
        .map(|i| Rgb { r: (i % 16) as u8, g: 80, b: 50 })
        .collect();
    let base = imgref::ImgVec::new(base_pixels, 16, 16);

    let mut exif = Vec::new();
    exif.extend_from_slice(b"II\x2a\x00");
    exif.extend_from_slice(&8u32.to_le_bytes());
    exif.extend_from_slice(&1u16.to_le_bytes());
    exif.extend_from_slice(&0x0112u16.to_le_bytes());
    exif.extend_from_slice(&3u16.to_le_bytes());
    exif.extend_from_slice(&1u32.to_le_bytes());
    exif.extend_from_slice(&6u32.to_le_bytes());
    exif.extend_from_slice(&0u32.to_le_bytes());

    let cfg = zenavif::AvifEncoderConfig::new();
    let mut job = cfg.job();
    let meta = zencodec::Metadata::none().with_exif(exif);
    job = job.with_metadata(meta);
    let encoder = job.encoder().expect("build encoder");

    let typed: zenpixels::PixelSlice<'_, Rgb<u8>> =
        zenpixels::PixelSlice::from(base.as_ref());
    let result = encoder.encode(typed.erase()).expect("encode");

    let parser = zenavif_parse::AvifParser::from_bytes(result.as_ref())
        .expect("parse");
    let exif_result = parser.exif();
    assert!(
        exif_result.is_some(),
        "zenavif trait path (job + with_metadata + encoder.encode) → AVIF \
         must contain Exif item; parser.exif() returned None"
    );
}

/// Bare zenavif test: bypass zencodecs entirely. Use
/// `zenavif::EncoderConfig::exif(...)` directly + `zenavif::encode_rgb8`,
/// then verify zenavif_parse sees an Exif item.
///
/// If THIS test passes, the EXIF write path works at the zenavif/ravif
/// level, and the zencodecs gap is in the wrapping (similar to the
/// gain-map type-mismatch bugs).
#[test]
fn bare_zenavif_with_exif_actually_writes_exif_item() {
    let base_pixels: Vec<Rgb<u8>> = (0..16 * 16)
        .map(|i| Rgb { r: (i % 16) as u8, g: 80, b: 50 })
        .collect();
    let base = imgref::ImgVec::new(base_pixels, 16, 16);

    let mut exif = Vec::new();
    exif.extend_from_slice(b"II\x2a\x00");
    exif.extend_from_slice(&8u32.to_le_bytes());
    exif.extend_from_slice(&1u16.to_le_bytes());
    exif.extend_from_slice(&0x0112u16.to_le_bytes());
    exif.extend_from_slice(&3u16.to_le_bytes());
    exif.extend_from_slice(&1u32.to_le_bytes());
    exif.extend_from_slice(&6u32.to_le_bytes());
    exif.extend_from_slice(&0u32.to_le_bytes());

    let cfg = zenavif::EncoderConfig::new().exif(exif.clone());
    let result = zenavif::encode_rgb8(
        base.as_ref(),
        &cfg,
        zencodec::StopToken::new(zencodec::enough::Unstoppable),
    )
    .expect("bare zenavif encode with EXIF");

    let parser = zenavif_parse::AvifParser::from_bytes(&result.avif_file)
        .expect("parse zenavif AVIF");
    let exif_result = parser.exif();
    assert!(
        exif_result.is_some(),
        "bare zenavif EncoderConfig::exif(...) → AVIF must contain Exif item"
    );
}

/// Diagnosis of the AVIF EXIF round-trip gap.
///
/// Both bare zenavif and zencodecs's full EncodeRequest path emit an
/// `Exif` item into the AVIF, but per ISO/IEC 23008-12 §A.2.1 the item
/// payload must begin with a 4-byte big-endian `tiff_header_offset`
/// before the TIFF data. zenavif-serialize at lib.rs:515 writes the
/// caller-provided bytes directly as the extent — no offset prefix —
/// so zenavif-parse rejects the item with `InvalidData("EXIF offset
/// exceeds item size")`.
///
/// This test captures both encoder outputs, demonstrates that the bytes
/// look right at the box-type level, and asserts the parser fails on
/// item resolution. When zenavif-serialize is fixed to prepend the
/// 4-byte offset prefix (or callers are required to pre-prefix), this
/// test should be updated to verify the offset is present and the
/// item resolves cleanly.
#[ignore = "zenavif-serialize doesn't write the AVIF Exif tiff_header_offset prefix per ISO 23008-12 §A.2.1"]
#[test]
fn compare_bare_vs_zencodecs_exif_byte_search() {
    use zencodecs::{EncodeRequest, ImageFormat, Metadata};

    let pixels: Vec<Rgb<u8>> = (0..16 * 16)
        .map(|i| Rgb { r: (i % 16) as u8, g: 80, b: 50 })
        .collect();
    let img = imgref::ImgVec::new(pixels, 16, 16);

    let mut exif = Vec::new();
    exif.extend_from_slice(b"II\x2a\x00");
    exif.extend_from_slice(&8u32.to_le_bytes());
    exif.extend_from_slice(&1u16.to_le_bytes());
    exif.extend_from_slice(&0x0112u16.to_le_bytes());
    exif.extend_from_slice(&3u16.to_le_bytes());
    exif.extend_from_slice(&1u32.to_le_bytes());
    exif.extend_from_slice(&6u32.to_le_bytes());
    exif.extend_from_slice(&0u32.to_le_bytes());

    // Bare zenavif
    let cfg = zenavif::EncoderConfig::new().exif(exif.clone());
    let bare = zenavif::encode_rgb8(
        img.as_ref(),
        &cfg,
        zencodec::StopToken::new(zencodec::enough::Unstoppable),
    )
    .expect("bare encode");

    // zencodecs full path
    let typed: zenpixels::PixelSlice<'_, Rgb<u8>> =
        zenpixels::PixelSlice::from(img.as_ref());
    let zen = EncodeRequest::new(ImageFormat::Avif)
        .with_quality(80.0)
        .with_metadata(Metadata::none().with_exif(exif.clone()))
        .encode(typed.erase(), false)
        .expect("zencodecs encode");
    let zen_bytes = zen.into_vec();

    // Find "Exif" 4-byte marker (box header convention).
    let bare_has_exif = bare
        .avif_file
        .windows(4)
        .any(|w| w == b"Exif" || w == b"infe" && bare.avif_file.windows(8).any(|w8| w8 == b"infeExif"));
    let zen_has_exif = zen_bytes
        .windows(4)
        .any(|w| w == b"Exif");

    let bare_unique_idx = bare.avif_file.windows(4).position(|w| w == b"Exif");
    let zen_unique_idx = zen_bytes.windows(4).position(|w| w == b"Exif");

    let bare_parser = zenavif_parse::AvifParser::from_bytes(&bare.avif_file)
        .expect("parse bare");
    let bare_parsed = bare_parser.exif().is_some();
    let zen_parser = zenavif_parse::AvifParser::from_bytes(&zen_bytes)
        .expect("parse zen");
    let zen_parsed = zen_parser.exif().is_some();

    let _ = bare_has_exif;
    let _ = zen_has_exif;
    panic!(
        "bare: len={} idx={:?} parsed={}; zen: len={} idx={:?} parsed={}",
        bare.avif_file.len(),
        bare_unique_idx,
        bare_parsed,
        zen_bytes.len(),
        zen_unique_idx,
        zen_parsed,
    );
}

/// Stricter than `bare_zenavif_with_exif_actually_writes_exif_item` —
/// not only must the parser SEE an Exif item, the item must also
/// RESOLVE to a non-empty payload. Today the item is emitted but
/// with a malformed (missing 4-byte `tiff_header_offset`) payload, so
/// resolution fails. Same upstream root cause as the ignored
/// `compare_bare_vs_zencodecs_exif_byte_search` above.
#[ignore = "zenavif-serialize doesn't write the AVIF Exif tiff_header_offset prefix per ISO 23008-12 §A.2.1"]
#[test]
fn zencodecs_with_metadata_exif_actually_writes_exif_item() {
    use zencodecs::{EncodeRequest, ImageFormat, Metadata};

    let base_pixels: Vec<Rgb<u8>> = (0..16 * 16)
        .map(|i| Rgb { r: (i % 16) as u8, g: 80, b: 50 })
        .collect();
    let base = imgref::ImgVec::new(base_pixels, 16, 16);
    let typed: zenpixels::PixelSlice<'_, Rgb<u8>> =
        zenpixels::PixelSlice::from(base.as_ref());

    // Minimal valid EXIF blob: TIFF header + 1 IFD entry (Orientation=6).
    let mut exif = Vec::new();
    exif.extend_from_slice(b"II\x2a\x00");
    exif.extend_from_slice(&8u32.to_le_bytes());
    exif.extend_from_slice(&1u16.to_le_bytes());
    exif.extend_from_slice(&0x0112u16.to_le_bytes());
    exif.extend_from_slice(&3u16.to_le_bytes());
    exif.extend_from_slice(&1u32.to_le_bytes());
    exif.extend_from_slice(&6u32.to_le_bytes());
    exif.extend_from_slice(&0u32.to_le_bytes());

    let meta = Metadata::none().with_exif(exif);
    let avif_bytes = EncodeRequest::new(ImageFormat::Avif)
        .with_quality(80.0)
        .with_metadata(meta)
        .encode(typed.erase(), false)
        .expect("zencodecs encode AVIF with EXIF metadata")
        .into_vec();

    let parser = zenavif_parse::AvifParser::from_bytes(&avif_bytes)
        .expect("parse zencodecs-emitted AVIF");
    let exif_result = parser.exif();
    assert!(
        exif_result.is_some(),
        "AVIF emitted by zencodecs with Metadata::exif must contain an Exif item; \
         parser.exif() returned None"
    );
    let exif_data = exif_result.unwrap().expect("Exif item resolves");
    assert!(
        !exif_data.is_empty(),
        "AVIF Exif item payload must be non-empty"
    );
}

/// Run the FULL zencodecs path that wraps zenavif's with_gain_map, and
/// inspect the output AVIF directly with zenavif_parse. If this test
/// fails but the bare-zenavif test (below) passes, the gap is in the
/// zencodecs wrapping.
#[cfg(feature = "jpeg-ultrahdr")]
#[test]
fn zencodecs_encode_with_precomputed_gainmap_actually_embeds_tmap() {
    use zencodecs::{EncodeRequest, GainMapSource, ImageFormat};

    // Build a synthetic gain map struct manually (no need for a donor).
    let gm_pixels: Vec<u8> = (0..16 * 16 * 3)
        .map(|i| ((i * 7) % 256) as u8)
        .collect();
    let gain_map = zencodecs::gainmap::GainMap {
        data: gm_pixels,
        width: 16,
        height: 16,
        channels: 3,
    };
    let mut metadata = zencodecs::gainmap::GainMapMetadata::default();
    metadata.gain_map_min = [-2.0, -2.0, -2.0];
    metadata.gain_map_max = [2.0, 2.0, 2.0];
    metadata.gamma = [1.0, 1.0, 1.0];
    metadata.base_offset = [0.015625, 0.015625, 0.015625];
    metadata.alternate_offset = [0.015625, 0.015625, 0.015625];
    metadata.alternate_hdr_headroom = 2.5;

    // SDR base.
    let base_pixels: Vec<Rgb<u8>> = (0..32 * 32)
        .map(|i| Rgb {
            r: (i % 32) as u8,
            g: 100,
            b: 50,
        })
        .collect();
    let base = imgref::ImgVec::new(base_pixels, 32, 32);
    let typed: zenpixels::PixelSlice<'_, Rgb<u8>> =
        zenpixels::PixelSlice::from(base.as_ref());

    let avif_bytes = EncodeRequest::new(ImageFormat::Avif)
        .with_quality(80.0)
        .with_gain_map(GainMapSource::Precomputed {
            gain_map: &gain_map,
            metadata: &metadata,
        })
        .encode(typed.erase(), false)
        .expect("zencodecs encode with gain map")
        .into_vec();

    let parser = zenavif_parse::AvifParser::from_bytes(&avif_bytes)
        .expect("parse zencodecs-emitted AVIF");
    let gm_result = parser.gain_map();
    assert!(
        gm_result.is_some(),
        "zencodecs encode_with_precomputed_gainmap → AVIF must contain tmap; \
         parser.gain_map() returned None"
    );
}

#[test]
fn zenavif_with_gain_map_actually_embeds_tmap_item() {
    // Build a minimal SDR base.
    let base_pixels: Vec<Rgb<u8>> = (0..32 * 32)
        .map(|i| Rgb {
            r: (i % 32) as u8,
            g: 100,
            b: 50,
        })
        .collect();
    let base = imgref::ImgVec::new(base_pixels, 32, 32);

    // Build a minimal gain map: 16x16 grayscale, encoded as a tiny AVIF.
    let gm_pixels: Vec<Rgb<u8>> = (0..16 * 16)
        .map(|i| {
            let v = ((i * 7) % 256) as u8;
            Rgb { r: v, g: v, b: v }
        })
        .collect();
    let gm_img = imgref::ImgVec::new(gm_pixels, 16, 16);

    // Encode the gain map to AV1 by encoding it as an AVIF and stripping.
    let gm_enc = zenavif::EncoderConfig::new();
    let gm_avif = zenavif::encode_rgb8(
        gm_img.as_ref(),
        &gm_enc,
        zencodec::StopToken::new(zencodec::enough::Unstoppable),
    )
    .expect("encode gain map AVIF");
    let parser = zenavif_parse::AvifParser::from_bytes(&gm_avif.avif_file)
        .expect("parse gain map AVIF");
    let av1_data = parser
        .primary_data()
        .expect("extract primary AV1")
        .to_vec();

    // Synthetic ISO 21496-1 metadata blob.
    let iso_metadata = vec![0u8; 64];

    // Encode the base WITH the gain map attached.
    let main_enc = zenavif::EncoderConfig::new().with_gain_map(
        av1_data.clone(),
        16,
        16,
        8,
        iso_metadata.clone(),
    );
    let main_avif = zenavif::encode_rgb8(
        base.as_ref(),
        &main_enc,
        zencodec::StopToken::new(zencodec::enough::Unstoppable),
    )
    .expect("encode main AVIF with gain map");

    // Inspect the output AVIF directly with zenavif_parse — does it see
    // a gain map item? This is the binary question we care about.
    let parser =
        zenavif_parse::AvifParser::from_bytes(&main_avif.avif_file)
            .expect("parse main AVIF");

    // The parser should expose some gain map indication. Try the most
    // likely accessor and assert it's not None.
    let gm_result = parser.gain_map();
    assert!(
        gm_result.is_some(),
        "AVIF encoded with .with_gain_map(...) must contain a tmap aux item — \
         parser.gain_map() returned None"
    );
    let gm = gm_result
        .unwrap()
        .expect("gain map item resolves cleanly");
    assert!(
        !gm.gain_map_data.is_empty(),
        "tmap AV1 data must be non-empty"
    );
}
