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

// ─── 16-bit (16 bpc) round-trip ──────────────────────────────────────────

/// zenpng preserves 16-bit precision end-to-end. Encoding RGBA16 input
/// and decoding must return the same u16 bytes (PNG is lossless) and
/// the decoded pixel buffer's channel type must be `U16`, not narrowed
/// to `U8` by the trait layer.
#[test]
fn png_rgba16_round_trip_is_lossless_pixel_byte_equal() {
    use imgref::ImgVec;
    use rgb::Rgba;
    use zenpixels::{ChannelType, PixelSlice};

    let pixels: Vec<Rgba<u16>> = (0..32 * 32)
        .map(|i| Rgba {
            r: (i * 257) as u16,
            g: (i * 251) as u16,
            b: (65535 - i * 257) as u16,
            a: 0xC0DE,
        })
        .collect();
    let img = ImgVec::new(pixels, 32, 32);
    let typed: PixelSlice<'_, Rgba<u16>> = PixelSlice::from(img.as_ref());

    let bytes = EncodeRequest::new(ImageFormat::Png)
        .encode(typed.erase(), false)
        .expect("encode PNG rgba16")
        .into_vec();

    let decoded = DecodeRequest::new(&bytes)
        .decode_full_frame()
        .expect("decode PNG");
    let desc = decoded.pixels().descriptor();
    assert_eq!(
        desc.channel_type(),
        ChannelType::U16,
        "16-bit PNG must not narrow to U8; got {:?}",
        desc.channel_type()
    );

    let actual: &[u8] = decoded.pixels().as_strided_bytes();
    let original: &[u8] = bytemuck::cast_slice(img.buf());
    assert_eq!(
        actual.len(),
        original.len(),
        "PNG 16bpc must preserve buffer size exactly"
    );
    assert_eq!(
        actual, original,
        "PNG is lossless — 16bpc pixels must be byte-equal"
    );
}

/// External 16-bit PNG fixtures from pngsuite (via image-rs test corpus):
/// decode must surface `ChannelType::U16` and `info.source_color.bit_depth
/// = Some(16)`.
#[ignore = "needs codec-corpus PNG pngsuite at /mnt/v/GitHub/codec-corpus/; run with cargo test -- --ignored"]
#[test]
fn png_external_16bit_samples_decode_at_16bpc() {
    use zenpixels::ChannelType;
    let files = [
        "/mnt/v/GitHub/codec-corpus/image-rs/test-images/png/16bpc/basi0g16.png",
        "/mnt/v/GitHub/codec-corpus/image-rs/test-images/png/16bpc/basn2c16.png",
        "/mnt/v/GitHub/codec-corpus/image-rs/test-images/png/16bpc/basn6a16.png",
    ];
    for f in &files {
        let bytes = std::fs::read(f).expect("fixture must be present");
        let decoded = DecodeRequest::new(&bytes)
            .decode_full_frame()
            .expect("decode 16bpc PNG");
        assert_eq!(
            decoded.pixels().descriptor().channel_type(),
            ChannelType::U16,
            "{}: expected U16 channel type",
            f
        );
        assert_eq!(
            decoded.info().source_color.bit_depth,
            Some(16),
            "{}: expected bit_depth=Some(16)",
            f
        );
    }
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

// ─── PNG cICP chunk round-trip ──────────────────────────────────────────

/// PNG 1.3+ defines the `cICP` chunk for CICP-style color metadata
/// (ITU-T H.273). zenpng's encode + decode both wire the chunk through
/// (zenpng/src/codec.rs:2349 → with_cicp on encode,
/// zenpng/src/decoder/mod.rs:58 → ancillary.cicp on decode), and the
/// zencodecs adapter forwards Metadata::cicp into PngWriteMetadata.
///
/// This test verifies the full round-trip:
/// `Metadata::with_cicp(BT2100_PQ)` → cICP chunk → decode →
/// `info.source_color.cicp == Some(BT2100_PQ)`.
#[test]
fn png_cicp_chunk_round_trips() {
    use zencodec::Cicp;
    let img = rgb8_image(32, 32);
    // BT.2100 PQ — primaries=9, transfer=16, matrix=9, full_range=true.
    // (Picked deliberately non-default to prove it's the value we set.)
    let cicp = Cicp::BT2100_PQ;
    let meta = Metadata::none().with_cicp(cicp);

    let bytes = encode_png_with_meta(img.as_ref(), meta);
    let decoded = decode_full(&bytes);

    let extracted = decoded
        .info()
        .source_color
        .cicp
        .expect("CICP must round-trip on PNG via the cICP chunk");
    assert_eq!(extracted.color_primaries, 9);
    assert_eq!(extracted.transfer_characteristics, 16);
    assert_eq!(extracted.matrix_coefficients, 9);
    assert!(extracted.full_range);
}
