//! PNG codec capability tests.
//!
//! Tier 2 codec — no gain map (per PNG spec for the standard chunks),
//! sRGB only by convention. PNG does have a `cICP` chunk in the spec
//! but `zenpng` doesn't currently surface it — that gap is captured as
//! an `#[ignore]`d test.
//!
//! See `docs/hdr-per-codec.md` for the full per-codec test plan.

#![cfg(feature = "png")]

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

fn encode_png_rgb8(img: ImgRef<'_, Rgb<u8>>) -> Vec<u8> {
    EncodeRequest::new(ImageFormat::Png)
        .encode_full_frame_rgb8(img)
        .expect("encode PNG rgb8")
        .into_vec()
}

fn encode_png_rgba8(img: ImgRef<'_, Rgba<u8>>) -> Vec<u8> {
    EncodeRequest::new(ImageFormat::Png)
        .encode_full_frame_rgba8(img)
        .expect("encode PNG rgba8")
        .into_vec()
}

fn encode_png_with_meta(img: ImgRef<'_, Rgb<u8>>, meta: Metadata) -> Vec<u8> {
    let typed: PixelSlice<'_, Rgb<u8>> = PixelSlice::from(img);
    EncodeRequest::new(ImageFormat::Png)
        .with_metadata(meta)
        .encode(typed.erase(), false)
        .expect("encode PNG with metadata")
        .into_vec()
}

fn decode_full(bytes: &[u8]) -> zencodecs::DecodeOutput {
    DecodeRequest::new(bytes)
        .decode_full_frame()
        .expect("decode PNG")
}

fn synthetic_icc(len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    v.extend_from_slice(&(len as u32).to_be_bytes());
    while v.len() < len {
        v.push((v.len() as u8).wrapping_mul(31));
    }
    v
}

// ─── Base decode/encode (lossless) ───────────────────────────────────────

#[test]
fn png_rgb8_round_trip_dimensions_preserved() {
    let img = rgb8_image(48, 32);
    let bytes = encode_png_rgb8(img.as_ref());
    let decoded = decode_full(&bytes);
    assert_eq!(decoded.info().width, 48);
    assert_eq!(decoded.info().height, 32);
    assert_eq!(decoded.info().format, ImageFormat::Png);
}

#[test]
fn png_rgba8_round_trip_alpha_preserved() {
    let img = rgba8_image(32, 32);
    let bytes = encode_png_rgba8(img.as_ref());
    let decoded = decode_full(&bytes);
    assert!(
        decoded.info().has_alpha,
        "PNG RGBA8 must round-trip with has_alpha=true"
    );
}

#[test]
fn png_rgba8_round_trip_is_lossless_pixel_byte_equal() {
    let img = rgba8_image(16, 16);
    let bytes = encode_png_rgba8(img.as_ref());
    let decoded = decode_full(&bytes);

    let pixels = decoded.pixels();
    let actual: &[u8] = pixels.as_strided_bytes();
    let original: &[u8] = bytemuck::cast_slice(img.buf());
    assert_eq!(
        actual.len(),
        original.len(),
        "PNG must preserve buffer size exactly"
    );
    assert_eq!(
        actual, original,
        "PNG is lossless — pixels must be byte-equal after round-trip"
    );
}

// ─── ICC profile round-trip (iCCP chunk) ─────────────────────────────────

#[test]
fn png_icc_profile_byte_equal_round_trip() {
    let img = rgb8_image(32, 32);
    let icc = synthetic_icc(256);
    let meta = Metadata::none().with_icc(icc.clone());

    let bytes = encode_png_with_meta(img.as_ref(), meta);
    let decoded = decode_full(&bytes);

    let extracted = decoded
        .info()
        .source_color
        .icc_profile
        .as_ref()
        .expect("ICC must round-trip on PNG (iCCP chunk)");
    assert_eq!(
        extracted.as_ref(),
        icc.as_slice(),
        "PNG must preserve ICC bytes verbatim"
    );
}

#[test]
fn png_no_icc_decodes_with_none_icc() {
    let img = rgb8_image(32, 32);
    let bytes = encode_png_rgb8(img.as_ref());
    let decoded = decode_full(&bytes);
    assert!(
        decoded.info().source_color.icc_profile.is_none(),
        "PNG without iCCP must report None"
    );
}

// ─── XMP round-trip (iTXt chunk) ─────────────────────────────────────────

#[test]
fn png_xmp_round_trip_preserves_marker() {
    let img = rgb8_image(32, 32);
    let xmp = br#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description xmlns:tiff="http://ns.adobe.com/tiff/1.0/">
<tiff:ImageDescription>png capability marker</tiff:ImageDescription>
</rdf:Description>
</rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#
        .to_vec();
    let meta = Metadata::none().with_xmp(xmp);
    let bytes = encode_png_with_meta(img.as_ref(), meta);
    let decoded = decode_full(&bytes);

    let extracted_meta = decoded.info().metadata();
    let extracted = extracted_meta
        .xmp
        .as_ref()
        .expect("XMP should round-trip on PNG (iTXt chunk)");
    let s = core::str::from_utf8(extracted).expect("XMP must be UTF-8");
    assert!(
        s.contains("png capability marker"),
        "XMP packet must preserve marker; got {s:?}"
    );
}

// ─── Negative gain map ───────────────────────────────────────────────────

#[cfg(feature = "jpeg-ultrahdr")]
#[test]
fn decode_gain_map_returns_none_for_png() {
    let img = rgb8_image(32, 32);
    let bytes = encode_png_rgb8(img.as_ref());
    let (_, gainmap) = DecodeRequest::new(&bytes)
        .decode_gain_map()
        .expect("decode_gain_map shouldn't error on PNG");
    assert!(
        gainmap.is_none(),
        "PNG must not produce a gain map (no spec support)"
    );
}

// ─── Robustness ──────────────────────────────────────────────────────────

#[test]
fn decode_handles_garbage_png_without_panic() {
    let bytes: Vec<u8> = b"\x89PNG\r\n\x1a\ngarbage past PNG signature".to_vec();
    let _ = DecodeRequest::new(&bytes).decode_full_frame();
}

// ─── PNG cICP chunk extraction (documented gap) ──────────────────────────

/// PNG 1.3+ defines the `cICP` chunk for CICP-style colour metadata
/// (ITU-T H.273). `zenpng` doesn't currently parse it and surface it as
/// `SourceColor.cicp`. This test pins the gap — the day zenpng surfaces
/// cICP, this passes without further code changes here.
///
/// The fixture would be a hand-crafted PNG with a `cICP` chunk
/// declaring (1, 13, 0, true) — sRGB code points. Today we lack a
/// generator; the assertion shape is recorded.
#[ignore = "zenpng doesn't parse the cICP chunk and surface it as SourceColor.cicp yet"]
#[test]
fn png_cicp_chunk_is_extracted_into_source_color() {
    panic!(
        "Pending: hand-craft a PNG with cICP chunk declaring (1,13,0,true). \
         Assertion: decoded.info().source_color.cicp == Some(Cicp {{ \
         color_primaries: 1, transfer_characteristics: 13, .. }})"
    );
}
