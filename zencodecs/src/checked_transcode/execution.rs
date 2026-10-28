//! The policy, conversion and output obligations shared by synchronous and async drivers.
use super::*;
use zencodec::encode::{EncodeDeclaration, EncodePlanError, InputRequirement};

pub(super) trait PreparedJob: Sized {
    fn prepare(
        request: &operation::EncodeRequest,
        codecs: &CodecSet,
    ) -> Result<Vec<Self>, At<operation::OperationError>>;
    fn offer(&self) -> operation::EncodeOffer;
    fn into_job(self) -> Box<dyn zencodec::encode::DynEncodeJob>;
}
impl PreparedJob for operation::PreparedEncode {
    fn prepare(
        request: &operation::EncodeRequest,
        codecs: &CodecSet,
    ) -> Result<Vec<Self>, At<operation::OperationError>> {
        request.prepare(codecs)
    }
    fn offer(&self) -> operation::EncodeOffer {
        self.offer()
    }
    fn into_job(self) -> Box<dyn zencodec::encode::DynEncodeJob> {
        self.into_job()
    }
}
#[cfg(feature = "checked-transcode-async")]
impl PreparedJob for operation::PreparedSendEncode {
    fn prepare(
        request: &operation::EncodeRequest,
        codecs: &CodecSet,
    ) -> Result<Vec<Self>, At<operation::OperationError>> {
        request.prepare_send(codecs)
    }
    fn offer(&self) -> operation::EncodeOffer {
        self.offer()
    }
    fn into_job(self) -> Box<dyn zencodec::encode::DynEncodeJob> {
        self.into_job()
    }
}

// Taking a phase makes every fallible transition terminal. The destination owns
// the running encoder/transform only after installation; Selected is committed
// after adoption succeeds. No alternate route can replace it afterwards.
#[allow(
    clippy::large_enum_variant,
    reason = "Keep one operation's evidence inline instead of adding an unbudgeted allocation"
)]
enum Phase<J> {
    Unprepared,
    Discovering(Vec<J>),
    Preparing,
    Selected {
        expected: ExpectedOutput,
        source: evidence::Epoch,
    },
    Verifying,
}

pub(super) struct Execution<'request, 'services, J> {
    request: &'request TranscodeRequest<'services>,
    codecs: &'request CodecSet,
    bound: Option<PolicySet>,
    schedule: Option<operation::OutputScheduleOffer>,
    input: evidence::Input<'request>,
    report: OperationReport,
    stage: Stage,
    phase: Phase<J>,
}
impl<'request, 'services, J: PreparedJob> Execution<'request, 'services, J> {
    pub(super) fn new(
        request: &'request TranscodeRequest<'services>,
        codecs: &'request CodecSet,
        bound: Option<PolicySet>,
        schedule: Option<operation::OutputScheduleOffer>,
        input: Option<&'request [u8]>,
    ) -> Self {
        Self {
            request,
            codecs,
            bound,
            schedule,
            input: evidence::Input::new(input, request.metadata, request.storage.clone()),
            report: OperationReport::default(),
            stage: Stage::Prepare,
            phase: Phase::Unprepared,
        }
    }
    pub(super) fn prepare(&mut self) -> Result<Box<zencodec::decode::DynControlledDecoder>, Cause> {
        self.request.preset.check_workspace(None)?;
        if let Some(bound) = self.bound {
            bound.check_workspace(None)?;
        }
        if !matches!(self.phase, Phase::Unprepared) {
            return Err(Refusal::RepeatedInterpretation.into());
        }
        if self.request.strip_rows == 0 {
            return Err(Refusal::InvalidStripRows.into());
        }
        if self.codecs.decoder_for(self.request.format).is_none() {
            return Err(zencodec::CodecSetError::NoDecoder(self.request.format).into());
        }
        self.phase = Phase::Discovering(J::prepare(&self.request.encode, self.codecs)?);
        Ok(self
            .request
            .decode
            .start_controlled(self.codecs, self.request.storage.clone())?)
    }
    pub(super) fn input_mut(&mut self) -> &mut evidence::Input<'request> {
        &mut self.input
    }
    pub(super) fn before_step(&mut self) {
        self.stage = Stage::Decode;
    }
    pub(super) fn step_error(&mut self, error: &Cause) {
        if matches!(
            zencodec::find_cause::<zencodec::decode::StepError<zencodec::decode::ErasedError>>(
                error.as_ref()
            ),
            Some(zencodec::decode::StepError::Sink(_))
        ) {
            self.stage = Stage::Encode;
        }
    }
    pub(super) fn advance<R: OperationPolicy + ?Sized>(
        &mut self,
        held: &mut Held<'_>,
        event: Yield<ControlledEvent>,
        resolver: &mut R,
    ) -> Result<bool, Cause> {
        match event {
            Yield::Output(ControlledEvent::Suspended(boundary)) => {
                if boundary.facts().state != InterpretationState::Final {
                    if !matches!(self.phase, Phase::Discovering(_)) {
                        return Err(Refusal::RepeatedInterpretation.into());
                    }
                    self.stage = Stage::Policy;
                    let (action, record) = operation::resolve_boundary(
                        boundary,
                        resolver,
                        self.request.preset,
                        self.codecs.pulse(),
                    )
                    .inspect_err(|e| {
                        self.report.record(e.error().record());
                    })?;
                    self.report.record(record);
                    held.resume(boundary.id(), action, self.codecs.pulse())?;
                    return Ok(false);
                }
                let Phase::Discovering(prepared) =
                    core::mem::replace(&mut self.phase, Phase::Preparing)
                else {
                    return Err(Refusal::RepeatedInterpretation.into());
                };
                self.stage = Stage::Plan;
                let info = held.info().ok_or(Refusal::MissingImageInfo)?;
                require_primary(info)?;
                // A final pixel boundary does not qualify metadata completeness.
                // Inspect the same immutable slice, never re-read a range source.
                let source = evidence::qualify(
                    self.codecs,
                    self.input.complete(),
                    info,
                    self.request.metadata,
                )?;
                self.input.release_capture();
                let descriptor = boundary
                    .facts()
                    .descriptor
                    .ok_or(Refusal::MissingDescriptor)?;
                if matches!(
                    descriptor.transfer(),
                    zenpixels::TransferFunction::Pq | zenpixels::TransferFunction::Hlg
                ) || info
                    .source_color
                    .cicp
                    .is_some_and(|c| matches!(c.transfer_characteristics, 16 | 18))
                {
                    return Err(Refusal::HdrRendering.into());
                }
                let current = Arc::new(info.source_color.to_color_context());
                let origin = origin(info);
                let (profile, output_format, options) = self.request.output.clone().unwrap_or((
                    OutputProfile::SameAsOrigin,
                    descriptor.pixel_format(),
                    ConvertOptions::retain_channels(),
                ));
                let mut conversion = OutputPlan::new(
                    descriptor,
                    Some(current.clone()),
                    &origin,
                    profile,
                    output_format,
                    &options,
                    self.request.cms,
                )?;
                conversion.prepare(info.width)?;
                let output_descriptor = conversion.output_descriptor();
                let output_color = sink::output_context(&conversion, &current);
                let declaration =
                    EncodeDeclaration::new(info.width, info.height, output_descriptor)
                        .with_color(&output_color);
                if !output_descriptor.channel_type().is_integer() {
                    return Err(Refusal::UnqualifiedSampleType.into());
                }
                if requires_alpha(output_descriptor)
                    && !self
                        .codecs
                        .encoder_for(self.request.format)
                        .ok_or(zencodec::CodecSetError::NoEncoder(self.request.format))?
                        .capabilities()
                        .native_alpha()
                {
                    return Err(Refusal::AlphaRetention.into());
                }
                let metadata = output_metadata(
                    info,
                    &conversion,
                    self.request.metadata,
                    self.request.format,
                )?;
                let resolution = if self.request.metadata == MetadataTransfer::PreserveReported {
                    info.resolution
                } else {
                    None
                };
                let mut jobs = Vec::new();
                let mut rejections = Vec::new();
                jobs.try_reserve_exact(prepared.len())?;
                rejections.try_reserve_exact(prepared.len())?;
                for prepared in prepared {
                    let mut offer = prepared.offer();
                    let mut job = prepared.into_job();
                    // Retention is an explicit request choice. Bind the
                    // actual carriers before the joint resolver runs.
                    #[allow(deprecated)]
                    job.set_metadata(metadata.clone());
                    if let Some(resolution) = resolution {
                        if let Err(cause) = job.try_set_resolution(resolution) {
                            rejections.push(operation::RouteRejection {
                                preference_index: offer.preference_index,
                                fidelity: offer.fidelity,
                                cause: zencodec::CodecSetError::Codec(cause),
                            });
                            continue;
                        }
                        offer.resolution = Some(resolution);
                    }
                    // Plan the retained job against the declared rows before
                    // the joint question: a route that cannot accept one
                    // ordered row pass, or refuses this color, is excluded
                    // here with its native cause instead of after selection.
                    let plan = match job.plan_encode(&declaration) {
                        Ok(plan) => plan,
                        Err(cause) => {
                            rejections.push(operation::RouteRejection {
                                preference_index: offer.preference_index,
                                fidelity: offer.fidelity,
                                cause: zencodec::CodecSetError::Codec(cause),
                            });
                            continue;
                        }
                    };
                    if let Err(cause) = plan.validate(&declaration).and_then(|()| {
                        if plan.input == InputRequirement::Rows {
                            Ok(())
                        } else {
                            Err(EncodePlanError::InputProtocol)
                        }
                    }) {
                        rejections.push(operation::RouteRejection {
                            preference_index: offer.preference_index,
                            fidelity: offer.fidelity,
                            cause: zencodec::CodecSetError::Codec(Box::new(cause)),
                        });
                        continue;
                    }
                    jobs.push((offer, job, plan));
                }
                if jobs.is_empty() {
                    return Err(
                        At::wrap(operation::OperationError::NoEncodeRoute(rejections))
                            .at_named("prepare transcode density")
                            .into(),
                    );
                }
                // An opaque copy may carry a second, contradictory density.
                // Neither legacy EXIF inspection nor carrier byte equality
                // proves reconciliation. No offered route claims that scope.
                if resolution.is_some() && (metadata.exif.is_some() || metadata.xmp.is_some()) {
                    return Err(Refusal::ResolutionMetadata.into());
                }
                let rows = PreparedRows::new(
                    info,
                    descriptor,
                    current,
                    conversion,
                    self.request.strip_rows,
                    &self.request.storage,
                )?;
                let mut offers = Vec::new();
                offers.try_reserve_exact(jobs.len())?;
                for job in &jobs {
                    let mut offer =
                        TranscodeOffer::new(job.0, rows.output_descriptor(), self.request.metadata);
                    // Accept both pinned bool and review Option<bool> evidence;
                    // the latter must be explicitly known, never left unknown.
                    #[allow(clippy::useless_conversion)]
                    let retained = (metadata.icc_profile.is_some()
                        && metadata.icc_profile == info.source_color.icc_profile)
                        .into();
                    offer.retains_opaque_profile = retained;
                    // These buffers are shared by all fidelity alternatives;
                    // each route retains this same allocation after selection.
                    offer.admitted_bytes = rows.admitted_bytes();
                    offer.output_schedule = self.schedule;
                    offers.push(offer);
                }
                self.stage = Stage::Policy;
                let (selected, record) = operation::resolve_transcode(
                    boundary,
                    &offers,
                    resolver,
                    self.request.preset,
                    self.codecs.pulse(),
                )
                .inspect_err(|e| {
                    self.report.record(e.error().record());
                })?;
                self.report.record(record);
                let (_, job, plan) = jobs.swap_remove(selected);
                drop(jobs);
                self.stage = Stage::Encode;
                let expectation = ExpectedOutput {
                    metadata,
                    width: info.width,
                    height: info.height,
                    descriptor: output_descriptor,
                    resolution,
                };
                let encoder = job.into_encoder()?;
                held.sink_mut()
                    .install(rows, encoder, Arc::new(output_color.clone()), plan)?;
                held.resume(
                    boundary.id(),
                    BoundaryAction::AdoptInterpretation,
                    self.codecs.pulse(),
                )?;
                self.phase = Phase::Selected {
                    expected: expectation,
                    source,
                };
            }
            Yield::Finished(ControlledEvent::Decode(Event::Done(_))) => {
                self.revalidate(held)?;
                return Ok(true);
            }
            Yield::Output(ControlledEvent::Decode(Event::Metadata(_))) => {
                if matches!(self.phase, Phase::Selected { .. }) {
                    self.revalidate(held)?;
                }
            }
            Yield::Output(ControlledEvent::Decode(_)) => {}
            _ => return Err(Refusal::UnexpectedProgress.into()),
        }
        Ok(false)
    }
    fn revalidate(&self, held: &Held<'_>) -> Result<(), Cause> {
        let Phase::Selected { source, .. } = &self.phase else {
            return Err(Refusal::MissingRoute.into());
        };
        source.check(held.info().ok_or(Refusal::MissingImageInfo)?)
    }
    pub(super) fn finish(&mut self, destination: Destination) -> Result<EncodeOutput, Cause> {
        let Phase::Selected { expected, .. } =
            core::mem::replace(&mut self.phase, Phase::Verifying)
        else {
            return Err(Refusal::MissingRoute.into());
        };
        self.stage = Stage::Encode;
        let encoded = destination.finish_encoder()?;
        self.stage = Stage::Verify;
        self.codecs
            .pulse()
            .check()
            .map_err(operation::OperationError::Stopped)?;
        if encoded.data().len() > self.request.output_limit {
            return Err(Refusal::OutputLimit {
                actual: encoded.data().len(),
                limit: self.request.output_limit,
            }
            .into());
        }
        let inspector = self.codecs.decode_job(self.request.format)?;
        let actual = inspector.probe_full(encoded.data())?;
        let decoded = inspector.output_info(encoded.data())?;
        evidence::qualify(
            self.codecs,
            Some(encoded.data()),
            &actual,
            self.request.metadata,
        )?;
        verify_output(&actual, &decoded, &expected)?;
        self.codecs
            .pulse()
            .check()
            .map_err(operation::OperationError::Stopped)?;
        Ok(encoded)
    }
    pub(super) fn outcome(
        self,
        result: Result<EncodeOutput, Cause>,
    ) -> Result<Outcome<EncodeOutput>, At<Error>> {
        match result {
            Ok(output) => Ok(Outcome::from_parts(output, self.report)),
            Err(cause) => Err(At::wrap(Error {
                stage: self.stage,
                report: Box::new(self.report),
                cause,
            })
            .at_named("checked pixel transcode")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use zencodec::decode::*;
    use zencodec::operation::{
        BoundPolicy, Decision, PolicyRequirement, Question, RequirementEvidence, UnmetRequirements,
    };

    static PREPARATIONS: AtomicUsize = AtomicUsize::new(0);
    struct RecordingJob;
    impl PreparedJob for RecordingJob {
        fn prepare(
            _: &operation::EncodeRequest,
            _: &CodecSet,
        ) -> Result<Vec<Self>, At<operation::OperationError>> {
            PREPARATIONS.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }
        fn offer(&self) -> operation::EncodeOffer {
            unreachable!("workspace refuses before offers")
        }
        fn into_job(self) -> Box<dyn zencodec::encode::DynEncodeJob> {
            unreachable!("workspace refuses before jobs")
        }
    }
    #[test]
    fn both_workspace_snapshots_refuse_before_native_preparation() {
        PREPARATIONS.store(0, Ordering::SeqCst);
        for legacy in [false, true] {
            for cap in [0, 1, u64::MAX] {
                let mut request = TranscodeRequest::new(ImageFormat::Png);
                let required = PolicySet::new().with_workspace_limit(cap);
                if legacy {
                    request.preset = required;
                }
                let codecs = CodecSet::new();
                let mut execution = Execution::<RecordingJob>::new(
                    &request,
                    &codecs,
                    Some(if legacy { PolicySet::new() } else { required }),
                    None,
                    None,
                );
                let result = execution.prepare();
                let cause = match result {
                    Err(cause) => cause,
                    Ok(_) => panic!("unknown workspace admitted"),
                };
                assert_eq!(
                    zencodec::find_cause::<UnmetRequirements>(cause.as_ref())
                        .unwrap()
                        .iter()
                        .collect::<Vec<_>>(),
                    [(PolicyRequirement::Workspace, RequirementEvidence::Unknown)]
                );
                assert_eq!(execution.stage, Stage::Prepare);
                assert_eq!(execution.report.decisions().count(), 0);
                assert_eq!(PREPARATIONS.load(Ordering::SeqCst), 0);
            }
        }
    }

    struct EvidenceDecoder(ImageInfo);
    impl IncrementalDecode for EvidenceDecoder {
        type Error = ErasedError;
        fn step(
            &mut self,
            _: Input<'_>,
            _: &mut dyn DecodeRowSink,
            _: &dyn zencodec::Pulse,
        ) -> Result<Progress, StepError<Self::Error>> {
            unreachable!("controlled path only")
        }
        fn requested(&self) -> &[InputRequest] {
            &[]
        }
        fn info(&self) -> Option<&ImageInfo> {
            Some(&self.0)
        }
    }
    impl ControlledIncrementalDecode for EvidenceDecoder {
        fn step_controlled(
            &mut self,
            _: Input<'_>,
            _: &mut dyn DecodeRowSink,
            _: &dyn zencodec::Pulse,
        ) -> Result<ControlledProgress, StepError<Self::Error>> {
            unreachable!("the test supplies the event being handled")
        }
        fn resume(
            &mut self,
            _: BoundaryId,
            _: BoundaryAction,
            _: &dyn zencodec::Pulse,
        ) -> Result<(), StepError<Self::Error>> {
            unreachable!()
        }
    }
    #[test]
    fn selected_route_rejects_late_metadata_and_unannounced_done_drift() {
        let mut original = ImageInfo::new(1, 1, ImageFormat::Png);
        original.gain_map = zencodec::GainMapPresence::Absent;
        let changes: &[fn(&mut ImageInfo)] = &[
            |i| i.source_color.icc_profile = Some(Arc::from(&b"late ICC"[..])),
            |i| i.embedded_metadata.exif = Some(Arc::from(&b"late EXIF"[..])),
            |i| i.embedded_metadata.xmp = Some(Arc::from(&b"late XMP"[..])),
            |i| i.orientation = zencodec::Orientation::Rotate90,
            |i| {
                i.resolution = Some(zencodec::Resolution {
                    x: 72.0,
                    y: 72.0,
                    unit: zencodec::ResolutionUnit::Inch,
                })
            },
        ];
        for change in changes {
            for done in [false, true] {
                let mut changed = original.clone();
                change(&mut changed);
                let source = evidence::qualify(
                    &CodecSet::new(),
                    None,
                    &original,
                    MetadataTransfer::DiscardDescriptive,
                )
                .unwrap();
                let request = TranscodeRequest::new(ImageFormat::Png);
                let codecs = CodecSet::new();
                let mut execution =
                    Execution::<RecordingJob>::new(&request, &codecs, None, None, None);
                execution.phase = Phase::Selected {
                    source,
                    expected: ExpectedOutput {
                        metadata: Metadata::none(),
                        width: 1,
                        height: 1,
                        descriptor: PixelDescriptor::RGB8_SRGB,
                        resolution: None,
                    },
                };
                let mut destination = Destination::default();
                let decoder: Box<DynControlledDecoder> = Box::new(EvidenceDecoder(changed));
                let mut held = ControlledStepper::new(decoder, &mut destination);
                let event = if done {
                    Yield::Finished(ControlledEvent::Decode(Event::Done(
                        OutputInfo::full_decode(1, 1, PixelDescriptor::RGB8_SRGB),
                    )))
                } else {
                    Yield::Output(ControlledEvent::Decode(Event::Metadata(
                        MetadataUpdate::HeaderReady,
                    )))
                };
                let mut policy = BoundPolicy::new(
                    PolicySet::new(),
                    |_: Question<'_>| -> Result<Decision, At<core::convert::Infallible>> {
                        panic!("late evidence cannot create another decision")
                    },
                );
                let error = execution
                    .advance(&mut held, event, &mut policy)
                    .unwrap_err();
                assert_eq!(
                    zencodec::find_cause::<Refusal>(error.as_ref()),
                    Some(&Refusal::LateMetadata)
                );
            }
        }
    }
}
