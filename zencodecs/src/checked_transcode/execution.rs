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

pub(super) struct Execution<'request, 'services, J> {
    request: &'request TranscodeRequest<'services>,
    codecs: &'request CodecSet,
    report: OperationReport,
    stage: Stage,
    prepared_jobs: Option<Vec<J>>,
    expected: Option<ExpectedOutput>,
    final_seen: bool,
}
impl<'request, 'services, J: PreparedJob> Execution<'request, 'services, J> {
    pub(super) fn new(
        request: &'request TranscodeRequest<'services>,
        codecs: &'request CodecSet,
    ) -> Self {
        Self {
            request,
            codecs,
            report: OperationReport::default(),
            stage: Stage::Prepare,
            prepared_jobs: None,
            expected: None,
            final_seen: false,
        }
    }
    pub(super) fn prepare(&mut self) -> Result<Box<zencodec::decode::DynControlledDecoder>, Cause> {
        if self.request.strip_rows == 0 {
            return Err(Refusal::InvalidStripRows.into());
        }
        if self.codecs.decoder_for(self.request.format).is_none() {
            return Err(zencodec::CodecSetError::NoDecoder(self.request.format).into());
        }
        self.prepared_jobs = Some(J::prepare(&self.request.encode, self.codecs)?);
        Ok(self
            .request
            .decode
            .start_controlled(self.codecs, self.request.storage.clone())?)
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
                if self.final_seen {
                    return Err(Refusal::RepeatedInterpretation.into());
                }
                self.final_seen = true;
                self.stage = Stage::Plan;
                let info = held.info().ok_or(Refusal::MissingImageInfo)?;
                require_primary(info)?;
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
                    ConvertOptions::forbid_lossy(),
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
                let prepared = self
                    .prepared_jobs
                    .take()
                    .ok_or(Refusal::RepeatedInterpretation)?;
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
                self.expected = Some(expectation);
                held.resume(
                    boundary.id(),
                    BoundaryAction::AdoptInterpretation,
                    self.codecs.pulse(),
                )?;
            }
            Yield::Finished(ControlledEvent::Decode(Event::Done(_))) => return Ok(true),
            Yield::Output(ControlledEvent::Decode(_)) => {}
            _ => return Err(Refusal::UnexpectedProgress.into()),
        }
        Ok(false)
    }
    pub(super) fn finish(&mut self, destination: Destination) -> Result<EncodeOutput, Cause> {
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
        let expected = self.expected.take().ok_or(Refusal::MissingRoute)?;
        let inspector = self.codecs.decode_job(self.request.format)?;
        let actual = inspector.probe_full(encoded.data())?;
        let decoded = inspector.output_info(encoded.data())?;
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
