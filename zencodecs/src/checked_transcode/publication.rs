//! Consuming publication of completed, verified transcode output.
//!
//! [`VerifiedOutput`] is the usable composition: metadata requirements and a
//! finish-first schedule, then publication under the operation's universal
//! pulse when supplied. `None` adds no publication control. A supplied pulse is
//! borrowed while the destination waits and may retain a whole raster. The
//! route cannot constrain a later caller's choice, so it still offers
//! unknown wait retention, and any required wait cap refuses. The owner keeps
//! the encoder's own Vec, moved with `EncodeOutput::into_vec` (extras dropped,
//! no copy); its spare capacity stays.
//!
//! The finite counterpart is a private test-only witness: a fixed owner
//! published through the concrete `NoPulse`, whose layout size is then all the
//! route retains at a destination wait.
use super::*;
use std::io::{self, Write};

// Cooperation granularity between pulse checks, not a time bound.
const WRITE_CHUNK: usize = 64 * 1024;

/// Verified bytes for one consuming publication. Owns no source, codec, CMS,
/// callback or backend extras; it is not `Clone`.
/// It retains no operation pulse; publication control is supplied to `write_to`.
pub struct VerifiedOutput {
    bytes: Vec<u8>,
    report: OperationReport,
}
impl fmt::Debug for VerifiedOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerifiedOutput")
            .field("len", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

/// Failure after publication started. The accepted prefix is irrevocable.
pub struct PublicationError {
    emitted: usize,
    report: Box<OperationReport>,
    cause: Cause,
}
impl PublicationError {
    /// Bytes the destination confirmed before the failure.
    pub fn emitted(&self) -> usize {
        self.emitted
    }
    /// The operation's bounded policy history.
    pub fn report(&self) -> &OperationReport {
        &self.report
    }
}
impl fmt::Debug for PublicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PublicationError")
            .field("emitted", &self.emitted)
            .finish_non_exhaustive()
    }
}
impl fmt::Display for PublicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "publication failed after {} accepted bytes",
            self.emitted
        )
    }
}
impl core::error::Error for PublicationError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

fn check(pulse: &dyn zencodec::Pulse) -> Result<(), Cause> {
    pulse
        .check()
        .map_err(|reason| operation::OperationError::Stopped(reason).into())
}

// Write `bytes` once. The pulse is checked before every write attempt, an
// `Interrupted` retry included, before the flush (so also after the last
// write and for empty output) and after a successful flush. Each accepted
// count is recorded before the next check. A write or flush error returns
// directly and wins over a coinciding stop. Nothing else is retried.
fn publish<W: Write>(
    bytes: &[u8],
    emitted: &mut usize,
    writer: &mut W,
    pulse: &dyn zencodec::Pulse,
) -> Result<(), Cause> {
    while *emitted < bytes.len() {
        check(pulse)?;
        let end = bytes.len().min(emitted.saturating_add(WRITE_CHUNK));
        let pending = &bytes[*emitted..end];
        match writer.write(pending) {
            Ok(0) => return Err(io::Error::from(io::ErrorKind::WriteZero).into()),
            Ok(n) if n <= pending.len() => *emitted += n,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "writer returned an invalid count",
                )
                .into());
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
    check(pulse)?;
    writer.flush()?;
    // A stop seen after a successful flush is still reported, with the full
    // count: every byte may already be visible.
    check(pulse)
}

fn receipt(
    result: Result<(), Cause>,
    emitted: usize,
    report: OperationReport,
) -> Result<Outcome<usize>, At<PublicationError>> {
    match result {
        Ok(()) => Ok(Outcome::from_parts(emitted, report)),
        Err(cause) => Err(At::wrap(PublicationError {
            emitted,
            report: Box::new(report),
            cause,
        })),
    }
}

impl VerifiedOutput {
    fn from_verified(outcome: Outcome<EncodeOutput>) -> Self {
        let (encoded, report) = outcome.into_parts();
        // Moves the encoder's Vec; its Extensions are dropped here, before any
        // destination wait. No copy, no shrink.
        Self {
            bytes: encoded.into_vec(),
            report,
        }
    }
    /// Write this verified output once, under `pulse` (including interrupted retries and flush). The
    /// `None` adds no per-call control; `Some(pulse)` is borrowed for this call.
    /// A blocking write or flush cannot
    /// be preempted. Dropping the owner never writes; an error consumes it.
    pub fn write_to<W: Write>(
        self,
        writer: &mut W,
        pulse: Option<&dyn zencodec::Pulse>,
    ) -> Result<Outcome<usize>, At<PublicationError>> {
        let mut emitted = 0;
        let result = publish(
            &self.bytes,
            &mut emitted,
            writer,
            pulse.unwrap_or(&zencodec::NoPulse),
        );
        receipt(result, emitted, self.report)
    }
}

impl TranscodeRequest<'_> {
    /// Checked transcode and its verification, then an owner for
    /// publication under the universal pulse. The same requirements reach
    /// discovery and the joint Transcode question; none is cleared or
    /// discharged separately. Finish-first with unknown wait retention: a
    /// required wait cap refuses before the joint callback.
    pub fn transcode_for_publication<R: OperationPolicy + ?Sized>(
        &self,
        codecs: &CodecSet,
        input: &[u8],
        policy: &mut R,
    ) -> Result<VerifiedOutput, At<Error>> {
        let schedule = operation::OutputScheduleOffer::new(operation::OutputSchedule::FinishFirst);
        let mut cursor = SliceCursor::new(self.io.read_size);
        let completed = self.execute(
            codecs,
            policy,
            Some(schedule),
            Some(input),
            |held, pulse, _| Ok(cursor.next(input, held, pulse)?),
        )?;
        Ok(VerifiedOutput::from_verified(completed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use zencodec::NoPulse;

    // PRIVATE proof witness, not a production representation. Only this closed
    // route fixes publication control to the concrete NoPulse.
    struct FixedProof<const N: usize> {
        bytes: [u8; N],
        len: usize,
        report: OperationReport,
    }
    impl<const N: usize> FixedProof<N> {
        fn wait_bound() -> u64 {
            size_of::<Self>() as u64
        }
        fn from_verified(outcome: Outcome<EncodeOutput>) -> Result<Self, At<Error>> {
            let (encoded, report) = outcome.into_parts();
            if encoded.len() > N {
                return Err(At::wrap(Error {
                    stage: Stage::Verify,
                    report: Box::new(report),
                    cause: Box::new(Refusal::OutputLimit {
                        actual: encoded.len(),
                        limit: N,
                    }),
                }));
            }
            let mut proof = Self {
                bytes: [0; N],
                len: encoded.len(),
                report,
            };
            proof.bytes[..proof.len].copy_from_slice(encoded.data());
            drop(encoded);
            Ok(proof)
        }
        fn write_to<W: Write>(
            self,
            writer: &mut W,
        ) -> Result<Outcome<usize>, At<PublicationError>> {
            let mut emitted = 0;
            let result = publish(&self.bytes[..self.len], &mut emitted, writer, &NoPulse);
            receipt(result, emitted, self.report)
        }
    }

    struct Released(Arc<AtomicBool>);
    impl Drop for Released {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    fn encoded(released: &Arc<AtomicBool>) -> Outcome<EncodeOutput> {
        // Deliberately oversized backend capacity.
        let mut bytes = Vec::with_capacity(4096);
        bytes.extend_from_slice(&[1, 2, 3]);
        Outcome::from_parts(
            EncodeOutput::new(bytes, ImageFormat::Png).with_extras(Released(released.clone())),
            OperationReport::default(),
        )
    }
    fn verified() -> VerifiedOutput {
        VerifiedOutput::from_verified(encoded(&Arc::new(AtomicBool::new(false))))
    }
    struct Cancel(Arc<AtomicBool>);
    impl enough::Stop for Cancel {
        fn check(&self) -> Result<(), enough::StopReason> {
            if self.0.load(Ordering::SeqCst) {
                Err(enough::StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }
    // Accepts one byte per write and records every effect.
    struct Writer {
        bytes: Vec<u8>,
        writes: usize,
        flushes: usize,
        cancelled: Arc<AtomicBool>,
        cancel_after: Option<usize>,
        cancel_flush: bool,
        fail: Option<io::ErrorKind>,
        fail_after: usize,
        fail_flush: Option<io::ErrorKind>,
        cancel_on_fail: bool,
    }
    impl Writer {
        fn new(cancelled: Arc<AtomicBool>) -> Self {
            Self {
                bytes: vec![],
                writes: 0,
                flushes: 0,
                cancelled,
                cancel_after: None,
                cancel_flush: false,
                fail: None,
                fail_after: 0,
                fail_flush: None,
                cancel_on_fail: false,
            }
        }
    }
    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            if self.bytes.len() == self.fail_after
                && let Some(kind) = self.fail.take()
            {
                if self.cancel_on_fail {
                    self.cancelled.store(true, Ordering::SeqCst);
                }
                return Err(kind.into());
            }
            self.bytes.push(bytes[0]);
            if self.cancel_after == Some(self.bytes.len()) {
                self.cancelled.store(true, Ordering::SeqCst);
            }
            Ok(1)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            if self.cancel_flush {
                self.cancelled.store(true, Ordering::SeqCst);
            }
            if let Some(kind) = self.fail_flush {
                return Err(kind.into());
            }
            Ok(())
        }
    }
    fn stopped(error: &At<PublicationError>) -> bool {
        matches!(
            zencodec::find_cause::<operation::OperationError>(error),
            Some(operation::OperationError::Stopped(
                enough::StopReason::Cancelled
            ))
        )
    }

    #[test]
    fn absent_or_supplied_noop_control_keeps_publication_bytes_and_receipts() {
        for pulse in [None, Some(&NoPulse as &dyn zencodec::Pulse)] {
            let mut writer = Writer::new(Arc::new(AtomicBool::new(false)));
            writer.fail = Some(io::ErrorKind::Interrupted);
            writer.fail_after = 1;
            let done = verified().write_to(&mut writer, pulse).unwrap();
            assert_eq!(*done.output(), 3);
            assert_eq!(writer.bytes, [1, 2, 3]);
            assert_eq!((writer.writes, writer.flushes), (4, 1));

            let mut writer = Writer::new(Arc::new(AtomicBool::new(false)));
            writer.fail = Some(io::ErrorKind::BrokenPipe);
            writer.fail_after = 1;
            let error = verified().write_to(&mut writer, pulse).unwrap_err();
            assert_eq!(error.error().emitted(), 1);
            assert_eq!(writer.bytes, [1]);
            assert_eq!(writer.flushes, 0);
            assert_eq!(
                zencodec::find_cause::<io::Error>(&error).unwrap().kind(),
                io::ErrorKind::BrokenPipe
            );
        }
    }

    #[test]
    fn cancellation_before_between_after_the_last_write_and_after_flush_keeps_effects() {
        // (cancelled at entry, cancel after byte, cancel in flush, bytes, flushes)
        for (before, after, flush, count, flushes) in [
            (true, None, false, 0, 0),
            (false, Some(1), false, 1, 0),
            (false, Some(3), false, 3, 0),
            (false, None, true, 3, 1),
        ] {
            let cancelled = Arc::new(AtomicBool::new(before));
            let pulse = zencodec::how_far::StopOnly::new(Cancel(cancelled.clone()));
            let mut writer = Writer::new(cancelled);
            writer.cancel_after = after;
            writer.cancel_flush = flush;
            let error = verified().write_to(&mut writer, Some(&pulse)).unwrap_err();
            assert_eq!(error.error().emitted(), count);
            assert_eq!(writer.bytes, [1, 2, 3][..count]);
            assert_eq!((writer.writes, writer.flushes), (count, flushes));
            assert!(stopped(&error));
        }
    }
    #[test]
    fn interrupted_is_retried_with_the_pulse_checked_again() {
        // Our existing I/O convention: Interrupted is retried, never silently.
        let mut writer = Writer::new(Arc::new(AtomicBool::new(false)));
        writer.fail = Some(io::ErrorKind::Interrupted);
        writer.fail_after = 1;
        let done = verified().write_to(&mut writer, Some(&NoPulse)).unwrap();
        assert_eq!(*done.output(), 3);
        assert_eq!(writer.bytes, [1, 2, 3]);
        assert_eq!((writer.writes, writer.flushes), (4, 1));
        // A stop raised during the interrupted attempt is seen before the retry.
        let cancelled = Arc::new(AtomicBool::new(false));
        let pulse = zencodec::how_far::StopOnly::new(Cancel(cancelled.clone()));
        let mut writer = Writer::new(cancelled);
        writer.fail = Some(io::ErrorKind::Interrupted);
        writer.fail_after = 1;
        writer.cancel_on_fail = true;
        let error = verified().write_to(&mut writer, Some(&pulse)).unwrap_err();
        assert_eq!(error.error().emitted(), 1);
        assert_eq!(writer.bytes, [1]);
        assert_eq!((writer.writes, writer.flushes), (2, 0));
        assert!(stopped(&error));
    }
    #[test]
    fn a_destination_failure_keeps_the_prefix_and_its_cause_without_retry() {
        let mut writer = Writer::new(Arc::new(AtomicBool::new(false)));
        writer.fail = Some(io::ErrorKind::BrokenPipe);
        writer.fail_after = 1;
        let error = verified()
            .write_to(&mut writer, Some(&NoPulse))
            .unwrap_err();
        assert_eq!(error.error().emitted(), 1);
        assert_eq!(writer.bytes, [1]);
        assert_eq!((writer.writes, writer.flushes), (2, 0));
        assert_eq!(
            zencodec::find_cause::<io::Error>(&error).unwrap().kind(),
            io::ErrorKind::BrokenPipe
        );
    }
    #[test]
    fn a_flush_failure_keeps_the_full_count_and_its_error_even_when_cancelled() {
        for cancel_flush in [false, true] {
            let cancelled = Arc::new(AtomicBool::new(false));
            let pulse = zencodec::how_far::StopOnly::new(Cancel(cancelled.clone()));
            let mut writer = Writer::new(cancelled);
            writer.cancel_flush = cancel_flush;
            writer.fail_flush = Some(io::ErrorKind::TimedOut);
            let error = verified().write_to(&mut writer, Some(&pulse)).unwrap_err();
            assert_eq!(error.error().emitted(), 3);
            assert_eq!(writer.bytes, [1, 2, 3]);
            assert_eq!((writer.writes, writer.flushes), (3, 1));
            assert_eq!(
                zencodec::find_cause::<io::Error>(&error).unwrap().kind(),
                io::ErrorKind::TimedOut
            );
        }
    }
    struct BrokenWriter {
        accepted: Vec<u8>,
        attempts: usize,
        result: usize,
        panic: bool,
        cancelled: Arc<AtomicBool>,
        fail_and_cancel: bool,
    }
    impl Write for BrokenWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.attempts += 1;
            if self.accepted.is_empty() {
                self.accepted.push(bytes[0]);
                return Ok(1);
            }
            if self.fail_and_cancel {
                self.cancelled.store(true, Ordering::SeqCst);
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            assert!(!self.panic, "writer unwind after accepted prefix");
            Ok(self.result)
        }
        fn flush(&mut self) -> io::Result<()> {
            panic!("failed writes must not reach flush")
        }
    }
    #[test]
    fn zero_and_invalid_counts_preserve_only_the_confirmed_prefix() {
        for (result, kind) in [
            (0, io::ErrorKind::WriteZero),
            (usize::MAX, io::ErrorKind::InvalidData),
        ] {
            let mut writer = BrokenWriter {
                accepted: Vec::new(),
                attempts: 0,
                result,
                panic: false,
                cancelled: Arc::new(AtomicBool::new(false)),
                fail_and_cancel: false,
            };
            let error = verified()
                .write_to(&mut writer, Some(&NoPulse))
                .unwrap_err();
            assert_eq!(error.error().emitted(), 1);
            assert_eq!(writer.accepted, [1]);
            assert_eq!(writer.attempts, 2);
            assert_eq!(
                zencodec::find_cause::<io::Error>(&error).unwrap().kind(),
                kind
            );
        }
    }
    #[test]
    fn native_write_error_wins_a_coincident_stop_after_the_same_prefix() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let pulse = zencodec::how_far::StopOnly::new(Cancel(cancelled.clone()));
        let mut writer = BrokenWriter {
            accepted: Vec::new(),
            attempts: 0,
            result: 0,
            panic: false,
            cancelled,
            fail_and_cancel: true,
        };
        let error = verified().write_to(&mut writer, Some(&pulse)).unwrap_err();
        assert_eq!(error.error().emitted(), 1);
        assert_eq!(writer.accepted, [1]);
        assert_eq!(writer.attempts, 2);
        assert_eq!(
            zencodec::find_cause::<io::Error>(&error).unwrap().kind(),
            io::ErrorKind::BrokenPipe
        );
        assert!(!stopped(&error));
    }
    #[test]
    fn writer_unwind_has_effects_but_returns_no_receipt() {
        let mut writer = BrokenWriter {
            accepted: Vec::new(),
            attempts: 0,
            result: 0,
            panic: true,
            cancelled: Arc::new(AtomicBool::new(false)),
            fail_and_cancel: false,
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            verified().write_to(&mut writer, Some(&NoPulse))
        }));
        assert!(result.is_err());
        assert_eq!(writer.accepted, [1]);
        assert_eq!(writer.attempts, 2);
    }
    #[test]
    fn empty_cancelled_output_does_not_flush() {
        let empty = VerifiedOutput::from_verified(Outcome::from_parts(
            EncodeOutput::new(vec![], ImageFormat::Png),
            OperationReport::default(),
        ));
        let cancelled = Arc::new(AtomicBool::new(true));
        let pulse = zencodec::how_far::StopOnly::new(Cancel(cancelled.clone()));
        let mut writer = Writer::new(cancelled);
        let error = empty.write_to(&mut writer, Some(&pulse)).unwrap_err();
        assert_eq!(error.error().emitted(), 0);
        assert_eq!((writer.writes, writer.flushes), (0, 0));
        assert!(stopped(&error));
    }
    #[test]
    fn the_owner_moves_the_encoder_vec_while_the_private_witness_copies() {
        let released = Arc::new(AtomicBool::new(false));
        let output = encoded(&released);
        let pointer = output.data().as_ptr();
        let owner = VerifiedOutput::from_verified(output);
        assert_eq!(owner.bytes.as_ptr(), pointer);
        assert!(owner.bytes.capacity() >= 4096);
        assert!(released.load(Ordering::SeqCst));
        drop(owner);
        let proof = FixedProof::<4>::from_verified(encoded(&released)).unwrap();
        let mut writer = Writer::new(Arc::new(AtomicBool::new(false)));
        proof.write_to(&mut writer).unwrap();
        assert_eq!(writer.bytes, [1, 2, 3]);
        assert_eq!(
            FixedProof::<4>::wait_bound(),
            size_of::<FixedProof<4>>() as u64
        );
        let Err(error) = FixedProof::<2>::from_verified(encoded(&released)) else {
            panic!("an over-capacity output built a proof");
        };
        assert!(matches!(
            zencodec::find_cause::<Refusal>(&error),
            Some(Refusal::OutputLimit {
                actual: 3,
                limit: 2
            })
        ));
    }
    #[cfg(all(feature = "jpeg", feature = "png", feature = "cms"))]
    #[test]
    fn the_private_fixed_no_pulse_witness_runs_the_actual_prepared_job() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zenpixels::{Cicp, PixelSlice};
        use zenpixels_convert::cms_moxcms::MoxCms;
        let input = zenjpeg::JpegEncoderConfig::new()
            .with_calibrated_quality(87.0)
            .job()
            .encoder()
            .unwrap()
            .encode(PixelSlice::new(&[23, 42, 61], 1, 1, 3, PixelDescriptor::RGB8_SRGB).unwrap())
            .unwrap()
            .into_vec();
        let codecs = CodecSet::new()
            .with_decoder(zenjpeg::JpegDecoderConfig::new())
            .with_decoder(zenpng::PngDecoderConfig::new())
            .with_encoder(zenpng::PngEncoderConfig::new());
        const CAP: usize = 65536; // Private fixture capacity, not a measured native bound.
        let request = TranscodeRequest::new(ImageFormat::Png)
            .with_cms(&MoxCms)
            .with_output(
                OutputProfile::Named(Cicp::SRGB),
                PixelFormat::Rgb8,
                ConvertOptions::retain_channels(),
            )
            .with_metadata(MetadataTransfer::DiscardDescriptive)
            .with_output_limit(CAP);
        let mut policy = PolicySet::new()
            .require_metadata(MetadataTransfer::DiscardDescriptive)
            .allow_opaque_profiles(false)
            .require_output_schedule(operation::OutputSchedule::FinishFirst)
            .with_output_wait_limit(FixedProof::<CAP>::wait_bound());
        let mut schedule =
            operation::OutputScheduleOffer::new(operation::OutputSchedule::FinishFirst);
        schedule.wait_retained_upper_bound = Some(FixedProof::<CAP>::wait_bound());
        let mut cursor = SliceCursor::new(request.io.read_size);
        let completed = request
            .execute(
                &codecs,
                &mut policy,
                Some(schedule),
                Some(&input),
                |held, pulse, _| Ok(cursor.next(&input, held, pulse)?),
            )
            .unwrap();
        let proof = FixedProof::<CAP>::from_verified(completed).unwrap();
        drop(cursor);
        drop(input);
        drop(codecs);
        drop(request);
        let mut bytes = Vec::new();
        let receipt = proof.write_to(&mut bytes).unwrap();
        assert_eq!(*receipt.output(), bytes.len());
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    }
}
