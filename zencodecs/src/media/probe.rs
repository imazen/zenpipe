//! Bounded prefix container probing across all registered [`DemuxRole`] factories.

use whereat::at;
use zencodec::decode::Input;
use zencodec::{CodecSet, RegistrationId};
use zencodec_media::registry::{ContainerId, ContainerProbe, DemuxRole, ProbeLimits};

use crate::engine::{EngineError, EngineResult, Failure, Stage};

/// Result of probing a container prefix against registered demuxers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContainerProbeOutcome {
    /// Confirmed container identity and winning demuxer registration ID.
    Confirmed {
        /// Confirmed container identity.
        container: ContainerId,
        /// Demuxer registration that produced the highest-confidence confirmation.
        probed_by: RegistrationId,
        /// Confidence (`1..=100`).
        confidence: u8,
    },
    /// At least one demuxer needs additional prefix bytes before confirming or rejecting.
    NeedMore {
        /// Minimum total prefix bytes required from offset 0.
        min_bytes: usize,
    },
    /// No registered demuxer recognized the prefix.
    NoMatch,
}

/// Probe `prefix` across all registered [`DemuxRole`] factories within `limits`.
///
/// Highest confidence wins ties; equal confidence preserves stable registration order.
pub fn probe_container_prefix(
    codecs: &CodecSet,
    prefix: &[u8],
    limits: &ProbeLimits,
) -> EngineResult<ContainerProbeOutcome> {
    let capped_len = prefix.len().min(limits.max_probe_bytes);
    let input = Input::new(0, &prefix[..capped_len]);

    let mut best: Option<(u8, ContainerId, RegistrationId)> = None;
    let mut need_bytes: Option<usize> = None;

    for (_key, entry) in codecs.role_entries::<DemuxRole>() {
        match entry.factory.probe(input, limits) {
            Ok(ContainerProbe::Confirmed {
                container,
                confidence,
                ..
            }) => {
                let replace = best
                    .as_ref()
                    .is_none_or(|(best_conf, _, _)| confidence > *best_conf);
                if replace {
                    best = Some((confidence, container, entry.id.clone()));
                }
            }
            Ok(ContainerProbe::Need { min_bytes }) => {
                let bounded = min_bytes.min(limits.max_probe_bytes);
                need_bytes = Some(match need_bytes {
                    Some(prev) => prev.min(bounded),
                    None => bounded,
                });
            }
            Ok(ContainerProbe::NoMatch) => {}
            Ok(_) => {}
            Err(err) => {
                return Err(at!(EngineError::simple(
                    Stage::Inspect,
                    Failure::InvalidData,
                    "container probe failed",
                )
                .with_cause(err)));
            }
        }
    }

    if let Some((confidence, container, probed_by)) = best {
        return Ok(ContainerProbeOutcome::Confirmed {
            container,
            probed_by,
            confidence,
        });
    }
    if let Some(min_bytes) = need_bytes {
        return Ok(ContainerProbeOutcome::NeedMore { min_bytes });
    }
    Ok(ContainerProbeOutcome::NoMatch)
}

/// Convenience: probe a complete byte buffer and return `Some((container, probed_by))` on confirmation.
pub fn probe_container_bytes(
    codecs: &CodecSet,
    bytes: &[u8],
    limits: &ProbeLimits,
) -> EngineResult<Option<(ContainerId, RegistrationId)>> {
    match probe_container_prefix(codecs, bytes, limits)? {
        ContainerProbeOutcome::Confirmed {
            container,
            probed_by,
            ..
        } => Ok(Some((container, probed_by))),
        ContainerProbeOutcome::NeedMore { .. } | ContainerProbeOutcome::NoMatch => Ok(None),
    }
}
