//! JPEG codec capability tests.
//!
//! Atomic tests that prove each leaf capability of the JPEG codec adapter
//! in isolation. Targets metadata round-trips and gain-map detection edges.
//!
//! See `docs/hdr-per-codec.md` for the full per-codec test plan.
//!
//! Capabilities asserted here:
//!   - Base 8-bit RGB / RGBA decode and encode.
//!   - ICC profile round-trip (byte-equal).
//!   - EXIF orientation tag survives round-trip.
//!   - XMP packet round-trip (marker preserved).
//!   - `decode_gain_map` returns `None` for plain JPEG (negative case).
//!   - `decode_gain_map` returns `Some` for UltraHDR JPEG (positive case).
//!   - All ISO 21496-1 metadata fields present after gain-map decode.
//!
//! NOTE: This file uses the modern `EncodeRequest::encode(PixelSlice, ...)`
//! API. Several existing tests in this directory still call the removed
//! convenience methods like `encode_full_frame_rgb8` and won't compile in
//! isolation — that's a separate cleanup.

#![cfg(feature = "jpeg")]

use imgref::{ImgRef, ImgVec};
use rgb::{Rgb, Rgba};
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat, Metadata};
use zenpixels::{PixelDescriptor, PixelSlice};

// Inline RGB/RGBA fixtures (tests/common/mod.rs uses removed convenience methods).
fn rgb8_image(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let pixels: Vec<Rgb<u8>> = (0..w * h)
        .map(|i| Rgb {
            r: (i % w) as u8,
            g: (i / w) as u8,
            b: 128,
        })
        .collect();
    ImgVec::new(pixels, w, h)
}

fn rgba8_image(w: usize, h: usize) -> ImgVec<Rgba<u8>> {
    let pixels: Vec<Rgba<u8>> = (0..w * h)
        .map(|i| Rgba {
            r: (i % w) as u8,
            g: (i / w) as u8,
            b: 128,
            a: 200,
        })
        .collect();
    ImgVec::new(pixels, w, h)
}

// ─── Encode helpers (modern PixelSlice API) ──────────────────────────────

fn encode_jpeg_rgb8(img: ImgRef<'_, Rgb<u8>>, quality: f32) -> Vec<u8> {
    let typed: PixelSlice<'_, Rgb<u8>> = PixelSlice::from(img);
    EncodeRequest::new(ImageFormat::Jpeg)
        .with_quality(quality)
        .encode(typed.erase(), false)
        .expect("encode rgb8 jpeg")
        .into_vec()
}

fn encode_jpeg_rgba8_ignore_alpha(img: ImgRef<'_, Rgba<u8>>, quality: f32) -> Vec<u8> {
    let typed: PixelSlice<'_, Rgba<u8>> = PixelSlice::from(img);
    let pixels = typed
        .with_descriptor(
            PixelDescriptor::RGBA8_SRGB
                .with_alpha(Some(zenpixels::AlphaMode::Undefined)),
        )
        .erase();
    EncodeRequest::new(ImageFormat::Jpeg)
        .with_quality(quality)
        .encode(pixels, false)
        .expect("encode rgba8 jpeg")
        .into_vec()
}

fn encode_jpeg_with_meta(img: ImgRef<'_, Rgb<u8>>, meta: Metadata, quality: f32) -> Vec<u8> {
    let typed: PixelSlice<'_, Rgb<u8>> = PixelSlice::from(img);
    EncodeRequest::new(ImageFormat::Jpeg)
        .with_quality(quality)
        .with_metadata(meta)
        .encode(typed.erase(), false)
        .expect("encode jpeg with metadata")
        .into_vec()
}

fn decode_full(bytes: &[u8]) -> zencodecs::DecodeOutput {
    DecodeRequest::new(bytes)
        .decode_full_frame()
        .expect("decode jpeg")
}

// A 256-byte synthetic ICC blob. Real ICC parsers validate structure, but
// JPEG embeds the bytes verbatim in APP2 ICC_PROFILE markers — that's the
// property we check.
fn synthetic_icc(len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    v.extend_from_slice(&(len as u32).to_be_bytes());
    while v.len() < len {
        v.push((v.len() as u8).wrapping_mul(31));
    }
    v
}

/// Build a minimal little-endian TIFF/EXIF blob containing only an
/// Orientation tag (0x0112) at the IFD0 entry.
fn build_minimal_exif_with_orientation(value: u16) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"II\x2a\x00");
    v.extend_from_slice(&8u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&0x0112u16.to_le_bytes());
    v.extend_from_slice(&3u16.to_le_bytes());
    v.extend_from_slice(&1u32.to_le_bytes());
    v.extend_from_slice(&(value as u32).to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v
}

// ─── Base decode/encode ───────────────────────────────────────────────────

#[test]
fn jpeg_rgb8_round_trip_dimensions_preserved() {
    let img = rgb8_image(64, 48);
    let bytes = encode_jpeg_rgb8(img.as_ref(), 90.0);
    let decoded = decode_full(&bytes);
    assert_eq!(decoded.info().width, 64);
    assert_eq!(decoded.info().height, 48);
}

#[test]
fn jpeg_rgba8_round_trip_alpha_dropped_silently() {
    let img = rgba8_image(32, 32);
    let bytes = encode_jpeg_rgba8_ignore_alpha(img.as_ref(), 90.0);
    let decoded = decode_full(&bytes);
    assert_eq!(decoded.info().width, 32);
    assert_eq!(decoded.info().height, 32);
}

#[test]
fn jpeg_quality_parameter_changes_size() {
    let img = rgb8_image(128, 128);
    let high = encode_jpeg_rgb8(img.as_ref(), 95.0);
    let low = encode_jpeg_rgb8(img.as_ref(), 30.0);
    assert!(
        high.len() > low.len(),
        "quality=95 should be larger than quality=30, got {} vs {}",
        high.len(),
        low.len(),
    );
}

// ─── ICC profile round-trip ───────────────────────────────────────────────

#[test]
fn jpeg_icc_profile_byte_equal_round_trip() {
    let img = rgb8_image(32, 32);
    let icc = synthetic_icc(256);
    let meta = Metadata::none().with_icc(icc.clone());

    let bytes = encode_jpeg_with_meta(img.as_ref(), meta, 90.0);
    let decoded = decode_full(&bytes);

    let extracted = decoded
        .info()
        .source_color
        .icc_profile
        .as_ref()
        .expect("ICC should round-trip on JPEG");
    assert_eq!(
        extracted.as_ref(),
        icc.as_slice(),
        "ICC bytes must be byte-equal"
    );
}

#[test]
fn jpeg_no_icc_decodes_with_none_icc() {
    let img = rgb8_image(32, 32);
    let bytes = encode_jpeg_rgb8(img.as_ref(), 80.0);
    let decoded = decode_full(&bytes);
    assert!(
        decoded.info().source_color.icc_profile.is_none(),
        "JPEG without ICC should report None"
    );
}

// ─── EXIF round-trip ──────────────────────────────────────────────────────

#[test]
fn jpeg_exif_orientation_round_trip() {
    let img = rgb8_image(32, 32);
    let exif = build_minimal_exif_with_orientation(6);
    let meta = Metadata::none().with_exif(exif);

    let bytes = encode_jpeg_with_meta(img.as_ref(), meta, 85.0);
    let decoded = decode_full(&bytes);

    // Orientation appears as a structured field on info.
    let extracted = decoded.info().orientation;
    assert_ne!(
        format!("{:?}", extracted),
        "Identity",
        "EXIF orientation 6 should not decode as Identity, got {:?}",
        extracted
    );
}

// ─── XMP round-trip ───────────────────────────────────────────────────────

#[test]
fn jpeg_xmp_round_trip_preserves_marker() {
    let img = rgb8_image(32, 32);
    let xmp = br#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description xmlns:tiff="http://ns.adobe.com/tiff/1.0/">
<tiff:ImageDescription>capability test marker</tiff:ImageDescription>
</rdf:Description>
</rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#
        .to_vec();
    let meta = Metadata::none().with_xmp(xmp.clone());

    let bytes = encode_jpeg_with_meta(img.as_ref(), meta, 85.0);
    let decoded = decode_full(&bytes);

    let extracted_meta = decoded.info().metadata();
    let extracted = extracted_meta
        .xmp
        .as_ref()
        .expect("XMP should round-trip");
    let s = core::str::from_utf8(extracted).expect("XMP must be UTF-8");
    assert!(
        s.contains("capability test marker"),
        "XMP packet must preserve our marker; got: {s:?}"
    );
}

// ─── Gain map detection (negative case) ───────────────────────────────────

#[cfg(feature = "jpeg-ultrahdr")]
#[test]
fn decode_gain_map_returns_none_for_plain_rgb_jpeg() {
    let img = rgb8_image(32, 32);
    let bytes = encode_jpeg_rgb8(img.as_ref(), 90.0);
    let (_decoded, gainmap) = DecodeRequest::new(&bytes)
        .decode_gain_map()
        .expect("decode_gain_map shouldn't error on plain JPEG");
    assert!(
        gainmap.is_none(),
        "plain JPEG must not produce a gain map (negative-case guarantee)"
    );
}

// ─── Gain map detection (positive case + metadata fields) ────────────────

#[cfg(feature = "jpeg-ultrahdr")]
#[test]
fn decode_gain_map_returns_some_for_ultrahdr_jpeg() {
    let img = make_hdr_gradient(64, 64, 4.0);

    let bytes = EncodeRequest::new(ImageFormat::Jpeg)
        .with_quality(90.0)
        .encode_ultrahdr_rgb_f32(img.as_ref())
        .expect("encode UltraHDR")
        .into_vec();

    let (_decoded, gm) = DecodeRequest::new(&bytes)
        .decode_gain_map()
        .expect("decode_gain_map on UltraHDR should not error");
    let gm = gm.expect("UltraHDR JPEG must produce Some(DecodedGainMap)");

    // JPEG UltraHDR direction: SDR base + gain map → HDR.
    assert!(!gm.base_is_hdr, "JPEG UltraHDR base is SDR");
    assert_eq!(gm.source_format, ImageFormat::Jpeg);
}

#[cfg(feature = "jpeg-ultrahdr")]
#[test]
fn ultrahdr_gain_map_metadata_has_iso21496_fields() {
    let img = make_hdr_gradient(32, 32, 3.0);

    let bytes = EncodeRequest::new(ImageFormat::Jpeg)
        .with_quality(80.0)
        .encode_ultrahdr_rgb_f32(img.as_ref())
        .expect("encode UltraHDR")
        .into_vec();

    let (_decoded, gm) = DecodeRequest::new(&bytes)
        .decode_gain_map()
        .expect("decode_gain_map");
    let gm = gm.expect("UltraHDR gain map");
    let meta = &gm.metadata;

    // ISO 21496-1: GainMapMetadata always carries [3] arrays — per-channel even
    // for single-channel maps (channels are equal then). All three arrays must
    // exist and be the same length by construction.
    assert_eq!(meta.gain_map_min.len(), 3);
    assert_eq!(meta.gain_map_max.len(), 3);
    assert_eq!(meta.gamma.len(), 3);
    assert_eq!(meta.base_offset.len(), 3);
    assert_eq!(meta.alternate_offset.len(), 3);

    // alternate_hdr_headroom is the log2 of the HDR target's peak luminance ratio.
    // For encoded HDR content, it should be strictly greater than the base headroom.
    assert!(
        meta.alternate_hdr_headroom >= meta.base_hdr_headroom,
        "alternate_hdr_headroom ({}) should be >= base_hdr_headroom ({})",
        meta.alternate_hdr_headroom,
        meta.base_hdr_headroom,
    );
    assert!(
        meta.alternate_hdr_headroom > 0.0,
        "encoded HDR content should declare positive HDR headroom; got {}",
        meta.alternate_hdr_headroom
    );

    // Direction flag: false = base SDR, alternate HDR (forward / JPEG case).
    assert!(
        !meta.backward_direction,
        "JPEG UltraHDR encode should yield forward-direction metadata"
    );

    // params_to_metadata round-trip (log ↔ linear domain).
    let params = gm.params();
    let round_tripped = zencodecs::gainmap::params_to_metadata(&params);
    for i in 0..3 {
        let a = meta.gain_map_min[i];
        let b = round_tripped.gain_map_min[i];
        assert!(
            (a - b).abs() < 1e-3,
            "gain_map_min[{i}] should round-trip: {a} vs {b}"
        );
    }
}

// ─── Robustness ───────────────────────────────────────────────────────────

#[cfg(feature = "jpeg-ultrahdr")]
#[test]
fn decode_gain_map_handles_garbage_without_panic() {
    let bytes: Vec<u8> = b"\xff\xd8\xff garbage past SOI marker".to_vec();
    let _result = DecodeRequest::new(&bytes).decode_gain_map();
    // Whether it returns Err or Ok(None), the sole requirement is no panic.
}

// ─── Helpers (HDR fixtures) ──────────────────────────────────────────────

#[cfg(feature = "jpeg-ultrahdr")]
fn make_hdr_gradient(w: usize, h: usize, peak: f32) -> ImgVec<Rgb<f32>> {
    let pixels: Vec<Rgb<f32>> = (0..w * h)
        .map(|i| {
            let x = (i % w) as f32 / w.max(1) as f32;
            let y = (i / w) as f32 / h.max(1) as f32;
            Rgb {
                r: x * peak,
                g: y * peak * 0.7,
                b: 0.2 + x * 0.3,
            }
        })
        .collect();
    ImgVec::new(pixels, w, h)
}
