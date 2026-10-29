//! Planned JPEG row encoding through `zenpipe::io`: byte identity with the
//! one-shot encode, which configurations offer a live schedule, and ICC plus
//! orientation carriage read back with the independent `jpeg-decoder` crate.

use std::io::Cursor;
use std::sync::Arc;

use zencodec::encode::{EncodeJob, EncoderConfig};
use zencodec::exif::{Exif, TextEncoding};
use zencodec::operation::{OutputSchedule, PolicySet};
use zencodec::storage::StorageBudget;
use zencodec::{Cicp, Metadata, MetadataPolicy, NoPulse, Orientation};
use zencodec_io::row_encode::{self, Options, Stage};
use zenpipe::io::PipelineRows;
use zenpipe::sources::MaterializedSource;
use zenpixels::{ColorContext, PixelDescriptor, PixelSlice};
use zenpixels_convert::icc_profiles::{SynthesizedIcc, synthesize_icc_for_cicp};

const W: u32 = 97;
const H: u32 = 65;

fn samples() -> Vec<u8> {
    let mut out = Vec::with_capacity((W * H * 3) as usize);
    for y in 0..H {
        for x in 0..W {
            out.extend_from_slice(&[(x * 2) as u8, (y * 3) as u8, ((x * 7) ^ (y * 5)) as u8]);
        }
    }
    out
}

fn rows(color: Option<Arc<ColorContext>>) -> PipelineRows<MaterializedSource> {
    let source = MaterializedSource::from_data(samples(), W, H, PixelDescriptor::RGB8_SRGB);
    PipelineRows::new(source, color).unwrap()
}

fn options() -> Options {
    Options::new(StorageBudget::new(16 << 20, 1024), 16 << 20)
}

fn configs() -> [(&'static str, zenjpeg::JpegEncoderConfig); 3] {
    [
        ("default", zenjpeg::JpegEncoderConfig::new()),
        (
            "streaming",
            zenjpeg::JpegEncoderConfig::new().with_prefer_streaming(true),
        ),
        (
            "progressive",
            zenjpeg::JpegEncoderConfig::new().with_progressive(true),
        ),
    ]
}

#[test]
fn planned_rows_reproduce_the_one_shot_bytes() {
    let samples = samples();
    for (label, config) in configs() {
        let pixels =
            PixelSlice::new(&samples, W, H, (W * 3) as usize, PixelDescriptor::RGB8_SRGB).unwrap();
        let one_shot = config.clone().job().encode(pixels).unwrap().into_vec();
        let mut out = Vec::new();
        let done = row_encode::encode_job(
            config.job(),
            rows(None),
            &mut out,
            PolicySet::new(),
            options(),
            Some(&NoPulse),
        )
        .unwrap();
        assert_eq!(out, one_shot, "{label}");
        eprintln!(
            "jpeg {label}: {} B, default schedule {:?}",
            out.len(),
            done.schedule
        );
    }
}

#[test]
fn a_required_live_schedule_succeeds_only_where_the_configuration_streams() {
    for (label, config) in configs() {
        let mut out = Vec::new();
        let result = row_encode::encode_job(
            config.job(),
            rows(None),
            &mut out,
            PolicySet::new().require_output_schedule(OutputSchedule::Live),
            options(),
            Some(&NoPulse),
        );
        match result {
            Ok(done) => {
                assert_eq!(done.schedule, OutputSchedule::Live);
                eprintln!("jpeg {label}: live offered");
            }
            Err(error) => {
                assert_eq!(error.error().stage, Stage::Policy, "{label}");
                assert!(out.is_empty(), "{label}");
                eprintln!("jpeg {label}: live refused before any effect");
            }
        }
        if label == "progressive" {
            assert!(
                out.is_empty(),
                "a progressive scan cannot publish rows live"
            );
        }
        if label == "streaming" {
            assert!(
                !out.is_empty(),
                "the streaming configuration refused live output"
            );
        }
    }
}

#[test]
fn icc_and_orientation_reach_the_jpeg_through_the_job() {
    let SynthesizedIcc::Profile(p3) = synthesize_icc_for_cicp(Cicp::DISPLAY_P3) else {
        panic!("no bundled Display P3 profile");
    };
    let p3 = p3.into_owned();
    let mut exif = Exif::new(TextEncoding::Ascii);
    exif.set_orientation(Orientation::Rotate90);
    #[allow(deprecated)] // compatibility writer; authoring a fixture
    let exif = exif.to_bytes();
    let meta = Metadata::none().with_icc(p3.clone()).with_exif(exif);
    // The rows are declared Display P3; the job's metadata agrees.
    let context = Arc::new(ColorContext::from_icc(p3.clone()));
    let job = zenjpeg::JpegEncoderConfig::new()
        .job()
        .with_metadata_policy(meta, MetadataPolicy::PreserveExact);
    let mut out = Vec::new();
    row_encode::encode_job(
        job,
        rows(Some(context)),
        &mut out,
        PolicySet::new(),
        options(),
        Some(&NoPulse),
    )
    .unwrap();
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(&out));
    let pixels = decoder.decode().unwrap();
    assert_eq!(pixels.len(), (W * H * 3) as usize);
    assert_eq!(
        decoder.icc_profile().as_deref(),
        Some(&p3[..]),
        "ICC lost or changed"
    );
    let orientation = decoder
        .exif_data()
        .and_then(Exif::parse)
        .and_then(|e| e.orientation());
    assert_eq!(orientation, Some(Orientation::Rotate90));
}
