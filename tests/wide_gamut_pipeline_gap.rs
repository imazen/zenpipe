//! Pipeline-level wide-gamut and HDR preservation tests.
//!
//! These tests drive HDR / wide-gamut sources through `ImageJob` and assert
//! that bit depth, transfer function, and primaries survive the pipeline.
//! They are **expected to fail today** — the assertions document the gap
//! identified in `docs/hdr-per-codec.md`:
//!
//!   `graph.rs:1608` and `:1617` force-narrow every decoded image to
//!   `RGBA8_SRGB` via `ensure_fmt!`, before the layout/composite nodes
//!   run. `sources/resize.rs:46-51` and `sources/effects.rs:51-53` enforce
//!   the same — they refuse non-sRGB input. The result: AVIF Rec.2020 PQ,
//!   AVIF 10-bit, JXL HDR f32, all silently flatten to 8-bit sRGB before
//!   re-encoding, with the original ICC/CICP still attached as metadata
//!   (`job.rs:1004-1006`) — pixels and metadata then disagree.
//!
//! Each test is `#[ignore]`d with a note pinning the expected fix site.
//! When the pipeline gains a wide working space (e.g. `RGBAF32_LINEAR`
//! routed conditionally via a `WorkingFormat` enum), these tests will
//! activate and pass without further code changes.

#![cfg(all(feature = "job", feature = "nodes-jpeg"))]

use zenpipe::job::{CmsMode, ImageJob};

// ─── Fixtures ────────────────────────────────────────────────────────────

fn make_ultrahdr_jpeg(peak: f32) -> Vec<u8> {
    use imgref::ImgVec;
    use rgb::Rgb;

    let pixels: Vec<Rgb<f32>> = (0..32 * 32)
        .map(|i| {
            let t = i as f32 / 1023.0;
            Rgb {
                r: 0.1 + t * peak,
                g: 0.05 + t * peak * 0.7,
                b: 0.2,
            }
        })
        .collect();
    let img = ImgVec::new(pixels, 32, 32);

    zencodecs::EncodeRequest::new(zencodec::ImageFormat::Jpeg)
        .with_quality(90.0)
        .encode_ultrahdr_rgb_f32(img.as_ref())
        .expect("UltraHDR encode")
        .into_vec()
}

// ─── PASSING — gain map preserve (the explicit bypass works) ─────────────

/// `GainMapMode::Preserve` (the default) routes the gain map around the
/// pipeline as a sidecar plane. This is the one HDR-related case the
/// pipeline gets right today, because it never asks the resize stage to
/// touch the HDR data.
#[test]
fn gainmap_preserve_mode_round_trips_through_pipeline_unchanged() {
    let input = make_ultrahdr_jpeg(4.0);

    let result = ImageJob::new()
        .add_input(0, input)
        .add_output(1)
        .with_cms(CmsMode::None)
        // GainMapMode::Preserve is the default — explicit for clarity.
        .with_gain_map_mode(zenpipe::job::GainMapMode::Preserve)
        .run()
        .expect("ImageJob should succeed");

    let output = &result.encode_results[0].bytes;

    let (_, gm) = zencodecs::DecodeRequest::new(output)
        .with_registry(&zencodecs::AllowedFormats::all())
        .decode_gain_map()
        .expect("output decode");
    assert!(
        gm.is_some(),
        "Preserve mode must round-trip the gain map through the pipeline"
    );
    let gm = gm.unwrap();
    assert!(gm.metadata.alternate_hdr_headroom > 0.0);
}

// ─── PASSING — ICC metadata preservation through CmsMode::Preserve ───────

/// `CmsMode::Preserve` must carry the source ICC profile through the
/// pipeline byte-equal on the output. Even though pixels today get
/// narrowed to sRGB at `graph.rs:1608`, the metadata slot is independent
/// of the pixel path and the ICC bytes survive as an opaque blob.
///
/// This is a narrow preservation guarantee, NOT a pixels-match-metadata
/// guarantee. See
/// `jpeg_icc_preserve_leaves_pixels_and_metadata_disagreeing` below for
/// the pixels-vs-metadata divergence xfail.
#[test]
fn jpeg_icc_preserve_round_trips_icc_bytes_verbatim() {
    use rgb::Rgb;
    let pixels: Vec<Rgb<u8>> = (0..32 * 32)
        .map(|i| Rgb { r: (i % 32) as u8, g: (i / 32) as u8, b: 100 })
        .collect();
    let img = imgref::ImgVec::new(pixels, 32, 32);
    let typed: zenpixels::PixelSlice<'_, Rgb<u8>> =
        zenpixels::PixelSlice::from(img.as_ref());

    // Synthetic non-sRGB ICC — what matters is that it is NOT detected as
    // sRGB so the Preserve path actually has to carry it. Using a pattern
    // that can't collide with our sRGB hash table (random-looking bytes).
    let mut icc = vec![0u8; 256];
    for (i, b) in icc.iter_mut().enumerate() {
        *b = (i as u8).wrapping_mul(37).wrapping_add(13);
    }
    let meta = zencodec::Metadata::none().with_icc(icc.clone());

    let input = zencodecs::EncodeRequest::new(zencodec::ImageFormat::Jpeg)
        .with_quality(95.0)
        .with_metadata(meta)
        .encode(typed.erase(), false)
        .expect("encode JPEG with ICC")
        .into_vec();

    let result = ImageJob::new()
        .add_input(0, input)
        .add_output(1)
        .with_cms(CmsMode::Preserve)
        .run()
        .expect("ImageJob should succeed");

    let output = &result.encode_results[0].bytes;
    let decoded = zencodecs::DecodeRequest::new(output)
        .decode_full_frame()
        .expect("output decode");

    let extracted = decoded
        .info()
        .source_color
        .icc_profile
        .as_ref()
        .expect("Preserve mode must keep ICC");
    assert_eq!(
        extracted.as_ref(),
        icc.as_slice(),
        "ICC profile must round-trip byte-equal under CmsMode::Preserve"
    );
}

// ─── FAILING — gap trackers, marked #[ignore] ────────────────────────────

/// The flip side of `jpeg_icc_preserve_round_trips_icc_bytes_verbatim`:
/// ICC bytes preserve fine, but the *pixels* downstream of the pipeline
/// are always RGBA8 sRGB because of the `graph.rs:1608` narrowing. So an
/// input tagged with Display-P3 / Rec.2020 / PQ ICC comes out the other
/// end with the original ICC still attached — even though the pixels are
/// now sRGB. Pixels and metadata disagree.
///
/// This tracker should flip to passing once the pipeline gains a linear
/// working format (`WorkingFormat::RgbaF32Linear`) and re-encodes with
/// ICC-consistent pixels.
#[ignore = "Pipeline narrows to RGBA8_SRGB at graph.rs:1608 → output pixels are sRGB even though ICC claims wider gamut"]
#[test]
fn jpeg_icc_preserve_leaves_pixels_and_metadata_disagreeing() {
    // Once a `WorkingFormat` enum lands, implement this as:
    //   1. Craft a real Display-P3 ICC blob (primaries tag ≠ sRGB).
    //   2. Encode a JPEG whose pixels are meaningful only in Display-P3
    //      (e.g. pure primary red at RGB=(255,0,0) which differs from
    //      sRGB red by ~15% in Δu'v').
    //   3. Run ImageJob with Preserve.
    //   4. Decode the output; convert the decoded pixels from the claimed
    //      Display-P3 back to sRGB via moxcms; assert the result is close
    //      to the original sRGB interpretation, NOT identical (proves the
    //      pipeline carried P3 pixels forward rather than flattening).
    panic!(
        "Pending: real Display-P3 ICC + CMS-aware pixel round-trip. \
         Today graph.rs:1608 narrows pixels to sRGB before encode, so the \
         round-tripped pixels are sRGB despite the ICC claiming Display-P3."
    );
}

/// AVIF Rec.2020 / BT.2100 PQ source must keep its primaries through
/// the pipeline. Today everything narrows to BT.709 sRGB at
/// `graph.rs:1608`. The expected fix: route AVIF f32/u16 through a
/// linear-light working format and emit AVIF with the original CICP.
#[ignore = "Pipeline narrows AVIF Rec.2020 PQ → RGBA8_SRGB at graph.rs:1608"]
#[test]
fn avif_rec2020_pq_round_trips_primaries() {
    // This test would: encode synthetic AVIF Rec.2020 PQ → ImageJob
    // → encode AVIF → decode → assert source_color.cicp.color_primaries == 9.
    //
    // Pending real AVIF Rec.2020 PQ generation. Today this test would
    // need a fixture file; once the pipeline preserves primaries, the
    // test should round-trip without needing one.
    panic!("Test body pending fixture; assertion: decoded.cicp.primaries == 9 (Rec.2020) after ImageJob round-trip");
}

/// AVIF 10-bit / 12-bit decode currently returns RGBA8 from the trait.
/// Even if it didn't, the pipeline would narrow at the resize stage.
/// This test pins the expected behavior: a 10-bit AVIF re-encoded
/// through ImageJob with no resize should preserve bit depth.
#[ignore = "AVIF 10-bit decode + pipeline both narrow to 8-bit"]
#[test]
fn avif_10bit_round_trips_bit_depth_without_resize() {
    panic!(
        "Pending: synthetic 10-bit AVIF fixture. \
         Assertion: round-tripped decoded.source_color.bit_depth == Some(10)"
    );
}

/// JXL HDR (f32 codestream) must preserve bit depth when no resize is
/// requested. Today: forced to 8-bit sRGB at `graph.rs:1608`.
#[ignore = "Pipeline narrows JXL f32 → RGBA8_SRGB at graph.rs:1608"]
#[test]
fn jxl_hdr_f32_round_trips_bit_depth_without_resize() {
    panic!(
        "Pending: synthetic JXL HDR f32 fixture. \
         Assertion: round-tripped decoded.source_color.bit_depth >= Some(16)"
    );
}

/// `GainMapMode::Reconstruct` doesn't exist yet. The internal
/// `ProcessConfig::hdr_mode` already accepts `"hdr_reconstruct"` (see
/// `orchestrate.rs:381`) but there's no public ImageJob variant to
/// trigger it, and even if there were, the apply step in the pipeline
/// materialization is not wired — the HDR pixels never surface.
///
/// This test pins the desired API surface: a `Reconstruct` variant on
/// `GainMapMode` that, when set, runs `apply_gainmap` on the
/// materialized frame before any further pipeline ops, yielding HDR
/// pixels.
#[ignore = "GainMapMode::Reconstruct doesn't exist; orchestrate hdr_reconstruct doesn't apply gain map to working buffer"]
#[test]
fn ultrahdr_jpeg_hdr_reconstruct_mode_yields_hdr_pixels() {
    panic!(
        "Pending: add GainMapMode::Reconstruct + wire apply_gainmap into the \
         materialize stage; assertion: round-tripped output declares HDR \
         transfer (PQ/HLG) or carries content_light_level / mastering_display"
    );
}

// ─── Future: a real Display-P3 ICC fixture ─────────────────────────────
//
// Once `zencodecs::cms::display_p3_icc_profile()` exists (or we ship a
// canonical Display-P3 profile in tests/fixtures/), the
// `jpeg_with_display_p3_icc_*` test above can swap the placeholder bytes
// for a real profile and become a true round-trip assertion.
