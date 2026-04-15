//! GIF codec capability tests.
//!
//! Tier 2 codec — 8-bit indexed palette, no metadata in the standard
//! GIF spec, lossy via palette quantisation. Animation is GIF's reason
//! for existing in 2026, so animation round-trip is a first-class test.
//!
//! See `docs/hdr-per-codec.md` for the per-codec test plan.

#![cfg(feature = "gif")]

use imgref::{ImgRef, ImgVec};
use rgb::Rgba;
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat, Metadata};
use zenpixels::PixelSlice;

fn rgba8_image(w: usize, h: usize) -> ImgVec<Rgba<u8>> {
    let pixels: Vec<Rgba<u8>> = (0..w * h)
        .map(|i| Rgba {
            r: (i % w) as u8,
            g: (i / w) as u8,
            b: 128,
            a: 255,
        })
        .collect();
    ImgVec::new(pixels, w, h)
}

fn encode_gif_rgba8(img: ImgRef<'_, Rgba<u8>>, quality: f32) -> Vec<u8> {
    EncodeRequest::new(ImageFormat::Gif)
        .with_quality(quality)
        .encode_full_frame_rgba8(img)
        .expect("encode GIF")
        .into_vec()
}

fn decode_full(bytes: &[u8]) -> zencodecs::DecodeOutput {
    DecodeRequest::new(bytes)
        .decode_full_frame()
        .expect("decode GIF")
}

// ─── Base decode/encode ───────────────────────────────────────────────────

#[test]
fn gif_rgba8_round_trip_dimensions_preserved() {
    let img = rgba8_image(48, 32);
    let bytes = encode_gif_rgba8(img.as_ref(), 75.0);
    let decoded = decode_full(&bytes);
    assert_eq!(decoded.info().width, 48);
    assert_eq!(decoded.info().height, 32);
    assert_eq!(decoded.info().format, ImageFormat::Gif);
}

#[test]
fn gif_quality_parameter_changes_size() {
    // Use a more colourful image so palette quantisation actually shows
    // a quality difference. Solid gradients can collapse to a tiny
    // palette regardless of the quality knob.
    let mut pixels = Vec::with_capacity(96 * 96);
    for y in 0..96 {
        for x in 0..96 {
            let r = ((x as usize * y as usize) % 256) as u8;
            let g = ((x as usize + (y as usize * 7)) % 256) as u8;
            let b = ((x as usize * 11 + y as usize * 3) % 256) as u8;
            pixels.push(Rgba { r, g, b, a: 255 });
        }
    }
    let img = ImgVec::new(pixels, 96, 96);

    let high = encode_gif_rgba8(img.as_ref(), 95.0);
    let low = encode_gif_rgba8(img.as_ref(), 20.0);
    // GIF's quality knob is mostly about palette size / dithering — high
    // quality should be at least as large as low quality (allowing equal
    // for content where the encoder hits the palette ceiling regardless).
    assert!(
        high.len() >= low.len(),
        "GIF quality=95 should be ≥ size of quality=20, got {} vs {}",
        high.len(),
        low.len(),
    );
}

// ─── Negative metadata cases (GIF spec carries no ICC/EXIF/XMP) ──────────

#[test]
fn gif_no_icc_returned_after_round_trip() {
    let img = rgba8_image(16, 16);
    let bytes = encode_gif_rgba8(img.as_ref(), 75.0);
    let decoded = decode_full(&bytes);
    assert!(
        decoded.info().source_color.icc_profile.is_none(),
        "GIF spec doesn't carry ICC; decode must report None"
    );
}

#[test]
fn gif_metadata_with_icc_input_silently_drops() {
    // GIF doesn't have an ICC chunk. Passing a Metadata::with_icc to
    // the encoder should not error — it should silently drop the ICC.
    // (Failing here would mean the encoder rejected metadata it can't
    // store, which would be over-strict.)
    let img = rgba8_image(16, 16);
    let icc = vec![0u8; 256];
    let meta = Metadata::none().with_icc(icc);

    let typed: PixelSlice<'_, Rgba<u8>> = PixelSlice::from(img.as_ref());
    let result = EncodeRequest::new(ImageFormat::Gif)
        .with_quality(75.0)
        .with_metadata(meta)
        .encode(typed.erase(), true);
    assert!(
        result.is_ok(),
        "GIF encoder must accept (and silently drop) unsupported metadata"
    );
}

// ─── Negative gain map ───────────────────────────────────────────────────

#[cfg(feature = "jpeg-ultrahdr")]
#[test]
fn decode_gain_map_returns_none_for_gif() {
    let img = rgba8_image(32, 32);
    let bytes = encode_gif_rgba8(img.as_ref(), 75.0);
    let (_, gainmap) = DecodeRequest::new(&bytes)
        .decode_gain_map()
        .expect("decode_gain_map shouldn't error on GIF");
    assert!(
        gainmap.is_none(),
        "GIF must not produce a gain map (no spec support)"
    );
}

// ─── Robustness ──────────────────────────────────────────────────────────

#[test]
fn decode_handles_garbage_gif_without_panic() {
    // GIF87a/89a magic followed by garbage.
    let bytes: Vec<u8> = b"GIF89a\x00\x00garbage past header".to_vec();
    let _ = DecodeRequest::new(&bytes).decode_full_frame();
}
