//! Orientation passthrough and rendering through `zenpipe::io`: stored pixels
//! with carried orientation, upright pixels with cleared orientation, the
//! buffering each rendering needs, and privacy-filtered metadata.

use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use zencodec::encode::{EncodeJob, EncoderConfig};
use zencodec::exif::{Exif, TextEncoding};
use zencodec::operation::{DecodeRequest, PolicySet};
use zencodec::storage::{StorageBudget, StorageError};
use zencodec::{CodecSet, Metadata, MetadataPolicy, NoPulse, Orientation};
use zencodec_io::row_encode::{self, Options};
use zenpipe::io::{DecodeOptions, DecodeSource, OrientationCost, PipelineRows, render_orientation};
use zenpipe::sources::MaterializedSource;
use zenpipe::{PipeResult, Source, Strip};
use zenpixels::{PixelDescriptor, PixelSlice};

const W: u32 = 97;
const H: u32 = 65;
const ALL: [Orientation; 8] = [
    Orientation::Identity,
    Orientation::FlipH,
    Orientation::Rotate180,
    Orientation::FlipV,
    Orientation::Transpose,
    Orientation::Rotate90,
    Orientation::Transverse,
    Orientation::Rotate270,
];

fn samples() -> Vec<u8> {
    let mut out = Vec::with_capacity((W * H * 3) as usize);
    for y in 0..H {
        for x in 0..W {
            out.extend_from_slice(&[(x * 2) as u8, (y * 3) as u8, ((x * 7) ^ (y * 5)) as u8]);
        }
    }
    out
}

#[allow(deprecated)] // compatibility writer; authoring a fixture
fn exif(orientation: Orientation, private: bool) -> Vec<u8> {
    let mut exif = Exif::new(TextEncoding::Ascii);
    exif.set_orientation(orientation);
    if private {
        exif.set_artist("Private Person");
        exif.set_copyright("(c) Private Person");
    }
    exif.to_bytes()
}

fn png_with(samples: &[u8], meta: Metadata) -> Vec<u8> {
    let pixels =
        PixelSlice::new(samples, W, H, (W * 3) as usize, PixelDescriptor::RGB8_SRGB).unwrap();
    zenpng::PngEncoderConfig::new()
        .job()
        .with_metadata_policy(meta, MetadataPolicy::PreserveExact)
        .encode(pixels)
        .unwrap()
        .into_vec()
}

fn codecs() -> CodecSet {
    CodecSet::new().with_decoder(zenpng::PngDecoderConfig::new())
}

fn options() -> DecodeOptions {
    DecodeOptions::new(StorageBudget::new(64 << 20, 64))
        .with_strip_rows(16)
        .with_queue_slots(256)
}

fn upright_reference(samples: &[u8], o: Orientation) -> (Vec<u8>, u32, u32) {
    let o = zenlayout::Orientation::from_exif(o.to_exif()).unwrap();
    zenpipe::execute_layout::orient_image(samples, W, H, o, 3)
}

fn collect(source: &mut dyn Source) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(strip) = source.next().unwrap() {
        for r in 0..strip.rows() {
            out.extend_from_slice(strip.row(r));
        }
    }
    out
}

/// Independent PNG read: samples plus the raw eXIf payload.
fn read_png(bytes: &[u8]) -> (u32, u32, Vec<u8>, Option<Vec<u8>>) {
    let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    let exif = reader.info().exif_metadata.as_ref().map(|e| e.to_vec());
    (info.width, info.height, buf, exif)
}

struct Counted<S> {
    inner: S,
    pulls: Arc<AtomicU64>,
}

impl<S: Source> Source for Counted<S> {
    fn next(&mut self) -> PipeResult<Option<Strip<'_>>> {
        self.pulls.fetch_add(1, Ordering::SeqCst);
        self.inner.next()
    }
    fn width(&self) -> u32 {
        self.inner.width()
    }
    fn height(&self) -> u32 {
        self.inner.height()
    }
    fn format(&self) -> zenpipe::PixelFormat {
        self.inner.format()
    }
    fn color_context(&self) -> Option<std::sync::Arc<zenpixels::ColorContext>> {
        self.inner.color_context()
    }
}

#[test]
fn costs_name_reversed_rows_and_axis_swaps() {
    for o in ALL {
        let cost = OrientationCost::new(o, W, H, PixelDescriptor::RGB8_SRGB);
        let local = matches!(o, Orientation::Identity | Orientation::FlipH);
        assert_eq!(cost.streams(), local, "{o:?}");
        assert_eq!(cost.held_rows, if local { 0 } else { H }, "{o:?}");
        assert_eq!(
            cost.held_bytes,
            Some(if local { 0 } else { u64::from(W * H * 3) })
        );
        assert_eq!(cost.swaps_axes, o.swaps_axes(), "{o:?}");
        assert_eq!(
            cost.reverses_rows,
            matches!(o, Orientation::FlipV | Orientation::Rotate180)
        );
        let (w, h) = if o.swaps_axes() { (H, W) } else { (W, H) };
        assert_eq!((cost.width, cost.height), (w, h), "{o:?}");
    }
}

#[test]
fn rendering_matches_the_whole_raster_reference_for_all_eight() {
    let samples = samples();
    for o in ALL {
        for strip_rows in [1, 7, 64] {
            let budget = StorageBudget::new(1 << 20, 4);
            let source =
                MaterializedSource::from_data(samples.clone(), W, H, PixelDescriptor::RGB8_SRGB);
            let mut upright = render_orientation(Box::new(source), o, &budget, strip_rows).unwrap();
            let (expected, w, h) = upright_reference(&samples, o);
            assert_eq!((upright.width(), upright.height()), (w, h), "{o:?}");
            assert_eq!(
                collect(upright.as_mut()),
                expected,
                "{o:?} strip_rows {strip_rows}"
            );
            if !matches!(o, Orientation::Identity | Orientation::FlipH) {
                // One stored raster plus one upright strip, never a second raster.
                let strip = u64::from(w) * 3 * u64::from(strip_rows.min(h));
                assert_eq!(budget.reserved_bytes() as u64, u64::from(W * H * 3) + strip);
            }
        }
    }
}

#[test]
fn an_insufficient_budget_refuses_rendering_before_any_pixel_decode() {
    let file = png_with(
        &samples(),
        Metadata::none().with_exif(exif(Orientation::Rotate90, false)),
    );
    let codecs = codecs();
    let decode = DecodeSource::from_bytes(&codecs, file, options()).unwrap();
    assert_eq!(decode.stats().strips, 0);
    let pulls = Arc::new(AtomicU64::new(0));
    let counted = Counted {
        inner: decode,
        pulls: pulls.clone(),
    };
    let budget = StorageBudget::new((W * H * 3 - 1) as usize, 4);
    let error = render_orientation(Box::new(counted), Orientation::Rotate90, &budget, 16)
        .err()
        .expect("must refuse");
    assert!(
        zencodec::find_cause::<StorageError>(&error).is_some(),
        "{error:?}"
    );
    assert_eq!(pulls.load(Ordering::SeqCst), 0);
}

#[test]
fn passthrough_keeps_stored_pixels_and_orientation_without_private_exif() {
    let samples = samples();
    let file = png_with(
        &samples,
        Metadata::none().with_exif(exif(Orientation::Rotate90, true)),
    );
    let codecs = codecs();
    let decode = DecodeSource::from_slice(&codecs, &file, options()).unwrap();
    assert_eq!(decode.info().orientation, Orientation::Rotate90);
    assert_eq!((decode.width(), decode.height()), (W, H), "decoder rotated");
    let meta = decode.info().metadata();
    let rows = PipelineRows::from_source(decode).unwrap();
    let job = zenpng::PngEncoderConfig::new()
        .with_generic_effort(0)
        .job()
        .with_metadata_policy(meta, MetadataPolicy::ColorAndRotation);
    let mut out = Vec::new();
    row_encode::encode_job(
        job,
        rows,
        &mut out,
        PolicySet::new(),
        encode_options(),
        &NoPulse,
    )
    .unwrap();
    let (w, h, pixels, exif) = read_png(&out);
    assert_eq!((w, h), (W, H));
    assert_eq!(pixels, samples, "passthrough changed stored pixels");
    let exif = exif.expect("orientation carrier dropped");
    let parsed = Exif::parse(&exif).unwrap();
    assert_eq!(parsed.orientation(), Some(Orientation::Rotate90));
    assert_eq!(parsed.artist(), None, "private field survived");
    assert_eq!(parsed.copyright(), None, "private field survived");
    assert!(!parsed.has_camera() && !parsed.has_gps());
}

#[test]
fn rendered_output_is_upright_and_clears_orientation() {
    let samples = samples();
    let file = png_with(
        &samples,
        Metadata::none().with_exif(exif(Orientation::Rotate90, true)),
    );
    let codecs = codecs();
    let decode = DecodeSource::from_bytes(&codecs, file, options()).unwrap();
    let meta = decode
        .info()
        .metadata()
        .with_orientation(Orientation::Identity);
    let budget = StorageBudget::new(1 << 20, 4);
    let upright = render_orientation(Box::new(decode), Orientation::Rotate90, &budget, 16).unwrap();
    let rows = PipelineRows::from_source(upright).unwrap();
    let job = zenpng::PngEncoderConfig::new()
        .with_generic_effort(0)
        .job()
        .with_metadata_policy(meta, MetadataPolicy::ColorAndRotation);
    let mut out = Vec::new();
    row_encode::encode_job(
        job,
        rows,
        &mut out,
        PolicySet::new(),
        encode_options(),
        &NoPulse,
    )
    .unwrap();
    let (expected, ew, eh) = upright_reference(&samples, Orientation::Rotate90);
    let (w, h, pixels, exif) = read_png(&out);
    assert_eq!((w, h), (ew, eh));
    assert_eq!(pixels, expected);
    // A viewer must not rotate the upright pixels again.
    let orientation = exif
        .as_deref()
        .and_then(Exif::parse)
        .and_then(|e| e.orientation())
        .unwrap_or(Orientation::Identity);
    assert_eq!(orientation, Orientation::Identity, "double rotation");
    let decoded = DecodeRequest::new().decode(&codecs, &out).unwrap();
    assert_eq!(decoded.output().info().orientation, Orientation::Identity);
}

/// The APP1 `Exif` payload of a baseline JPEG, read by walking its markers.
fn jpeg_exif(bytes: &[u8]) -> Option<Vec<u8>> {
    assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
    let mut at = 2;
    while at + 4 <= bytes.len() && bytes[at] == 0xFF {
        let marker = bytes[at + 1];
        if marker == 0xDA {
            return None;
        }
        let len = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
        let body = &bytes[at + 4..at + 2 + len];
        if marker == 0xE1 && body.starts_with(b"Exif\0\0") {
            return Some(body[6..].to_vec());
        }
        at += 2 + len;
    }
    None
}

#[test]
fn a_direct_job_carries_orientation_without_a_source_exif_blob() {
    // DiscardDescriptive-style metadata: orientation only, no EXIF payload,
    // as a source whose orientation came from a non-EXIF carrier would give.
    // Each layer is checked so a loss names the layer that lost it.
    let samples = samples();
    let meta = Metadata::none().with_orientation(Orientation::Rotate90);
    // 1. Retention keeps the authoritative field (what `with_metadata_policy`
    //    hands the codec).
    #[allow(deprecated)]
    let filtered = meta.filtered(&MetadataPolicy::PreserveExact);
    assert_eq!(
        filtered.orientation,
        Orientation::Rotate90,
        "retention dropped the field"
    );
    assert!(filtered.exif.is_none());
    let pixels =
        || PixelSlice::new(&samples, W, H, (W * 3) as usize, PixelDescriptor::RGB8_SRGB).unwrap();
    let png = zenpng::PngEncoderConfig::new()
        .job()
        .with_metadata_policy(meta.clone(), MetadataPolicy::PreserveExact)
        .encode(pixels())
        .unwrap()
        .into_vec();
    let jpeg = zenjpeg::JpegEncoderConfig::new()
        .job()
        .with_metadata_policy(meta, MetadataPolicy::PreserveExact)
        .encode(pixels())
        .unwrap()
        .into_vec();
    let codecs = codecs().with_decoder(zenjpeg::JpegDecoderConfig::new());
    let mut lost = Vec::new();
    for (name, file, carrier) in [
        ("png", &png, read_png(&png).3),
        ("jpeg", &jpeg, jpeg_exif(&jpeg)),
    ] {
        // 2. JPEG and PNG carry orientation only in EXIF: the encoder must
        //    write one (read back independently: `png` crate, marker walk).
        let written = carrier
            .as_deref()
            .and_then(Exif::parse)
            .and_then(|e| e.orientation());
        if written != Some(Orientation::Rotate90) {
            lost.push(format!(
                "{name}: encoder wrote EXIF orientation {written:?}"
            ));
        }
        // 3. What a reader of the output sees.
        let decoded = DecodeRequest::new().decode(&codecs, file).unwrap();
        let seen = decoded.output().info().orientation;
        if seen != Orientation::Rotate90 {
            lost.push(format!("{name}: decoded orientation {seen:?}"));
        }
    }
    assert!(
        lost.is_empty(),
        "orientation without EXIF was not carried: {lost:#?}"
    );
}

fn encode_options() -> Options {
    Options::new(StorageBudget::new(16 << 20, 64), 16 << 20)
}
