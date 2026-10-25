//! Native output and cleanup through delayed, short, failed async reads.
use super::*;
use core::{
    future::{Future, poll_fn},
    task::{Context, Poll, Waker},
};
use zencodec_io::{DriveOptions, WorkQuota, tokio_io::AsyncSource};

type SourceError = Box<dyn std::error::Error + Send + Sync>;
pub(super) struct Network<'a> {
    bytes: &'a [u8],
    chunk: usize,
    known_len: bool,
    reads: usize,
    fail_at: Option<usize>,
}
impl<'a> Network<'a> {
    pub(super) fn new(bytes: &'a [u8], chunk: usize, known_len: bool) -> Self {
        Self {
            bytes,
            chunk,
            known_len,
            reads: 0,
            fail_at: None,
        }
    }
}
impl AsyncSource for Network<'_> {
    async fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<usize, SourceError> {
        let mut yielded = false;
        poll_fn(|cx| {
            if yielded {
                Poll::Ready(())
            } else {
                yielded = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await;
        self.reads += 1;
        if self.fail_at == Some(self.reads) {
            return Err(Box::new(
                At::wrap(SourceFailure(503)).at_named("async range source"),
            ));
        }
        let offset = usize::try_from(offset)?;
        let available = self.bytes.get(offset..).unwrap_or_default();
        let count = available.len().min(out.len()).min(self.chunk);
        out[..count].copy_from_slice(&available[..count]);
        Ok(count)
    }
    fn len_hint(&self) -> Option<u64> {
        self.known_len.then_some(self.bytes.len() as u64)
    }
    async fn len(&mut self) -> Result<u64, SourceError> {
        Ok(self.bytes.len() as u64)
    }
}

// Sources here wake after each one-poll delay, so no reactor is needed.
// A bounded manual executor also makes a non-progressing driver a test failure.
pub(super) fn drive<F: Future>(future: F) -> (F::Output, usize) {
    let mut future = core::pin::pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    for pending in 0..100_000 {
        if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
            return (result, pending);
        }
    }
    panic!("async operation did not finish within the test poll budget")
}
fn options() -> DriveOptions {
    DriveOptions::new(0, 41).with_work_quota(WorkQuota::new(1).unwrap())
}

#[test]
fn delayed_short_sources_match_verified_slice_output_and_choose_once() {
    let codecs = codecs();
    for progressive in [false, true] {
        let input = fixture(progressive);
        for chunk in [1, 7, 97] {
            for known in [false, true] {
                let budget = StorageBudget::new(512 * 1024, 16);
                let request = request(budget.clone()).with_io_options(options());
                let expected = request.transcode(&codecs, &input).unwrap();
                let mut source = Network::new(&input, chunk, known);
                let mut choices = 0;
                let mut resolver = |q: Question<'_>| -> Result<Decision, At<Infallible>> {
                    if matches!(q, Question::Transcode { .. }) {
                        choices += 1;
                    }
                    Ok(Decision::UsePreset)
                };
                let (actual, pending) =
                    drive(request.transcode_async_with(&codecs, &mut source, &mut resolver));
                let actual = actual.unwrap_or_else(|error| panic!("{}", explain(&error)));
                assert_eq!(actual.data(), expected.data());
                assert_eq!(choices, 1, "I/O waits must not create policy decisions");
                assert!(pending > 1);
                assert!(source.reads > 1);
                assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
            }
        }
    }
}

#[test]
fn source_failure_and_callback_failure_retain_native_traces() {
    let input = fixture(false);
    let codecs = codecs();
    let budget = StorageBudget::new(512 * 1024, 16);
    let request = request(budget.clone()).with_io_options(options());
    let mut source = Network::new(&input, 7, false);
    source.fail_at = Some(2);
    let error = drive(request.transcode_async(&codecs, &mut source))
        .0
        .unwrap_err();
    assert_eq!(error.error().stage(), Stage::Decode);
    assert_eq!(
        zencodec::find_cause::<SourceFailure>(&error).unwrap().0,
        503
    );
    assert!(zencodec::find_cause::<At<SourceFailure>>(&error).is_some());
    assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
    let mut source = Network::new(&input, 97, true);
    let mut resolver = |q: Question<'_>| -> Result<Decision, At<TenantFailure>> {
        if matches!(q, Question::Transcode { .. }) {
            return Err(At::wrap(TenantFailure).at_named("tenant refusal"));
        }
        Ok(Decision::UsePreset)
    };
    let error = drive(request.transcode_async_with(&codecs, &mut source, &mut resolver))
        .0
        .unwrap_err();
    assert_eq!(error.error().stage(), Stage::Policy);
    assert!(zencodec::find_cause::<TenantFailure>(&error).is_some());
    assert!(!format!("{:?}", error.error()).contains(SECRET));
    assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
}

#[test]
fn dropping_or_cancelling_a_pending_future_releases_admitted_source() {
    let input = fixture(true);
    for cancel in [false, true] {
        let stopped = Arc::new(AtomicBool::new(false));
        let codecs = codecs().with_stop(StopToken::new(Stop(stopped.clone())));
        let budget = StorageBudget::new(512 * 1024, 16);
        let request = request(budget.clone()).with_io_options(options());
        let mut source = Network::new(&input, 7, false);
        let mut future = Box::pin(request.transcode_async(&codecs, &mut source));
        let mut cx = Context::from_waker(Waker::noop());
        for _ in 0..100_000 {
            assert!(future.as_mut().poll(&mut cx).is_pending());
            if budget.reserved_bytes() > 0 {
                break;
            }
        }
        assert!(
            budget.reserved_bytes() > 0,
            "exercise a retained native source, not an empty future"
        );
        if cancel {
            stopped.store(true, Ordering::Relaxed);
            let error = drive(future.as_mut()).0.unwrap_err();
            assert!(
                zencodec::find_cause::<zencodec::decode::StepError<zencodec::decode::ErasedError>>(
                    &error
                )
                .is_some()
            );
        }
        drop(future);
        assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
    }
}

#[test]
fn async_admission_precedes_reads_and_default_future_can_move_between_threads() {
    fn require_send<T: Send>(_: T) {}
    let input = fixture(false);
    let codecs = codecs();
    let budget = StorageBudget::new(512 * 1024, 16);
    let request = request(budget.clone())
        .with_io_options(options().with_limits(zencodec_io::IoLimits::new(0, 0, 0)));
    let mut source = Network::new(&input, 7, true);
    require_send(request.transcode_async(&codecs, &mut source));
    let error = drive(request.transcode_async(&codecs, &mut source))
        .0
        .unwrap_err();
    assert_eq!(error.error().stage(), Stage::Decode);
    assert!(zencodec::find_cause::<zencodec_io::InputError>(&error).is_some());
    assert_eq!(source.reads, 0);
    assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
}

#[test]
fn async_transfer_registration_is_checked_before_input() {
    let input = fixture(false);
    let codecs = CodecSet::new()
        .with_decoder(zenjpeg::JpegDecoderConfig::new())
        .with_decoder(zenpng::PngDecoderConfig::new())
        .with_encoder(zenpng::PngEncoderConfig::new());
    let budget = StorageBudget::new(512 * 1024, 16);
    let request = request(budget.clone());
    let mut source = Network::new(&input, 7, true);
    let error = drive(request.transcode_async(&codecs, &mut source))
        .0
        .unwrap_err();
    assert_eq!(error.error().stage(), Stage::Prepare);
    assert!(matches!(
        zencodec::find_cause::<zencodec::CodecSetError>(&error),
        Some(zencodec::CodecSetError::EncoderJobNotSend(ImageFormat::Png))
    ));
    assert_eq!(source.reads, 0);
    assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
}

#[test]
fn png_network_sources_match_sync_with_alpha_and_sixteen_bit_samples() {
    let codecs = codecs();
    for descriptor in [PixelDescriptor::RGBA8_SRGB, PixelDescriptor::RGB16_SRGB] {
        let (input, _) = super::png_source::png_fixture(descriptor, false);
        let budget = StorageBudget::new(512 * 1024, 16);
        let request = TranscodeRequest::new(ImageFormat::Png)
            .with_fidelity(zencodec::encode::Fidelity::Lossless)
            .with_storage(budget.clone())
            .with_io_options(options());
        let expected = request.transcode(&codecs, &input).unwrap();
        for chunk in [1, 31] {
            for known in [false, true] {
                let mut source = Network::new(&input, chunk, known);
                let (result, pending) = drive(request.transcode_async(&codecs, &mut source));
                let output = result.unwrap_or_else(|e| panic!("{}", explain(&e)));
                assert_eq!(output.data(), expected.data());
                assert!(pending > 1);
                assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
            }
        }
    }
}
