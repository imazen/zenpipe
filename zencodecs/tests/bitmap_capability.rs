//! Bitmap-family capability tests: BMP (and PNM/Farbfeld when their
//! features are enabled).
//!
//! Tier 2 codecs — minimal metadata-free formats. The matrix-cell
//! coverage focuses on:
//!   - lossless byte-equal round-trip,
//!   - confirming metadata is silently dropped (none of these formats
//!     carry ICC/EXIF/XMP),
//!   - negative gain map.
//!
//! See `docs/hdr-per-codec.md` for the per-codec test plan.

#![cfg(feature = "bitmaps-bmp")]

use imgref::{ImgRef, ImgVec};
use rgb::{Rgb, Rgba};
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat, Metadata};
use zenpixels::PixelSlice;

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

fn encode_bmp_rgb8(img: ImgRef<'_, Rgb<u8>>) -> Vec<u8> {
    EncodeRequest::new(ImageFormat::Bmp)
        .encode_full_frame_rgb8(img)
        .expect("encode BMP rgb8")
        .into_vec()
}

fn encode_bmp_rgba8(img: ImgRef<'_, Rgba<u8>>) -> Vec<u8> {
    EncodeRequest::new(ImageFormat::Bmp)
        .encode_full_frame_rgba8(img)
        .expect("encode BMP rgba8")
        .into_vec()
}

fn decode_full(bytes: &[u8]) -> zencodecs::DecodeOutput {
    DecodeRequest::new(bytes)
        .decode_full_frame()
        .expect("decode")
}

// ─── BMP: base round-trip ────────────────────────────────────────────────

#[test]
fn bmp_rgb8_round_trip_dimensions_preserved() {
    let img = rgb8_image(48, 32);
    let bytes = encode_bmp_rgb8(img.as_ref());
    let decoded = decode_full(&bytes);
    assert_eq!(decoded.info().width, 48);
    assert_eq!(decoded.info().height, 32);
    assert_eq!(decoded.info().format, ImageFormat::Bmp);
}

#[test]
fn bmp_rgba8_round_trip_alpha_preserved() {
    let img = rgba8_image(32, 32);
    let bytes = encode_bmp_rgba8(img.as_ref());
    let decoded = decode_full(&bytes);
    assert!(
        decoded.info().has_alpha,
        "32-bit BMP must round-trip with has_alpha=true"
    );
}

#[test]
fn bmp_rgb8_round_trip_is_lossless_byte_equal() {
    let img = rgb8_image(16, 16);
    let bytes = encode_bmp_rgb8(img.as_ref());
    let decoded = decode_full(&bytes);

    let pixels = decoded.pixels();
    let actual: &[u8] = pixels.as_strided_bytes();
    let original: &[u8] = bytemuck::cast_slice(img.buf());
    // BMP is uncompressed lossless; pixel buffers must be byte-equal.
    assert_eq!(
        actual.len(),
        original.len(),
        "BMP must preserve buffer size exactly"
    );
    assert_eq!(actual, original, "BMP is lossless — pixels byte-equal");
}

// ─── BMP: metadata silently dropped ──────────────────────────────────────

#[test]
fn bmp_no_icc_returned_after_round_trip() {
    let img = rgb8_image(16, 16);
    let bytes = encode_bmp_rgb8(img.as_ref());
    let decoded = decode_full(&bytes);
    assert!(
        decoded.info().source_color.icc_profile.is_none(),
        "BMP spec doesn't carry ICC; decode must report None"
    );
}

#[test]
fn bmp_metadata_with_icc_input_silently_drops() {
    let img = rgb8_image(16, 16);
    let icc = vec![0u8; 256];
    let meta = Metadata::none().with_icc(icc);

    let typed: PixelSlice<'_, Rgb<u8>> = PixelSlice::from(img.as_ref());
    let result = EncodeRequest::new(ImageFormat::Bmp)
        .with_metadata(meta)
        .encode(typed.erase(), false);
    assert!(
        result.is_ok(),
        "BMP encoder must accept (and silently drop) unsupported metadata"
    );
}

// ─── Negative gain map ───────────────────────────────────────────────────

#[cfg(feature = "jpeg-ultrahdr")]
#[test]
fn decode_gain_map_returns_none_for_bmp() {
    let img = rgb8_image(32, 32);
    let bytes = encode_bmp_rgb8(img.as_ref());
    let (_, gainmap) = DecodeRequest::new(&bytes)
        .decode_gain_map()
        .expect("decode_gain_map shouldn't error on BMP");
    assert!(
        gainmap.is_none(),
        "BMP must not produce a gain map (no spec support)"
    );
}

// ─── Robustness ──────────────────────────────────────────────────────────

#[test]
fn decode_handles_garbage_bmp_without_panic() {
    // BM magic followed by garbage.
    let bytes: Vec<u8> = b"BMgarbage past BMP header".to_vec();
    let _ = DecodeRequest::new(&bytes).decode_full_frame();
}

// ─── PNM (PBM/PGM/PPM/PAM) — gated on the `bitmaps` umbrella feature ─────
//
// PNM is enabled by the `bitmaps` feature alone (without `bitmaps-bmp`),
// but because `bitmaps-bmp` implies `bitmaps`, the PNM tests below are
// always available when this file compiles. Farbfeld is the same.

#[cfg(feature = "bitmaps")]
#[test]
fn pnm_rgb8_round_trip_dimensions_preserved() {
    let img = rgb8_image(16, 16);
    let bytes = EncodeRequest::new(ImageFormat::Pnm)
        .encode_full_frame_rgb8(img.as_ref())
        .expect("encode PNM")
        .into_vec();
    let decoded = decode_full(&bytes);
    assert_eq!(decoded.info().width, 16);
    assert_eq!(decoded.info().height, 16);
    assert_eq!(decoded.info().format, ImageFormat::Pnm);
}

// ─── Farbfeld — 16-bit native ────────────────────────────────────────────

#[cfg(feature = "bitmaps")]
#[test]
fn farbfeld_rgba8_round_trip() {
    // Farbfeld is RGBA16 natively; encoding RGBA8 widens to 16 then
    // narrows on decode — round-trip dimensions must still match.
    let img = rgba8_image(16, 16);
    let bytes = EncodeRequest::new(ImageFormat::Farbfeld)
        .encode_full_frame_rgba8(img.as_ref())
        .expect("encode Farbfeld")
        .into_vec();
    let decoded = decode_full(&bytes);
    assert_eq!(decoded.info().width, 16);
    assert_eq!(decoded.info().height, 16);
    assert_eq!(decoded.info().format, ImageFormat::Farbfeld);
}

/// Farbfeld is designed to preserve 16-bit RGBA precision. Today
/// zencodecs surfaces it as 8-bit through the trait — when bit depth
/// preservation is wired (likely the same fix as the wide-gamut
/// pipeline gap), this test becomes a true precision assertion.
#[cfg(feature = "bitmaps")]
#[ignore = "Farbfeld decode currently narrows to 8-bit through the trait interface"]
#[test]
fn farbfeld_round_trip_preserves_16bit_precision() {
    panic!(
        "Pending: 16-bit pipeline support. \
         Assertion: encode RGBA16 → decode RGBA16 → byte-equal pixels"
    );
}
