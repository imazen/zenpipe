//! Native JPEG/PNG controls for consuming publication.
//! The independent PNG/CMS oracles remain in the parent suite.
use super::*;
use std::io::{self, Write};
use zencodec::NoPulse;
use zencodec::operation::{BoundPolicy, OutputSchedule, PolicySet};

// The usable composition: metadata treatment plus a finish-first schedule.
fn requirements() -> PolicySet {
    PolicySet::new()
        .require_metadata(MetadataTransfer::DiscardDescriptive)
        .allow_opaque_profiles(false)
        .require_output_schedule(OutputSchedule::FinishFirst)
}
struct ReleasedWriter {
    budget: StorageBudget,
    bytes: Vec<u8>,
}
impl Write for ReleasedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        assert_eq!(
            (self.budget.reserved_bytes(), self.budget.reserved_objects()),
            (0, 0)
        );
        let n = bytes.len().min(7);
        self.bytes.extend_from_slice(&bytes[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn metadata_and_finish_first_share_one_retained_route_with_unknown_wait() {
    for progressive in [false, true] {
        let budget = StorageBudget::new(512 * 1024, 16);
        let mut joint = 0;
        let (ready, expected) = {
            let input = fixture(progressive);
            let codecs = codecs();
            let req = request(budget.clone());
            let expected = req
                .transcode(&codecs, &input)
                .unwrap()
                .into_output()
                .into_vec();
            let mut policy = BoundPolicy::new(
                requirements(),
                |question: Question<'_>| -> Result<Decision, At<Infallible>> {
                    if let Question::Transcode { choices, .. } = question {
                        joint += 1;
                        for choice in choices.iter() {
                            let offer = choice.value();
                            assert_eq!(offer.metadata, MetadataTransfer::DiscardDescriptive);
                            assert_eq!(offer.retains_opaque_profile, Some(false));
                            let schedule = offer.output_schedule.unwrap();
                            assert_eq!(schedule.schedule, OutputSchedule::FinishFirst);
                            // A borrowed pulse may retain anything.
                            assert_eq!(schedule.wait_retained_upper_bound, None);
                            assert_eq!(offer.workspace_upper_bound, None);
                        }
                    }
                    Ok(Decision::UsePreset)
                },
            );
            let ready = req
                .transcode_for_publication(&codecs, &input, &mut policy)
                .unwrap();
            (ready, expected)
        }; // Input, codecs, request and callback do not survive into the writer.
        assert_eq!(joint, 1);
        let mut writer = ReleasedWriter {
            budget: budget.clone(),
            bytes: vec![],
        };
        let completed = ready.write_to(&mut writer, Some(&NoPulse)).unwrap();
        assert_eq!(*completed.output(), writer.bytes.len());
        assert_eq!(writer.bytes, expected);
        assert!(
            !writer
                .bytes
                .windows(SECRET.len())
                .any(|b| b == SECRET.as_bytes())
        );
    }
}

#[test]
fn unsupported_schedule_wait_or_workspace_refuses_before_the_joint_callback() {
    let input = fixture(false);
    let codecs = codecs();
    for required in [
        requirements().require_output_schedule(OutputSchedule::Live),
        requirements().with_output_wait_limit(1 << 20),
        // Even the largest cap cannot turn unknown retention into evidence.
        requirements().with_output_wait_limit(u64::MAX),
        requirements().with_workspace_limit(u64::MAX),
    ] {
        let budget = StorageBudget::new(512 * 1024, 16);
        let mut policy = BoundPolicy::new(
            required,
            |question: Question<'_>| -> Result<Decision, At<Infallible>> {
                assert!(!matches!(question, Question::Transcode { .. }));
                Ok(Decision::UsePreset)
            },
        );
        let error = request(budget.clone())
            .transcode_for_publication(&codecs, &input, &mut policy)
            .unwrap_err();
        assert_eq!(
            error.error().stage(),
            if required.check_workspace(None).is_err() {
                Stage::Prepare
            } else {
                Stage::Policy
            }
        );
        assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
    }
}

#[test]
fn ordinary_routes_still_offer_unknown_schedule_evidence() {
    // The ordinary checked route has no schedule evidence: the same finish-first
    // requirement refuses there, because nothing supplies its evidence.
    let budget = StorageBudget::new(512 * 1024, 16);
    let mut policy = requirements();
    let error = request(budget.clone())
        .transcode_with(&codecs(), &fixture(false), &mut policy)
        .unwrap_err();
    assert_eq!(error.error().stage(), Stage::Policy);
    assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
}

#[test]
fn selected_route_output_overflow_never_returns_a_publication_owner() {
    let budget = StorageBudget::new(512 * 1024, 16);
    let mut policy = requirements();
    let error = request(budget.clone())
        .with_output_limit(1)
        .transcode_for_publication(&codecs(), &fixture(false), &mut policy)
        .unwrap_err();
    assert_eq!(error.error().stage(), Stage::Verify);
    assert!(matches!(
        zencodec::find_cause::<zencodecs::checked_transcode::Refusal>(&error),
        Some(zencodecs::checked_transcode::Refusal::OutputLimit { .. })
    ));
    assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
}

#[test]
fn prepared_job_metadata_claim_still_needs_successful_postconditions() {
    let codecs = CodecSet::new()
        .with_decoder(JpegDecoderConfig::new())
        .with_encoder(IgnoresMetadata(jpeg_config(false)));
    let mut policy = PolicySet::new()
        .require_metadata(MetadataTransfer::PreserveReported)
        .require_output_schedule(OutputSchedule::FinishFirst);
    let error = TranscodeRequest::new(ImageFormat::Jpeg)
        .with_cms(&MoxCms)
        .transcode_for_publication(&codecs, &fixture(false), &mut policy)
        .unwrap_err();
    assert_eq!(error.error().stage(), Stage::Verify);
}

#[test]
fn callback_unwind_drops_prepared_route_without_a_publication_owner() {
    let budget = StorageBudget::new(512 * 1024, 16);
    let input = fixture(false);
    let codecs = codecs();
    let mut policy = BoundPolicy::new(
        requirements(),
        |question: Question<'_>| -> Result<Decision, At<Infallible>> {
            if matches!(question, Question::Transcode { .. }) {
                panic!("review callback unwind")
            }
            Ok(Decision::UsePreset)
        },
    );
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        request(budget.clone()).transcode_for_publication(&codecs, &input, &mut policy)
    }));
    assert!(result.is_err());
    assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
}

// A caller's control is not necessarily small just because it is borrowed.
struct RasterControl {
    raster: Vec<u8>,
    cancelled: Arc<AtomicBool>,
    dropped: Arc<AtomicBool>,
}
impl enough::Stop for RasterControl {
    fn check(&self) -> Result<(), enough::StopReason> {
        assert!(!self.raster.is_empty());
        if self.cancelled.load(Ordering::SeqCst) {
            Err(enough::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}
impl Drop for RasterControl {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}
#[test]
fn a_raster_retaining_pulse_stays_alive_and_a_wait_cap_refuses_the_route() {
    let input = fixture(false);
    let cancelled = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let pulse = zencodec::how_far::StopOnly::new(RasterControl {
        raster: vec![7; 8192],
        cancelled: cancelled.clone(),
        dropped: dropped.clone(),
    });
    let budget = StorageBudget::new(512 * 1024, 16);
    let mut capped = requirements().with_output_wait_limit(u64::MAX);
    let error = request(budget.clone())
        .transcode_for_publication(&codecs(), &input, &mut capped)
        .unwrap_err();
    assert_eq!(error.error().stage(), Stage::Policy);
    // A separate operation without a cap, not a retry of the refused one.
    let mut uncapped = requirements();
    let ready = request(budget.clone())
        .transcode_for_publication(&codecs(), &input, &mut uncapped)
        .unwrap();
    cancelled.store(true, Ordering::SeqCst);
    let mut writer = ReleasedWriter {
        budget,
        bytes: vec![],
    };
    let error = ready.write_to(&mut writer, Some(&pulse)).unwrap_err();
    assert_eq!(error.error().emitted(), 0);
    assert!(writer.bytes.is_empty());
    assert!(
        error
            .error()
            .report()
            .decisions()
            .any(|r| matches!(r.selected, Some(Selection::Transcode(_))))
    );
    // The control and its raster lived through publication; the route never
    // owned it, which is exactly why its retention is unknown.
    assert!(!dropped.load(Ordering::SeqCst));
    drop(pulse);
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn workspace_entry_refusal_has_typed_evidence_and_no_callback_source_or_writer_effects() {
    use zencodec::operation::{PolicyRequirement, RequirementEvidence, UnmetRequirements};
    let bytes = fixture(false);
    for legacy in [false, true] {
        let budget = StorageBudget::new(512 * 1024, 16);
        let required = requirements().with_workspace_limit(u64::MAX);
        let req = request(budget.clone());
        #[allow(deprecated)]
        let req = if legacy {
            req.with_policy_set(required)
        } else {
            req
        };
        let mut policy = BoundPolicy::new(
            if legacy { requirements() } else { required },
            |_: Question<'_>| -> Result<Decision, At<TenantFailure>> {
                panic!("entry refusal must precede callbacks")
            },
        );
        let codecs = codecs();
        let error = req
            .transcode_for_publication(&codecs, &bytes, &mut policy)
            .unwrap_err();
        assert_eq!(error.error().stage(), Stage::Prepare);
        assert_eq!(
            zencodec::find_cause::<UnmetRequirements>(&error)
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            [(PolicyRequirement::Workspace, RequirementEvidence::Unknown)]
        );
        assert_eq!(error.error().report().decisions().count(), 0);
        let mut source = RangeSource {
            bytes: &bytes,
            reads: 0,
            fail_after: None,
        };
        let error = req
            .transcode_from_read_at_with(&codecs, &mut source, &mut policy)
            .unwrap_err();
        assert_eq!(error.error().stage(), Stage::Prepare);
        assert_eq!(source.reads, 0);
        assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
        let writer = ReleasedWriter {
            budget,
            bytes: vec![],
        };
        assert!(writer.bytes.is_empty()); // No owner exists to invoke it.
    }
}

// Fixture framing reuses zencodec's jpeg-policy-validation experiment at
// 20b63f47; parsing stays with native MarkerIter, not a second JPEG parser.
#[derive(Clone, Copy, Debug)]
enum Placement {
    Header,
    BeforeEoi,
    SplitIcc,
}
fn segment(marker: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0xff, marker];
    bytes.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
    bytes.extend_from_slice(payload);
    bytes
}
fn icc_part(bytes: &[u8], part: u8, total: u8) -> Vec<u8> {
    let mut payload = b"ICC_PROFILE\0".to_vec();
    payload.extend_from_slice(&[part, total]);
    payload.extend_from_slice(bytes);
    segment(0xe2, &payload)
}
fn late_fixture(progressive: bool, placement: Placement, color: bool) -> Vec<u8> {
    use zenjpeg::container::{MarkerKind, marker};
    let raw = [23, 101, 211].repeat(WIDTH as usize * HEIGHT as usize);
    let base = jpeg_config(progressive)
        .encode(
            PixelSlice::new(
                &raw,
                WIDTH,
                HEIGHT,
                WIDTH as usize * 3,
                PixelDescriptor::RGB8_SRGB,
            )
            .unwrap(),
        )
        .unwrap()
        .into_vec();
    let spans = marker::iter(&base).collect::<Vec<_>>();
    let eoi = spans
        .iter()
        .find(|s| s.kind == MarkerKind::Eoi)
        .unwrap()
        .offset;
    let insertion = match placement {
        Placement::Header => 2,
        _ => eoi,
    };
    let mut out = base[..insertion].to_vec();
    if color {
        let profile = ADOBE_RGB;
        let mid = profile.len() / 2;
        if matches!(placement, Placement::SplitIcc) {
            out.splice(2..2, icc_part(&profile[..mid], 1, 2));
        } else {
            out.extend(icc_part(&profile[..mid], 1, 2));
        }
        out.extend(icc_part(&profile[mid..], 2, 2));
    }
    let mut exif = zencodec::exif::Exif::new(zencodec::exif::TextEncoding::Ascii);
    exif.set_artist(SECRET);
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend(zenmeta::exif::serialize(&exif));
    out.extend(segment(0xe1, &payload));
    let mut xmp = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
    xmp.extend_from_slice(SECRET.as_bytes());
    out.extend(segment(0xe1, &xmp));
    out.extend_from_slice(&base[insertion..]);
    out
}
#[test]
fn native_late_descriptions_are_preserved_or_discarded_with_independent_png_readback() {
    for progressive in [false, true] {
        for placement in [Placement::Header, Placement::BeforeEoi] {
            // The color interpretation is stable; descriptions really sit after
            // the scans in BeforeEoi. probe_full must include them before adopt.
            let input = late_fixture(progressive, placement, false);
            for treatment in [
                MetadataTransfer::PreserveReported,
                MetadataTransfer::DiscardDescriptive,
            ] {
                let codecs = codecs();
                let mut policy = PolicySet::new()
                    .require_metadata(treatment)
                    .require_output_schedule(OutputSchedule::FinishFirst);
                let ready = TranscodeRequest::new(ImageFormat::Png)
                    .with_metadata(treatment)
                    .transcode_for_publication(&codecs, &input, &mut policy)
                    .unwrap_or_else(|e| panic!("{placement:?}: {}", explain(&e)));
                let mut output = vec![];
                ready.write_to(&mut output, Some(&NoPulse)).unwrap();
                let mut decoder = png::Decoder::new(Cursor::new(&output));
                decoder.set_transformations(png::Transformations::EXPAND);
                let mut reader = decoder.read_info().unwrap();
                let keep = treatment == MetadataTransfer::PreserveReported;
                assert_eq!(reader.info().exif_metadata.is_some(), keep);
                assert_eq!(
                    reader
                        .info()
                        .utf8_text
                        .iter()
                        .any(|t| t.keyword == "XML:com.adobe.xmp"),
                    keep
                );
                let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
                let info = reader.next_frame(&mut pixels).unwrap();
                assert_eq!((info.width, info.height), (WIDTH, HEIGHT));
                // Separate native source decode; independent PNG decompression.
                let source = operation_source(&codecs, &input);
                assert_eq!(&pixels[..info.buffer_size()], source.as_slice());
                assert_eq!(
                    output.windows(SECRET.len()).any(|b| b == SECRET.as_bytes()),
                    keep
                );
            }
        }
    }
}
fn operation_source(codecs: &CodecSet, input: &[u8]) -> Vec<u8> {
    let source = zencodec::operation::DecodeRequest::new()
        .decode(codecs, input)
        .unwrap();
    (0..source.height())
        .flat_map(|y| source.pixels().row(y).to_vec())
        .collect()
}
#[test]
fn native_late_and_split_icc_never_return_a_publication_owner() {
    for progressive in [false, true] {
        for placement in [Placement::BeforeEoi, Placement::SplitIcc] {
            let input = late_fixture(progressive, placement, true);
            let budget = StorageBudget::new(512 * 1024, 16);
            let mut policy = requirements();
            let error = request(budget.clone())
                .transcode_for_publication(&codecs(), &input, &mut policy)
                .unwrap_err();
            assert_eq!(error.error().stage(), Stage::Policy);
            assert!(
                !error
                    .error()
                    .report()
                    .decisions()
                    .any(|r| matches!(r.selected, Some(Selection::Transcode(_))))
            );
            assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
        }
    }
}

#[test]
fn native_png_late_metadata_is_unknown_before_encoder_configuration() {
    let metadata = Metadata::none().with_xmp(SECRET.as_bytes().to_vec());
    let pixels = [23, 45, 67];
    let input = zenpng::PngEncoderConfig::new()
        .job()
        .with_metadata_policy(metadata, MetadataPolicy::PreserveExact)
        .encoder()
        .unwrap()
        .encode(PixelSlice::new(&pixels, 1, 1, 3, PixelDescriptor::RGB8_SRGB).unwrap())
        .unwrap()
        .into_vec();
    let codecs = codecs();
    let inventory = codecs
        .decode_job(ImageFormat::Png)
        .unwrap()
        .inventory(&input)
        .unwrap();
    let part = inventory
        .parts()
        .iter()
        .find(|p| {
            p.parent().is_none() && p.tag() == &zencodec::inventory::PartTag::FourCc(*b"iTXt")
        })
        .unwrap()
        .range();
    let mut late = input.clone();
    let chunk = late
        .drain(part.start as usize..part.end as usize)
        .collect::<Vec<_>>();
    let inventory = codecs
        .decode_job(ImageFormat::Png)
        .unwrap()
        .inventory(&late)
        .unwrap();
    let end = inventory
        .parts()
        .iter()
        .find(|p| {
            p.parent().is_none() && p.tag() == &zencodec::inventory::PartTag::FourCc(*b"IEND")
        })
        .unwrap()
        .range()
        .start as usize;
    late.splice(end..end, chunk);
    let mut joint = 0;
    let mut policy = BoundPolicy::new(
        PolicySet::new(),
        |q: Question<'_>| -> Result<Decision, At<Infallible>> {
            if matches!(q, Question::Transcode { .. }) {
                joint += 1;
            }
            Ok(Decision::UsePreset)
        },
    );
    let error = TranscodeRequest::new(ImageFormat::Png)
        .transcode_for_publication(&codecs, &late, &mut policy)
        .unwrap_err();
    assert_eq!(error.error().stage(), Stage::Plan);
    assert_eq!(
        zencodec::find_cause::<zencodecs::checked_transcode::Refusal>(&error),
        Some(&zencodecs::checked_transcode::Refusal::MetadataCompleteness)
    );
    assert_eq!(joint, 0);
}
