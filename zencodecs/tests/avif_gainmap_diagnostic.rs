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
