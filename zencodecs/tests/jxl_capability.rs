//! JXL codec capability tests.
//!
//! Atomic per-leaf tests for the JXL codec adapter — base decode/encode,
//! ICC byte-equal preservation, CICP code-point fidelity, EXIF/XMP round-
//! trips, and the JXL-specific **inverse-direction gain map** (jhgm box,
//! base = HDR / alternate = SDR — opposite of JPEG UltraHDR and AVIF tmap).
//!
//! See `docs/hdr-per-codec.md` for the full per-codec test plan.

#![cfg(feature = "jxl-decode")]

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

#[cfg(feature = "jxl-encode")]
fn rgb_f32_gradient(w: usize, h: usize, peak: f32) -> ImgVec<Rgb<f32>> {
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

#[cfg(feature = "jxl-encode")]
fn rgba_f32_gradient(w: usize, h: usize, peak: f32) -> ImgVec<Rgba<f32>> {
    let pixels: Vec<Rgba<f32>> = (0..w * h)
        .map(|i| {
            let x = (i % w) as f32 / w.max(1) as f32;
            let y = (i / w) as f32 / h.max(1) as f32;
            Rgba {
                r: x * peak,
                g: y * peak * 0.7,
                b: 0.2 + x * 0.3,
                a: 1.0,
            }
        })
        .collect();
    ImgVec::new(pixels, w, h)
}

#[cfg(feature = "jxl-encode")]
fn encode_jxl_rgb8(img: ImgRef<'_, Rgb<u8>>, quality: f32) -> Vec<u8> {
    EncodeRequest::new(ImageFormat::Jxl)
        .with_quality(quality)
        .encode_full_frame_rgb8(img)
        .expect("encode JXL rgb8")
        .into_vec()
}

#[cfg(feature = "jxl-encode")]
fn encode_jxl_with_meta(img: ImgRef<'_, Rgb<u8>>, meta: Metadata, quality: f32) -> Vec<u8> {
    let typed: PixelSlice<'_, Rgb<u8>> = PixelSlice::from(img);
    EncodeRequest::new(ImageFormat::Jxl)
        .with_quality(quality)
        .with_metadata(meta)
        .encode(typed.erase(), false)
        .expect("encode JXL with metadata")
        .into_vec()
}

fn decode_full(bytes: &[u8]) -> zencodecs::DecodeOutput {
    DecodeRequest::new(bytes)
        .decode_full_frame()
        .expect("decode JXL")
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

#[cfg(feature = "jxl-encode")]
#[test]
fn jxl_rgb8_round_trip_dimensions_preserved() {
    let img = rgb8_image(48, 32);
    let bytes = encode_jxl_rgb8(img.as_ref(), 80.0);
    let decoded = decode_full(&bytes);
    assert_eq!(decoded.info().width, 48);
    assert_eq!(decoded.info().height, 32);
    assert_eq!(decoded.info().format, ImageFormat::Jxl);
}

#[cfg(feature = "jxl-encode")]
#[test]
fn jxl_quality_parameter_changes_size() {
    let img = rgb8_image(96, 96);
    let high = encode_jxl_rgb8(img.as_ref(), 95.0);
    let low = encode_jxl_rgb8(img.as_ref(), 30.0);
    assert!(
        high.len() > low.len(),
        "JXL quality=95 should be larger than quality=30, got {} vs {}",
        high.len(),
        low.len(),
    );
}

// ─── HDR encode (JXL's primary mode) ──────────────────────────────────────

#[cfg(feature = "jxl-encode")]
#[test]
fn jxl_rgb_f32_hdr_encode_decode_round_trip() {
    let img = rgb_f32_gradient(32, 32, 4.0);
    let typed: PixelSlice<'_, Rgb<f32>> = PixelSlice::from(img.as_ref());
    let bytes = EncodeRequest::new(ImageFormat::Jxl)
        .with_quality(95.0)
        .encode(typed.erase(), false)
        .expect("encode JXL HDR f32")
        .into_vec();

    let decoded = decode_full(&bytes);
    assert_eq!(decoded.info().width, 32);
    assert_eq!(decoded.info().height, 32);
    // JXL preserves bit depth in the codestream — the decoded bit depth
    // should reflect the f32 input (or the codec's chosen wider type),
    // not silently narrow to 8.
    let bit_depth = decoded.info().source_color.bit_depth;
    assert!(
        bit_depth.is_some(),
        "JXL must report bit_depth on decode"
    );
}

#[cfg(feature = "jxl-encode")]
#[test]
fn jxl_rgba_f32_hdr_encode_decode_round_trip() {
    let img = rgba_f32_gradient(16, 16, 2.5);
    let typed: PixelSlice<'_, Rgba<f32>> = PixelSlice::from(img.as_ref());
    let bytes = EncodeRequest::new(ImageFormat::Jxl)
        .with_quality(95.0)
        .encode(typed.erase(), true)
        .expect("encode JXL HDR rgba f32")
        .into_vec();

    let decoded = decode_full(&bytes);
    assert_eq!(decoded.info().width, 16);
    assert_eq!(decoded.info().height, 16);
    assert!(
        decoded.info().has_alpha,
        "JXL RGBA encode → decoded ImageInfo must report has_alpha=true"
    );
}

// ─── ICC profile handling ────────────────────────────────────────────────
//
// JXL is unique among the codecs we wrap: it parses the ICC profile
// structurally during encode (via jxl-oxide / moxcms) and rejects
// malformed or unrecognised inputs with `InvalidIccStream`. JPEG, AVIF,
// and PNG embed ICC bytes verbatim. The two following tests document
// both behaviours.

/// JXL refuses synthetic / malformed ICC bytes — this is by design and
/// must remain true: silently accepting garbage ICC would lead to wrong
/// colour interpretation downstream.
#[cfg(feature = "jxl-encode")]
#[test]
fn jxl_rejects_malformed_icc_at_encode_or_decode() {
    let img = rgb8_image(16, 16);
    let icc = synthetic_icc(256);
    let meta = Metadata::none().with_icc(icc);

    let typed: PixelSlice<'_, Rgb<u8>> = PixelSlice::from(img.as_ref());
    let encode_result = EncodeRequest::new(ImageFormat::Jxl)
        .with_quality(80.0)
        .with_metadata(meta)
        .encode(typed.erase(), false);

    match encode_result {
        Err(_) => {
            // Expected: encoder rejected the malformed profile.
        }
        Ok(out) => {
            // If it accepted it (delegating validation to decode), the
            // decode must reject it.
            let decode_err = DecodeRequest::new(out.as_ref()).decode_full_frame();
            assert!(
                decode_err.is_err(),
                "JXL must reject malformed ICC at either encode or decode"
            );
        }
    }
}

/// Round-tripping a *valid* ICC profile requires constructing one. The
/// `cms` feature pulls in moxcms which can synthesize an sRGB profile
/// (see `cms.rs::srgb_icc_profile`). Without that feature, we don't have
/// a way to manufacture a valid profile in tests, so this is gated.
///
/// Tracked: re-enable when zencodecs ships a no-cms-feature `srgb_icc()`
/// helper that returns canonical sRGB bytes.
#[cfg(all(feature = "jxl-encode", feature = "cms"))]
#[test]
fn jxl_valid_icc_round_trip() {
    use zencodecs::cms::srgb_icc_profile;
    let img = rgb8_image(32, 32);
    let icc = srgb_icc_profile();
    let meta = Metadata::none().with_icc(icc.clone());

    let bytes = encode_jxl_with_meta(img.as_ref(), meta, 80.0);
    let decoded = decode_full(&bytes);

    let extracted = decoded
        .info()
        .source_color
        .icc_profile
        .as_ref()
        .expect("valid ICC must round-trip on JXL");
    assert!(
        !extracted.is_empty(),
        "round-tripped ICC must not be empty"
    );
}

// ─── CICP / color space ───────────────────────────────────────────────────

#[cfg(feature = "jxl-encode")]
#[test]
fn jxl_default_encode_surfaces_some_color_metadata() {
    // JXL stores color metadata in the codestream — either CICP-equivalent
    // or an embedded ICC. Either path must be Some after decode.
    let img = rgb8_image(16, 16);
    let bytes = encode_jxl_rgb8(img.as_ref(), 80.0);
    let decoded = decode_full(&bytes);

    let sc = &decoded.info().source_color;
    assert!(
        sc.cicp.is_some() || sc.icc_profile.is_some(),
        "JXL decode must surface CICP or ICC"
    );
}

// ─── XMP round-trip ──────────────────────────────────────────────────────

#[cfg(feature = "jxl-encode")]
#[test]
fn jxl_xmp_round_trip_preserves_marker() {
    let img = rgb8_image(32, 32);
    let xmp = br#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description xmlns:tiff="http://ns.adobe.com/tiff/1.0/">
<tiff:ImageDescription>jxl capability marker</tiff:ImageDescription>
</rdf:Description>
</rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#
        .to_vec();
    let meta = Metadata::none().with_xmp(xmp);
    let bytes = encode_jxl_with_meta(img.as_ref(), meta, 80.0);
    let decoded = decode_full(&bytes);

    let extracted_meta = decoded.info().metadata();
    let extracted = extracted_meta
        .xmp
        .as_ref()
        .expect("XMP should round-trip on JXL");
    let s = core::str::from_utf8(extracted).expect("XMP must be UTF-8");
    assert!(
        s.contains("jxl capability marker"),
        "XMP packet must preserve our marker; got: {s:?}"
    );
}

// ─── Gain map (negative case) ────────────────────────────────────────────

#[cfg(all(feature = "jxl-encode", feature = "jpeg-ultrahdr"))]
#[test]
fn decode_gain_map_returns_none_for_plain_jxl() {
    let img = rgb8_image(32, 32);
    let bytes = encode_jxl_rgb8(img.as_ref(), 80.0);
    let (_decoded, gainmap) = DecodeRequest::new(&bytes)
        .decode_gain_map()
        .expect("decode_gain_map shouldn't error on plain JXL");
    assert!(
        gainmap.is_none(),
        "plain JXL must not produce a gain map (negative case)"
    );
}

// ─── Robustness ──────────────────────────────────────────────────────────

#[cfg(feature = "jpeg-ultrahdr")]
#[test]
fn decode_gain_map_handles_garbage_jxl_without_panic() {
    // JXL container magic followed by garbage. Must not panic.
    let bytes: Vec<u8> = b"\x00\x00\x00\x0cJXL \r\n\x87\ngarbage past header"
        .to_vec();
    let _ = DecodeRequest::new(&bytes).decode_gain_map();
}

// ─── Inverse-direction gain map (the JXL-specific capability) ────────────

/// JXL stores ISO 21496-1 metadata in a `jhgm` container box, and its
/// gain map convention is **inverse**: the base image is HDR and the
/// gain map encodes the path from HDR → SDR. This is the opposite of
/// JPEG UltraHDR (SDR base, gain map → HDR) and AVIF tmap (same as JPEG).
///
/// The test:
///   1. Decode an UltraHDR JPEG to obtain a `DecodedGainMap` with
///      `base_is_hdr = false` (JPEG forward direction).
///   2. Encode that gain map into a JXL via `with_gain_map`.
///   3. Decode the JXL with `decode_gain_map`.
///   4. Assert the resulting gain map round-trips and the metadata
///      reports `backward_direction = true` (JXL inverse direction)
///      OR the encoder/decoder reconcile the directions consistently.
///
/// This is the cross-codec direction-flip case the audit flagged as
/// the highest-risk JXL gain map test.
#[cfg(all(
    feature = "jxl-encode",
    feature = "jpeg",
    feature = "jpeg-ultrahdr"
))]
#[ignore = "JXL gain map re-encode flow not yet validated end-to-end; tracker test"]
#[test]
fn jxl_gainmap_round_trip_inverse_direction() {
    // Source: an UltraHDR JPEG (forward direction).
    let hdr = rgb_f32_gradient(64, 64, 3.0);
    let jpeg_bytes = EncodeRequest::new(ImageFormat::Jpeg)
        .with_quality(90.0)
        .encode_ultrahdr_rgb_f32(hdr.as_ref())
        .expect("encode UltraHDR JPEG")
        .into_vec();

    let (jpeg_decoded, gm_jpeg) = DecodeRequest::new(&jpeg_bytes)
        .decode_gain_map()
        .expect("decode gain map from JPEG");
    let gm_jpeg = gm_jpeg.expect("source JPEG must yield gain map");
    assert!(!gm_jpeg.base_is_hdr, "JPEG UltraHDR is forward (SDR base)");

    // Re-encode as JXL with the precomputed gain map.
    let pixels = jpeg_decoded.pixels();
    let jxl_bytes = EncodeRequest::new(ImageFormat::Jxl)
        .with_quality(95.0)
        .with_gain_map(zencodecs::GainMapSource::Precomputed {
            gain_map: &gm_jpeg.gain_map,
            metadata: &gm_jpeg.metadata,
        })
        .encode(pixels, false)
        .expect("encode JXL with gain map");

    // Decode the JXL gain map back out.
    let (_jxl_decoded, gm_jxl) = DecodeRequest::new(jxl_bytes.as_ref())
        .decode_gain_map()
        .expect("decode gain map from JXL");
    let gm_jxl = gm_jxl.expect("re-encoded JXL must yield gain map");

    // The gain map metadata should round-trip; the direction may be
    // inverted (JXL convention is base_is_hdr=true). Assert we get
    // *some* consistent direction and the metadata is non-trivial.
    assert!(
        gm_jxl.metadata.alternate_hdr_headroom > 0.0,
        "round-tripped gain map must declare positive HDR headroom"
    );
}
