//! Media output publication modes (`Verified` private staging + admitted readback vs `Incremental` live prefix).

use alloc::vec::Vec;
use whereat::at;
use zencodec::decode::{Input, InputRequest};
use zencodec::source::SourceAccess;
use zencodec::storage::{PreparedStorage, StorageBudget};
use zencodec_media::container::DemuxEvent;
use zencodec_media::registry::{ContainerId, DemuxNeeds, ProbeLimits};

use crate::engine::{
    EffectiveRun, Engine, EngineError, EngineResult, Failure, PublicationMode, RouteReport, Stage,
};
use crate::media::probe::{ContainerProbeOutcome, probe_container_prefix};
use crate::media::route::select_demuxer;

/// Verification summary produced before publishing a [`PublicationMode::Verified`] container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedCheck {
    /// Confirmed container identity from bounded readback probe.
    pub confirmed_container: ContainerId,
    /// Total verified byte length.
    pub byte_len: u64,
    /// Total packets confirmed readable during structural verification.
    pub verified_packets: u64,
}

/// Completed published media artifact.
#[derive(Clone, Debug)]
pub struct PublishedMedia {
    /// Emitted container identity.
    pub container: ContainerId,
    /// Owned output bytes.
    pub bytes: Vec<u8>,
    /// Verification receipt when [`PublicationMode::Verified`] ran.
    pub verified: Option<VerifiedCheck>,
    /// Total bytes committed incrementally when [`PublicationMode::Incremental`] ran.
    pub committed_bytes: u64,
}

/// Bounded, fallible, [`StorageBudget`]-accounted in-memory staging / patchable output buffer
/// honoring `max_output_bytes`, `max_memory_bytes`, and the active run's `StorageBudget`.
///
/// Accounts for peak simultaneous live capacity during any growth reallocation (`old_cap + new_cap`)
/// against BOTH `max_bytes` and `StorageBudget`, and transfers the finalized `Vec<u8>` by move
/// with zero handoff copy.
pub struct PublicationBuffer {
    mode: PublicationMode,
    max_bytes: usize,
    storage: StorageBudget,
    budget_hold: Option<PreparedStorage<u8>>,
    bytes: Vec<u8>,
    committed_bytes: u64,
}

impl core::fmt::Debug for PublicationBuffer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PublicationBuffer")
            .field("mode", &self.mode)
            .field("max_bytes", &self.max_bytes)
            .field("len", &self.bytes.len())
            .field("capacity", &self.bytes.capacity())
            .field("committed_bytes", &self.committed_bytes)
            .finish()
    }
}

impl PublicationBuffer {
    /// Create a new publication buffer with `mode`, hard byte cap `max_bytes`, and `storage`.
    pub fn new(mode: PublicationMode, max_bytes: usize, storage: StorageBudget) -> EngineResult<Self> {
        let cap = max_bytes.min(storage.max_bytes());
        Ok(Self {
            mode,
            max_bytes: cap,
            storage,
            budget_hold: None,
            bytes: Vec::new(),
            committed_bytes: 0,
        })
    }

    /// Configured publication mode.
    #[must_use]
    pub const fn mode(&self) -> PublicationMode {
        self.mode
    }

    /// Bytes committed externally so far (always `0` before verification in `Verified` mode).
    #[must_use]
    pub const fn committed_bytes(&self) -> u64 {
        self.committed_bytes
    }

    /// Append bytes, checking peak simultaneous live allocation (`old_cap + new_cap`) against
    /// BOTH `max_bytes` and `StorageBudget` before growing.
    pub fn append(&mut self, chunk: &[u8]) -> EngineResult<()> {
        let next = self.bytes.len().checked_add(chunk.len()).ok_or_else(|| {
            at!(EngineError::simple(
                Stage::Publish,
                Failure::ResourceLimit,
                "output buffer length overflow",
            )
            .with_committed_bytes(self.committed_bytes))
        })?;
        if next > self.max_bytes {
            return Err(at!(EngineError::simple(
                Stage::Publish,
                Failure::ResourceLimit,
                "encoded media output exceeds staging/output byte limit",
            )
            .with_committed_bytes(self.committed_bytes)));
        }
        if next > self.bytes.capacity() {
            let old_cap = self.bytes.capacity();
            // For small bounded staging caps (<= 64 KiB) allocate `max_bytes` once on first
            // append so subsequent appends never reallocate or hold two simultaneous buffers.
            let target_cap = if old_cap == 0 && self.max_bytes <= 65_536 {
                self.max_bytes
            } else {
                let remaining_peak = self.max_bytes.saturating_sub(old_cap);
                if remaining_peak < next {
                    return Err(at!(EngineError::simple(
                        Stage::Publish,
                        Failure::ResourceLimit,
                        "peak simultaneous staging growth capacity exceeds memory/output limit",
                    )
                    .with_committed_bytes(self.committed_bytes)));
                }
                old_cap
                    .saturating_mul(2)
                    .max(next)
                    .min(remaining_peak)
            };
            // Reserve `target_cap` from `StorageBudget` WHILE `self.budget_hold` (holding `old_cap`)
            // is STILL LIVE so simultaneous `old_cap + target_cap` peak is enforced!
            let new_hold = self.storage.prepare_filled::<u8>(target_cap, 0).map_err(|e| {
                at!(EngineError::simple(
                    Stage::Publish,
                    Failure::ResourceLimit,
                    "staged media buffer growth exceeded StorageBudget",
                )
                .with_committed_bytes(self.committed_bytes)
                .with_cause(e))
            })?;
            let extra = target_cap.saturating_sub(self.bytes.len());
            self.bytes.try_reserve_exact(extra).map_err(|_| {
                at!(EngineError::simple(
                    Stage::Publish,
                    Failure::ResourceLimit,
                    "staged media buffer fallible allocation failed",
                )
                .with_committed_bytes(self.committed_bytes))
            })?;
            let actual_cap = self.bytes.capacity();
            if actual_cap > target_cap || old_cap.saturating_add(actual_cap) > self.max_bytes {
                return Err(at!(EngineError::simple(
                    Stage::Publish,
                    Failure::ResourceLimit,
                    "allocator rounded staging buffer capacity beyond admitted cap",
                )
                .with_committed_bytes(self.committed_bytes)));
            }
            // Drop old reservation now that old backing storage has been freed by reallocation.
            self.budget_hold = Some(new_hold);
        }
        self.bytes.extend_from_slice(chunk);
        if self.mode == PublicationMode::Incremental {
            self.committed_bytes = self.bytes.len() as u64;
        }
        Ok(())
    }

    /// Apply a seek-back patch (only legal in `Verified` / seekable staging, never on forward incremental).
    pub fn apply_patch(&mut self, offset: u64, patch: &[u8]) -> EngineResult<()> {
        if self.mode == PublicationMode::Incremental {
            return Err(at!(EngineError::simple(
                Stage::Publish,
                Failure::UnsupportedEffect,
                "cannot apply seek-back container patch in Incremental forward publication mode",
            )
            .with_committed_bytes(self.committed_bytes)));
        }
        let start = usize::try_from(offset).map_err(|_| {
            at!(EngineError::simple(
                Stage::Publish,
                Failure::ResourceLimit,
                "patch offset exceeds addressable memory",
            ))
        })?;
        let end = start.checked_add(patch.len()).ok_or_else(|| {
            at!(EngineError::simple(
                Stage::Publish,
                Failure::ResourceLimit,
                "patch range overflow",
            ))
        })?;
        if end > self.bytes.len() {
            return Err(at!(EngineError::simple(
                Stage::Verify,
                Failure::CorruptOutput,
                "container patch range exceeds staged output length",
            )));
        }
        self.bytes[start..end].copy_from_slice(patch);
        Ok(())
    }

    /// Verify (when `Verified`) and finalize the published media payload, returning the live
    /// [`StorageBudget`] reservation alongside the payload so streaming drivers (like `MediaPump`)
    /// can hold the budget charge until the writer finishes acknowledging the payload.
    pub(crate) fn finish_retained(
        mut self,
        expected_container: &ContainerId,
        engine: &Engine,
        eff: &mut EffectiveRun,
        report: &mut RouteReport,
    ) -> EngineResult<(PublishedMedia, Option<PreparedStorage<u8>>)> {
        let staged: &[u8] = &self.bytes;
        let verified = if self.mode == PublicationMode::Verified {
            if staged.is_empty() {
                return Err(at!(EngineError::simple(
                    Stage::Verify,
                    Failure::CorruptOutput,
                    "muxer finished with zero output bytes",
                )));
            }
            // Select and admit readback demuxer under the active Engine + EffectiveRun ceiling!
            let mut verify_report = report.clone();
            let (demux_id, demux_factory) =
                select_demuxer(engine, expected_container, eff, &mut verify_report)?;
            let total_len = staged.len() as u64;
            let access = SourceAccess::borrowed(total_len);
            let offer = demux_factory
                .quote(&DemuxNeeds::default(), &access, &eff.resources)
                .map_err(|e| {
                    at!(EngineError::simple(
                        Stage::Verify,
                        Failure::CorruptOutput,
                        "verification demuxer quote failed",
                    )
                    .with_cause(e))
                })?;
            eff.admit_workspace(demux_id.as_str(), offer.workspace, &mut verify_report)?;
            // Preserve unknown-workspace admissions from readback on the caller's RouteReport.
            for comp in verify_report.unknown_workspace_components {
                if !report.unknown_workspace_components.contains(&comp) {
                    report.unknown_workspace_components.push(comp);
                }
            }

            match probe_container_prefix(engine.codecs(), staged, &ProbeLimits::default())? {
                ContainerProbeOutcome::Confirmed { container, .. }
                    if &container == expected_container => {}
                _ => {
                    return Err(at!(EngineError::simple(
                        Stage::Verify,
                        Failure::CorruptOutput,
                        "staged output failed readback container header verification",
                    )));
                }
            }

            let mut demux = demux_factory.open(&offer, &eff.resources).map_err(|e| {
                at!(EngineError::simple(
                    Stage::Verify,
                    Failure::CorruptOutput,
                    "verification demuxer open failed",
                )
                .with_cause(e))
            })?;
            let pulse: Option<&dyn zencodec::Pulse> = Some(eff.resources.pulse());
            let mut offset = 0usize;
            let mut saw_headers = false;
            let mut declared_tracks: usize = 0;
            let mut saw_done = false;
            let mut verified_packets = 0u64;
            let mut steps = 0usize;
            let max_steps = 1_000_000usize;
            while steps < max_steps {
                steps += 1;
                eff.check_stop(Stage::Verify)?;
                let slice = staged.get(offset..).ok_or_else(|| {
                    at!(EngineError::simple(
                        Stage::Verify,
                        Failure::CorruptOutput,
                        "verification demuxer cursor exceeded staged output length",
                    ))
                })?;
                let prog = demux
                    .step(
                        Input::new(offset as u64, slice).with_total_len(total_len),
                        pulse,
                    )
                    .map_err(|e| {
                        at!(EngineError::simple(
                            Stage::Verify,
                            Failure::CorruptOutput,
                            "staged output failed full container structural verification",
                        )
                        .with_cause(e))
                    })?;
                if prog.consumed > slice.len() {
                    return Err(at!(EngineError::simple(
                        Stage::Verify,
                        Failure::CorruptOutput,
                        "verification demuxer overconsumed offered staged slice",
                    )));
                }
                offset += prog.consumed;
                match prog.event {
                    DemuxEvent::Headers => {
                        let count = demux.tracks().len();
                        if count == 0 {
                            return Err(at!(EngineError::simple(
                                Stage::Verify,
                                Failure::CorruptOutput,
                                "verified container declared zero tracks",
                            )));
                        }
                        declared_tracks = count;
                        saw_headers = true;
                    }
                    DemuxEvent::Packet(pkt) => {
                        if !saw_headers {
                            return Err(at!(EngineError::simple(
                                Stage::Verify,
                                Failure::CorruptOutput,
                                "verification demuxer emitted packet before container headers",
                            )));
                        }
                        if (pkt.track() as usize) >= declared_tracks {
                            return Err(at!(EngineError::simple(
                                Stage::Verify,
                                Failure::CorruptOutput,
                                "verification demuxer emitted packet for undeclared track index",
                            )));
                        }
                        verified_packets = verified_packets.saturating_add(1);
                    }
                    DemuxEvent::Done => {
                        if !saw_headers {
                            return Err(at!(EngineError::simple(
                                Stage::Verify,
                                Failure::CorruptOutput,
                                "verification demuxer finished before emitting container headers",
                            )));
                        }
                        saw_done = true;
                        break;
                    }
                    DemuxEvent::Need => {
                        let req = demux.requested().first().copied().ok_or_else(|| {
                            at!(EngineError::simple(
                                Stage::Verify,
                                Failure::CorruptOutput,
                                "verification demuxer emitted Need without any InputRequest",
                            ))
                        })?;
                        match req {
                            InputRequest::Range(r) => {
                                let end = r.offset.checked_add(r.length).ok_or_else(|| {
                                    at!(EngineError::simple(
                                        Stage::Verify,
                                        Failure::CorruptOutput,
                                        "verification demuxer Range request overflowed",
                                    ))
                                })?;
                                if end > total_len {
                                    return Err(at!(EngineError::simple(
                                        Stage::Verify,
                                        Failure::CorruptOutput,
                                        "verification demuxer requested exact Range past end of staged output",
                                    )));
                                }
                                let target = usize::try_from(r.offset).map_err(|_| {
                                    at!(EngineError::simple(
                                        Stage::Verify,
                                        Failure::CorruptOutput,
                                        "verification demuxer Range offset out of addressable memory",
                                    ))
                                })?;
                                if target == offset && prog.consumed == 0 {
                                    return Err(at!(EngineError::simple(
                                        Stage::Verify,
                                        Failure::CorruptOutput,
                                        "verification demuxer stalled on satisfied Range request",
                                    )));
                                }
                                offset = target;
                            }
                            InputRequest::AtLeast {
                                offset: req_off,
                                min_len,
                            } => {
                                if req_off > total_len {
                                    return Err(at!(EngineError::simple(
                                        Stage::Verify,
                                        Failure::CorruptOutput,
                                        "verification demuxer requested offset past end of staged output",
                                    )));
                                }
                                let target = usize::try_from(req_off).map_err(|_| {
                                    at!(EngineError::simple(
                                        Stage::Verify,
                                        Failure::CorruptOutput,
                                        "verification demuxer offset out of addressable memory",
                                    ))
                                })?;
                                let avail = staged.len().saturating_sub(target);
                                if target == offset && prog.consumed == 0 && avail < min_len {
                                    return Err(at!(EngineError::simple(
                                        Stage::Verify,
                                        Failure::CorruptOutput,
                                        "staged container truncated before EOF during verification",
                                    )));
                                }
                                if target == offset && prog.consumed == 0 {
                                    return Err(at!(EngineError::simple(
                                        Stage::Verify,
                                        Failure::CorruptOutput,
                                        "verification demuxer made zero progress",
                                    )));
                                }
                                offset = target;
                            }
                            _ => {
                                return Err(at!(EngineError::simple(
                                    Stage::Verify,
                                    Failure::CorruptOutput,
                                    "verification demuxer emitted unsupported InputRequest variant",
                                )));
                            }
                        }
                    }
                    _ => {}
                }
            }
            if !saw_headers || !saw_done || verified_packets == 0 {
                return Err(at!(EngineError::simple(
                    Stage::Verify,
                    Failure::CorruptOutput,
                    "staged container did not yield valid headers, packets, and clean EOF during verification",
                )));
            }
            let len = staged.len() as u64;
            self.committed_bytes = len;
            Some(VerifiedCheck {
                confirmed_container: expected_container.clone(),
                byte_len: len,
                verified_packets,
            })
        } else {
            self.committed_bytes = staged.len() as u64;
            None
        };

        // Transfer ownership of the staged buffer directly with zero copy or second allocation,
        // keeping the StorageBudget reservation alive for the caller to release once published.
        Ok((
            PublishedMedia {
                container: expected_container.clone(),
                bytes: self.bytes,
                verified,
                committed_bytes: self.committed_bytes,
            },
            self.budget_hold,
        ))
    }

    /// Verify (when `Verified`) and finalize the published media payload, transferring ownership
    /// of the staged `Vec<u8>` directly with zero copy or second allocation.
    pub fn finish(
        self,
        expected_container: &ContainerId,
        engine: &Engine,
        eff: &mut EffectiveRun,
        report: &mut RouteReport,
    ) -> EngineResult<PublishedMedia> {
        let (published, _hold) = self.finish_retained(expected_container, engine, eff, report)?;
        Ok(published)
    }
}
