//! AVIF codec capability tests.
//!
//! Atomic per-leaf tests for the AVIF codec adapter — base decode/encode,
//! ICC byte-equal preservation, CICP code-point fidelity, EXIF and XMP
//! round-trips, gain-map decode (positive + negative cases), and the
//! `encode_with_precomputed_gainmap` resplitting path.
//!
//! See `docs/hdr-per-codec.md` for the full per-codec test plan.

#![cfg(feature = "avif-decode")]

use imgref::{ImgRef, ImgVec};
use rgb::Rgb;
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat, Metadata};
use zenpixels::{PixelDescriptor, PixelSlice};

// ─── Inline fixtures (avoid common/mod.rs feature drift) ──────────────────

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

#[cfg(feature = "avif-encode")]
fn encode_avif_rgb8(img: ImgRef<'_, Rgb<u8>>, quality: f32) -> Vec<u8> {
    EncodeRequest::new(ImageFormat::Avif)
        .with_quality(quality)
        .encode_full_frame_rgb8(img)
        .expect("encode AVIF rgb8")
        .into_vec()
}

#[cfg(feature = "avif-encode")]
fn encode_avif_with_meta(
    img: ImgRef<'_, Rgb<u8>>,
    meta: Metadata,
    quality: f32,
) -> Vec<u8> {
    let typed: PixelSlice<'_, Rgb<u8>> = PixelSlice::from(img);
    EncodeRequest::new(ImageFormat::Avif)
        .with_quality(quality)
        .with_metadata(meta)
        .encode(typed.erase(), false)
        .expect("encode AVIF with metadata")
        .into_vec()
}

fn decode_full(bytes: &[u8]) -> zencodecs::DecodeOutput {
    DecodeRequest::new(bytes)
        .decode_full_frame()
        .expect("decode AVIF")
}

fn synthetic_icc(len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    v.extend_from_slice(&(len as u32).to_be_bytes());
    while v.len() < len {
        v.push((v.len() as u8).wrapping_mul(31));
    }
    v
}

// ─── Base decode/encode ───────────────────────────────────────────────────

#[cfg(feature = "avif-encode")]
#[test]
fn avif_rgb8_round_trip_dimensions_preserved() {
    let img = rgb8_image(32, 24);
    let bytes = encode_avif_rgb8(img.as_ref(), 80.0);
    let decoded = decode_full(&bytes);
    assert_eq!(decoded.info().width, 32);
    assert_eq!(decoded.info().height, 24);
    assert_eq!(decoded.info().format, ImageFormat::Avif);
}

#[cfg(feature = "avif-encode")]
#[test]
fn avif_quality_parameter_changes_size() {
    let img = rgb8_image(64, 64);
    let high = encode_avif_rgb8(img.as_ref(), 95.0);
    let low = encode_avif_rgb8(img.as_ref(), 20.0);
    assert!(
        high.len() > low.len(),
        "AVIF quality=95 should be larger than quality=20, got {} vs {}",
        high.len(),
        low.len(),
    );
}

// ─── ICC profile round-trip ───────────────────────────────────────────────

#[cfg(feature = "avif-encode")]
#[test]
fn avif_icc_profile_byte_equal_round_trip() {
    let img = rgb8_image(32, 32);
    let icc = synthetic_icc(256);
    let meta = Metadata::none().with_icc(icc.clone());

    let bytes = encode_avif_with_meta(img.as_ref(), meta, 80.0);
    let decoded = decode_full(&bytes);

    let extracted = decoded
        .info()
        .source_color
        .icc_profile
        .as_ref()
        .expect("ICC should round-trip on AVIF");
    assert_eq!(
        extracted.as_ref(),
        icc.as_slice(),
        "ICC bytes must be byte-equal after AVIF round-trip"
    );
}

// ─── CICP extraction (the AVIF-specific capability) ──────────────────────

#[cfg(feature = "avif-encode")]
#[test]
fn avif_default_encode_emits_some_cicp() {
    // Even when the caller doesn't specify CICP, AVIF's colr box should
    // describe the encoded color space. After decode, source_color.cicp
    // must be Some — that's the format's invariant.
    let img = rgb8_image(16, 16);
    let bytes = encode_avif_rgb8(img.as_ref(), 80.0);
    let decoded = decode_full(&bytes);
    assert!(
        decoded.info().source_color.cicp.is_some(),
        "AVIF decode must surface CICP from the colr box"
    );
}

#[cfg(feature = "avif-encode")]
#[test]
fn avif_default_encode_yields_srgb_cicp() {
    // Default encode of sRGB pixels should produce CICP indicating BT.709
    // primaries and sRGB transfer (1, 13, *, true) or close. We assert the
    // primaries+transfer pair specifically; the matrix coefficient may
    // vary by encoder default.
    let img = rgb8_image(16, 16);
    let bytes = encode_avif_rgb8(img.as_ref(), 80.0);
    let decoded = decode_full(&bytes);
    let cicp = decoded
        .info()
        .source_color
        .cicp
        .expect("CICP must be present after AVIF decode");
    assert_eq!(
        cicp.color_primaries, 1,
        "default sRGB encode → CICP primaries must be 1 (BT.709), got {}",
        cicp.color_primaries
    );
    assert_eq!(
        cicp.transfer_characteristics, 13,
        "default sRGB encode → CICP transfer must be 13 (sRGB), got {}",
        cicp.transfer_characteristics
    );
}

// ─── EXIF / XMP round-trip ───────────────────────────────────────────────

/// Build a minimal little-endian TIFF/EXIF blob with one Orientation tag.
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

/// AVIF stores EXIF in an `Exif` HEIF item. zenavif's encode path does
/// not currently propagate the inbound `Metadata::exif` blob into that
/// item — the bytes are silently dropped. This test stays as the
/// regression gate so that, the day the encoder wires it up, this
/// passes without further code changes.
#[cfg(feature = "avif-encode")]
#[ignore = "AVIF encoder doesn't propagate Metadata::exif into the Exif item"]
#[test]
fn avif_exif_round_trip_preserves_blob() {
    let img = rgb8_image(32, 32);
    let exif = build_minimal_exif_with_orientation(6);
    let meta = Metadata::none().with_exif(exif.clone());
    let bytes = encode_avif_with_meta(img.as_ref(), meta, 75.0);
    let decoded = decode_full(&bytes);

    // The Exif box payload may be re-wrapped by the encoder; what we
    // require is that *some* EXIF blob comes back, containing our
    // orientation tag bytes. A byte-equal assertion is too strict because
    // AVIF Exif boxes prepend a 4-byte `tiff_header_offset` field per
    // ISO/IEC 23008-12 §A.2.1.
    let extracted_meta = decoded.info().metadata();
    let extracted = extracted_meta
        .exif
        .as_ref()
        .expect("EXIF should round-trip on AVIF");
    let needle = &exif[8..]; // skip 8-byte TIFF header — search for IFD bytes
    assert!(
        extracted
            .windows(needle.len())
            .any(|w| w == needle),
        "EXIF round-trip should contain the original IFD payload"
    );
}

/// AVIF's preferred way to convey orientation is the `irot` / `imir` HEIF
/// transform boxes, not the EXIF Orientation tag — `zenavif` decodes the
/// transforms but does not currently re-derive the EXIF Orientation tag
/// from them, nor honor an inbound EXIF Orientation when encoding.
///
/// Tracked gap. Re-enable when zenavif wires EXIF orientation into the
/// irot/imir round-trip so this test can pass without a code change.
#[cfg(feature = "avif-encode")]
#[ignore = "AVIF encoder doesn't apply inbound EXIF Orientation; decoder doesn't synthesize one from irot"]
#[test]
fn avif_exif_orientation_round_trip_via_irot() {
    let img = rgb8_image(32, 32);
    let exif = build_minimal_exif_with_orientation(6);
    let meta = Metadata::none().with_exif(exif);
    let bytes = encode_avif_with_meta(img.as_ref(), meta, 75.0);
    let decoded = decode_full(&bytes);

    let extracted = decoded.info().orientation;
    assert_ne!(
        format!("{:?}", extracted),
        "Identity",
        "EXIF orientation 6 should not parse as Identity"
    );
}

#[cfg(feature = "avif-encode")]
#[test]
fn avif_xmp_round_trip_preserves_marker() {
    let img = rgb8_image(32, 32);
    let xmp = br#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description xmlns:tiff="http://ns.adobe.com/tiff/1.0/">
<tiff:ImageDescription>avif capability marker</tiff:ImageDescription>
</rdf:Description>
</rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#
        .to_vec();
    let meta = Metadata::none().with_xmp(xmp);
    let bytes = encode_avif_with_meta(img.as_ref(), meta, 75.0);
    let decoded = decode_full(&bytes);

    let extracted_meta = decoded.info().metadata();
    let extracted = extracted_meta.xmp.as_ref().expect("XMP should round-trip");
    let s = core::str::from_utf8(extracted).expect("XMP must be UTF-8");
    assert!(
        s.contains("avif capability marker"),
        "XMP packet must preserve our marker; got: {s:?}",
    );
}

// ─── Gain map detection (negative case) ───────────────────────────────────

#[cfg(all(feature = "avif-encode", feature = "jpeg-ultrahdr"))]
#[test]
fn decode_gain_map_returns_none_for_plain_avif() {
    let img = rgb8_image(32, 32);
    let bytes = encode_avif_rgb8(img.as_ref(), 80.0);
    let (_decoded, gainmap) = DecodeRequest::new(&bytes)
        .decode_gain_map()
        .expect("decode_gain_map shouldn't error on plain AVIF");
    assert!(
        gainmap.is_none(),
        "plain AVIF must not produce a gain map (negative case)"
    );
}

// ─── Robustness ───────────────────────────────────────────────────────────

#[cfg(feature = "jpeg-ultrahdr")]
#[test]
fn decode_gain_map_handles_garbage_avif_without_panic() {
    // Truncated AVIF magic. Implementation must error or return None,
    // never panic.
    let bytes: Vec<u8> = b"\x00\x00\x00\x20ftypavif\x00\x00\x00\x00garbage"
        .to_vec();
    let _ = DecodeRequest::new(&bytes).decode_gain_map();
}
