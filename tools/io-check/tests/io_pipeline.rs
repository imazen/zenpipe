//! `zenpipe::io` end to end with native JPEG and PNG: slice and file sources,
//! pipeline processing, planned row encoding, schedules and refusals.
//!
//! Source samples are authored here, so lossless paths compare against the
//! exact input; PNG output is read back with the independent `png` crate.

use std::io::Cursor;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use zencodec::encode::{EncodeJob, EncoderConfig};
use zencodec::operation::{DecodeRequest, OutputSchedule, PolicyRequirement, PolicySet};
use zencodec::storage::StorageBudget;
use zencodec::{CodecSet, ImageFormat, NoPulse};
use zencodec_io::row_encode::{self, Options, Stage};
use zencodec_io::{DriveOptions, FileReadAt};
use zenpipe::io::{DecodeOptions, DecodeSource, ErasedJob, PipelineRows};
use zenpipe::{PipeResult, Source, Strip};
use zenpixels::{PixelDescriptor, PixelSlice};

const W: u32 = 97;
const H: u32 = 65;

fn pattern(width: u32, height: u32, channels: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height * channels) as usize);
    for y in 0..height {
        for x in 0..width {
            out.push((x * 255 / width) as u8);
            out.push((y * 255 / height) as u8);
            out.push(((x ^ y) * 7 % 256) as u8);
            if channels == 4 {
                out.push(((x + 3 * y) % 251) as u8);
            }
        }
    }
    out
}

fn png_fixture(width: u32, height: u32, channels: u32) -> (Vec<u8>, Vec<u8>) {
    let samples = pattern(width, height, channels);
    let descriptor = if channels == 4 {
        PixelDescriptor::RGBA8_SRGB
    } else {
        PixelDescriptor::RGB8_SRGB
    };
    let pixels = PixelSlice::new(
        &samples,
        width,
        height,
        (width * channels) as usize,
        descriptor,
    )
    .unwrap();
    let file = zenpng::PngEncoderConfig::new()
        .job()
        .encode(pixels)
        .unwrap()
        .into_vec();
    (file, samples)
}

fn codecs() -> CodecSet {
    CodecSet::new()
        .with_decoder(zenjpeg::JpegDecoderConfig::new())
        .with_decoder(zenpng::PngDecoderConfig::new())
        .with_encoder(zenpng::PngEncoderConfig::new().with_generic_effort(0))
}

fn options() -> DecodeOptions {
    // Queue bounds have their own test; elsewhere the native batch height is
    // not what is under test.
    DecodeOptions::new(StorageBudget::new(64 << 20, 64)).with_queue_slots(256)
}

fn encode_options() -> Options {
    Options::new(StorageBudget::new(16 << 20, 64), 16 << 20)
}

/// Independent PNG read: (width, height, channels, samples).
fn read_png(bytes: &[u8]) -> (u32, u32, usize, Vec<u8>) {
    let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    let channels = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        other => panic!("unexpected PNG color type {other:?}"),
    };
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    (info.width, info.height, channels, buf)
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

/// Counts pipeline pulls so tests can prove no row was requested.
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
fn png_rows_equal_authored_samples_and_the_queue_never_grows() {
    let (file, samples) = png_fixture(W, H, 3);
    let codecs = codecs();
    for strip_rows in [1, 7, 64] {
        // Generous slots first, to learn the decoder's widest step.
        let opts = || options().with_strip_rows(strip_rows);
        let mut source =
            DecodeSource::from_slice(&codecs, &file, opts().with_queue_slots(256)).unwrap();
        assert_eq!(source.stats().strips, 0, "construction decoded rows");
        assert_eq!((source.width(), source.height()), (W, H));
        assert_eq!(source.format(), PixelDescriptor::RGB8_SRGB);
        assert_eq!(collect(&mut source), samples, "strip_rows {strip_rows}");
        let stats = source.stats();
        eprintln!("png decode strip_rows={strip_rows}: {stats:?}");
        assert!(stats.max_strip_rows <= strip_rows, "{stats:?}");
        let widest = stats.max_strips_per_step;
        assert!(widest >= 1);
        // Exactly the widest step's slots suffice and are all that get admitted.
        let mut exact =
            DecodeSource::from_slice(&codecs, &file, opts().with_queue_slots(widest)).unwrap();
        assert_eq!(collect(&mut exact), samples);
        assert!(exact.stats().slots <= widest);
        if widest > 1 {
            // One slot fewer refuses with the typed reason; it never grows.
            let mut short =
                DecodeSource::from_slice(&codecs, &file, opts().with_queue_slots(widest - 1))
                    .unwrap();
            let error = loop {
                match short.next() {
                    Ok(Some(_)) => continue,
                    Ok(None) => panic!("{widest} strips per step fit in {} slots", widest - 1),
                    Err(e) => break e,
                }
            };
            assert!(
                matches!(
                    zencodec::find_cause::<zenpipe::io::Refusal>(&error),
                    Some(zenpipe::io::Refusal::StripQueueFull { .. })
                ),
                "{error:?}"
            );
        }
    }
}

#[test]
fn png_crop_then_planned_png_encode_is_exact() {
    let (file, samples) = png_fixture(W, H, 3);
    let codecs = codecs();
    let source = DecodeSource::from_bytes(&codecs, file, options().with_strip_rows(16)).unwrap();
    let crop = zenpipe::sources::CropSource::new(Box::new(source), 5, 3, 61, 47).unwrap();
    let rows = PipelineRows::from_source(crop).unwrap();
    let job = zenpng::PngEncoderConfig::new().with_generic_effort(0).job();
    let mut out = Vec::new();
    let done = row_encode::encode_job(
        job,
        rows,
        &mut out,
        PolicySet::new(),
        encode_options().with_batch_rows(NonZeroU32::new(5).unwrap()),
        &NoPulse,
    )
    .unwrap();
    assert_eq!(done.schedule, OutputSchedule::FinishFirst);
    assert_eq!(done.emitted, out.len() as u64);
    let (w, h, channels, decoded) = read_png(&out);
    assert_eq!((w, h, channels), (61, 47, 3));
    let mut expected = Vec::new();
    for y in 3..50 {
        let start = ((y * W + 5) * 3) as usize;
        expected.extend_from_slice(&samples[start..start + 61 * 3]);
    }
    assert_eq!(decoded, expected);
}

#[test]
fn live_and_finish_first_publish_identical_bytes_for_a_streaming_png() {
    let (file, _) = png_fixture(W, H, 3);
    let codecs = codecs();
    let mut outputs = Vec::new();
    for schedule in [OutputSchedule::FinishFirst, OutputSchedule::Live] {
        let source = DecodeSource::from_slice(&codecs, &file, options()).unwrap();
        let rows = PipelineRows::from_source(source).unwrap();
        let job = zenpng::PngEncoderConfig::new().with_generic_effort(0).job();
        let mut out = Vec::new();
        let done = row_encode::encode_job(
            job,
            rows,
            &mut out,
            PolicySet::new().require_output_schedule(schedule),
            encode_options(),
            &NoPulse,
        )
        .unwrap();
        assert_eq!(done.schedule, schedule);
        outputs.push(out);
    }
    assert_eq!(outputs[0], outputs[1]);
}

/// Expected to fail at zencodec `f5a7fd46` (policy composition gap): the
/// requirement passed only as the resolver is refused before any pixel work,
/// but the report names nothing, because the core admits and names
/// requirements from the driver's preset alone. Kept unchanged as the visible
/// finding; the preset form below is the working placement.
#[test]
fn required_live_schedule_refuses_a_buffered_png_before_any_pixel_work() {
    let (file, _) = png_fixture(W, H, 3);
    let codecs = codecs();
    let source = DecodeSource::from_slice(&codecs, &file, options()).unwrap();
    assert_eq!(source.stats().strips, 0);
    let pulls = Arc::new(AtomicU64::new(0));
    let counted = Counted {
        inner: source,
        pulls: pulls.clone(),
    };
    let rows = PipelineRows::from_source(counted).unwrap();
    // Effort 19 buffers the image and emits at finish: no live output exists.
    let job = zenpng::PngEncoderConfig::new()
        .with_generic_effort(19)
        .job();
    let mut out = Vec::new();
    let error = row_encode::encode_job(
        job,
        rows,
        &mut out,
        PolicySet::new().require_output_schedule(OutputSchedule::Live),
        encode_options(),
        &NoPulse,
    )
    .unwrap_err();
    assert_eq!(error.error().stage, Stage::Policy);
    assert_eq!(error.error().emitted, 0);
    assert!(out.is_empty());
    assert_eq!(pulls.load(Ordering::SeqCst), 0, "a row was requested");
    let unmet: Vec<_> = error
        .error()
        .report
        .decisions()
        .flat_map(|d| d.unmet_requirements.iter())
        .collect();
    assert!(
        unmet
            .iter()
            .any(|(r, _)| *r == PolicyRequirement::OutputSchedule),
        "{unmet:?}"
    );
}

/// The working placement: the requirement in the driver's preset (and the same
/// policy as the resolver) refuses before any pixel work and names
/// `OutputSchedule`.
#[test]
fn required_live_schedule_in_the_preset_is_named_before_any_pixel_work() {
    let (file, _) = png_fixture(W, H, 3);
    let codecs = codecs();
    let source = DecodeSource::from_slice(&codecs, &file, options()).unwrap();
    assert_eq!(source.stats().strips, 0);
    let pulls = Arc::new(AtomicU64::new(0));
    let counted = Counted {
        inner: source,
        pulls: pulls.clone(),
    };
    let rows = PipelineRows::from_source(counted).unwrap();
    // Effort 19 buffers the image and emits at finish: no live output exists.
    let job = zenpng::PngEncoderConfig::new()
        .with_generic_effort(19)
        .job();
    let live = PolicySet::new().require_output_schedule(OutputSchedule::Live);
    let mut out = Vec::new();
    let error = row_encode::encode_job(
        job,
        rows,
        &mut out,
        live,
        encode_options().with_policy_set(live),
        &NoPulse,
    )
    .unwrap_err();
    assert_eq!(error.error().stage, Stage::Policy);
    assert_eq!(error.error().emitted, 0);
    assert!(out.is_empty());
    assert_eq!(pulls.load(Ordering::SeqCst), 0, "a row was requested");
    let unmet: Vec<_> = error
        .error()
        .report
        .decisions()
        .flat_map(|d| d.unmet_requirements.iter())
        .collect();
    assert!(
        unmet
            .iter()
            .any(|(r, _)| *r == PolicyRequirement::OutputSchedule),
        "{unmet:?}"
    );
}

#[test]
fn erased_registry_job_matches_the_typed_job_bytes() {
    let (file, _) = png_fixture(W, H, 3);
    let codecs = codecs();
    let mut outputs = Vec::new();
    for erased in [false, true] {
        let source = DecodeSource::from_slice(&codecs, &file, options()).unwrap();
        let rows = PipelineRows::from_source(source).unwrap();
        let mut out = Vec::new();
        if erased {
            let job = ErasedJob::new(codecs.encode_job(ImageFormat::Png).unwrap());
            row_encode::encode_job(
                job,
                rows,
                &mut out,
                PolicySet::new(),
                encode_options(),
                &NoPulse,
            )
            .unwrap();
        } else {
            let job = zenpng::PngEncoderConfig::new().with_generic_effort(0).job();
            row_encode::encode_job(
                job,
                rows,
                &mut out,
                PolicySet::new(),
                encode_options(),
                &NoPulse,
            )
            .unwrap();
        }
        outputs.push(out);
    }
    assert_eq!(outputs[0], outputs[1]);
}

#[test]
fn file_source_to_seek_back_file_matches_the_slice_route() {
    let (file, samples) = png_fixture(W, H, 3);
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("io_pipeline");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("file_source_input.png");
    let output = dir.join("file_source_output.png");
    std::fs::write(&input, &file).unwrap();
    let codecs = codecs();
    let source = DecodeSource::new(
        &codecs,
        &DecodeRequest::new(),
        FileReadAt::open(&input).unwrap(),
        PolicySet::new(),
        options().with_drive(DriveOptions::new(0, 4096)),
    )
    .unwrap();
    let construction = source.stats();
    let rows = PipelineRows::from_source(source).unwrap();
    let job = zenpng::PngEncoderConfig::new().with_generic_effort(0).job();
    let writer = std::fs::File::create(&output).unwrap();
    let destination = zencodec_io::destination::SeekBack::new(writer).unwrap();
    row_encode::encode_job_to_destination(
        job,
        rows,
        destination,
        PolicySet::new(),
        encode_options(),
        &NoPulse,
    )
    .unwrap();
    let (w, h, _, decoded) = read_png(&std::fs::read(&output).unwrap());
    assert_eq!((w, h), (W, H));
    assert_eq!(decoded, samples);
    // The whole PNG is read once, in 4 KiB rounds, before its interpretation.
    assert_eq!(construction.drive.bytes_read, file.len() as u64);
    eprintln!(
        "file source: {} bytes, construction {:?}",
        file.len(),
        construction.drive
    );
}

#[test]
fn rgba_png_resize_streams_through_the_planned_encoder() {
    let (file, samples) = png_fixture(W, H, 4);
    let codecs = codecs();
    let source = DecodeSource::from_bytes(&codecs, file, options().with_strip_rows(8)).unwrap();
    assert_eq!(source.format(), PixelDescriptor::RGBA8_SRGB);
    let config = zenresize::ResizeConfig::builder(W, H, 48, 32)
        .format(PixelDescriptor::RGBA8_SRGB)
        .build();
    let resized = zenpipe::sources::ResizeSource::new(Box::new(source), &config, 16).unwrap();
    let rows = PipelineRows::from_source(resized).unwrap();
    let job = zenpng::PngEncoderConfig::new().with_generic_effort(0).job();
    let mut streamed = Vec::new();
    row_encode::encode_job(
        job,
        rows,
        &mut streamed,
        PolicySet::new(),
        encode_options(),
        &NoPulse,
    )
    .unwrap();

    // Control: the same resize over the whole authored raster.
    let whole =
        zenpipe::sources::MaterializedSource::from_data(samples, W, H, PixelDescriptor::RGBA8_SRGB);
    let mut control = zenpipe::sources::ResizeSource::new(Box::new(whole), &config, 16).unwrap();
    let expected = collect(&mut control);
    let (w, h, channels, decoded) = read_png(&streamed);
    assert_eq!((w, h, channels), (48, 32, 4));
    assert_eq!(decoded, expected);
}

#[test]
fn jpeg_controlled_strips_equal_the_one_shot_decode() {
    let samples = pattern(W, H, 3);
    let codecs = codecs();
    let configs = [
        ("default", zenjpeg::JpegEncoderConfig::new()),
        (
            "progressive",
            zenjpeg::JpegEncoderConfig::new().with_progressive(true),
        ),
    ];
    for (label, config) in configs {
        let pixels =
            PixelSlice::new(&samples, W, H, (W * 3) as usize, PixelDescriptor::RGB8_SRGB).unwrap();
        let file = config.job().encode(pixels).unwrap().into_vec();
        let one_shot = DecodeRequest::new().decode(&codecs, &file).unwrap();
        let reference_pixels = one_shot.output().pixels();
        let mut reference = Vec::new();
        for r in 0..reference_pixels.rows() {
            reference.extend_from_slice(reference_pixels.row(r));
        }
        for strip_rows in [16, 64] {
            let mut source =
                DecodeSource::from_slice(&codecs, &file, options().with_strip_rows(strip_rows))
                    .unwrap();
            assert_eq!(source.format(), reference_pixels.descriptor(), "{label}");
            let decoded = collect(&mut source);
            assert_eq!(decoded, reference, "{label} strip_rows {strip_rows}");
            eprintln!("jpeg {label} strip_rows={strip_rows}: {:?}", source.stats());
        }
    }
}

/// A sequential stream that returns at most `max` bytes per read.
struct Trickle {
    data: Vec<u8>,
    at: usize,
    max: usize,
}

impl std::io::Read for Trickle {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = (self.data.len() - self.at).min(out.len()).min(self.max);
        out[..n].copy_from_slice(&self.data[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

#[test]
fn a_sequential_stream_through_a_bounded_window_matches_the_samples() {
    let (file, samples) = png_fixture(W, H, 3);
    let reader = Trickle {
        data: file,
        at: 0,
        max: 7,
    };
    let buffered = zencodec_io::BufferedReadAt::new(
        reader,
        zencodec_io::BufferPolicy::Window {
            window_bytes: 64 * 1024,
        },
    );
    let mut source = DecodeSource::new(
        &codecs(),
        &DecodeRequest::new(),
        buffered,
        PolicySet::new(),
        options(),
    )
    .unwrap();
    assert_eq!(collect(&mut source), samples);
}

/// A wrapper that forwards rows but does not vouch for their interpretation.
struct Opaque<S>(S);

impl<S: Source> Source for Opaque<S> {
    fn next(&mut self) -> PipeResult<Option<Strip<'_>>> {
        self.0.next()
    }
    fn width(&self) -> u32 {
        self.0.width()
    }
    fn height(&self) -> u32 {
        self.0.height()
    }
    fn format(&self) -> zenpipe::PixelFormat {
        self.0.format()
    }
}

#[test]
fn a_pipeline_that_cannot_state_its_color_must_declare_it() {
    let (file, _) = png_fixture(W, H, 3);
    let source = DecodeSource::from_bytes(&codecs(), file, options()).unwrap();
    let known = source.color_context();
    assert!(known.is_some());
    let error = PipelineRows::from_source(Opaque(source))
        .err()
        .expect("unknown color accepted");
    assert!(matches!(
        zencodec::find_cause::<zenpipe::io::Refusal>(&error),
        Some(zenpipe::io::Refusal::UnknownColor)
    ));
}
