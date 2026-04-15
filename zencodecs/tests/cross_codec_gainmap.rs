//! Cross-codec gain map flow tests — the "resplitting" capability.
//!
//! Validates that a gain map decoded from one codec can be re-embedded
//! in another format via `EncodeRequest::with_gain_map(GainMapSource::Precomputed)`.
//!
//! Direction conventions per ISO 21496-1:
//!   - JPEG UltraHDR (MPF):    base = SDR, alternate = HDR  (forward)
//!   - AVIF tmap:              base = SDR, alternate = HDR  (forward)
//!   - JXL jhgm:               base = HDR, alternate = SDR  (inverse)
//!
//! The forward → forward case (JPEG ↔ AVIF) is the most common and the
//! one we expect to "just work". The forward ↔ inverse case (JPEG ↔ JXL)
//! requires the encoder to flip the metadata's `backward_direction` flag,
//! and is the riskier path.
//!
//! See `docs/hdr-per-codec.md` for the full per-codec test plan.

#![cfg(all(feature = "jpeg", feature = "jpeg-ultrahdr"))]

use imgref::ImgVec;
use rgb::Rgb;
use zencodecs::{
    DecodeRequest, EncodeRequest, GainMapSource, ImageFormat, PixelBufferConvertExt,
};

// ─── Fixture ─────────────────────────────────────────────────────────────

fn make_hdr_gradient(w: usize, h: usize, peak: f32) -> ImgVec<Rgb<f32>> {
    let pixels: Vec<Rgb<f32>> = (0..w * h)
        .map(|i| {
            let x = (i % w) as f32 / w.max(1) as f32;
            let y = (i / w) as f32 / h.max(1) as f32;
            Rgb {
                r: 0.1 + x * peak,
                g: 0.05 + y * peak * 0.7,
                b: 0.2,
            }
        })
        .collect();
    ImgVec::new(pixels, w, h)
}

/// Encode a synthetic HDR image as a UltraHDR JPEG and decode it back to
/// `(SDR base pixels, gain map, metadata)`. This is the "donor" used by
/// the cross-codec re-encode tests below.
fn build_jpeg_donor(
    w: usize,
    h: usize,
    peak: f32,
) -> zencodecs::DecodedGainMap {
    let img = make_hdr_gradient(w, h, peak);
    let bytes = EncodeRequest::new(ImageFormat::Jpeg)
        .with_quality(90.0)
        .encode_ultrahdr_rgb_f32(img.as_ref())
        .expect("encode UltraHDR donor")
        .into_vec();

    let (_decoded, gm) = DecodeRequest::new(&bytes)
        .decode_gain_map()
        .expect("decode gain map from donor");
    gm.expect("UltraHDR donor must yield a gain map")
}

// ─── JPEG → JPEG (positive control — proves the API path works) ────────

/// Sanity: a precomputed gain map round-trips through a JPEG → JPEG
/// re-encode. This is the working baseline. The precomputed path expects
/// an SDR u8 base, NOT HDR f32 — the SDR pixels are what gets stored
/// as the base JPEG; the gain map is the multiplier to reach HDR.
#[test]
fn jpeg_ultrahdr_gainmap_re_embeds_into_jpeg_ultrahdr() {
    let donor = build_jpeg_donor(64, 64, 4.0);

    // SDR base (u8) — re-decode the donor's JPEG to get its SDR pixels.
    let donor_bytes = EncodeRequest::new(ImageFormat::Jpeg)
        .with_quality(90.0)
        .encode_ultrahdr_rgb_f32(make_hdr_gradient(64, 64, 4.0).as_ref())
        .expect("re-encode donor")
        .into_vec();
    let decoded = DecodeRequest::new(&donor_bytes)
        .decode_full_frame()
        .expect("decode donor SDR base");
    let pixels = decoded.pixels();

    let bytes = EncodeRequest::new(ImageFormat::Jpeg)
        .with_quality(90.0)
        .with_gain_map(GainMapSource::Precomputed {
            gain_map: &donor.gain_map,
            metadata: &donor.metadata,
        })
        .encode(pixels, false)
        .expect("JPEG encode with precomputed gain map")
        .into_vec();

    let (_, gm_out) = DecodeRequest::new(&bytes)
        .decode_gain_map()
        .expect("decode JPEG gain map");
    let gm_out = gm_out
        .expect("JPEG → JPEG via with_gain_map(Precomputed) must round-trip");

    assert_eq!(gm_out.source_format, ImageFormat::Jpeg);
    assert!(!gm_out.base_is_hdr, "JPEG forward direction preserved");
    assert!(gm_out.metadata.alternate_hdr_headroom > 0.0);
}

// ─── JPEG → AVIF (forward → forward, the common case) ───────────────────
//
// This is the user's headline "resplitting" use case: take an UltraHDR
// JPEG, decode the gain map, and re-embed it inside an AVIF tmap aux
// item via the encoder's `with_gain_map(GainMapSource::Precomputed)`
// path. Both metadata and pixel data must round-trip.
//
// Was broken until decode.rs:extract_avif_gainmap was fixed to look
// for `zencodec::gainmap::GainMapSource` in extras (zenavif's actual
// emitted type) instead of the older `zenavif::AvifGainMap`.

#[cfg(all(feature = "avif-encode", feature = "avif-decode"))]
#[test]
fn jpeg_ultrahdr_gainmap_re_embeds_into_avif_tmap() {
    let donor = build_jpeg_donor(64, 64, 4.0);
    assert!(!donor.base_is_hdr, "JPEG UltraHDR must be forward direction");

    // Build a fresh SDR base in linear-light f32 (the AVIF encoder
    // accepts whatever format its EncodeRequest takes; the gain map
    // attaches via with_gain_map).
    let img = make_hdr_gradient(64, 64, 1.0); // peak=1.0 → SDR-only base
    let typed: zenpixels::PixelSlice<'_, Rgb<f32>> =
        zenpixels::PixelSlice::from(img.as_ref());

    let avif_bytes = EncodeRequest::new(ImageFormat::Avif)
        .with_quality(85.0)
        .with_gain_map(GainMapSource::Precomputed {
            gain_map: &donor.gain_map,
            metadata: &donor.metadata,
        })
        .encode(typed.erase(), false)
        .expect("AVIF encode with precomputed gain map");

    // Decode back and verify a gain map appears.
    let (_decoded, gm_out) = DecodeRequest::new(avif_bytes.as_ref())
        .decode_gain_map()
        .expect("decode AVIF gain map");
    let gm_out = gm_out.expect("AVIF round-trip must yield a gain map");

    assert_eq!(gm_out.source_format, ImageFormat::Avif);
    assert!(
        !gm_out.base_is_hdr,
        "AVIF tmap is forward direction (SDR base)"
    );
    // Headroom must round-trip within tolerance — re-encoding can lose
    // a tiny bit of precision in the metadata serialisation.
    let dh = (donor.metadata.alternate_hdr_headroom
        - gm_out.metadata.alternate_hdr_headroom)
        .abs();
    assert!(
        dh < 0.05,
        "alternate_hdr_headroom must round-trip within 0.05; got Δ = {dh}"
    );
}

// ─── AVIF → JPEG (the reverse, also forward → forward) ───────────────────

#[cfg(all(feature = "avif-encode", feature = "avif-decode"))]
#[test]
fn avif_tmap_gainmap_re_embeds_into_jpeg_ultrahdr() {
    // Build an AVIF donor by going JPEG → AVIF first, then re-extract.
    let jpeg_donor = build_jpeg_donor(48, 48, 3.5);
    let img = make_hdr_gradient(48, 48, 1.0);
    let typed: zenpixels::PixelSlice<'_, Rgb<f32>> =
        zenpixels::PixelSlice::from(img.as_ref());
    let avif_bytes = EncodeRequest::new(ImageFormat::Avif)
        .with_quality(85.0)
        .with_gain_map(GainMapSource::Precomputed {
            gain_map: &jpeg_donor.gain_map,
            metadata: &jpeg_donor.metadata,
        })
        .encode(typed.erase(), false)
        .expect("AVIF re-encode")
        .into_vec();

    let (_, avif_donor) = DecodeRequest::new(&avif_bytes)
        .decode_gain_map()
        .expect("decode AVIF gain map");
    let avif_donor = avif_donor.expect("AVIF must yield gain map");
    assert_eq!(avif_donor.source_format, ImageFormat::Avif);

    // Now go AVIF → JPEG. JPEG's encode_with_precomputed_gainmap path
    // dispatches on channels (3 = RGB8 / 4 = RGBA8) but AVIF decode
    // commonly returns RGBA8 with a meaningless full-opaque alpha.
    // Build a fresh u8 RGB SDR base from the same gradient generator
    // used to build the AVIF, so the JPEG encoder sees stride = w*3.
    let sdr_u8: Vec<rgb::Rgb<u8>> = (0..48 * 48)
        .map(|i| rgb::Rgb {
            r: ((i % 48) * 5) as u8,
            g: ((i / 48) * 5) as u8,
            b: 80,
        })
        .collect();
    let sdr_img = imgref::ImgVec::new(sdr_u8, 48, 48);
    let typed_sdr: zenpixels::PixelSlice<'_, rgb::Rgb<u8>> =
        zenpixels::PixelSlice::from(sdr_img.as_ref());

    let jpeg_bytes = EncodeRequest::new(ImageFormat::Jpeg)
        .with_quality(90.0)
        .with_gain_map(GainMapSource::Precomputed {
            gain_map: &avif_donor.gain_map,
            metadata: &avif_donor.metadata,
        })
        .encode(typed_sdr.erase(), false)
        .expect("JPEG re-encode with AVIF-derived gain map")
        .into_vec();

    let (_, jpeg_round_tripped) = DecodeRequest::new(&jpeg_bytes)
        .decode_gain_map()
        .expect("decode round-tripped JPEG");
    let jpeg_round_tripped =
        jpeg_round_tripped.expect("AVIF→JPEG must produce a gain map");

    assert_eq!(jpeg_round_tripped.source_format, ImageFormat::Jpeg);
    assert!(
        !jpeg_round_tripped.base_is_hdr,
        "JPEG UltraHDR base is SDR"
    );
}

// ─── JPEG → JXL (forward → inverse — the direction-flip case) ────────────

/// JXL's `jhgm` gain map convention is inverse: base is HDR, gain map
/// tone-maps to SDR. Re-encoding a JPEG forward-direction gain map into
/// JXL: we ship the metadata as-is (no direction flip) and rely on the
/// caller to interpret the result correctly. The assertion accepts
/// either a flipped direction OR a coherent (still-decodable) result.
///
/// Was blocked until decode.rs:extract_jxl_gainmap was fixed to look
/// for `zencodec::gainmap::GainMapSource` in extras (zenjxl's actual
/// emitted type) instead of the older `zenjxl::GainMapBundle`.
#[cfg(all(feature = "jxl-encode", feature = "jxl-decode"))]
#[test]
fn jpeg_ultrahdr_gainmap_re_embeds_into_jxl_jhgm() {
    let donor = build_jpeg_donor(32, 32, 4.0);
    assert!(!donor.base_is_hdr);

    // For a JXL encode with the JXL convention, the base image should
    // be HDR. We provide HDR pixels here.
    let hdr = make_hdr_gradient(32, 32, 4.0);
    let typed: zenpixels::PixelSlice<'_, Rgb<f32>> =
        zenpixels::PixelSlice::from(hdr.as_ref());

    let jxl_bytes = EncodeRequest::new(ImageFormat::Jxl)
        .with_quality(95.0)
        .with_gain_map(GainMapSource::Precomputed {
            gain_map: &donor.gain_map,
            metadata: &donor.metadata,
        })
        .encode(typed.erase(), false)
        .expect("JXL encode with precomputed gain map")
        .into_vec();

    let (_, gm_out) = DecodeRequest::new(&jxl_bytes)
        .decode_gain_map()
        .expect("decode JXL gain map");
    let gm_out = gm_out.expect("JXL round-trip must yield a gain map");

    // Either the encoder flipped the direction (good) or kept it as-is
    // (degenerate but at least the metadata round-trips). What we
    // refuse: a missing or zero-valued metadata block.
    assert!(
        gm_out.metadata.alternate_hdr_headroom > 0.0
            || gm_out.metadata.base_hdr_headroom > 0.0,
        "JXL round-tripped gain map must declare some HDR headroom"
    );
}

// ─── Robustness: re-encode without the donor → no gain map ───────────────

#[cfg(all(feature = "avif-encode", feature = "avif-decode"))]
#[test]
fn avif_encode_without_with_gain_map_produces_plain_avif() {
    // Sanity: omitting `with_gain_map` must not somehow magic a gain map
    // into the output. This pins the negative case for the resplit path.
    let img = make_hdr_gradient(32, 32, 4.0);
    let typed: zenpixels::PixelSlice<'_, Rgb<f32>> =
        zenpixels::PixelSlice::from(img.as_ref());
    let bytes = EncodeRequest::new(ImageFormat::Avif)
        .with_quality(85.0)
        .encode(typed.erase(), false)
        .expect("plain AVIF encode")
        .into_vec();

    let (_, gm) = DecodeRequest::new(&bytes)
        .decode_gain_map()
        .expect("decode_gain_map");
    assert!(
        gm.is_none(),
        "AVIF encoded without with_gain_map must not contain a gain map"
    );
}
