//! WebP codec capability tests.
//!
//! Tier 2 codec — no gain map (per WebP spec), no HDR, sRGB only.
//! Asserts the metadata round-trip surface and the lossy/lossless
//! distinction.
//!
//! See `docs/hdr-per-codec.md` for the full per-codec test plan.

#![cfg(feature = "webp")]

use imgref::{ImgRef, ImgVec};
use rgb::{Rgb, Rgba};
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat, Metadata};
use zenpixels::PixelSlice;

// ─── Inline fixtures ─────────────────────────────────────────────────────

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

fn encode_webp_rgb8(img: ImgRef<'_, Rgb<u8>>, quality: f32) -> Vec<u8> {
    EncodeRequest::new(ImageFormat::WebP)
        .with_quality(quality)
        .encode_full_frame_rgb8(img)
        .expect("encode WebP rgb8")
        .into_vec()
}

fn encode_webp_rgba8(img: ImgRef<'_, Rgba<u8>>, quality: f32) -> Vec<u8> {
    EncodeRequest::new(ImageFormat::WebP)
        .with_quality(quality)
        .encode_full_frame_rgba8(img)
        .expect("encode WebP rgba8")
        .into_vec()
}

fn encode_webp_with_meta(
    img: ImgRef<'_, Rgb<u8>>,
    meta: Metadata,
    quality: f32,
) -> Vec<u8> {
    let typed: PixelSlice<'_, Rgb<u8>> = PixelSlice::from(img);
    EncodeRequest::new(ImageFormat::WebP)
        .with_quality(quality)
        .with_metadata(meta)
        .encode(typed.erase(), false)
        .expect("encode WebP with metadata")
        .into_vec()
}

fn decode_full(bytes: &[u8]) -> zencodecs::DecodeOutput {
    DecodeRequest::new(bytes)
        .decode_full_frame()
        .expect("decode WebP")
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

#[test]
fn webp_rgb8_round_trip_dimensions_preserved() {
    let img = rgb8_image(48, 32);
    let bytes = encode_webp_rgb8(img.as_ref(), 80.0);
    let decoded = decode_full(&bytes);
    assert_eq!(decoded.info().width, 48);
    assert_eq!(decoded.info().height, 32);
    assert_eq!(decoded.info().format, ImageFormat::WebP);
}

#[test]
fn webp_rgba8_round_trip_dimensions_and_alpha() {
    let img = rgba8_image(32, 32);
    let bytes = encode_webp_rgba8(img.as_ref(), 80.0);
    let decoded = decode_full(&bytes);
    assert_eq!(decoded.info().width, 32);
    assert_eq!(decoded.info().height, 32);
    assert!(
        decoded.info().has_alpha,
        "WebP RGBA encode → decoded info must report has_alpha"
    );
}

#[test]
fn webp_quality_parameter_changes_size() {
    let img = rgb8_image(96, 96);
    let high = encode_webp_rgb8(img.as_ref(), 95.0);
    let low = encode_webp_rgb8(img.as_ref(), 20.0);
    assert!(
        high.len() > low.len(),
        "WebP quality=95 should be larger than quality=20, got {} vs {}",
        high.len(),
        low.len(),
    );
}

#[test]
fn webp_lossless_round_trip_is_lossless() {
    // WebP lossless mode: encode → decode must produce byte-identical pixels.
    let img = rgba8_image(16, 16);
    let typed: PixelSlice<'_, Rgba<u8>> = PixelSlice::from(img.as_ref());

    let bytes = EncodeRequest::new(ImageFormat::WebP)
        .with_lossless(true)
        .encode(typed.erase(), true)
        .expect("encode WebP lossless")
        .into_vec();

    let decoded = decode_full(&bytes);
    assert_eq!(decoded.info().width, 16);
    assert_eq!(decoded.info().height, 16);
    // Lossless flag should round-trip, or at minimum: pixel byte-equality.
    let pixels = decoded.pixels();
    let actual: &[u8] = pixels.as_strided_bytes();
    let original: &[u8] = bytemuck::cast_slice(img.buf());
    // Both should be the same length and (in the lossless case) byte-equal.
    assert_eq!(
        actual.len(),
        original.len(),
        "lossless WebP must preserve buffer size"
    );
    assert_eq!(
        actual, original,
        "lossless WebP must produce byte-identical pixels"
    );
}

// ─── ICC profile round-trip ───────────────────────────────────────────────

#[test]
fn webp_icc_profile_byte_equal_round_trip() {
    let img = rgb8_image(32, 32);
    let icc = synthetic_icc(256);
    let meta = Metadata::none().with_icc(icc.clone());

    let bytes = encode_webp_with_meta(img.as_ref(), meta, 80.0);
    let decoded = decode_full(&bytes);

    let extracted = decoded
        .info()
        .source_color
        .icc_profile
        .as_ref()
        .expect("ICC should round-trip on WebP");
    assert_eq!(
        extracted.as_ref(),
        icc.as_slice(),
        "WebP must preserve ICC profile bytes"
    );
}

#[test]
fn webp_no_icc_decodes_with_none_icc() {
    let img = rgb8_image(32, 32);
    let bytes = encode_webp_rgb8(img.as_ref(), 80.0);
    let decoded = decode_full(&bytes);
    assert!(
        decoded.info().source_color.icc_profile.is_none(),
        "WebP without ICC should report None, got {:?}",
        decoded.info().source_color.icc_profile
    );
}

// ─── XMP round-trip ──────────────────────────────────────────────────────

#[test]
fn webp_xmp_round_trip_preserves_marker() {
    let img = rgb8_image(32, 32);
    let xmp = br#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description xmlns:tiff="http://ns.adobe.com/tiff/1.0/">
<tiff:ImageDescription>webp capability marker</tiff:ImageDescription>
</rdf:Description>
</rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#
        .to_vec();
    let meta = Metadata::none().with_xmp(xmp);
    let bytes = encode_webp_with_meta(img.as_ref(), meta, 80.0);
    let decoded = decode_full(&bytes);

    let extracted_meta = decoded.info().metadata();
    let extracted = extracted_meta
        .xmp
        .as_ref()
        .expect("XMP should round-trip on WebP");
    let s = core::str::from_utf8(extracted).expect("XMP must be UTF-8");
    assert!(
        s.contains("webp capability marker"),
        "XMP packet must preserve marker; got {s:?}"
    );
}

// ─── Negative cases (gain map, depth map) ─────────────────────────────────

#[cfg(feature = "jpeg-ultrahdr")]
#[test]
fn decode_gain_map_returns_none_for_webp() {
    let img = rgb8_image(32, 32);
    let bytes = encode_webp_rgb8(img.as_ref(), 80.0);
    let (_, gainmap) = DecodeRequest::new(&bytes)
        .decode_gain_map()
        .expect("decode_gain_map shouldn't error on WebP");
    assert!(
        gainmap.is_none(),
        "WebP must not produce a gain map (no spec support)"
    );
}

// ─── Robustness ──────────────────────────────────────────────────────────

#[test]
fn decode_handles_garbage_webp_without_panic() {
    // RIFF/WEBP magic followed by garbage.
    let bytes: Vec<u8> = b"RIFF\x00\x00\x00\x10WEBPgarbage".to_vec();
    let _ = DecodeRequest::new(&bytes).decode_full_frame();
}
