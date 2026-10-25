//! Async byte demand uses the same execution state and postconditions.
use super::*;
use zencodec_io::tokio_io::{AsyncCursor, AsyncSource};

impl TranscodeRequest<'_> {
    /// Transcode from asynchronous byte demand using the configured policy.
    ///
    /// Encoders must be registered with `CodecSet::with_send_encoder`; refusal
    /// precedes source I/O when a job lacks transferable registration.
    /// Source I/O is awaited; codec and conversion steps run on the polling task.
    /// `DriveOptions::work_quota` yields between complete steps, and cannot
    /// preempt a native step. CPU-heavy configurations belong on a worker runtime.
    /// No encoded bytes are returned before the shared output checks complete.
    pub async fn transcode_async<S: AsyncSource + ?Sized>(
        &self,
        codecs: &CodecSet,
        source: &mut S,
    ) -> Result<Outcome<EncodeOutput>, At<Error>> {
        let mut resolver = self.preset;
        self.transcode_async_with(codecs, source, &mut resolver)
            .await
    }

    /// Asynchronous source I/O with the same fallible interpretation/route resolver.
    /// Dropping this future releases its managed source and row owners. Source,
    /// conversion, codec and callback errors retain their native traced causes.
    pub async fn transcode_async_with<S, R>(
        &self,
        codecs: &CodecSet,
        source: &mut S,
        resolver: &mut R,
    ) -> Result<Outcome<EncodeOutput>, At<Error>>
    where
        S: AsyncSource + ?Sized,
        R: PolicyResolver + ?Sized,
    {
        let mut execution =
            execution::Execution::<operation::PreparedSendEncode>::new(self, codecs);
        let result = async {
            let decoder = execution.prepare()?;
            let mut destination = Destination::default();
            let mut held = ControlledStepper::new(decoder, &mut destination);
            let mut cursor = AsyncCursor::new(self.io);
            loop {
                execution.before_step();
                let event = cursor
                    .next(source, &mut held, codecs.stop())
                    .await
                    .map_err(|error| -> Cause { Box::new(DriverError(error)) })
                    .inspect_err(|error| execution.step_error(error))?;
                if execution.advance(&mut held, event, resolver)? {
                    break;
                }
            }
            drop(held);
            execution.finish(destination)
        }
        .await;
        execution.outcome(result)
    }
}

// The source boundary intentionally carries arbitrary boxed native errors.
// Keep the complete driver failure and expose its underlying native cause;
// Box<dyn Error> does not satisfy DriveFailure's generic Error implementation.
#[derive(Debug)]
struct DriverError(
    zencodec_io::DriveFailure<zencodec::decode::StepError<zencodec::decode::ErasedError>, Cause>,
);
impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl core::error::Error for DriverError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        use zencodec_io::DriveFailure;
        match &self.0 {
            DriveFailure::Source(error) => Some(error.as_ref()),
            DriveFailure::Step(error) => Some(error),
            DriveFailure::Protocol(error) => Some(error),
            DriveFailure::Input(error) => Some(error),
            _ => None,
        }
    }
}
