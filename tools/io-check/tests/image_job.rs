//! `ImageJob`'s planned output route (`with_output_policy`) against its
//! streaming-encoder route, on the same input and intent.

use std::io::Cursor;

use zencodec::ImageFormat;
use zencodec::encode::{EncodeJob, EncoderConfig};
use zencodec::operation::{OutputSchedule, PolicyRequirement, PolicySet};
use zencodecs::{CodecIntent, FormatChoice};
use zenpipe::job::ImageJob;
use zenpixels::{PixelDescriptor, PixelSlice};

const W: u32 = 97;
const H: u32 = 65;

fn png_input() -> Vec<u8> {
    let mut samples = Vec::with_capacity((W * H * 3) as usize);
    for y in 0..H {
        for x in 0..W {
            samples.extend_from_slice(&[(x * 2) as u8, (y * 3) as u8, ((x * 7) ^ (y * 5)) as u8]);
        }
    }
    let pixels =
        PixelSlice::new(&samples, W, H, (W * 3) as usize, PixelDescriptor::RGB8_SRGB).unwrap();
    zenpng::PngEncoderConfig::new()
        .job()
        .encode(pixels)
        .unwrap()
        .into_vec()
}

fn intent(format: ImageFormat) -> CodecIntent {
    CodecIntent {
        format: Some(FormatChoice::Specific(format)),
        ..Default::default()
    }
}

fn decode_png(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

fn run(policy: Option<PolicySet>) -> zenpipe::PipeResult<Vec<u8>> {
    let mut job = ImageJob::new()
        .add_input(0, png_input())
        .add_output(1)
        .with_intent(intent(ImageFormat::Png));
    if let Some(policy) = policy {
        job = job.with_output_policy(policy);
    }
    let result = job.run()?;
    Ok(result.encode_results[0].bytes.clone())
}

#[test]
fn the_planned_route_publishes_the_streaming_routes_pixels() {
    let legacy = run(None).unwrap();
    let planned = run(Some(PolicySet::new())).unwrap();
    let (lw, lh, legacy_pixels) = decode_png(&legacy);
    let (pw, ph, planned_pixels) = decode_png(&planned);
    assert_eq!((lw, lh), (W, H));
    assert_eq!((pw, ph), (W, H));
    assert_eq!(planned_pixels, legacy_pixels);
    eprintln!(
        "ImageJob PNG: legacy {} B, planned {} B, byte-identical: {}",
        legacy.len(),
        planned.len(),
        legacy == planned
    );
}

#[test]
fn a_required_schedule_is_honored_or_refused_before_output() {
    for schedule in [OutputSchedule::FinishFirst, OutputSchedule::Live] {
        match run(Some(PolicySet::new().require_output_schedule(schedule))) {
            Ok(bytes) => {
                assert!(!bytes.is_empty());
                eprintln!("ImageJob {schedule:?}: published {} B", bytes.len());
            }
            Err(error) => {
                let failure = zencodec::find_cause::<zencodec_io::row_encode::Failure>(&error)
                    .unwrap_or_else(|| panic!("untyped refusal: {error:?}"));
                assert_eq!(failure.stage, zencodec_io::row_encode::Stage::Policy);
                assert_eq!(failure.emitted, 0);
                // ImageJob installs the policy as the driver's preset, so the
                // refusal names the requirement it could not meet.
                let unmet: Vec<_> = failure
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
                eprintln!("ImageJob {schedule:?}: refused before output; unmet {unmet:?}");
            }
        }
    }
    // Finish-first is always offered.
    run(Some(
        PolicySet::new().require_output_schedule(OutputSchedule::FinishFirst),
    ))
    .unwrap();
}

/// The input samples (RGB8) and a PNG carrying them with EXIF orientation `o`.
fn oriented_png_input(o: zencodec::Orientation) -> (Vec<u8>, Vec<u8>) {
    let mut samples = Vec::with_capacity((W * H * 3) as usize);
    for y in 0..H {
        for x in 0..W {
            samples.extend_from_slice(&[(x * 2) as u8, (y * 3) as u8, ((x * 7) ^ (y * 5)) as u8]);
        }
    }
    let mut exif = zencodec::exif::Exif::new(zencodec::exif::TextEncoding::Ascii);
    exif.set_orientation(o);
    #[allow(deprecated)] // compatibility writer; authoring a fixture
    let exif = exif.to_bytes();
    let pixels =
        PixelSlice::new(&samples, W, H, (W * 3) as usize, PixelDescriptor::RGB8_SRGB).unwrap();
    let png = zenpng::PngEncoderConfig::new()
        .job()
        .with_metadata_policy(
            zencodec::Metadata::none().with_exif(exif),
            zencodec::MetadataPolicy::PreserveExact,
        )
        .encode(pixels)
        .unwrap()
        .into_vec();
    (samples, png)
}

/// What a viewer shows: the pixels with their EXIF orientation applied, as RGB.
fn displayed(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    let channels = info.color_type.samples();
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    let rgb: Vec<u8> = buf
        .chunks_exact(channels)
        .flat_map(|p| {
            if channels == 4 {
                assert_eq!(p[3], 255, "opaque input gained transparency");
            }
            [p[0], p[1], p[2]]
        })
        .collect();
    let orientation = reader
        .info()
        .exif_metadata
        .as_deref()
        .and_then(zencodec::exif::Exif::parse)
        .and_then(|e| e.orientation())
        .unwrap_or(zencodec::Orientation::Identity);
    let o = zenlayout::Orientation::from_exif(orientation.to_exif()).unwrap();
    let (shown, w, h) = zenpipe::execute_layout::orient_image(&rgb, info.width, info.height, o, 3);
    (w, h, shown)
}

#[test]
fn displayed_orientation_survives_every_metadata_policy_on_both_routes() {
    use zenpipe::job::MetadataPolicy;
    let o = zencodec::Orientation::Rotate90;
    let (samples, input) = oriented_png_input(o);
    let lo = zenlayout::Orientation::from_exif(o.to_exif()).unwrap();
    let (expected, ew, eh) = zenpipe::execute_layout::orient_image(&samples, W, H, lo, 3);
    let mut wrong = Vec::new();
    for policy in [
        MetadataPolicy::PreserveAll,
        MetadataPolicy::WebDefault,
        MetadataPolicy::StripAll,
    ] {
        for planned in [false, true] {
            // Lossless, so the comparison is about orientation, not quantization.
            let lossless = CodecIntent {
                lossless: Some(zencodecs::BoolKeep::True),
                ..intent(ImageFormat::Png)
            };
            let mut job = ImageJob::new()
                .add_input(0, input.clone())
                .add_output(1)
                .with_intent(lossless)
                .with_metadata_policy(policy);
            if planned {
                job = job.with_output_policy(PolicySet::new());
            }
            let cell = format!("{policy:?} planned={planned}");
            match job.run() {
                Ok(result) => {
                    let (w, h, shown) = displayed(&result.encode_results[0].bytes);
                    let ok = (w, h) == (ew, eh) && shown == expected;
                    eprintln!("{cell}: displayed {w}x{h}, upright {ok}");
                    if !ok {
                        wrong.push(format!("{cell}: displayed {w}x{h}, not the upright source"));
                    }
                }
                Err(e) => wrong.push(format!("{cell}: {e}")),
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "orientation not displayed upright: {wrong:#?}"
    );
}
