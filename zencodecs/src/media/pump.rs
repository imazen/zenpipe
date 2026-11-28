//! Sans-I/O demand-driven media pump (`MediaPump` / `PreparedMediaTranscode`),
//! poster extraction, video transcode, and animation ↔ video bridge over one
//! [`Engine`](crate::engine::Engine) + [`JobResources`](zencodec::JobResources) chain.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use whereat::{At, at};
use zencodec::decode::{BoxedError, Input, InputRequest};
use zencodec::encode::Fidelity;
use zencodec::plane::{AnyPlane, PlanarLayout, Plane, PlaneEncoding, PlaneRole, PlaneSpec};
use zencodec::source::SourceAccess;
use zencodec::time::{FrameDuration, TimeBase, Timestamp};
use zencodec_media::adapt::{CaptureFrame, RenderFrame};
use zencodec_media::container::{
    DemuxEvent, DynDemuxer, DynMuxer, MuxFinish, MuxHint, PacketWrite,
};
use zencodec_media::frame::{
    ColorClaim, Frame, FrameGeometry, FrameInterpretation, FrameTiming, OwnedPlane, OwnedPlanes,
};
use zencodec_media::packet::Packet;
use zencodec_media::registry::{
    ContainerId, DemuxNeeds, LocalVideoDecoder, LocalVideoEncoder,
    TimingPolicy as MediaTimingPolicy, VideoDecodeNeeds, VideoEncodeNeeds,
};
use zencodec_media::session::{CodecConfig, Receive, SendStatus};
use zencodec_media::timeline::{TimingEvidence, TrimTreatment};
use zencodec_media::track::{CodecId as MediaCodecId, TrackInfo, TrackKind, VideoTrackInfo};
use zencodec_render::media::{VideoCapturePlan, VideoRenderPlan};
use zenpixels::sample::{ChromaSiting, ChromaSubsampling, SampleEncoding};
use zenpixels::{Cicp, PixelBuf, PixelFormat, PixelView, PixelViewMut, PlaneFormat, SampleType};
use zenpixels_convert::PlanarReconstructionOptions;

use crate::engine::{
    Coding, Completion, EffectiveRun, Encoded, Engine, EngineError, EngineResult, Failure,
    LoopCount, LoopPolicy, Metric, MetricTarget, OutputIntent, PublicationMode, QualityLevel,
    RouteKind, RouteReport, Run, SequencePolicy, Stage, TargetFormat, TimingPolicy,
};
use crate::media::publication::{PublicationBuffer, PublishedMedia, VerifiedCheck};
use crate::media::route::{
    select_demuxer, select_muxer, select_video_decoder, select_video_encoder,
    validate_container_codec_tuple, validate_track_policy,
};
use crate::media::select::{
    DecodeSelection, SeekMode, cmp_timestamp_exact, is_strictly_closer_to_target,
};
use crate::media::timeline::{
    MappedPresentation, TimelineBridge, normalize_loop_for_target, quantize_endpoints,
};

static NEXT_PUMP_ID: AtomicUsize = AtomicUsize::new(1);

/// Opaque pump-scoped ticket tying a [`PumpEvent::NeedInput`] or [`PumpEvent::NeedWriter`]
/// request to its matching [`MediaPump::supply_input`] or [`MediaPump::confirm_written`] reply.
///
/// Tickets encode both the originating [`MediaPump`] identity and a monotonic non-wrapping
/// sequence number so tickets from concurrent pumps never alias.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IoTicket(u64);

impl IoTicket {
    /// Inspect numeric ticket value (for diagnostics or FFI wire encoding).
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Reconstruct a ticket from a raw FFI wire value.
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

/// Output write offer emitted by [`MediaPump::advance`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteOffer<'a> {
    /// Append bytes sequentially at the current output end.
    Append(&'a [u8]),
    /// Patch bytes at an earlier seekable byte offset (only legal for seekable destinations).
    Patch {
        /// Byte offset from start of output stream.
        offset: u64,
        /// Exact replacement bytes.
        bytes: &'a [u8],
    },
}

/// Single step outcome from [`MediaPump::advance`].
#[derive(Debug)]
pub enum PumpEvent<'a> {
    /// Pump requires input bytes satisfying one of `requests` via [`MediaPump::supply_input`].
    NeedInput {
        /// Ticket that must be echoed back to `supply_input`.
        ticket: IoTicket,
        /// Byte span alternatives requested by the demuxer.
        requests: &'a [InputRequest],
    },
    /// Pump has encoded container bytes ready to write via [`MediaPump::confirm_written`].
    NeedWriter {
        /// Ticket that must be echoed back to `confirm_written`.
        ticket: IoTicket,
        /// Append chunk or seek-back patch offer.
        write: WriteOffer<'a>,
    },
    /// One frame/packet made measurable internal progress (caller may continue advancing).
    Presented,
    /// Pipeline drained cleanly and all container bytes/patches have been confirmed written.
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingTicket {
    None,
    Input(IoTicket),
    WriteMuxAppend {
        ticket: IoTicket,
        offered_len: usize,
    },
    WriteTailAppend {
        ticket: IoTicket,
        offered_len: usize,
    },
    WriteVerifiedAppend {
        ticket: IoTicket,
        offered_len: usize,
    },
    WritePatch {
        ticket: IoTicket,
        offered_len: usize,
    },
}

/// Type alias matching the canonical architecture name.
pub type PreparedMediaTranscode = MediaPump;

/// Sans-I/O demand-driven media pump orchestrating `Demux -> VideoDecode -> Timeline ->
/// VideoEncode -> BoundedMux` with pump-scoped non-wrapping tickets, exact `Range` request
/// preservation, downstream-first backpressure, staged-before-emit `Verified` publication,
/// and terminal fault fusing (zero extra native calls after any fault or stop).
pub struct MediaPump {
    pump_id: u32,
    engine: Engine,
    effective: EffectiveRun,
    intent: OutputIntent,
    target_container: ContainerId,
    target_codec: MediaCodecId,
    demuxer: Box<dyn DynDemuxer>,
    video_track_idx: Option<usize>,
    decoder: Option<Box<LocalVideoDecoder>>,
    timeline: Option<TimelineBridge>,
    encoder: Option<Box<LocalVideoEncoder>>,
    muxer: Option<Box<dyn DynMuxer>>,
    mux_begun: bool,
    pending_packet_for_dec: Option<Packet>,
    pending_frames_for_enc: VecDeque<Frame>,
    frames_accepted: u64,
    pending_packet_for_mux: Option<Packet>,
    pending_tail: Vec<u8>,
    pending_tail_offset: usize,
    pending_patches: VecDeque<(u64, Vec<u8>)>,
    /// Private staging buffer used exclusively under [`PublicationMode::Verified`].
    /// Zero bytes are emitted via [`PumpEvent::NeedWriter`] until mux finish, patches,
    /// and complete container structural readback succeed.
    staged_pub: Option<PublicationBuffer>,
    /// Verified complete output ready to stream out via [`PumpEvent::NeedWriter`], together with
    /// its live [`StorageBudget`] reservation held until writer acknowledgement or handoff.
    verified_payload: Option<Vec<u8>>,
    verified_hold: Option<zencodec::storage::PreparedStorage<u8>>,
    verified_offset: usize,
    verified_check: Option<VerifiedCheck>,
    demux_done: bool,
    dec_ended: bool,
    dec_done: bool,
    enc_ended: bool,
    enc_done: bool,
    mux_finished: bool,
    input_buf: Vec<u8>,
    input_offset: u64,
    input_total_len: Option<u64>,
    pending_requests: Vec<InputRequest>,
    stalled_at_eof: bool,
    next_seq: u32,
    pending_ticket: PendingTicket,
    committed_bytes: u64,
    mux_committed_clock: Option<TimeBase>,
    pending_unknown_end: bool,
    faulted: bool,
    report: RouteReport,
}

impl MediaPump {
    /// Prepare a sans-I/O [`MediaPump`] for a container-to-container video transcode.
    ///
    /// Extracts the destination container and video codec directly from `intent.target`
    /// (`TargetFormat::Video { container, codec }`) as the single source of truth, and
    /// validates coding, loop policy, routing, and demuxer workspace upfront.
    pub fn prepare_transcode(
        engine: Engine,
        source_container: ContainerId,
        source_access: &SourceAccess,
        intent: OutputIntent,
        run: &Run<'_>,
    ) -> EngineResult<Self> {
        intent.validate()?;
        let TargetFormat::Video {
            container: target_container,
            codec: target_codec,
        } = intent.target.clone()
        else {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::InvalidRequest,
                "MediaPump::prepare_transcode requires TargetFormat::Video in OutputIntent",
            )));
        };
        validate_container_codec_tuple(&target_container, &target_codec)?;
        validate_video_coding(&intent.coding)?;

        // Validate explicit loop directives upfront so repeating loop targets refuse cleanly.
        let loop_policy = match intent.sequence {
            SequencePolicy::Preserve { loops, .. } => loops,
            SequencePolicy::FirstPresentation => LoopPolicy::Set(LoopCount::once()),
        };
        let _ = normalize_loop_for_target(LoopCount::once(), loop_policy, &target_container)?;

        let mut eff = engine.resolve_run(run)?;
        eff.check_stop(Stage::Prepare)?;
        if let Some(known) = source_access.known_len
            && let Some(max_in) = eff.resources.limits().max_input_bytes
            && known > max_in
        {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::ResourceLimit,
                "media source length exceeds max_input_bytes",
            )));
        }
        if !eff.routing.permits_output_container(&target_container) {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::PolicyRefusal,
                "target output container is denied by Routing",
            )));
        }

        let mut report = RouteReport {
            route: Some(RouteKind::Media),
            output_container: Some(target_container.clone()),
            output_video_codec: Some(target_codec.clone()),
            ..Default::default()
        };

        let (demux_id, demux_factory) =
            select_demuxer(&engine, &source_container, &mut eff, &mut report)?;
        let demux_offer = demux_factory
            .quote(&DemuxNeeds::default(), source_access, &eff.resources)
            .map_err(|e| {
                at!(EngineError::simple(
                    Stage::Prepare,
                    Failure::UnsupportedEffect,
                    "demuxer quote failed",
                )
                .with_cause(e))
            })?;
        eff.admit_workspace(demux_id.as_str(), demux_offer.workspace, &mut report)?;
        let demuxer = demux_factory
            .open(&demux_offer, &eff.resources)
            .map_err(|e| {
                at!(EngineError::simple(
                    Stage::Prepare,
                    Failure::BackendFailure,
                    "demuxer open failed",
                )
                .with_cause(e))
            })?;

        let max_out_cap = {
            let out_lim = eff
                .resources
                .limits()
                .max_output_bytes
                .and_then(|v| usize::try_from(v).ok());
            let mem_lim = eff
                .resources
                .limits()
                .max_memory_bytes
                .and_then(|v| usize::try_from(v).ok());
            match (out_lim, mem_lim) {
                (Some(a), Some(b)) => a.min(b),
                (Some(a), None) | (None, Some(a)) => a,
                (None, None) => 64 * 1024 * 1024,
            }
        };
        let staged_pub = if intent.publication == PublicationMode::Verified {
            Some(PublicationBuffer::new(
                PublicationMode::Verified,
                max_out_cap,
                eff.resources.storage().clone(),
            )?)
        } else {
            None
        };

        let pump_id = (NEXT_PUMP_ID.fetch_add(1, Ordering::Relaxed) as u32).max(1);

        Ok(Self {
            pump_id,
            engine,
            effective: eff,
            intent,
            target_container,
            target_codec,
            demuxer,
            video_track_idx: None,
            decoder: None,
            timeline: None,
            encoder: None,
            muxer: None,
            mux_begun: false,
            pending_packet_for_dec: None,
            pending_frames_for_enc: VecDeque::new(),
            frames_accepted: 0,
            pending_packet_for_mux: None,
            pending_tail: Vec::new(),
            pending_tail_offset: 0,
            pending_patches: VecDeque::new(),
            staged_pub,
            verified_payload: None,
            verified_hold: None,
            verified_offset: 0,
            verified_check: None,
            demux_done: false,
            dec_ended: false,
            dec_done: false,
            enc_ended: false,
            enc_done: false,
            mux_finished: false,
            input_buf: Vec::new(),
            input_offset: 0,
            input_total_len: source_access.known_len,
            pending_requests: Vec::new(),
            stalled_at_eof: false,
            next_seq: 1,
            pending_ticket: PendingTicket::None,
            committed_bytes: 0,
            mux_committed_clock: None,
            pending_unknown_end: false,
            faulted: false,
            report,
        })
    }

    /// Borrow the current route report.
    #[must_use]
    pub const fn report(&self) -> &RouteReport {
        &self.report
    }

    /// Configured publication mode for this pump.
    #[must_use]
    pub const fn publication_mode(&self) -> PublicationMode {
        self.intent.publication
    }

    /// Target output container for this pump.
    #[must_use]
    pub const fn target_container(&self) -> &ContainerId {
        &self.target_container
    }

    /// Verification receipt populated only after [`PublicationMode::Verified`] finishes
    /// full structural readback verification (`Headers` -> all `Packet`s -> `Done`).
    #[must_use]
    pub const fn verified_receipt(&self) -> Option<&VerifiedCheck> {
        self.verified_check.as_ref()
    }

    /// Bytes confirmed written externally so far (guaranteed `0` prior to verification
    /// completing under [`PublicationMode::Verified`]).
    #[must_use]
    pub const fn committed_bytes(&self) -> u64 {
        self.committed_bytes
    }

    /// Record an external transport error, fusing the pump so zero further native calls run.
    pub fn fail_transport(&mut self, cause: BoxedError) -> EngineError {
        self.faulted = true;
        self.pending_ticket = PendingTicket::None;
        self.verified_payload = None;
        self.verified_hold = None;
        EngineError::simple(Stage::Execute, Failure::Io, "media transport failed")
            .with_boxed_cause(cause)
            .with_committed_bytes(self.committed_bytes)
            .with_diagnostics(self.report.clone(), self.effective.trace.clone())
    }

    fn alloc_ticket(&mut self) -> EngineResult<IoTicket> {
        let seq = self.next_seq;
        self.next_seq = seq.checked_add(1).ok_or_else(|| {
            at!(EngineError::simple(
                Stage::Execute,
                Failure::ResourceLimit,
                "MediaPump IoTicket sequence exhausted",
            ))
        })?;
        Ok(IoTicket((u64::from(self.pump_id) << 32) | u64::from(seq)))
    }

    fn fail(&mut self, err: At<EngineError>) -> At<EngineError> {
        self.faulted = true;
        self.pending_ticket = PendingTicket::None;
        self.verified_payload = None;
        self.verified_hold = None;
        err
    }

    /// Take ownership of a ready verified payload directly with zero copy (so in-memory drivers
    /// transfer the single staged allocation rather than peaking at 2N bytes).
    fn take_ready_verified_payload(&mut self) -> Option<Vec<u8>> {
        if self.verified_check.is_some()
            && self.verified_offset == 0
            && self.pending_ticket == PendingTicket::None
        {
            let bytes = self.verified_payload.take()?;
            self.committed_bytes = self.committed_bytes.saturating_add(bytes.len() as u64);
            self.verified_offset = 0;
            self.verified_hold = None;
            return Some(bytes);
        }
        None
    }

    fn check_output_budget(&self, offered_len: usize) -> EngineResult<()> {
        if let Some(max_out) = self.effective.resources.limits().max_output_bytes {
            let projected = self
                .committed_bytes
                .checked_add(offered_len as u64)
                .ok_or_else(|| {
                    at!(EngineError::simple(
                        Stage::Publish,
                        Failure::ResourceLimit,
                        "media output byte counter overflow",
                    )
                    .with_committed_bytes(self.committed_bytes))
                })?;
            if projected > max_out {
                return Err(at!(EngineError::simple(
                    Stage::Publish,
                    Failure::ResourceLimit,
                    "media output exceeds max_output_bytes",
                )
                .with_committed_bytes(self.committed_bytes)));
            }
        }
        Ok(())
    }

    /// Supply input bytes answering an outstanding [`PumpEvent::NeedInput`] ticket.
    ///
    /// Validates ticket identity, direction, request-offset alignment, `total_len`
    /// consistency, exact `Range` EOF semantics, and exact `max_memory_bytes` /
    /// `max_input_bytes` limits BEFORE mutating any internal state. Any protocol
    /// violation permanently fuses the pump.
    pub fn supply_input(&mut self, ticket: IoTicket, input: Input<'_>) -> EngineResult<usize> {
        if self.faulted {
            return Err(at!(EngineError::simple(
                Stage::Execute,
                Failure::BackendFailure,
                "MediaPump is faulted; no further input accepted",
            )));
        }
        match self.pending_ticket {
            PendingTicket::Input(expected) if expected == ticket => {}
            _ => {
                return Err(self.fail(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::InvalidRequest,
                    "stale or wrong-direction IoTicket passed to supply_input",
                ))));
            }
        }
        if let Err(e) = self.effective.check_stop(Stage::Execute) {
            return Err(self.fail(e));
        }

        // 1. Validate total_len consistency before mutating state.
        if let (Some(known), Some(reported)) = (self.input_total_len, input.total_len)
            && known != reported
        {
            return Err(self.fail(at!(EngineError::simple(
                Stage::Execute,
                Failure::InvalidRequest,
                "conflicting total_len supplied to MediaPump::supply_input",
            ))));
        }
        let effective_total = input.total_len.or(self.input_total_len);
        if let Some(max_in) = self.effective.resources.limits().max_input_bytes {
            let reply_end = input
                .offset
                .checked_add(input.bytes.len() as u64)
                .ok_or_else(|| {
                    self.fail(at!(EngineError::simple(
                        Stage::Execute,
                        Failure::ResourceLimit,
                        "input offset + len overflow",
                    )))
                })?;
            if reply_end > max_in || effective_total.is_some_and(|t| t > max_in) {
                return Err(self.fail(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::ResourceLimit,
                    "media input exceeds max_input_bytes",
                ))));
            }
        }
        if let Some(total) = effective_total {
            let reply_end = input
                .offset
                .checked_add(input.bytes.len() as u64)
                .ok_or_else(|| {
                    self.fail(at!(EngineError::simple(
                        Stage::Execute,
                        Failure::InvalidRequest,
                        "input offset + len overflow",
                    )))
                })?;
            if reply_end > total {
                return Err(self.fail(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::InvalidRequest,
                    "input slice extends past known total_len",
                ))));
            }
        }

        // 2. Validate that input.offset matches one of the outstanding ticketed requests.
        let matching_req = self
            .pending_requests
            .iter()
            .copied()
            .find(|req| match *req {
                InputRequest::AtLeast { offset, .. } => input.offset == offset,
                InputRequest::Range(r) => input.offset == r.offset,
                _ => false,
            });
        let Some(matched) = matching_req else {
            return Err(self.fail(at!(EngineError::simple(
                Stage::Execute,
                Failure::InvalidRequest,
                "supply_input offset did not match any outstanding NeedInput request",
            ))));
        };

        let bytes = input.bytes;
        // Exact positive-length Range requests MUST be fulfilled completely before stepping;
        // a short nonempty (or empty) reply cannot leak a partial window into the demuxer.
        if let InputRequest::Range(r) = matched
            && (bytes.len() as u64) < r.length
        {
            return Err(self.fail(at!(EngineError::simple(
                Stage::Execute,
                Failure::InvalidData,
                "input reply was shorter than requested exact byte Range",
            ))));
        }
        // Honor caller's exact max_memory_bytes with NO arbitrary floor!
        let max_buf = self
            .effective
            .resources
            .limits()
            .max_memory_bytes
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(32 * 1024 * 1024);

        let current_end = self
            .input_offset
            .saturating_add(self.input_buf.len() as u64);
        let prev_len = self.input_buf.len();
        let prev_off = self.input_offset;

        if bytes.is_empty() {
            // Empty reply signals EOF at `input.offset`. Exact positive-length Range requests
            // CANNOT be satisfied by EOF!
            if let InputRequest::Range(r) = matched
                && r.length > 0
            {
                return Err(self.fail(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::InvalidData,
                    "media stream reached EOF before satisfying exact Range request",
                ))));
            }
            if self.stalled_at_eof && input.offset == current_end {
                return Err(self.fail(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::InvalidData,
                    "media stream reached EOF before completing container structure",
                ))));
            }
            // All checks passed -> commit state mutations.
            if let Some(t) = effective_total {
                self.input_total_len = Some(t);
            }
            if input.offset != current_end {
                self.input_offset = input.offset;
                self.input_buf.clear();
            }
            if self.input_total_len.is_none() {
                self.input_total_len = Some(
                    self.input_offset
                        .saturating_add(self.input_buf.len() as u64),
                );
            }
            self.stalled_at_eof = true;
        } else if !self.input_buf.is_empty() && input.offset == current_end {
            // Forward-only streaming append without re-reading earlier bytes.
            let next_len = self
                .input_buf
                .len()
                .checked_add(bytes.len())
                .ok_or_else(|| {
                    self.fail(at!(EngineError::simple(
                        Stage::Execute,
                        Failure::ResourceLimit,
                        "MediaPump input staging buffer overflow",
                    )))
                })?;
            if next_len > max_buf {
                return Err(self.fail(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::ResourceLimit,
                    "MediaPump input staging buffer exceeded max_memory_bytes",
                ))));
            }
            if self.input_buf.try_reserve(bytes.len()).is_err() {
                return Err(self.fail(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::ResourceLimit,
                    "MediaPump input staging allocation failed",
                ))));
            }
            if let Some(t) = effective_total {
                self.input_total_len = Some(t);
            }
            self.input_buf.extend_from_slice(bytes);
            self.stalled_at_eof = false;
        } else if !self.input_buf.is_empty()
            && input.offset >= self.input_offset
            && input.offset < current_end
        {
            let overlap = (current_end - input.offset) as usize;
            if bytes.len() <= overlap {
                return Err(self.fail(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::InvalidRequest,
                    "supply_input provided bytes already buffered without advancing stream",
                ))));
            }
            let extra = &bytes[overlap..];
            let next_len = self
                .input_buf
                .len()
                .checked_add(extra.len())
                .ok_or_else(|| {
                    self.fail(at!(EngineError::simple(
                        Stage::Execute,
                        Failure::ResourceLimit,
                        "MediaPump input staging buffer overflow",
                    )))
                })?;
            if next_len > max_buf {
                return Err(self.fail(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::ResourceLimit,
                    "MediaPump input staging buffer exceeded max_memory_bytes",
                ))));
            }
            if self.input_buf.try_reserve(extra.len()).is_err() {
                return Err(self.fail(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::ResourceLimit,
                    "MediaPump input staging allocation failed",
                ))));
            }
            if let Some(t) = effective_total {
                self.input_total_len = Some(t);
            }
            self.input_buf.extend_from_slice(extra);
            self.stalled_at_eof = false;
        } else {
            if bytes.len() > max_buf {
                return Err(self.fail(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::ResourceLimit,
                    "MediaPump input staging chunk exceeded max_memory_bytes",
                ))));
            }
            let mut next_buf = Vec::new();
            if next_buf.try_reserve_exact(bytes.len()).is_err() {
                return Err(self.fail(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::ResourceLimit,
                    "MediaPump input staging allocation failed",
                ))));
            }
            next_buf.extend_from_slice(bytes);
            if let Some(t) = effective_total {
                self.input_total_len = Some(t);
            }
            self.input_offset = input.offset;
            self.input_buf = next_buf;
            self.stalled_at_eof = false;
        }

        if self.input_offset == prev_off && self.input_buf.len() == prev_len && !bytes.is_empty() {
            return Err(self.fail(at!(EngineError::simple(
                Stage::Execute,
                Failure::InvalidRequest,
                "supply_input provided bytes already buffered without advancing stream",
            ))));
        }
        self.pending_requests.clear();
        self.pending_ticket = PendingTicket::None;
        Ok(bytes.len())
    }

    /// Confirm that `bytes` from the outstanding [`PumpEvent::NeedWriter`] offer were written.
    ///
    /// - For [`WriteOffer::Append`], partial positive writes (`1..=offered_len`) are allowed
    ///   and the remaining slice will be re-offered on the next [`Self::advance`]. Passing `0`
    ///   for a non-empty append offer is treated as a protocol/write-zero error and fuses the pump.
    /// - For [`WriteOffer::Patch`], the entire patch (`bytes == offered_len`) must commit
    ///   atomically.
    /// - Any stale, wrong-direction, or out-of-range confirmation permanently fuses the pump.
    pub fn confirm_written(&mut self, ticket: IoTicket, bytes: usize) -> EngineResult<()> {
        if self.faulted {
            return Err(at!(EngineError::simple(
                Stage::Publish,
                Failure::BackendFailure,
                "MediaPump is faulted; no further writes accepted",
            )));
        }
        match self.pending_ticket {
            PendingTicket::WriteMuxAppend {
                ticket: expected,
                offered_len,
            } if expected == ticket => {
                if bytes == 0 && offered_len > 0 {
                    return Err(self.fail(at!(EngineError::simple(
                        Stage::Publish,
                        Failure::Io,
                        "confirm_written reported zero bytes written for non-empty append offer",
                    ))));
                }
                if bytes > offered_len {
                    return Err(self.fail(at!(EngineError::simple(
                        Stage::Publish,
                        Failure::InvalidRequest,
                        "confirm_written byte count exceeded offered write length",
                    ))));
                }
                if let Some(mux) = self.muxer.as_mut() {
                    mux.consume_output(bytes);
                }
                self.committed_bytes = self.committed_bytes.saturating_add(bytes as u64);
                self.pending_ticket = PendingTicket::None;
                Ok(())
            }
            PendingTicket::WriteTailAppend {
                ticket: expected,
                offered_len,
            } if expected == ticket => {
                if bytes == 0 && offered_len > 0 {
                    return Err(self.fail(at!(EngineError::simple(
                        Stage::Publish,
                        Failure::Io,
                        "confirm_written reported zero bytes written for non-empty tail offer",
                    ))));
                }
                if bytes > offered_len {
                    return Err(self.fail(at!(EngineError::simple(
                        Stage::Publish,
                        Failure::InvalidRequest,
                        "confirm_written byte count exceeded offered tail write length",
                    ))));
                }
                self.pending_tail_offset = self.pending_tail_offset.saturating_add(bytes);
                if self.pending_tail_offset >= self.pending_tail.len() {
                    self.pending_tail.clear();
                    self.pending_tail_offset = 0;
                }
                self.committed_bytes = self.committed_bytes.saturating_add(bytes as u64);
                self.pending_ticket = PendingTicket::None;
                Ok(())
            }
            PendingTicket::WriteVerifiedAppend {
                ticket: expected,
                offered_len,
            } if expected == ticket => {
                if bytes == 0 && offered_len > 0 {
                    return Err(self.fail(at!(EngineError::simple(
                        Stage::Publish,
                        Failure::Io,
                        "confirm_written reported zero bytes written for verified payload offer",
                    ))));
                }
                if bytes > offered_len {
                    return Err(self.fail(at!(EngineError::simple(
                        Stage::Publish,
                        Failure::InvalidRequest,
                        "confirm_written byte count exceeded offered verified write length",
                    ))));
                }
                self.verified_offset = self.verified_offset.saturating_add(bytes);
                if self
                    .verified_payload
                    .as_ref()
                    .is_some_and(|p| self.verified_offset >= p.len())
                {
                    self.verified_payload = None;
                    self.verified_hold = None;
                    self.verified_offset = 0;
                }
                self.committed_bytes = self.committed_bytes.saturating_add(bytes as u64);
                self.pending_ticket = PendingTicket::None;
                Ok(())
            }
            PendingTicket::WritePatch {
                ticket: expected,
                offered_len,
            } if expected == ticket => {
                if bytes != offered_len {
                    return Err(self.fail(at!(EngineError::simple(
                        Stage::Publish,
                        Failure::Io,
                        "short patch write is not allowed; full patch must commit atomically",
                    ))));
                }
                let _ = self.pending_patches.pop_front();
                self.pending_ticket = PendingTicket::None;
                Ok(())
            }
            _ => Err(self.fail(at!(EngineError::simple(
                Stage::Publish,
                Failure::InvalidRequest,
                "stale or wrong-direction IoTicket passed to confirm_written",
            )))),
        }
    }

    /// Advance the pump by one downstream-first step.
    ///
    /// Calling `advance()` while a prior [`IoTicket`] is unresolved is a terminal protocol
    /// error that permanently fuses the pump.
    pub fn advance(&mut self) -> EngineResult<PumpEvent<'_>> {
        if self.faulted {
            return Err(at!(EngineError::simple(
                Stage::Execute,
                Failure::BackendFailure,
                "MediaPump is faulted",
            )));
        }
        if self.pending_ticket != PendingTicket::None {
            return Err(self.fail(at!(EngineError::simple(
                Stage::Execute,
                Failure::InvalidRequest,
                "cannot advance MediaPump while an IoTicket is unresolved",
            ))));
        }
        if let Err(e) = self.effective.check_stop(Stage::Execute) {
            return Err(self.fail(e));
        }

        // 1. Downstream-first: drain pending muxer bytes (staging privately under Verified,
        //    or emitting NeedWriter directly under Incremental with end-to-end budget checks).
        if let Some(mux) = self.muxer.as_mut()
            && !mux.pending_output().is_empty()
        {
            if self.intent.publication == PublicationMode::Verified {
                let mut tmp = Vec::new();
                mux.take_output(&mut tmp);
                if let Some(staged) = self.staged_pub.as_mut()
                    && let Err(e) = staged.append(&tmp)
                {
                    return Err(self.fail(e));
                }
                return Ok(PumpEvent::Presented);
            }
            let offered_len = mux.pending_output().len();
            if let Err(e) = self.check_output_budget(offered_len) {
                return Err(self.fail(e));
            }
            let ticket = match self.alloc_ticket() {
                Ok(t) => t,
                Err(e) => return Err(self.fail(e)),
            };
            self.pending_ticket = PendingTicket::WriteMuxAppend {
                ticket,
                offered_len,
            };
            let bytes = self.muxer.as_ref().expect("checked").pending_output();
            return Ok(PumpEvent::NeedWriter {
                ticket,
                write: WriteOffer::Append(bytes),
            });
        }

        // 2. Drain verified post-readback payload (PublicationMode::Verified only).
        if let Some(ref payload) = self.verified_payload
            && self.verified_offset < payload.len()
        {
            let offered_len = payload.len() - self.verified_offset;
            if let Err(e) = self.check_output_budget(offered_len) {
                return Err(self.fail(e));
            }
            let ticket = match self.alloc_ticket() {
                Ok(t) => t,
                Err(e) => return Err(self.fail(e)),
            };
            self.pending_ticket = PendingTicket::WriteVerifiedAppend {
                ticket,
                offered_len,
            };
            let bytes = &self.verified_payload.as_ref().expect("checked")[self.verified_offset..];
            return Ok(PumpEvent::NeedWriter {
                ticket,
                write: WriteOffer::Append(bytes),
            });
        }

        // 3. Drain any queued finish tail bytes (Incremental mode).
        if self.pending_tail_offset < self.pending_tail.len() {
            let offered_len = self.pending_tail.len() - self.pending_tail_offset;
            if let Err(e) = self.check_output_budget(offered_len) {
                return Err(self.fail(e));
            }
            let ticket = match self.alloc_ticket() {
                Ok(t) => t,
                Err(e) => return Err(self.fail(e)),
            };
            self.pending_ticket = PendingTicket::WriteTailAppend {
                ticket,
                offered_len,
            };
            let bytes = &self.pending_tail[self.pending_tail_offset..];
            return Ok(PumpEvent::NeedWriter {
                ticket,
                write: WriteOffer::Append(bytes),
            });
        }

        // 4. Drain any queued finish patches (if any).
        if let Some((offset, patch_bytes)) = self.pending_patches.front() {
            let offset = *offset;
            let offered_len = patch_bytes.len();
            let ticket = match self.alloc_ticket() {
                Ok(t) => t,
                Err(e) => return Err(self.fail(e)),
            };
            self.pending_ticket = PendingTicket::WritePatch {
                ticket,
                offered_len,
            };
            let bytes = self.pending_patches.front().expect("checked").1.as_slice();
            return Ok(PumpEvent::NeedWriter {
                ticket,
                write: WriteOffer::Patch { offset, bytes },
            });
        }

        if self.mux_finished {
            return Ok(PumpEvent::Finished);
        }

        // 5. Push any queued encoded packet into the required BoundedMuxer.
        if self.pending_packet_for_mux.is_some() {
            let pulse: Option<&dyn zencodec::Pulse> = Some(self.effective.resources.pulse());
            if let Some(mux) = self.muxer.as_mut() {
                let Some(bounded) = mux.as_bounded_mut() else {
                    return Err(self.fail(at!(EngineError::simple(
                        Stage::Execute,
                        Failure::UnsupportedEffect,
                        "MediaPump requires a BoundedMuxer for backpressured output admission",
                    ))));
                };
                match bounded.try_write_packet(&mut self.pending_packet_for_mux, pulse) {
                    Ok(PacketWrite::Accepted) => return Ok(PumpEvent::Presented),
                    Ok(PacketWrite::DrainOutput) => {
                        let offered_len = bounded.pending_output().len();
                        if offered_len == 0 {
                            return Err(self.fail(at!(EngineError::simple(
                                Stage::Execute,
                                Failure::BackendFailure,
                                "BoundedMuxer requested DrainOutput with empty pending buffer",
                            ))));
                        }
                        if self.intent.publication == PublicationMode::Verified {
                            let mut tmp = Vec::new();
                            bounded.take_output(&mut tmp);
                            if let Some(staged) = self.staged_pub.as_mut()
                                && let Err(e) = staged.append(&tmp)
                            {
                                return Err(self.fail(e));
                            }
                            return Ok(PumpEvent::Presented);
                        }
                        if let Err(e) = self.check_output_budget(offered_len) {
                            return Err(self.fail(e));
                        }
                        let ticket = match self.alloc_ticket() {
                            Ok(t) => t,
                            Err(e) => return Err(self.fail(e)),
                        };
                        self.pending_ticket = PendingTicket::WriteMuxAppend {
                            ticket,
                            offered_len,
                        };
                        let bytes = self.muxer.as_ref().expect("checked").pending_output();
                        return Ok(PumpEvent::NeedWriter {
                            ticket,
                            write: WriteOffer::Append(bytes),
                        });
                    }
                    Ok(_) => return Ok(PumpEvent::Presented),
                    Err(e) => {
                        return Err(self.fail(at!(EngineError::simple(
                            Stage::Execute,
                            Failure::BackendFailure,
                            "bounded muxer packet write failed",
                        )
                        .with_cause(e))));
                    }
                }
            }
        }

        // 6. Drain encoded packets from the video encoder.
        if !self.enc_done
            && let Some(enc) = self.encoder.as_mut()
        {
            let pulse: Option<&dyn zencodec::Pulse> = Some(self.effective.resources.pulse());
            match enc.receive(pulse) {
                Ok(Receive::Ready(pkt)) => {
                    if let Err(e) = self.ensure_muxer_begun() {
                        return Err(self.fail(e));
                    }
                    self.pending_packet_for_mux = Some(pkt);
                    return Ok(PumpEvent::Presented);
                }
                Ok(Receive::Progress { .. }) => return Ok(PumpEvent::Presented),
                Ok(Receive::EndOfStream) => {
                    self.enc_done = true;
                    return Ok(PumpEvent::Presented);
                }
                Ok(Receive::NeedInput) => {}
                Ok(_) => return Ok(PumpEvent::Presented),
                Err(e) => {
                    return Err(self.fail(at!(EngineError::simple(
                        Stage::Execute,
                        Failure::BackendFailure,
                        "video encoder receive failed",
                    )
                    .with_cause(e))));
                }
            }
        }

        // 7. Feed queued mapped frames to the video encoder (lazy encoder init on first frame).
        if let Some(frame) = self.pending_frames_for_enc.pop_front() {
            let frame = match repack_frame_tight(frame, self.effective.resources.storage()) {
                Ok(f) => f,
                Err(e) => return Err(self.fail(e)),
            };
            if let Err(e) = self.ensure_encoder_opened(&frame) {
                return Err(self.fail(e));
            }
            let pulse: Option<&dyn zencodec::Pulse> = Some(self.effective.resources.pulse());
            let enc = self.encoder.as_mut().expect("opened above");
            match enc.send(&frame, pulse) {
                Ok(SendStatus::Accepted) => return Ok(PumpEvent::Presented),
                Ok(SendStatus::ReceivePending) => {
                    self.pending_frames_for_enc.push_front(frame);
                    return Ok(PumpEvent::Presented);
                }
                Ok(_) => return Ok(PumpEvent::Presented),
                Err(e) => {
                    return Err(self.fail(at!(EngineError::simple(
                        Stage::Execute,
                        Failure::BackendFailure,
                        "video encoder send failed",
                    )
                    .with_cause(e))));
                }
            }
        }

        // 8. If decoder is done and no frames remain, close encoder input or finish muxer.
        if self.dec_done && self.pending_frames_for_enc.is_empty() {
            if let Some(enc) = self.encoder.as_mut() {
                if !self.enc_ended {
                    let pulse: Option<&dyn zencodec::Pulse> =
                        Some(self.effective.resources.pulse());
                    if let Err(e) = enc.end_input(pulse) {
                        return Err(self.fail(at!(EngineError::simple(
                            Stage::Execute,
                            Failure::BackendFailure,
                            "video encoder end_input failed",
                        )
                        .with_cause(e))));
                    }
                    self.enc_ended = true;
                    return Ok(PumpEvent::Presented);
                }
            } else {
                self.enc_done = true;
            }

            if self.enc_done && !self.mux_finished {
                if let Some(mut mux) = self.muxer.take() {
                    let pulse: Option<&dyn zencodec::Pulse> =
                        Some(self.effective.resources.pulse());
                    let mut pre_finish = Vec::new();
                    if !mux.pending_output().is_empty() {
                        mux.take_output(&mut pre_finish);
                    }
                    let finished: MuxFinish = match mux.finish(pulse) {
                        Ok(f) => f,
                        Err(e) => {
                            return Err(self.fail(at!(EngineError::simple(
                                Stage::Publish,
                                Failure::BackendFailure,
                                "muxer finish failed",
                            )
                            .with_cause(e))));
                        }
                    };
                    if self.intent.publication == PublicationMode::Verified {
                        let mut staged = self.staged_pub.take().ok_or_else(|| {
                            self.fail(at!(EngineError::simple(
                                Stage::Verify,
                                Failure::BackendFailure,
                                "missing Verified staging buffer",
                            )))
                        })?;
                        if !pre_finish.is_empty()
                            && let Err(e) = staged.append(&pre_finish)
                        {
                            return Err(self.fail(e));
                        }
                        if !finished.tail.is_empty()
                            && let Err(e) = staged.append(&finished.tail)
                        {
                            return Err(self.fail(e));
                        }
                        for patch in finished.patches {
                            if let Err(e) = staged.apply_patch(patch.offset, &patch.bytes) {
                                return Err(self.fail(e));
                            }
                        }
                        let (published, hold) = match staged.finish_retained(
                            &self.target_container,
                            &self.engine,
                            &mut self.effective,
                            &mut self.report,
                        ) {
                            Ok(p) => p,
                            Err(e) => return Err(self.fail(e)),
                        };
                        self.verified_check = published.verified;
                        self.verified_payload = Some(published.bytes);
                        self.verified_hold = hold;
                        self.verified_offset = 0;
                        self.mux_finished = true;
                        return Ok(PumpEvent::Presented);
                    }
                    if !finished.patches.is_empty() {
                        return Err(self.fail(at!(EngineError::simple(
                            Stage::Publish,
                            Failure::UnsupportedEffect,
                            "muxer emitted seek-back patches in Incremental forward publication mode",
                        ))));
                    }
                    if !pre_finish.is_empty() {
                        pre_finish.extend_from_slice(&finished.tail);
                        self.pending_tail = pre_finish;
                    } else {
                        self.pending_tail = finished.tail;
                    }
                    self.pending_tail_offset = 0;
                    self.mux_finished = true;
                    return Ok(PumpEvent::Presented);
                } else {
                    self.mux_finished = true;
                    return Ok(PumpEvent::Finished);
                }
            }
        }

        // 9. Drain frames from the video decoder into PresentationMap.
        if !self.dec_done
            && let Some(dec) = self.decoder.as_mut()
        {
            let pulse: Option<&dyn zencodec::Pulse> = Some(self.effective.resources.pulse());
            match dec.receive(pulse) {
                Ok(Receive::Ready(frame)) => {
                    if self.timeline.is_some() {
                        let ready: Vec<MappedPresentation> = {
                            let tl = self.timeline.as_mut().expect("checked");
                            if let Err(e) = tl.push_frame(frame) {
                                return Err(self.fail(e));
                            }
                            tl.drain_ready().collect()
                        };
                        for item in ready {
                            match item {
                                MappedPresentation::Frame(f) => {
                                    if let Err(e) = self.enqueue_decoded_frame(f) {
                                        return Err(self.fail(e));
                                    }
                                    if self.dec_done {
                                        break;
                                    }
                                }
                                MappedPresentation::Gap { .. } => {
                                    return Err(self.fail(at!(EngineError::simple(
                                        Stage::Execute,
                                        Failure::UnsupportedEffect,
                                        "explicit timeline edit gap cannot be silently dropped during video transcode",
                                    ))));
                                }
                            }
                        }
                    } else if let Err(e) = self.enqueue_decoded_frame(frame) {
                        return Err(self.fail(e));
                    }
                    return Ok(PumpEvent::Presented);
                }
                Ok(Receive::Progress { .. }) => return Ok(PumpEvent::Presented),
                Ok(Receive::EndOfStream) => {
                    if self.timeline.is_some() {
                        let ready: Vec<MappedPresentation> = {
                            let tl = self.timeline.as_mut().expect("checked");
                            if let Err(e) = tl.finish() {
                                return Err(self.fail(e));
                            }
                            tl.drain_ready().collect()
                        };
                        for item in ready {
                            match item {
                                MappedPresentation::Frame(f) => {
                                    if let Err(e) = self.enqueue_decoded_frame(f) {
                                        return Err(self.fail(e));
                                    }
                                    if self.dec_done {
                                        break;
                                    }
                                }
                                MappedPresentation::Gap { .. } => {
                                    return Err(self.fail(at!(EngineError::simple(
                                        Stage::Execute,
                                        Failure::UnsupportedEffect,
                                        "explicit timeline edit gap cannot be silently dropped during video transcode",
                                    ))));
                                }
                            }
                        }
                    }
                    if self.pending_unknown_end
                        && matches!(
                            self.intent.sequence,
                            SequencePolicy::Preserve {
                                timing: TimingPolicy::QuantizeEndpoints { .. },
                                ..
                            }
                        )
                    {
                        return Err(self.fail(at!(EngineError::simple(
                            Stage::Verify,
                            Failure::UnsupportedEffect,
                            "final video presentation missing duration_ticks; cannot verify endpoint quantization within max_boundary_error",
                        ))));
                    }
                    self.dec_done = true;
                    return Ok(PumpEvent::Presented);
                }
                Ok(Receive::NeedInput) => {}
                Ok(_) => return Ok(PumpEvent::Presented),
                Err(e) => {
                    return Err(self.fail(at!(EngineError::simple(
                        Stage::Execute,
                        Failure::BackendFailure,
                        "video decoder receive failed",
                    )
                    .with_cause(e))));
                }
            }
        }

        // 10. Push pending demuxed packet into the video decoder (recording EVERY accepted packet's
        //     PTS + optional discard ONLY after `SendStatus::Accepted` so retry/failure never
        //     pollutes the queue and hidden/visible same-PTS ambiguity is refused).
        if !self.dec_done
            && let Some(mut pkt) = self.pending_packet_for_dec.take()
            && let Some(dec) = self.decoder.as_mut()
        {
            let pkt_pts = pkt.pts();
            let pkt_discard = pkt.discard();
            let pulse: Option<&dyn zencodec::Pulse> = Some(self.effective.resources.pulse());
            match dec.send(&mut pkt, pulse) {
                Ok(SendStatus::Accepted) => {
                    if let Some(tl) = self.timeline.as_mut()
                        && let Err(e) = tl.record_accepted_packet(pkt_pts, pkt_discard)
                    {
                        return Err(self.fail(e));
                    }
                    return Ok(PumpEvent::Presented);
                }
                Ok(SendStatus::ReceivePending) => {
                    self.pending_packet_for_dec = Some(pkt);
                    return Ok(PumpEvent::Presented);
                }
                Ok(_) => return Ok(PumpEvent::Presented),
                Err(e) => {
                    return Err(self.fail(at!(EngineError::simple(
                        Stage::Execute,
                        Failure::BackendFailure,
                        "video decoder send failed",
                    )
                    .with_cause(e))));
                }
            }
        }

        // 11. If demuxer is done, end decoder input.
        if self.demux_done {
            if !self.dec_done
                && let Some(dec) = self.decoder.as_mut()
            {
                if !self.dec_ended {
                    let pulse: Option<&dyn zencodec::Pulse> =
                        Some(self.effective.resources.pulse());
                    if let Err(e) = dec.end_input(pulse) {
                        return Err(self.fail(at!(EngineError::simple(
                            Stage::Execute,
                            Failure::BackendFailure,
                            "video decoder end_input failed",
                        )
                        .with_cause(e))));
                    }
                    self.dec_ended = true;
                    return Ok(PumpEvent::Presented);
                }
            } else {
                self.dec_done = true;
            }
            return Ok(PumpEvent::Presented);
        }

        // 12. Step the demuxer with currently buffered input bytes.
        let input =
            Input::new(self.input_offset, &self.input_buf).with_total_len_opt(self.input_total_len);
        let pulse: Option<&dyn zencodec::Pulse> = Some(self.effective.resources.pulse());
        let prog = match self.demuxer.step(input, pulse) {
            Ok(p) => p,
            Err(e) => {
                return Err(self.fail(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::InvalidData,
                    "demuxer step failed",
                )
                .with_cause(e))));
            }
        };
        if prog.consumed > self.input_buf.len() {
            return Err(self.fail(at!(EngineError::simple(
                Stage::Execute,
                Failure::BackendFailure,
                "demuxer overconsumed offered input buffer",
            ))));
        }
        if prog.consumed > 0 {
            self.input_buf.drain(..prog.consumed);
            self.input_offset = self.input_offset.saturating_add(prog.consumed as u64);
            self.stalled_at_eof = false;
        }

        match prog.event {
            DemuxEvent::Need => {
                let mut buf_start = self.input_offset;
                let buf_end = self
                    .input_offset
                    .saturating_add(self.input_buf.len() as u64);
                let at_known_eof = self.input_total_len.is_some_and(|t| buf_end >= t);

                // Validate exact Range requests against known EOF upfront: a truncated Range
                // can NEVER be satisfied by EOF!
                if let Some(total) = self.input_total_len {
                    let all_ranges_past_eof = !self.demuxer.requested().is_empty()
                        && self.demuxer.requested().iter().all(|req| match *req {
                            InputRequest::Range(r) => {
                                r.offset.checked_add(r.length).is_none_or(|end| end > total)
                            }
                            _ => false,
                        });
                    if all_ranges_past_eof {
                        return Err(self.fail(at!(EngineError::simple(
                            Stage::Execute,
                            Failure::InvalidData,
                            "demuxer requested exact byte Range extending past known EOF",
                        ))));
                    }
                }

                // Case A: Demuxer consumed bytes and still needs data at `buf_start` that is
                // already satisfied by `self.input_buf` (exact Range NEVER satisfied by EOF alone).
                if prog.consumed > 0
                    && self.demuxer.requested().iter().any(|req| match *req {
                        InputRequest::AtLeast { offset, min_len } => {
                            offset == buf_start && (self.input_buf.len() >= min_len || at_known_eof)
                        }
                        InputRequest::Range(r) => {
                            r.offset == buf_start && (self.input_buf.len() as u64) >= r.length
                        }
                        _ => false,
                    })
                {
                    return Ok(PumpEvent::Presented);
                }

                // Case B: Demuxer requested a forward offset strictly inside our buffered window
                // already satisfied in-memory (exact Range NEVER satisfied by EOF alone).
                if let Some(advance_to) = self.demuxer.requested().iter().find_map(|req| match *req
                {
                    InputRequest::AtLeast { offset, min_len }
                        if offset > buf_start && offset <= buf_end =>
                    {
                        let avail = (buf_end - offset) as usize;
                        if avail >= min_len || at_known_eof {
                            Some(offset)
                        } else {
                            None
                        }
                    }
                    InputRequest::Range(r) if r.offset > buf_start && r.offset <= buf_end => {
                        let avail = buf_end - r.offset;
                        if avail >= r.length {
                            Some(r.offset)
                        } else {
                            None
                        }
                    }
                    _ => None,
                }) {
                    let skip = (advance_to - buf_start) as usize;
                    self.input_buf.drain(..skip);
                    self.input_offset = advance_to;
                    self.stalled_at_eof = false;
                    return Ok(PumpEvent::Presented);
                }

                // Case B2: All requested alternatives start strictly inside `(buf_start..buf_end)`
                // (or beyond) and extend past `buf_end`. Drain the unused prefix up to the
                // earliest requested offset so Case C can ask solely for the missing suffix.
                let min_inside = self
                    .demuxer
                    .requested()
                    .iter()
                    .filter_map(|req| req.offset())
                    .min();
                if let Some(min_off) = min_inside
                    && min_off > buf_start
                    && min_off < buf_end
                    && self
                        .demuxer
                        .requested()
                        .iter()
                        .all(|req| req.offset().is_some_and(|o| o >= min_off))
                {
                    let skip = (min_off - buf_start) as usize;
                    self.input_buf.drain(..skip);
                    self.input_offset = min_off;
                    buf_start = min_off;
                    self.stalled_at_eof = false;
                }

                let ticket = match self.alloc_ticket() {
                    Ok(t) => t,
                    Err(e) => return Err(self.fail(e)),
                };
                self.pending_ticket = PendingTicket::Input(ticket);
                self.pending_requests.clear();
                for &req in self.demuxer.requested() {
                    // Case C: Request starts at `buf_start` and needs more bytes past `buf_end`.
                    // Normalize to the missing suffix at `buf_end` while PRESERVING exact
                    // `InputRequest::Range` semantics (never downgrading `Range` to `AtLeast`).
                    let normalized = match req {
                        InputRequest::AtLeast { offset, min_len }
                            if !self.input_buf.is_empty() && offset == buf_start =>
                        {
                            let want_end = offset
                                .saturating_add(min_len as u64)
                                .max(buf_end.saturating_add(1));
                            let extra = usize::try_from(want_end - buf_end).unwrap_or(1).max(1);
                            InputRequest::at_least(buf_end, extra)
                        }
                        InputRequest::Range(r)
                            if !self.input_buf.is_empty() && r.offset == buf_start =>
                        {
                            let want_end = r
                                .offset
                                .saturating_add(r.length)
                                .max(buf_end.saturating_add(1));
                            let missing = want_end - buf_end;
                            InputRequest::range(buf_end, missing)
                        }
                        // Case D: Random seek outside current window (backward or past `buf_end`).
                        other => other,
                    };
                    self.pending_requests.push(normalized);
                }
                if self.pending_requests.is_empty() {
                    self.pending_requests
                        .push(InputRequest::at_least(buf_end, 1));
                }
                Ok(PumpEvent::NeedInput {
                    ticket,
                    requests: &self.pending_requests,
                })
            }
            DemuxEvent::Headers => {
                if let Err(e) = self.on_demux_headers() {
                    return Err(self.fail(e));
                }
                Ok(PumpEvent::Presented)
            }
            DemuxEvent::Packet(pkt) => {
                if Some(pkt.track() as usize) == self.video_track_idx {
                    self.pending_packet_for_dec = Some(pkt);
                }
                Ok(PumpEvent::Presented)
            }
            DemuxEvent::ConfigChanged { track, .. } => {
                if Some(track as usize) == self.video_track_idx
                    && let Some(t) = self.demuxer.tracks().get(track as usize)
                    && let Some(dec) = self.decoder.as_mut()
                {
                    let cfg = CodecConfig::from(t);
                    let pulse: Option<&dyn zencodec::Pulse> =
                        Some(self.effective.resources.pulse());
                    if let Err(e) = dec.configure(&cfg, pulse) {
                        return Err(self.fail(at!(EngineError::simple(
                            Stage::Execute,
                            Failure::BackendFailure,
                            "decoder reconfigure on ConfigChanged failed",
                        )
                        .with_cause(e))));
                    }
                }
                Ok(PumpEvent::Presented)
            }
            DemuxEvent::Seeked(_) => Ok(PumpEvent::Presented),
            DemuxEvent::Done => {
                self.demux_done = true;
                Ok(PumpEvent::Presented)
            }
            _ => Ok(PumpEvent::Presented),
        }
    }

    fn enqueue_decoded_frame(&mut self, frame: Frame) -> EngineResult<()> {
        if let Some(max_frames) = self.effective.resources.limits().max_frames
            && self.frames_accepted >= u64::from(max_frames)
        {
            return Err(at!(EngineError::simple(
                Stage::Execute,
                Failure::ResourceLimit,
                "video stream frame count exceeded max_frames",
            )));
        }
        // Enforce QuantizeEndpoints { max_boundary_error } against the ACTUAL committed encoder
        // and container output clocks (opening encoder + muxer first so no fallback clock is used)
        // on BOTH start AND end endpoints; track missing final durations so an unknown end endpoint
        // at EOF is refused rather than silently accepted.
        if let SequencePolicy::Preserve {
            timing: TimingPolicy::QuantizeEndpoints { max_boundary_error },
            ..
        } = self.intent.sequence
        {
            self.ensure_encoder_opened(&frame)?;
            self.ensure_muxer_begun()?;
            let enc_clock = self
                .encoder
                .as_ref()
                .and_then(|e| e.output_config().map(|c| c.time_base))
                .ok_or_else(|| {
                    at!(EngineError::simple(
                        Stage::Verify,
                        Failure::BackendFailure,
                        "video encoder did not commit an output time_base for quantization verification",
                    ))
                })?;
            let pts = frame.timing().pts.ok_or_else(|| {
                at!(EngineError::simple(
                    Stage::Verify,
                    Failure::UnsupportedEffect,
                    "video presentation missing timestamp cannot satisfy QuantizeEndpoints bound",
                ))
            })?;
            // Require a proven committed output scale from the selected muxer (either via
            // Exact timing support or a known built-in muxer's scale rule); never assume a scale
            // for an unknown third-party routed muxer!
            let mux_clock = self.mux_committed_clock.ok_or_else(|| {
                at!(EngineError::simple(
                    Stage::Verify,
                    Failure::UnsupportedEffect,
                    "selected muxer does not commit a known output timestamp scale to verify QuantizeEndpoints",
                ))
            })?;
            check_endpoint_quantization(
                i128::from(pts.ticks()),
                pts.base(),
                enc_clock,
                max_boundary_error,
            )?;
            if mux_clock != enc_clock {
                check_endpoint_quantization(
                    i128::from(pts.ticks()),
                    pts.base(),
                    mux_clock,
                    max_boundary_error,
                )?;
            }
            if let Some(dur_ticks) = frame.timing().duration_ticks {
                let end_ticks = i128::from(pts.ticks())
                    .checked_add(i128::from(dur_ticks))
                    .ok_or_else(|| {
                        at!(EngineError::simple(
                            Stage::Verify,
                            Failure::ResourceLimit,
                            "presentation end timestamp overflow",
                        ))
                    })?;
                check_endpoint_quantization(end_ticks, pts.base(), enc_clock, max_boundary_error)?;
                if mux_clock != enc_clock {
                    check_endpoint_quantization(
                        end_ticks,
                        pts.base(),
                        mux_clock,
                        max_boundary_error,
                    )?;
                }
                self.pending_unknown_end = false;
            } else {
                // Interior frame's end endpoint will be resolved by the next frame's checked PTS;
                // if this turns out to be the final frame at EOF, `pending_unknown_end` causes refusal.
                self.pending_unknown_end = true;
            }
        }
        self.frames_accepted = self.frames_accepted.saturating_add(1);
        self.pending_frames_for_enc.push_back(frame);
        if matches!(self.intent.sequence, SequencePolicy::FirstPresentation) {
            self.dec_done = true;
            self.demux_done = true;
            self.pending_packet_for_dec = None;
        }
        Ok(())
    }

    fn on_demux_headers(&mut self) -> EngineResult<()> {
        let tracks = self.demuxer.tracks().to_vec();
        // MediaPump builds a single output video track; never silently drop audio/side tracks!
        let v_idx = validate_track_policy(&tracks, &self.intent.tracks, true)?;
        self.video_track_idx = Some(v_idx);
        let track = &tracks[v_idx];
        let cfg = CodecConfig::from(track);
        let max_w = self.effective.resources.limits().max_width.unwrap_or(4_096);
        let max_h = self
            .effective
            .resources
            .limits()
            .max_height
            .unwrap_or(4_096);
        let extent = track
            .video_info()
            .filter(|v| v.coded_width > 0 && v.coded_height > 0)
            .map(|v| (v.coded_width, v.coded_height))
            .unwrap_or((max_w.min(4_096), max_h.min(4_096)));
        let need = VideoDecodeNeeds::new(extent, 16);

        let (_id, factory, plan) = select_video_decoder(
            &self.engine,
            &cfg,
            &need,
            &mut self.effective,
            &mut self.report,
        )?;
        let mut dec = factory
            .open(&plan, &self.effective.resources)
            .map_err(|e| {
                at!(EngineError::simple(
                    Stage::Prepare,
                    Failure::BackendFailure,
                    "video decoder open failed",
                )
                .with_cause(e))
            })?;
        let pulse: Option<&dyn zencodec::Pulse> = Some(self.effective.resources.pulse());
        dec.configure(&cfg, pulse).map_err(|e| {
            at!(EngineError::simple(
                Stage::Prepare,
                Failure::BackendFailure,
                "video decoder initial configure failed",
            )
            .with_cause(e))
        })?;
        self.decoder = Some(dec);

        let evidence = TimingEvidence::new(
            TrimTreatment::ApplyInPresentation,
            TrimTreatment::ApplyInPresentation,
        );
        self.timeline = Some(TimelineBridge::prepare(
            track,
            evidence,
            &self.effective.resources,
        )?);
        Ok(())
    }

    fn ensure_encoder_opened(&mut self, sample_frame: &Frame) -> EngineResult<()> {
        if self.encoder.is_some() {
            return Ok(());
        }
        let interp = Arc::clone(sample_frame.interpretation());
        let clock = sample_frame
            .timing()
            .pts
            .map(Timestamp::base)
            .unwrap_or_else(|| TimeBase::new(1, 1000).expect("valid 1/1000 clock"));
        let mut need = VideoEncodeNeeds::new(self.target_codec.clone(), interp, clock, 16);
        if let Some(fid) = video_fidelity_from_coding(&self.intent.coding, &mut self.report)? {
            need = need.with_fidelity(fid);
        }
        let (_id, factory, plan) =
            select_video_encoder(&self.engine, &need, &mut self.effective, &mut self.report)?;
        let enc = factory
            .open(&plan, &self.effective.resources)
            .map_err(|e| {
                at!(EngineError::simple(
                    Stage::Prepare,
                    Failure::BackendFailure,
                    "video encoder open failed",
                )
                .with_cause(e))
            })?;
        self.encoder = Some(enc);
        Ok(())
    }

    fn ensure_muxer_begun(&mut self) -> EngineResult<()> {
        if self.mux_begun {
            return Ok(());
        }
        let enc = self.encoder.as_ref().ok_or_else(|| {
            at!(EngineError::simple(
                Stage::Prepare,
                Failure::BackendFailure,
                "encoder not initialized before mux begin",
            ))
        })?;
        let out_cfg = enc.output_config().ok_or_else(|| {
            at!(EngineError::simple(
                Stage::Prepare,
                Failure::BackendFailure,
                "encoder did not report output_config before first packet",
            ))
        })?;

        let mut tb = TrackInfo::video(
            0,
            out_cfg.codec.clone(),
            out_cfg.time_base,
            out_cfg.video.unwrap_or_else(|| VideoTrackInfo::new(64, 64)),
        );
        if let Some(priv_bytes) = &out_cfg.private {
            tb = tb.with_codec_private(Arc::clone(priv_bytes));
        }
        let track_info = tb.build().map_err(|e| {
            at!(EngineError::simple(
                Stage::Prepare,
                Failure::InvalidData,
                "failed to construct output TrackInfo",
            )
            .with_cause(e))
        })?;

        let tracks: Arc<[TrackInfo]> = Arc::from(vec![track_info]);
        let hint = match self.intent.publication {
            PublicationMode::Incremental => MuxHint::forward_only(),
            PublicationMode::Verified => MuxHint::default(),
        };
        // Bind `mux_committed_clock` directly to the plan ACTUALLY opened and begun:
        // - For `TimingPolicy::Exact`: select & begin with `MediaTimingPolicy::Exact`.
        // - For `TimingPolicy::QuantizeEndpoints`: try `MediaTimingPolicy::Exact` FIRST and,
        //   if admitted, actually OPEN and `begin_exact_timing` on that exact plan! Only fall
        //   back to `BestEffort` when `Exact` is declined, and only assign a committed clock
        //   when concrete factory pointer identity proves a built-in muxer's known scale rule.
        let (mux_factory, mux_plan, opened_exact) = match self.intent.sequence {
            SequencePolicy::Preserve {
                timing: TimingPolicy::Exact,
                ..
            } => {
                let (_id, f, p) = select_muxer(
                    &self.engine,
                    &self.target_container,
                    Arc::clone(&tracks),
                    self.intent.publication,
                    MediaTimingPolicy::Exact,
                    &mut self.effective,
                    &mut self.report,
                )?;
                (f, p, true)
            }
            SequencePolicy::Preserve {
                timing: TimingPolicy::QuantizeEndpoints { .. },
                ..
            } => {
                if let Ok((_id, f, p)) = select_muxer(
                    &self.engine,
                    &self.target_container,
                    Arc::clone(&tracks),
                    self.intent.publication,
                    MediaTimingPolicy::Exact,
                    &mut self.effective,
                    &mut self.report,
                ) {
                    (f, p, true)
                } else {
                    let (_id, f, p) = select_muxer(
                        &self.engine,
                        &self.target_container,
                        Arc::clone(&tracks),
                        self.intent.publication,
                        MediaTimingPolicy::BestEffort,
                        &mut self.effective,
                        &mut self.report,
                    )?;
                    (f, p, false)
                }
            }
            _ => {
                let (_id, f, p) = select_muxer(
                    &self.engine,
                    &self.target_container,
                    Arc::clone(&tracks),
                    self.intent.publication,
                    MediaTimingPolicy::BestEffort,
                    &mut self.effective,
                    &mut self.report,
                )?;
                (f, p, false)
            }
        };
        self.mux_committed_clock = if opened_exact {
            Some(out_cfg.time_base)
        } else if self.target_container == ContainerId::Webm
            && crate::bundles::is_builtin_webm_mux(&mux_factory)
        {
            Some(webm_effective_clock(out_cfg.time_base))
        } else if self.target_container == ContainerId::Ivf
            && crate::bundles::is_builtin_ivf_mux(&mux_factory)
        {
            Some(out_cfg.time_base)
        } else {
            None
        };
        let mut mux = mux_factory
            .open(&mux_plan, &self.effective.resources)
            .map_err(|e| {
                at!(EngineError::simple(
                    Stage::Prepare,
                    Failure::BackendFailure,
                    "muxer open failed",
                )
                .with_cause(e))
            })?;
        if mux.as_bounded_mut().is_none() {
            return Err(at!(EngineError::simple(
                Stage::Prepare,
                Failure::UnsupportedEffect,
                "MediaPump requires a BoundedMuxer for backpressured output admission",
            )));
        }
        if opened_exact {
            mux.begin_exact_timing(&tracks, hint)
        } else {
            mux.begin(&tracks, hint)
        }
        .map_err(|e| {
            at!(EngineError::simple(
                Stage::Prepare,
                Failure::BackendFailure,
                "muxer begin failed",
            )
            .with_cause(e))
        })?;
        self.muxer = Some(mux);
        self.mux_begun = true;
        Ok(())
    }
}

fn validate_video_coding(coding: &Coding) -> EngineResult<()> {
    match coding {
        Coding::Verify(_) => Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::UnsupportedEffect,
            "Coding::Verify is qualified for still SDR images only, not video targets",
        ))),
        Coding::KeepBytes | Coding::ExactJpegRoundTrip => Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::UnsupportedEffect,
            "still-only passthrough/JPEG coding mode is not supported on video targets",
        ))),
        _ => Ok(()),
    }
}

/// Compute the exact Matroska/WebM `TimestampScale` clock selected by `zenwebm::mux::choose_scale`
/// for a single-track output with committed track `TimeBase`: when `num * 1_000_000_000` is evenly
/// divisible by `den` and fits in `u32`, WebM uses that exact nanosecond tick; otherwise 1 ms.
fn webm_effective_clock(track_clock: TimeBase) -> TimeBase {
    const NS_PER_SECOND: u64 = 1_000_000_000;
    const DEFAULT_SCALE_NS: u32 = 1_000_000;
    let ns = u64::from(track_clock.numerator()).saturating_mul(NS_PER_SECOND);
    let den = u64::from(track_clock.denominator());
    let scale_ns = if den > 0 && ns.is_multiple_of(den) {
        u32::try_from(ns / den)
            .ok()
            .filter(|&s| s > 0)
            .unwrap_or(DEFAULT_SCALE_NS)
    } else {
        DEFAULT_SCALE_NS
    };
    TimeBase::new(scale_ns, NS_PER_SECOND as u32).expect("valid WebM nanosecond clock")
}

fn check_endpoint_quantization(
    ticks: i128,
    src_base: TimeBase,
    out_clock: TimeBase,
    max_boundary_error: FrameDuration,
) -> EngineResult<()> {
    let s_num = ticks
        .checked_mul(i128::from(src_base.numerator()))
        .ok_or_else(|| {
            at!(EngineError::simple(
                Stage::Verify,
                Failure::ResourceLimit,
                "presentation endpoint numerator overflow",
            ))
        })?;
    let s_den = u128::from(src_base.denominator());
    let out_num = u128::from(out_clock.numerator());
    let out_den = u128::from(out_clock.denominator());
    let mag_num = s_num.unsigned_abs().checked_mul(out_den).ok_or_else(|| {
        at!(EngineError::simple(
            Stage::Verify,
            Failure::ResourceLimit,
            "quantized endpoint numerator overflow",
        ))
    })?;
    let den_ticks = s_den.checked_mul(out_num).ok_or_else(|| {
        at!(EngineError::simple(
            Stage::Verify,
            Failure::ResourceLimit,
            "quantized endpoint denominator overflow",
        ))
    })?;
    let rem = mag_num % den_ticks;
    // Distance to nearest output tick in scaled units: min(rem, den_ticks - rem) / (s_den * out_den) seconds.
    let d_num = rem.min(den_ticks - rem);
    let err_den = s_den.checked_mul(out_den).ok_or_else(|| {
        at!(EngineError::simple(
            Stage::Verify,
            Failure::ResourceLimit,
            "quantization error denominator overflow",
        ))
    })?;
    let m_num = u128::from(max_boundary_error.numerator());
    let m_den = u128::from(max_boundary_error.denominator());
    if crate::media::select::mul_u128_wide(d_num, m_den)
        > crate::media::select::mul_u128_wide(m_num, err_den)
    {
        return Err(at!(EngineError::simple(
            Stage::Verify,
            Failure::UnsupportedEffect,
            "video presentation boundary quantization error exceeded max_boundary_error",
        )));
    }
    Ok(())
}

fn video_fidelity_from_coding(
    coding: &Coding,
    report: &mut RouteReport,
) -> EngineResult<Option<Fidelity>> {
    validate_video_coding(coding)?;
    Ok(match coding {
        Coding::Auto => None,
        Coding::Lossless => Some(Fidelity::Lossless),
        Coding::CalibratedQuality(q) => Some(Fidelity::codec_quality(*q)),
        Coding::QualityProfile { level, dpr } => {
            Some(Fidelity::codec_quality(level.calibrated_quality(*dpr)))
        }
        Coding::AimFor(t) => {
            t.validate()?;
            if let Some(ref rev) = t.revision
                && rev != crate::engine::AIM_SEED_REVISION
            {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    "Coding::AimFor calibration revision does not match pinned target revision",
                )));
            }
            let est_q = video_aim_seed(t);
            report.aim_metric = Some(t.metric);
            report.aim_target = Some(t.value);
            report.aim_revision = Some(crate::engine::AIM_SEED_REVISION.to_string());
            report.aim_estimated_quality = Some(est_q);
            Some(Fidelity::codec_quality(est_q))
        }
        Coding::LosslessOrKeep => Some(Fidelity::codec_quality(
            QualityLevel::Good.calibrated_quality(1.0),
        )),
        Coding::Verify(_) | Coding::KeepBytes | Coding::ExactJpegRoundTrip => unreachable!(),
    })
}

fn video_aim_seed(target: &MetricTarget) -> f32 {
    match target.metric {
        Metric::Ssimulacra2 | Metric::ZensimA | Metric::ZensimB => target.value.clamp(10.0, 99.0),
        Metric::ButteraugliMax => (100.0 - target.value * 12.0).clamp(15.0, 98.0),
    }
}

/// Compact a decoded planar [`Frame`] into tightly packed, `StorageBudget`-accounted
/// planes so downstream encoders never see trailing-stride discrepancies and decoder DPB
/// slots are released immediately.
fn repack_frame_tight(
    frame: Frame,
    storage: &zencodec::storage::StorageBudget,
) -> EngineResult<Frame> {
    let interp = Arc::clone(frame.interpretation());
    let timing = frame.timing();
    let pres = frame.presentation();
    let specs = interp.layout.planes();
    let mapped = frame.picture().map().map_err(|e| {
        at!(EngineError::simple(
            Stage::Execute,
            Failure::BackendFailure,
            "failed to map decoded frame planes for repacking",
        )
        .with_cause(e))
    })?;
    let mut encoded_planes = Vec::with_capacity(specs.len());
    for (idx, spec) in specs.iter().enumerate() {
        let plane = mapped.plane(idx).ok_or_else(|| {
            at!(EngineError::simple(
                Stage::Execute,
                Failure::BackendFailure,
                "decoded frame missing declared plane",
            ))
        })?;
        let owned = match plane {
            AnyPlane::U8(p) => {
                let (w, h) = (p.width(), p.height());
                let total = w.checked_mul(h).ok_or_else(|| {
                    at!(EngineError::simple(
                        Stage::Execute,
                        Failure::ResourceLimit,
                        "u8 plane size overflow",
                    ))
                })?;
                let mut buf = storage.filled(total, 0u8).map_err(|e| {
                    at!(EngineError::simple(
                        Stage::Execute,
                        Failure::ResourceLimit,
                        "storage budget exhausted repacking u8 plane",
                    )
                    .with_cause(e))
                })?;
                let slice = buf.as_mut_slice();
                for y in 0..h {
                    let row = p.row(y).ok_or_else(|| {
                        at!(EngineError::simple(
                            Stage::Execute,
                            Failure::BackendFailure,
                            "decoded u8 plane row out of bounds",
                        ))
                    })?;
                    slice[y * w..(y + 1) * w].copy_from_slice(row);
                }
                let tight = Plane::new(buf.freeze(), w, h, w).map_err(|e| {
                    at!(EngineError::simple(
                        Stage::Execute,
                        Failure::BackendFailure,
                        "failed to build tight u8 plane",
                    )
                    .with_cause(e))
                })?;
                OwnedPlane::AccountedU8(tight)
            }
            AnyPlane::U16(p) => {
                let (w, h) = (p.width(), p.height());
                let total = w.checked_mul(h).ok_or_else(|| {
                    at!(EngineError::simple(
                        Stage::Execute,
                        Failure::ResourceLimit,
                        "u16 plane size overflow",
                    ))
                })?;
                let mut buf = storage.filled(total, 0u16).map_err(|e| {
                    at!(EngineError::simple(
                        Stage::Execute,
                        Failure::ResourceLimit,
                        "storage budget exhausted repacking u16 plane",
                    )
                    .with_cause(e))
                })?;
                let slice = buf.as_mut_slice();
                for y in 0..h {
                    let row = p.row(y).ok_or_else(|| {
                        at!(EngineError::simple(
                            Stage::Execute,
                            Failure::BackendFailure,
                            "decoded u16 plane row out of bounds",
                        ))
                    })?;
                    slice[y * w..(y + 1) * w].copy_from_slice(row);
                }
                let tight = Plane::new(buf.freeze(), w, h, w).map_err(|e| {
                    at!(EngineError::simple(
                        Stage::Execute,
                        Failure::BackendFailure,
                        "failed to build tight u16 plane",
                    )
                    .with_cause(e))
                })?;
                OwnedPlane::AccountedU16(tight)
            }
            AnyPlane::F32(p) => {
                let (w, h) = (p.width(), p.height());
                let total = w.checked_mul(h).ok_or_else(|| {
                    at!(EngineError::simple(
                        Stage::Execute,
                        Failure::ResourceLimit,
                        "f32 plane size overflow",
                    ))
                })?;
                let mut buf = storage.filled(total, 0.0f32).map_err(|e| {
                    at!(EngineError::simple(
                        Stage::Execute,
                        Failure::ResourceLimit,
                        "storage budget exhausted repacking f32 plane",
                    )
                    .with_cause(e))
                })?;
                let slice = buf.as_mut_slice();
                for y in 0..h {
                    let row = p.row(y).ok_or_else(|| {
                        at!(EngineError::simple(
                            Stage::Execute,
                            Failure::BackendFailure,
                            "decoded f32 plane row out of bounds",
                        ))
                    })?;
                    slice[y * w..(y + 1) * w].copy_from_slice(row);
                }
                let tight = Plane::new(buf.freeze(), w, h, w).map_err(|e| {
                    at!(EngineError::simple(
                        Stage::Execute,
                        Failure::BackendFailure,
                        "failed to build tight f32 plane",
                    )
                    .with_cause(e))
                })?;
                OwnedPlane::AccountedF32(tight)
            }
            _ => {
                return Err(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::UnsupportedEffect,
                    "unsupported plane sample word type in repack",
                )));
            }
        };
        encoded_planes.push((owned, spec.encoding()));
    }
    let pic = OwnedPlanes::from_encoded_planes(encoded_planes).map_err(|e| {
        at!(EngineError::simple(
            Stage::Execute,
            Failure::BackendFailure,
            "OwnedPlanes construction failed",
        )
        .with_cause(e))
    })?;
    Ok(Frame::new(Arc::new(pic), interp, timing, pres))
}

/// Drive a prepared [`MediaPump`] in memory over a complete byte slice (supporting arbitrary
/// chunk sizes and random-access range requests).
///
/// Derives publication mode, output limits, target container, and codec registry directly
/// from `pump` as the single source of truth.
pub fn drive_pump_in_memory(
    pump: &mut MediaPump,
    source_bytes: &[u8],
    max_chunk: usize,
) -> EngineResult<PublishedMedia> {
    let chunk_cap = max_chunk.max(1);
    let total_len = source_bytes.len() as u64;
    if let Some(max_in) = pump.effective.resources.limits().max_input_bytes
        && total_len > max_in
    {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::ResourceLimit,
            "media input length exceeds max_input_bytes",
        )));
    }
    let max_out_cap = {
        let out_lim = pump
            .effective
            .resources
            .limits()
            .max_output_bytes
            .and_then(|v| usize::try_from(v).ok());
        let mem_lim = pump
            .effective
            .resources
            .limits()
            .max_memory_bytes
            .and_then(|v| usize::try_from(v).ok());
        match (out_lim, mem_lim) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => 64 * 1024 * 1024,
        }
    };
    let target_container = pump.target_container.clone();
    let publication = pump.intent.publication;
    let mut out_buf = PublicationBuffer::new(
        PublicationMode::Incremental,
        max_out_cap,
        pump.effective.resources.storage().clone(),
    )?;
    let mut moved_verified: Option<Vec<u8>> = None;

    loop {
        if let Some(verified_bytes) = pump.take_ready_verified_payload() {
            moved_verified = Some(verified_bytes);
        }
        match pump.advance()? {
            PumpEvent::NeedInput { ticket, requests } => {
                let req = requests
                    .first()
                    .copied()
                    .unwrap_or(InputRequest::at_least(0, 1));
                let (offset, max_take) = match req {
                    InputRequest::AtLeast { offset, min_len } => (offset, chunk_cap.max(min_len)),
                    InputRequest::Range(r) => {
                        let len = usize::try_from(r.length).unwrap_or(usize::MAX);
                        (r.offset, len)
                    }
                    _ => (0, chunk_cap),
                };
                let start = usize::try_from(offset).unwrap_or(source_bytes.len());
                if start >= source_bytes.len() {
                    let _ = pump
                        .supply_input(ticket, Input::new(offset, &[]).with_total_len(total_len))?;
                } else {
                    let avail = source_bytes.len() - start;
                    let take = avail.min(max_take);
                    let slice = &source_bytes[start..start + take];
                    let inp = Input::new(offset, slice).with_total_len(total_len);
                    let _ = pump.supply_input(ticket, inp)?;
                }
            }
            PumpEvent::NeedWriter { ticket, write } => match write {
                WriteOffer::Append(chunk) => {
                    let len = chunk.len();
                    out_buf.append(chunk)?;
                    pump.confirm_written(ticket, len)?;
                }
                WriteOffer::Patch { offset, bytes } => {
                    let patch_len = bytes.len();
                    out_buf.apply_patch(offset, bytes)?;
                    pump.confirm_written(ticket, patch_len)?;
                }
            },
            PumpEvent::Presented => {}
            PumpEvent::Finished => break,
        }
    }

    let verified = match publication {
        PublicationMode::Verified => {
            let receipt = pump.verified_receipt().cloned().ok_or_else(|| {
                at!(EngineError::simple(
                    Stage::Verify,
                    Failure::CorruptOutput,
                    "Verified MediaPump finished without a verification receipt",
                ))
            })?;
            Some(receipt)
        }
        PublicationMode::Incremental => None,
    };

    if let Some(bytes) = moved_verified {
        return Ok(PublishedMedia {
            container: target_container,
            committed_bytes: pump.committed_bytes(),
            bytes,
            verified,
        });
    }
    let mut published = out_buf.finish(
        &target_container,
        &pump.engine,
        &mut pump.effective,
        &mut pump.report,
    )?;
    published.committed_bytes = pump.committed_bytes();
    published.verified = verified;
    Ok(published)
}

/// Extract a single decoded RGB/RGBA poster [`PixelBuf`] from a video container (MP4/WebM/IVF)
/// by presentation index or presentation timestamp using `PresentationMap` + `VideoRenderPlan`.
///
/// Enforces `max_input_bytes`, `max_frames`, and `max_memory_bytes`, rejects non-single
/// selections (`All`, `Page`) upfront, and retains only the single winning frame in memory.
pub fn extract_poster_from_bytes(
    engine: &Engine,
    bytes: &[u8],
    selection: DecodeSelection,
    output_format: PixelFormat,
    run: &Run<'_>,
) -> EngineResult<Completion<(PixelBuf, Timestamp, u64)>> {
    // Reject non-single selections upfront before demuxing or decoding anything.
    let requested_track = match selection {
        DecodeSelection::PresentationIndex { track, .. } => track,
        DecodeSelection::PresentationTime { track, .. } => track,
        DecodeSelection::All => {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::InvalidRequest,
                "extract_poster_from_bytes requires PresentationIndex or PresentationTime (not All)",
            )));
        }
        DecodeSelection::Page(_) => {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::InvalidRequest,
                "DecodeSelection::Page is not valid for video poster extraction",
            )));
        }
    };

    let mut eff = engine.resolve_run(run)?;
    eff.check_stop(Stage::Inspect)?;
    if let Some(max_in) = eff.resources.limits().max_input_bytes
        && (bytes.len() as u64) > max_in
    {
        return Err(at!(EngineError::simple(
            Stage::Inspect,
            Failure::ResourceLimit,
            "media input length exceeds max_input_bytes",
        )));
    }
    let max_frames = eff.resources.limits().max_frames.unwrap_or(1_000_000) as usize;
    let max_pkt_bytes = eff
        .resources
        .limits()
        .max_memory_bytes
        .unwrap_or(64 * 1024 * 1024);

    let mut report = RouteReport {
        route: Some(RouteKind::Media),
        ..Default::default()
    };

    let container = crate::media::probe::probe_container_bytes(
        engine.codecs(),
        bytes,
        &zencodec_media::registry::ProbeLimits::default(),
    )?
    .ok_or_else(|| {
        at!(EngineError::simple(
            Stage::Inspect,
            Failure::UnrecognizedFormat,
            "bytes did not match a recognized video container",
        ))
    })?
    .0;

    let (demux_id, demux_factory) = select_demuxer(engine, &container, &mut eff, &mut report)?;
    let total_len = bytes.len() as u64;
    let access = SourceAccess::borrowed(total_len);
    let demux_offer = demux_factory
        .quote(&DemuxNeeds::default(), &access, &eff.resources)
        .map_err(|e| {
            at!(EngineError::simple(
                Stage::Prepare,
                Failure::UnsupportedEffect,
                "demux quote failed"
            )
            .with_cause(e))
        })?;
    eff.admit_workspace(demux_id.as_str(), demux_offer.workspace, &mut report)?;
    let mut demux = demux_factory
        .open(&demux_offer, &eff.resources)
        .map_err(|e| {
            at!(
                EngineError::simple(Stage::Prepare, Failure::BackendFailure, "demux open failed")
                    .with_cause(e)
            )
        })?;

    // Stream demux -> decode directly with O(1) frame memory and O(1) online selection
    // (zero growing PresentationEntry Vec and zero quadratic prefix cloning)!
    let mut offset: usize = 0;
    let mut video_track_idx: Option<usize> = None;
    let mut dec: Option<Box<LocalVideoDecoder>> = None;
    let mut timeline: Option<TimelineBridge> = None;
    let mut fallback_tb = TimeBase::new(1, 1000).expect("valid 1/1000 clock");
    let mut frames_seen: u64 = 0;
    let mut best_frame: Option<(u64, Timestamp, Frame)> = None;
    let mut packet_bytes_seen: u64 = 0;
    let mut demux_done = false;
    let mut early_found = false;

    let mut collect_ready = |tl: &mut TimelineBridge,
                             tb: TimeBase,
                             frames_seen: &mut u64,
                             best_frame: &mut Option<(u64, Timestamp, Frame)>,
                             early_found: &mut bool|
     -> EngineResult<()> {
        for item in tl.drain_ready() {
            if let MappedPresentation::Frame(mf) = item {
                if *frames_seen >= max_frames as u64 {
                    return Err(at!(EngineError::simple(
                        Stage::Execute,
                        Failure::ResourceLimit,
                        "video stream frame count exceeded max_frames",
                    )));
                }
                let ord = *frames_seen;
                *frames_seen = ord.saturating_add(1);
                let pts = mf
                    .timing()
                    .pts
                    .unwrap_or_else(|| Timestamp::new(ord as i64, tb));

                match selection {
                    DecodeSelection::PresentationIndex {
                        index: target_idx, ..
                    } => {
                        if ord == target_idx {
                            *best_frame = Some((ord, pts, mf));
                            *early_found = true;
                            break;
                        }
                    }
                    DecodeSelection::PresentationTime { time, mode, .. } => match mode {
                        SeekMode::AtOrBefore => {
                            if cmp_timestamp_exact(pts, time) != core::cmp::Ordering::Greater {
                                *best_frame = Some((ord, pts, mf));
                            }
                        }
                        SeekMode::AtOrAfter => {
                            if cmp_timestamp_exact(pts, time) != core::cmp::Ordering::Less {
                                *best_frame = Some((ord, pts, mf));
                                *early_found = true;
                                break;
                            }
                        }
                        SeekMode::Nearest => {
                            let replace = match best_frame {
                                None => true,
                                Some((_, best_pts, _)) => {
                                    is_strictly_closer_to_target(pts, *best_pts, time)
                                }
                            };
                            if replace {
                                *best_frame = Some((ord, pts, mf));
                            }
                        }
                    },
                    _ => {}
                }
            }
        }
        Ok(())
    };

    while !demux_done && !early_found {
        eff.check_stop(Stage::Execute)?;
        let pulse: Option<&dyn zencodec::Pulse> = Some(eff.resources.pulse());
        let slice = if offset <= bytes.len() {
            &bytes[offset..]
        } else {
            &[]
        };
        let prog = demux
            .step(
                Input::new(offset as u64, slice).with_total_len(total_len),
                pulse,
            )
            .map_err(|e| {
                at!(
                    EngineError::simple(Stage::Execute, Failure::InvalidData, "demux step failed")
                        .with_cause(e)
                )
            })?;
        offset = offset.saturating_add(prog.consumed);
        match prog.event {
            DemuxEvent::Need => {
                if let Some(req) = demux.requested().first().copied() {
                    let target_off = req.offset().map(|o| o as usize).unwrap_or(offset);
                    if target_off == offset && offset >= bytes.len() {
                        break;
                    }
                    offset = target_off.min(bytes.len());
                } else {
                    break;
                }
            }
            DemuxEvent::Headers => {
                let tracks = demux.tracks().to_vec();
                let v_idx = match requested_track {
                    Some(t) => {
                        if tracks
                            .get(t as usize)
                            .is_none_or(|tr| tr.kind() != TrackKind::Video)
                        {
                            return Err(at!(EngineError::simple(
                                Stage::Admit,
                                Failure::InvalidRequest,
                                "requested track index is not a video track",
                            )));
                        }
                        t as usize
                    }
                    None => tracks
                        .iter()
                        .position(|t| t.kind() == TrackKind::Video)
                        .ok_or_else(|| {
                            at!(EngineError::simple(
                                Stage::Inspect,
                                Failure::InvalidData,
                                "media container has no video track",
                            ))
                        })?,
                };
                video_track_idx = Some(v_idx);
                let track = &tracks[v_idx];
                fallback_tb = track.time_base();
                let cfg = CodecConfig::from(track);
                let max_w = eff.resources.limits().max_width.unwrap_or(4_096);
                let max_h = eff.resources.limits().max_height.unwrap_or(4_096);
                let extent = track
                    .video_info()
                    .filter(|v| v.coded_width > 0 && v.coded_height > 0)
                    .map(|v| (v.coded_width, v.coded_height))
                    .unwrap_or((max_w.min(4_096), max_h.min(4_096)));
                let need = VideoDecodeNeeds::new(extent, 16);
                let (_dec_id, dec_factory, dec_plan) =
                    select_video_decoder(engine, &cfg, &need, &mut eff, &mut report)?;
                let mut d = dec_factory.open(&dec_plan, &eff.resources).map_err(|e| {
                    at!(EngineError::simple(
                        Stage::Prepare,
                        Failure::BackendFailure,
                        "video decoder open failed"
                    )
                    .with_cause(e))
                })?;
                let pulse: Option<&dyn zencodec::Pulse> = Some(eff.resources.pulse());
                d.configure(&cfg, pulse).map_err(|e| {
                    at!(EngineError::simple(
                        Stage::Prepare,
                        Failure::BackendFailure,
                        "decoder configure failed"
                    )
                    .with_cause(e))
                })?;
                dec = Some(d);
                timeline = Some(TimelineBridge::prepare(
                    track,
                    TimingEvidence::new(
                        TrimTreatment::ApplyInPresentation,
                        TrimTreatment::ApplyInPresentation,
                    ),
                    &eff.resources,
                )?);
            }
            DemuxEvent::Packet(mut pkt) => {
                if Some(pkt.track() as usize) != video_track_idx {
                    continue;
                }
                packet_bytes_seen = packet_bytes_seen.saturating_add(pkt.len() as u64);
                if packet_bytes_seen > max_pkt_bytes.saturating_mul(4) {
                    return Err(at!(EngineError::simple(
                        Stage::Execute,
                        Failure::ResourceLimit,
                        "demuxed packet bytes exceeded resource budget",
                    )));
                }
                let Some(d) = dec.as_mut() else { continue };
                let Some(tl) = timeline.as_mut() else {
                    continue;
                };
                let pkt_pts = pkt.pts();
                let pkt_discard = pkt.discard();
                let pulse: Option<&dyn zencodec::Pulse> = Some(eff.resources.pulse());
                loop {
                    match d.send(&mut pkt, pulse) {
                        Ok(SendStatus::Accepted) => {
                            tl.record_accepted_packet(pkt_pts, pkt_discard)?;
                            break;
                        }
                        Ok(SendStatus::ReceivePending) => {
                            drain_decoder_frames(
                                d,
                                tl,
                                fallback_tb,
                                &mut frames_seen,
                                &mut best_frame,
                                &mut early_found,
                                &mut collect_ready,
                                pulse,
                            )?;
                            if early_found {
                                break;
                            }
                        }
                        Ok(_) => break,
                        Err(e) => {
                            return Err(at!(EngineError::simple(
                                Stage::Execute,
                                Failure::BackendFailure,
                                "video decoder send failed",
                            )
                            .with_cause(e)));
                        }
                    }
                }
                if !early_found {
                    drain_decoder_frames(
                        d,
                        tl,
                        fallback_tb,
                        &mut frames_seen,
                        &mut best_frame,
                        &mut early_found,
                        &mut collect_ready,
                        pulse,
                    )?;
                }
            }
            DemuxEvent::Done => {
                demux_done = true;
            }
            _ => {}
        }
    }

    if !early_found && let (Some(d), Some(tl)) = (dec.as_mut(), timeline.as_mut()) {
        let pulse: Option<&dyn zencodec::Pulse> = Some(eff.resources.pulse());
        d.end_input(pulse).map_err(|e| {
            at!(EngineError::simple(
                Stage::Execute,
                Failure::BackendFailure,
                "decoder end_input failed"
            )
            .with_cause(e))
        })?;
        drain_decoder_frames(
            d,
            tl,
            fallback_tb,
            &mut frames_seen,
            &mut best_frame,
            &mut early_found,
            &mut collect_ready,
            pulse,
        )?;
        if !early_found {
            tl.finish()?;
            collect_ready(
                tl,
                fallback_tb,
                &mut frames_seen,
                &mut best_frame,
                &mut early_found,
            )?;
        }
    }

    if frames_seen == 0 {
        return Err(at!(EngineError::simple(
            Stage::Execute,
            Failure::InvalidData,
            "cannot select presentation from empty stream",
        )));
    }
    let (chosen_idx, chosen_pts, chosen_frame) = best_frame.ok_or_else(|| {
        at!(EngineError::simple(
            Stage::Execute,
            Failure::InvalidRequest,
            "no presentation matched requested DecodeSelection",
        ))
    })?;

    let buf = render_frame_to_pixel_buf(&chosen_frame, output_format, &eff)?;
    report.selected_primary = true;
    Ok(Completion {
        output: (buf, chosen_pts, chosen_idx),
        decisions: Box::new(zencodec::operation::OperationReport::default()),
        report,
        trace: eff.trace,
    })
}

#[allow(clippy::too_many_arguments)]
fn drain_decoder_frames<F>(
    dec: &mut Box<LocalVideoDecoder>,
    timeline: &mut TimelineBridge,
    fallback_tb: TimeBase,
    frames_seen: &mut u64,
    best_frame: &mut Option<(u64, Timestamp, Frame)>,
    early_found: &mut bool,
    collect_ready: &mut F,
    pulse: Option<&dyn zencodec::Pulse>,
) -> EngineResult<()>
where
    F: FnMut(
        &mut TimelineBridge,
        TimeBase,
        &mut u64,
        &mut Option<(u64, Timestamp, Frame)>,
        &mut bool,
    ) -> EngineResult<()>,
{
    loop {
        match dec.receive(pulse) {
            Ok(Receive::Ready(f)) => {
                timeline.push_frame(f)?;
                collect_ready(timeline, fallback_tb, frames_seen, best_frame, early_found)?;
                if *early_found {
                    break;
                }
            }
            Ok(Receive::Progress { .. }) => {}
            Ok(Receive::NeedInput | Receive::EndOfStream) => break,
            Ok(_) => {}
            Err(e) => {
                return Err(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::BackendFailure,
                    "video decoder receive failed",
                )
                .with_cause(e)));
            }
        }
    }
    Ok(())
}

/// Render one planar video [`Frame`] to a packed [`PixelBuf`] using `zencodec_render::media::VideoRenderPlan`
/// and the run's frozen [`ColorServices`](zenpixels_convert::ColorServices).
pub fn render_frame_to_pixel_buf(
    frame: &Frame,
    output_format: PixelFormat,
    eff: &EffectiveRun,
) -> EngineResult<PixelBuf> {
    let crop = frame.interpretation().geometry.crop;
    let (w, h) = (crop.width, crop.height);
    let bpp = output_format.layout.bytes_per_pixel();
    let stride = (w as usize).checked_mul(bpp).ok_or_else(|| {
        at!(EngineError::simple(
            Stage::Execute,
            Failure::ResourceLimit,
            "render stride overflow"
        ))
    })?;
    let total = stride.checked_mul(h as usize).ok_or_else(|| {
        at!(EngineError::simple(
            Stage::Execute,
            Failure::ResourceLimit,
            "render size overflow"
        ))
    })?;
    if let Some(max_mem) = eff.resources.limits().max_memory_bytes
        && (total as u64) > max_mem
    {
        return Err(at!(EngineError::simple(
            Stage::Execute,
            Failure::ResourceLimit,
            "rendered poster buffer exceeds max_memory_bytes",
        )));
    }
    let mut raw = Vec::new();
    raw.try_reserve_exact(total).map_err(|_| {
        at!(EngineError::simple(
            Stage::Execute,
            Failure::ResourceLimit,
            "render buffer allocation failed",
        ))
    })?;
    raw.resize(total, 0u8);
    let mut plan = VideoRenderPlan::new(output_format.clone())
        .with_reconstruction(
            PlanarReconstructionOptions::bilinear()
                .with_unknown_siting_fallback(ChromaSiting::Center),
        )
        .with_color_services(eff.color_services.clone())
        .with_storage(eff.resources.storage().clone());
    {
        let mut view_mut =
            PixelViewMut::new(&mut raw, w, h, stride, &output_format).map_err(|e| {
                at!(EngineError::simple(
                    Stage::Execute,
                    Failure::BackendFailure,
                    "output PixelViewMut failed"
                )
                .with_cause(e))
            })?;
        plan.render(frame, &mut view_mut).map_err(|e| {
            at!(EngineError::simple(
                Stage::Execute,
                Failure::BackendFailure,
                "VideoRenderPlan render failed"
            )
            .with_boxed_cause(e))
        })?;
    }
    PixelBuf::from_bytes(raw, w, h, output_format).map_err(|e| {
        at!(EngineError::simple(
            Stage::Execute,
            Failure::BackendFailure,
            "PixelBuf creation failed"
        )
        .with_cause(e))
    })
}

fn gcd_u128(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a.max(1)
}

/// Compute exact rational common clock (`1 / lcm_den`) for `durations` under `TimingPolicy::Exact`.
fn exact_common_time_base(durations: &[FrameDuration]) -> EngineResult<TimeBase> {
    let mut lcm_den: u128 = 1;
    for d in durations {
        let num = u128::from(d.numerator());
        let den = u128::from(d.denominator());
        let g = gcd_u128(num, den);
        let red_den = den / g;
        let lg = gcd_u128(lcm_den, red_den);
        lcm_den = (lcm_den / lg).checked_mul(red_den).ok_or_else(|| {
            at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                "exact rational timeline denominator overflowed u128",
            ))
        })?;
        if lcm_den > u128::from(u32::MAX) {
            return Err(at!(EngineError::simple(
                Stage::Admit,
                Failure::UnsupportedEffect,
                "exact rational timeline common denominator exceeds TimeBase u32 range",
            )));
        }
    }
    let den_u32 = (lcm_den as u32).max(1);
    TimeBase::new(1, den_u32).map_err(|_| {
        at!(EngineError::simple(
            Stage::Admit,
            Failure::UnsupportedEffect,
            "failed to construct exact rational TimeBase",
        ))
    })
}

/// Encode a sequence of packed RGB/RGBA frames with exact [`FrameDuration`]s into a video
/// container (`MP4`, `WebM`, or `IVF`) using `VideoCapturePlan` -> `VideoEncodeRole` -> `MuxRole`.
///
/// Enforces [`normalize_loop_for_target`], exact rational timing (or bounded endpoint
/// quantization via [`quantize_endpoints`]), video fidelity forwarding, and complete container
/// readback under [`PublicationMode::Verified`].
pub fn encode_frames_to_video(
    engine: &Engine,
    frames: &[(PixelView<'_>, FrameDuration)],
    source_loops: LoopCount,
    intent: &OutputIntent,
    run: &Run<'_>,
) -> EngineResult<Completion<Encoded>> {
    intent.validate()?;
    let TargetFormat::Video {
        container: target_container,
        codec: target_codec,
    } = &intent.target
    else {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::InvalidRequest,
            "encode_frames_to_video requires TargetFormat::Video",
        )));
    };
    validate_container_codec_tuple(target_container, target_codec)?;
    let mut aim_report = RouteReport::default();
    let requested_fidelity = video_fidelity_from_coding(&intent.coding, &mut aim_report)?;
    if frames.is_empty() {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::InvalidRequest,
            "encode_frames_to_video requires at least one frame",
        )));
    }
    let (timing_policy, loop_policy) = match intent.sequence {
        SequencePolicy::Preserve { timing, loops } => (timing, loops),
        SequencePolicy::FirstPresentation => {
            (TimingPolicy::Exact, LoopPolicy::Set(LoopCount::once()))
        }
    };
    let _ = normalize_loop_for_target(source_loops, loop_policy, target_container)?;

    let mut eff = engine.resolve_run(run)?;
    eff.check_stop(Stage::Prepare)?;
    let mut report = RouteReport {
        route: Some(RouteKind::Media),
        output_container: Some(target_container.clone()),
        output_video_codec: Some(target_codec.clone()),
        aim_metric: aim_report.aim_metric,
        aim_target: aim_report.aim_target,
        aim_revision: aim_report.aim_revision,
        aim_estimated_quality: aim_report.aim_estimated_quality,
        ..Default::default()
    };

    let frame_slice = match intent.sequence {
        SequencePolicy::FirstPresentation => &frames[..1],
        SequencePolicy::Preserve { .. } => frames,
    };
    if let Some(max_f) = eff.resources.limits().max_frames
        && frame_slice.len() > max_f as usize
    {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::ResourceLimit,
            "frame count exceeds max_frames",
        )));
    }

    // Resolve exact clock and tick durations BEFORE any encoding or muxing!
    let durs: Vec<FrameDuration> = frame_slice.iter().map(|(_, d)| *d).collect();
    let clock = match timing_policy {
        TimingPolicy::Exact => exact_common_time_base(&durs)?,
        TimingPolicy::QuantizeEndpoints { .. } => {
            TimeBase::new(1, 1000).expect("valid 1/1000 clock")
        }
    };
    let quantized = quantize_endpoints(&durs, clock, timing_policy)?;

    let (w, h) = (frame_slice[0].0.width(), frame_slice[0].0.height());

    // Standard 8-bit 4:2:0 BT.709 limited-range planar capture target (supported by AV1, VP8, and VP9).
    let y_fmt = PlaneFormat::new(zenpixels::PlaneRole::Y, SampleType::U8);
    let mut cb_fmt = PlaneFormat::new(zenpixels::PlaneRole::Cb, SampleType::U8);
    cb_fmt.subsampling = (2, 2);
    let mut cr_fmt = PlaneFormat::new(zenpixels::PlaneRole::Cr, SampleType::U8);
    cr_fmt.subsampling = (2, 2);
    let signal = Cicp::new(1, 1, 1, false);
    let mut capture = VideoCapturePlan::new(
        [y_fmt, cb_fmt, cr_fmt],
        ChromaSiting::TopLeft,
        signal,
        eff.resources.storage().clone(),
    )
    .with_color_services(eff.color_services.clone());

    let enc8 = SampleEncoding::new(SampleType::U8, 8, 0).expect("valid u8");
    let planar_layout = PlanarLayout::new(
        vec![
            PlaneSpec::new(PlaneRole::Luma, PlaneEncoding::Code(enc8)),
            PlaneSpec::new(PlaneRole::Cb, PlaneEncoding::Code(enc8)),
            PlaneSpec::new(PlaneRole::Cr, PlaneEncoding::Code(enc8)),
        ],
        ChromaSubsampling::Yuv420,
        ChromaSiting::TopLeft,
    )
    .map_err(|e| {
        at!(EngineError::simple(
            Stage::Prepare,
            Failure::InvalidRequest,
            "PlanarLayout construction failed",
        )
        .with_cause(e))
    })?;
    let interp = Arc::new(
        FrameInterpretation::new(FrameGeometry::full(w, h), planar_layout)
            .with_color(ColorClaim::new(Some(signal), Some(signal), Some(signal))),
    );
    let mut enc_need = VideoEncodeNeeds::new(target_codec.clone(), Arc::clone(&interp), clock, 16);
    if let Some(fid) = requested_fidelity {
        enc_need = enc_need.with_fidelity(fid);
    }
    let (_enc_id, enc_factory, enc_plan) =
        select_video_encoder(engine, &enc_need, &mut eff, &mut report)?;
    let mut enc = enc_factory.open(&enc_plan, &eff.resources).map_err(|e| {
        at!(EngineError::simple(
            Stage::Prepare,
            Failure::BackendFailure,
            "video encoder open failed"
        )
        .with_cause(e))
    })?;

    let mut video_track = VideoTrackInfo::new(w, h);
    video_track.color = Some(signal);
    let cap_cfg = CodecConfig::new(target_codec.clone(), clock).with_video(video_track);
    let pulse: Option<&dyn zencodec::Pulse> = Some(eff.resources.pulse());

    let mut mux_packets: Vec<Packet> = Vec::new();
    let mut pts_ticks: i64 = 0;

    for ((view, _), &dur_ticks) in frame_slice.iter().zip(quantized.durations.iter()) {
        eff.check_stop(Stage::Execute)?;
        if (view.width(), view.height()) != (w, h) {
            return Err(at!(EngineError::simple(
                Stage::Execute,
                Failure::InvalidRequest,
                "all video frames must share identical canvas dimensions",
            )));
        }
        let timing =
            FrameTiming::at(Timestamp::new(pts_ticks, clock)).with_duration_ticks(dur_ticks);
        let dur_i64 = i64::try_from(dur_ticks).map_err(|_| {
            at!(EngineError::simple(
                Stage::Execute,
                Failure::ResourceLimit,
                "frame tick duration overflowed i64",
            ))
        })?;
        pts_ticks = pts_ticks.checked_add(dur_i64).ok_or_else(|| {
            at!(EngineError::simple(
                Stage::Execute,
                Failure::ResourceLimit,
                "cumulative presentation timestamp overflowed i64",
            ))
        })?;

        let frame = capture.capture(view, &cap_cfg, timing).map_err(|e| {
            at!(EngineError::simple(
                Stage::Execute,
                Failure::BackendFailure,
                "VideoCapturePlan capture failed"
            )
            .with_boxed_cause(e))
        })?;
        loop {
            match enc.send(&frame, pulse) {
                Ok(SendStatus::Accepted) => break,
                Ok(SendStatus::ReceivePending) => {
                    drain_enc_packets(&mut enc, &mut mux_packets, pulse)?;
                }
                Ok(_) => break,
                Err(e) => {
                    return Err(at!(EngineError::simple(
                        Stage::Execute,
                        Failure::BackendFailure,
                        "video encoder send failed",
                    )
                    .with_cause(e)));
                }
            }
        }
        drain_enc_packets(&mut enc, &mut mux_packets, pulse)?;
    }

    enc.end_input(pulse).map_err(|e| {
        at!(EngineError::simple(
            Stage::Execute,
            Failure::BackendFailure,
            "video encoder end_input failed"
        )
        .with_cause(e))
    })?;
    loop {
        match enc.receive(pulse) {
            Ok(Receive::Ready(pkt)) => mux_packets.push(pkt),
            Ok(Receive::Progress { .. }) => {}
            Ok(Receive::EndOfStream | Receive::NeedInput) => break,
            Ok(_) => {}
            Err(e) => {
                return Err(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::BackendFailure,
                    "video encoder drain failed",
                )
                .with_cause(e)));
            }
        }
    }

    let out_cfg = enc.output_config().cloned().unwrap_or_else(|| {
        CodecConfig::new(target_codec.clone(), clock).with_video(VideoTrackInfo::new(w, h))
    });
    let mut tb = TrackInfo::video(
        0,
        out_cfg.codec.clone(),
        out_cfg.time_base,
        out_cfg.video.unwrap_or_else(|| VideoTrackInfo::new(w, h)),
    );
    if let Some(p) = &out_cfg.private {
        tb = tb.with_codec_private(Arc::clone(p));
    }
    let track = tb.build().map_err(|e| {
        at!(EngineError::simple(
            Stage::Prepare,
            Failure::InvalidData,
            "TrackInfo build failed"
        )
        .with_cause(e))
    })?;
    let tracks: Arc<[TrackInfo]> = Arc::from(vec![track]);
    let media_timing = match timing_policy {
        TimingPolicy::Exact => MediaTimingPolicy::Exact,
        TimingPolicy::QuantizeEndpoints { .. } => MediaTimingPolicy::BestEffort,
    };
    let (_mux_id, mux_factory, mux_plan) = select_muxer(
        engine,
        target_container,
        Arc::clone(&tracks),
        intent.publication,
        media_timing,
        &mut eff,
        &mut report,
    )?;
    let pulse: Option<&dyn zencodec::Pulse> = Some(eff.resources.pulse());
    let mut mux = mux_factory.open(&mux_plan, &eff.resources).map_err(|e| {
        at!(
            EngineError::simple(Stage::Prepare, Failure::BackendFailure, "muxer open failed")
                .with_cause(e)
        )
    })?;
    let hint = match intent.publication {
        PublicationMode::Incremental => MuxHint::forward_only(),
        PublicationMode::Verified => MuxHint::default(),
    };
    match media_timing {
        MediaTimingPolicy::Exact => mux.begin_exact_timing(&tracks, hint),
        _ => mux.begin(&tracks, hint),
    }
    .map_err(|e| {
        at!(EngineError::simple(
            Stage::Prepare,
            Failure::BackendFailure,
            "muxer begin failed"
        )
        .with_cause(e))
    })?;

    let max_out = {
        let out_lim = eff
            .resources
            .limits()
            .max_output_bytes
            .and_then(|v| usize::try_from(v).ok());
        let mem_lim = eff
            .resources
            .limits()
            .max_memory_bytes
            .and_then(|v| usize::try_from(v).ok());
        match (out_lim, mem_lim) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => 64 * 1024 * 1024,
        }
    };
    let mut pub_buf =
        PublicationBuffer::new(intent.publication, max_out, eff.resources.storage().clone())?;

    let drain_pending = |m: &mut dyn DynMuxer, pb: &mut PublicationBuffer| -> EngineResult<()> {
        if !m.pending_output().is_empty() {
            let mut tmp = Vec::new();
            m.take_output(&mut tmp);
            pb.append(&tmp)?;
        }
        Ok(())
    };
    drain_pending(mux.as_mut(), &mut pub_buf)?;

    for pkt in mux_packets {
        eff.check_stop(Stage::Execute)?;
        let mut slot = Some(pkt);
        if let Some(bounded) = mux.as_bounded_mut() {
            loop {
                match bounded.try_write_packet(&mut slot, pulse) {
                    Ok(PacketWrite::Accepted) => break,
                    Ok(PacketWrite::DrainOutput) => {
                        let mut tmp = Vec::new();
                        bounded.take_output(&mut tmp);
                        pub_buf.append(&tmp)?;
                    }
                    Ok(_) => break,
                    Err(e) => {
                        return Err(at!(EngineError::simple(
                            Stage::Execute,
                            Failure::BackendFailure,
                            "bounded mux write failed",
                        )
                        .with_cause(e)));
                    }
                }
            }
        } else if let Some(p) = slot.take() {
            mux.write_packet(p, pulse).map_err(|e| {
                at!(EngineError::simple(
                    Stage::Execute,
                    Failure::BackendFailure,
                    "mux write failed"
                )
                .with_cause(e))
            })?;
        }
        drain_pending(mux.as_mut(), &mut pub_buf)?;
    }

    let finished = mux.finish(pulse).map_err(|e| {
        at!(
            EngineError::simple(Stage::Publish, Failure::BackendFailure, "mux finish failed")
                .with_cause(e)
        )
    })?;
    if !finished.tail.is_empty() {
        pub_buf.append(&finished.tail)?;
    }
    for patch in finished.patches {
        pub_buf.apply_patch(patch.offset, &patch.bytes)?;
    }

    let published = pub_buf.finish(target_container, engine, &mut eff, &mut report)?;
    Ok(Completion {
        output: Encoded::container(published.bytes, published.container),
        decisions: Box::new(zencodec::operation::OperationReport::default()),
        report,
        trace: eff.trace,
    })
}

fn drain_enc_packets(
    enc: &mut Box<LocalVideoEncoder>,
    out: &mut Vec<Packet>,
    pulse: Option<&dyn zencodec::Pulse>,
) -> EngineResult<()> {
    loop {
        match enc.receive(pulse) {
            Ok(Receive::Ready(pkt)) => out.push(pkt),
            Ok(Receive::Progress { .. }) => {}
            Ok(Receive::NeedInput | Receive::EndOfStream) => break,
            Ok(_) => {}
            Err(e) => {
                return Err(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::BackendFailure,
                    "video encoder receive failed",
                )
                .with_cause(e)));
            }
        }
    }
    Ok(())
}

/// High-level convenience: transcode a media container byte slice to a target video container+codec
/// via the sans-I/O [`MediaPump`] and [`drive_pump_in_memory`].
pub fn transcode_media_bytes(
    engine: &Engine,
    bytes: &[u8],
    intent: &OutputIntent,
    run: &Run<'_>,
) -> EngineResult<Completion<Encoded>> {
    let source_container = crate::media::probe::probe_container_bytes(
        engine.codecs(),
        bytes,
        &zencodec_media::registry::ProbeLimits::default(),
    )?
    .ok_or_else(|| {
        at!(EngineError::simple(
            Stage::Inspect,
            Failure::UnrecognizedFormat,
            "input bytes did not match a registered media container",
        ))
    })?
    .0;

    let access = SourceAccess::borrowed(bytes.len() as u64);
    let mut pump = MediaPump::prepare_transcode(
        engine.clone(),
        source_container,
        &access,
        intent.clone(),
        run,
    )?;

    let published = drive_pump_in_memory(&mut pump, bytes, 64 * 1024)?;

    Ok(Completion {
        output: Encoded::container(published.bytes, published.container),
        decisions: Box::new(zencodec::operation::OperationReport::default()),
        report: pump.report.clone(),
        trace: pump.effective.trace,
    })
}
