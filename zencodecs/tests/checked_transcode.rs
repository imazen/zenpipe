//! Real native codecs, with independent PNG parsing and CMS conversion oracles.
use std::{
    convert::Infallible,
    io::Cursor,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
use zencodec::operation::{Decision, DecisionCode, Decline, MetadataTransfer, Question, Selection};
use zencodec::storage::StorageBudget;
use zencodec::{At, CodecSet, ImageFormat, Metadata, MetadataPolicy, ResourceLimits, StopToken};
use zencodecs::checked_transcode::{Stage, TranscodeRequest};
use zenjpeg::{JpegDecoderConfig, JpegEncoderConfig};
use zenpixels::{
    Cicp, ColorProfileSource, ConvertOptions, PixelDescriptor, PixelFormat, PixelSlice,
};
use zenpixels_convert::{
    OutputProfile, cms::PluggableCms, cms_moxcms::MoxCms, icc_profiles::ADOBE_RGB,
};

const WIDTH: u32 = 97;
const HEIGHT: u32 = 65;
const SECRET: &str = "PRIVATE-fixture@example.test";
fn jpeg_config(progressive: bool) -> JpegEncoderConfig {
    JpegEncoderConfig::new()
        .with_calibrated_quality(87.0)
        .with_progressive(progressive)
        .with_huffman(true)
}
#[allow(deprecated)] // Existing serializer constructs this synthetic fixture.
fn fixture(progressive: bool) -> Vec<u8> {
    fixture_oriented(progressive, zencodec::Orientation::Identity)
}
#[allow(deprecated)]
fn fixture_oriented(progressive: bool, orientation: zencodec::Orientation) -> Vec<u8> {
    let stride = WIDTH as usize * 3 + 21;
    let mut raw = vec![0xa5; stride * HEIGHT as usize];
    for y in 0..HEIGHT as usize {
        for x in 0..WIDTH as usize {
            raw[y * stride + 3 * x..y * stride + 3 * x + 3].copy_from_slice(&[
                (x * 7) as u8,
                (y * 11) as u8,
                ((x + y) * 3) as u8,
            ]);
        }
    }
    let mut exif = zencodec::exif::Exif::new(zencodec::exif::TextEncoding::Ascii);
    exif.set_artist(SECRET);
    exif.set_orientation(orientation);
    let metadata = Metadata::none()
        .with_icc(ADOBE_RGB)
        .with_exif(exif.to_bytes())
        .with_xmp(format!("<malformed-private>{SECRET}").into_bytes());
    jpeg_config(progressive)
        .job()
        .with_metadata_policy(metadata, MetadataPolicy::PreserveExact)
        .encoder()
        .unwrap()
        .encode(PixelSlice::new(&raw, WIDTH, HEIGHT, stride, PixelDescriptor::RGB8_SRGB).unwrap())
        .unwrap()
        .data()
        .to_vec()
}
fn codecs() -> CodecSet {
    CodecSet::new()
        .with_decoder(JpegDecoderConfig::new())
        .with_decoder(zenpng::PngDecoderConfig::new())
        .with_send_encoder(jpeg_config(false))
        .with_send_encoder(zenpng::PngEncoderConfig::new())
}
fn request<'a>(budget: StorageBudget) -> TranscodeRequest<'a> {
    TranscodeRequest::new(ImageFormat::Png)
        .with_storage(budget)
        .with_cms(&MoxCms)
        .with_output(
            OutputProfile::Named(Cicp::SRGB),
            PixelFormat::Rgb8,
            ConvertOptions::forbid_lossy(),
        )
        .with_metadata(MetadataTransfer::DiscardDescriptive)
}
fn explain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut out = error.to_string();
    let mut cause = error.source();
    while let Some(e) = cause {
        out += &format!("\n  caused by {e:?}");
        cause = e.source();
    }
    out
}

#[test]
fn native_jpeg_to_png_matches_separate_cms_and_png_oracles() {
    let codecs = codecs();
    for progressive in [false, true] {
        let input = fixture(progressive);
        let source = zencodec::operation::DecodeRequest::new()
            .decode(&codecs, &input)
            .unwrap();
        let mut cms = MoxCms
            .build_source_transform(
                ColorProfileSource::Icc(ADOBE_RGB),
                ColorProfileSource::Cicp(Cicp::SRGB),
                PixelFormat::Rgb8,
                PixelFormat::Rgb8,
                &ConvertOptions::forbid_lossy(),
            )
            .unwrap()
            .unwrap();
        let mut expected = vec![0; WIDTH as usize * HEIGHT as usize * 3];
        for y in 0..HEIGHT {
            cms.transform_row(
                source.pixels().row(y),
                &mut expected
                    [y as usize * WIDTH as usize * 3..(y + 1) as usize * WIDTH as usize * 3],
                WIDTH,
            );
        }
        let budget = StorageBudget::new(512 * 1024, 16);
        let mut joint = 0;
        let output = request(budget.clone())
            .transcode_with(&codecs, &input, &mut |question: Question<'_>| -> Result<
                Decision,
                At<Infallible>,
            > {
                if let Question::Transcode { facts, choices } = question {
                    joint += 1;
                    assert_eq!((facts.width, facts.height), (WIDTH, HEIGHT));
                    for choice in choices.iter() {
                        assert!(!choice.value().retains_opaque_profile);
                        assert_eq!(choice.value().workspace_upper_bound, None);
                        assert!(choice.value().admitted_bytes > 0);
                        assert!(budget.reserved_bytes() >= choice.value().admitted_bytes);
                    }
                }
                Ok(Decision::UsePreset)
            })
            .unwrap_or_else(|e| panic!("{}", explain(&e)));
        assert_eq!(joint, 1);
        assert!(matches!(
            output.report().decisions().last().unwrap().selected,
            Some(Selection::Transcode(_))
        ));
        let mut reader = png::Decoder::new(Cursor::new(output.data()))
            .read_info()
            .unwrap();
        let mut actual = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut actual).unwrap();
        assert_eq!(
            (info.width, info.height, info.color_type, info.bit_depth),
            (WIDTH, HEIGHT, png::ColorType::Rgb, png::BitDepth::Eight)
        );
        assert_eq!(&actual[..info.buffer_size()], expected);
        assert!(
            !output
                .data()
                .windows(SECRET.len())
                .any(|w| w == SECRET.as_bytes())
        );
        assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
    }
}

#[test]
fn opaque_profile_policy_is_explicit_and_refusal_releases_all_managed_owners() {
    let input = fixture(true);
    let codecs = codecs();
    let budget = StorageBudget::new(512 * 1024, 16);
    let request = TranscodeRequest::new(ImageFormat::Jpeg)
        .with_storage(budget.clone())
        .with_cms(&MoxCms);
    let error = request
        .transcode_with(&codecs, &input, &mut |question: Question<'_>| -> Result<
            Decision,
            At<Infallible>,
        > {
            if let Question::Transcode { choices, .. } = question {
                assert!(choices.iter().all(|c| c.value().retains_opaque_profile));
                return Ok(Decision::decline(Decline::ApplicationRule));
            }
            Ok(Decision::UsePreset)
        })
        .unwrap_err();
    assert_eq!(error.error().stage(), Stage::Policy, "{}", explain(&error));
    assert_eq!(
        error.error().report().decisions().last().unwrap().code,
        DecisionCode::Declined
    );
    assert!(!format!("{error:?}").contains(SECRET));
    assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
    let output = request
        .transcode(&codecs, &input)
        .unwrap_or_else(|e| panic!("{}", explain(&e)));
    let original = codecs
        .decode_job(ImageFormat::Jpeg)
        .unwrap()
        .probe_full(&input)
        .unwrap()
        .metadata();
    let retained = codecs
        .decode_job(ImageFormat::Jpeg)
        .unwrap()
        .probe_full(output.data())
        .unwrap()
        .metadata();
    assert_eq!(retained.icc_profile, original.icc_profile);
    assert_eq!(retained.exif, original.exif);
    assert_eq!(retained.xmp, original.xmp);
    assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
}

#[derive(Clone)]
struct Stop(Arc<AtomicBool>);
impl enough::Stop for Stop {
    fn check(&self) -> Result<(), enough::StopReason> {
        if self.0.load(Ordering::SeqCst) {
            Err(enough::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}
#[test]
fn callback_cancellation_and_publication_failure_preserve_reports_and_release_storage() {
    let input = fixture(false);
    let stop = Arc::new(AtomicBool::new(false));
    let codecs = codecs().with_stop(StopToken::new(Stop(stop.clone())));
    let budget = StorageBudget::new(512 * 1024, 16);
    let error = request(budget.clone())
        .transcode_with(&codecs, &input, &mut |question: Question<'_>| -> Result<
            Decision,
            At<Infallible>,
        > {
            if matches!(question, Question::Transcode { .. }) {
                stop.store(true, Ordering::SeqCst);
            }
            Ok(Decision::UsePreset)
        })
        .unwrap_err();
    assert_eq!(error.error().stage(), Stage::Policy, "{}", explain(&error));
    assert_eq!(
        error.error().report().decisions().last().unwrap().code,
        DecisionCode::Stopped
    );
    assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
    stop.store(false, Ordering::SeqCst);
    let error = request(budget.clone())
        .with_output_limit(1)
        .transcode(&codecs, &input)
        .unwrap_err();
    assert_eq!(error.error().stage(), Stage::Verify, "{}", explain(&error));
    assert!(matches!(
        error.error().report().decisions().last().unwrap().selected,
        Some(Selection::Transcode(_))
    ));
    assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
}

// This adapter deliberately drops requested metadata while using a real native
// encoder. Configuration success must not become false successful publication.
#[derive(Clone)]
struct IgnoresMetadata<C>(C);
struct IgnoresMetadataJob<J>(J);
impl<C: EncoderConfig> EncoderConfig for IgnoresMetadata<C> {
    type Error = C::Error;
    type Job = IgnoresMetadataJob<C::Job>;
    fn format() -> ImageFormat {
        C::format()
    }
    fn supported_descriptors() -> &'static [PixelDescriptor] {
        C::supported_descriptors()
    }
    fn job(self) -> Self::Job {
        IgnoresMetadataJob(self.0.job())
    }
}
impl<J: EncodeJob> EncodeJob for IgnoresMetadataJob<J> {
    type Error = J::Error;
    type Enc = J::Enc;
    type AnimationFrameEnc = J::AnimationFrameEnc;
    fn with_stop(self, stop: StopToken) -> Self {
        Self(self.0.with_stop(stop))
    }
    fn with_limits(self, limits: ResourceLimits) -> Self {
        Self(self.0.with_limits(limits))
    }
    #[allow(deprecated)] // Deliberately keep color while violating descriptive retention.
    fn with_metadata(self, metadata: Metadata) -> Self {
        let mut retained = Metadata::none();
        retained.icc_profile = metadata.icc_profile;
        retained.cicp = metadata.cicp;
        Self(self.0.with_metadata(retained))
    }
    fn try_set_policy(
        &mut self,
        policy: zencodec::encode::EncodePolicy,
    ) -> Result<(), zencodec::UnsupportedOperation> {
        self.0.try_set_policy(policy)
    }
    fn encoder(self) -> Result<Self::Enc, Self::Error> {
        self.0.encoder()
    }
    fn animation_frame_encoder(self) -> Result<Self::AnimationFrameEnc, Self::Error> {
        self.0.animation_frame_encoder()
    }
}
#[test]
fn a_real_encoder_that_ignores_metadata_cannot_publish_success() {
    let input = fixture(false);
    let codecs = CodecSet::new()
        .with_decoder(JpegDecoderConfig::new())
        .with_encoder(IgnoresMetadata(jpeg_config(false)));
    let budget = StorageBudget::new(512 * 1024, 16);
    let error = TranscodeRequest::new(ImageFormat::Jpeg)
        .with_cms(&MoxCms)
        .with_storage(budget.clone())
        .transcode(&codecs, &input)
        .unwrap_err();
    assert_eq!(error.error().stage(), Stage::Verify, "{}", explain(&error));
    assert!(matches!(
        error.error().report().decisions().last().unwrap().selected,
        Some(Selection::Transcode(_))
    ));
    assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
}

#[test]
fn discarded_descriptions_keep_rendering_orientation_and_builtin_policy_is_enforced() {
    let input = fixture_oriented(false, zencodec::Orientation::Rotate90);
    let codecs = codecs();
    let budget = StorageBudget::new(512 * 1024, 16);
    let privacy = zencodec::operation::PolicySet::new()
        .require_metadata(MetadataTransfer::DiscardDescriptive)
        .allow_opaque_profiles(false);
    let output = request(budget.clone())
        .with_policy_set(privacy)
        .transcode(&codecs, &input)
        .unwrap();
    let info = codecs
        .decode_job(ImageFormat::Png)
        .unwrap()
        .probe_full(output.data())
        .unwrap();
    assert_eq!(info.orientation, zencodec::Orientation::Rotate90);
    assert_eq!((info.width, info.height), (WIDTH, HEIGHT));
    assert!(info.metadata().xmp.is_none());
    assert!(
        !output
            .data()
            .windows(SECRET.len())
            .any(|w| w == SECRET.as_bytes())
    );
    let error = TranscodeRequest::new(ImageFormat::Jpeg)
        .with_cms(&MoxCms)
        .with_storage(budget.clone())
        .with_policy_set(privacy)
        .transcode(&codecs, &input)
        .unwrap_err();
    assert_eq!(error.error().stage(), Stage::Policy);
    let error = request(budget.clone())
        .with_policy_set(privacy.with_workspace_limit(u64::MAX))
        .transcode(&codecs, &input)
        .unwrap_err();
    assert_eq!(error.error().stage(), Stage::Policy); // Unknown is not an upper bound, even at this limit.
    assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
}

#[derive(Debug)]
struct SourceFailure(u32);
impl std::fmt::Display for SourceFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "source status {}", self.0)
    }
}
impl std::error::Error for SourceFailure {}
struct RangeSource<'a> {
    bytes: &'a [u8],
    reads: usize,
    fail_after: Option<usize>,
}
impl zencodec::source::ReadAt for RangeSource<'_> {
    type Error = SourceFailure;
    fn len(&mut self) -> Result<u64, Self::Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Self::Error> {
        self.reads += 1;
        if self.fail_after.is_some_and(|n| self.reads >= n) {
            return Err(SourceFailure(503));
        }
        out.copy_from_slice(&self.bytes[offset as usize..offset as usize + out.len()]);
        Ok(())
    }
}
#[test]
fn file_and_range_sources_match_slice_and_preserve_native_source_errors() {
    use std::io::Write;
    let input = fixture(true);
    let codecs = codecs();
    let budget = StorageBudget::new(512 * 1024, 16);
    let request = request(budget.clone()).with_io_options(zencodec_io::DriveOptions::new(0, 41));
    let expected = request.transcode(&codecs, &input).unwrap();
    let mut source = RangeSource {
        bytes: &input,
        reads: 0,
        fail_after: None,
    };
    let ranged = request
        .transcode_from_read_at(&codecs, &mut source)
        .unwrap();
    assert!(source.reads > 0);
    assert_eq!(ranged.data(), expected.data());
    std::fs::create_dir_all("target/checked-transcode-fixtures").unwrap();
    let path = format!(
        "target/checked-transcode-fixtures/{}.jpg",
        std::process::id()
    );
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(&input).unwrap();
    drop(file);
    let mut source = zencodec_io::FileReadAt::open(&path).unwrap();
    let disk = request
        .transcode_from_read_at(&codecs, &mut source)
        .unwrap();
    assert_eq!(disk.data(), expected.data());
    let mut source = RangeSource {
        bytes: &input,
        reads: 0,
        fail_after: Some(1),
    };
    let error = request
        .transcode_from_read_at(&codecs, &mut source)
        .unwrap_err();
    assert_eq!(error.error().stage(), Stage::Decode);
    assert_eq!(
        zencodec::find_cause::<SourceFailure>(&error).unwrap().0,
        503
    );
    assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
}

#[derive(Debug)]
struct TenantFailure;
impl std::fmt::Display for TenantFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(SECRET)
    }
}
impl std::error::Error for TenantFailure {}
#[test]
fn callback_trace_and_unwind_release_prepared_native_routes() {
    let input = fixture(false);
    let codecs = codecs();
    let budget = StorageBudget::new(512 * 1024, 16);
    let error = request(budget.clone())
        .transcode_with(&codecs, &input, &mut |q: Question<'_>| -> Result<
            Decision,
            At<TenantFailure>,
        > {
            if matches!(q, Question::Transcode { .. }) {
                return Err(At::wrap(TenantFailure).at_named("tenant policy lookup"));
            }
            Ok(Decision::UsePreset)
        })
        .unwrap_err();
    assert_eq!(error.error().stage(), Stage::Policy);
    assert!(zencodec::find_cause::<TenantFailure>(&error).is_some());
    assert!(!format!("{error:?}").contains(SECRET));
    assert_eq!(
        error.error().report().decisions().last().unwrap().code,
        DecisionCode::CallbackFailed
    );
    assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = request(budget.clone()).transcode_with(
            &codecs,
            &input,
            &mut |q: Question<'_>| -> Result<Decision, At<Infallible>> {
                if matches!(q, Question::Transcode { .. }) {
                    panic!("injected callback panic")
                }
                Ok(Decision::UsePreset)
            },
        );
    }));
    assert!(panicked.is_err());
    assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
}

#[test]
fn reported_physical_resolution_cannot_silently_disappear() {
    let mut input = fixture(false);
    // Replace or insert a valid JFIF density marker: 300 x 150 pixels per inch.
    let app0: &[u8] = b"\xff\xe0\x00\x10JFIF\x00\x01\x01\x01\x01\x2c\x00\x96\x00\x00";
    if input.get(2..4) == Some(&[0xff, 0xe0]) {
        let length = u16::from_be_bytes([input[4], input[5]]) as usize;
        input.splice(2..4 + length, app0.iter().copied());
    } else {
        input.splice(2..2, app0.iter().copied());
    }
    let codecs = codecs();
    let source = codecs
        .decode_job(ImageFormat::Jpeg)
        .unwrap()
        .probe_full(&input)
        .unwrap();
    assert_eq!(source.resolution.unwrap().dpi(), (300.0, 150.0));
    let error = TranscodeRequest::new(ImageFormat::Png)
        .with_cms(&MoxCms)
        .transcode(&codecs, &input)
        .unwrap_err();
    assert_eq!(error.error().stage(), Stage::Verify, "{}", explain(&error));
    assert_eq!(
        zencodec::find_cause::<zencodecs::checked_transcode::Refusal>(&error),
        Some(&zencodecs::checked_transcode::Refusal::PhysicalResolution)
    );
    // Dropping descriptive metadata is an explicit separate request.
    request(StorageBudget::new(512 * 1024, 16))
        .transcode(&codecs, &input)
        .unwrap();
}

#[cfg(feature = "checked-transcode-async")]
#[path = "checked_transcode/async_source.rs"]
mod asynchronous;

#[test]
fn default_p3_preservation_keeps_authority_and_required_policies_still_win() {
    use zencodec::decode::{DecodeJob, DecoderConfig};
    use zencodec::{ColorAuthority, UnsupportedOperation, encode::EncodePolicy};
    use zenpixels_convert::icc_profiles::DISPLAY_P3_V4;
    let bytes = [19, 31, 63].repeat(17 * 9);
    let codecs = codecs();
    for progressive in [false, true] {
        let input = jpeg_config(progressive)
            .job()
            .with_metadata_policy(
                Metadata::none().with_icc(DISPLAY_P3_V4),
                MetadataPolicy::PreserveExact,
            )
            .encoder()
            .unwrap()
            .encode(PixelSlice::new(&bytes, 17, 9, 51, PixelDescriptor::RGB8_SRGB).unwrap())
            .unwrap();
        let expected = codecs.decode(input.data()).unwrap();
        let output = TranscodeRequest::new(ImageFormat::Png)
            .transcode(&codecs, input.data())
            .unwrap_or_else(|error| panic!("{}", explain(&error)));
        let info = zenpng::PngDecoderConfig::new()
            .job()
            .probe_full(output.data())
            .unwrap();
        assert_eq!(info.source_color.color_authority, ColorAuthority::Icc);
        assert_eq!(
            info.source_color.icc_profile.as_deref(),
            Some(DISPLAY_P3_V4)
        );
        assert!(info.source_color.cicp.is_none());
        assert!(
            info.source_color.bit_depth.unwrap() < 8,
            "exercise packed palette indices"
        );
        let mut oracle = png::Decoder::new(Cursor::new(output.data()));
        oracle.set_transformations(png::Transformations::EXPAND);
        let mut oracle = oracle.read_info().unwrap();
        let mut raw = vec![0; oracle.output_buffer_size().unwrap()];
        let frame = oracle.next_frame(&mut raw).unwrap();
        assert_eq!(
            (frame.color_type, frame.bit_depth),
            (png::ColorType::Rgb, png::BitDepth::Eight)
        );
        assert_eq!(frame.buffer_size(), 17 * 9 * 3);
        for y in 0..9 {
            assert_eq!(&raw[y as usize * 51..][..51], expected.pixels().row(y));
        }
        let actual = codecs.decode(output.data()).unwrap();
        for y in 0..9 {
            assert_eq!(actual.pixels().row(y), expected.pixels().row(y));
        }
        // Conflicting required fields are not replaced by the adapter default.
        // This codec has not qualified the strict/embed fields, so even an
        // invalid source is rejected at preparation with the policy cause.
        for required in [
            EncodePolicy::none().with_strict(true),
            EncodePolicy::none().with_embed_icc(false),
        ] {
            for request in [
                TranscodeRequest::new(ImageFormat::Png),
                TranscodeRequest::new(ImageFormat::Png).with_encode_policy(required),
            ] {
                let error = request
                    .transcode(&codecs.clone().with_encode_policy(required), b"invalid")
                    .unwrap_err();
                assert_eq!(error.error().stage(), Stage::Prepare);
                assert_eq!(
                    zencodec::find_cause::<UnsupportedOperation>(&error),
                    Some(&UnsupportedOperation::EncodePolicy)
                );
            }
        }
    }
}

#[path = "checked_transcode/png_source.rs"]
mod png_source;
