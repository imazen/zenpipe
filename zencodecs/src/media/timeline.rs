//! Timeline presentation mapping, loop-count normalization, and rational endpoint quantization.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use whereat::at;
use zencodec::time::{FrameDuration, TimeBase, Timestamp};
use zencodec::JobResources;
use zencodec_media::frame::Frame;
use zencodec_media::packet::Discard;
use zencodec_media::registry::ContainerId;
use zencodec_media::timeline::{PresentationMap, PresentationSink, TimelineError, TimingEvidence};
use zencodec_media::track::TrackInfo;

use crate::engine::{
    EngineError, EngineResult, Failure, LoopCount, LoopPolicy, Stage, TimingPolicy,
};

/// Maximum in-flight per-packet discard entries retained between decoder send and receive.
const MAX_PENDING_DISCARDS: usize = 64;

/// Whether a video container can natively carry a repeat/loop count.
///
/// MP4, WebM, and IVF are one-pass video streams without native animation repeat loops.
#[must_use]
pub fn container_supports_loops(container: &ContainerId) -> bool {
    let _ = container;
    false
}

/// Resolve and validate an output loop count for a video container target.
///
/// Converting a repeating animation (`Infinite` or `Plays(>1)`) to MP4/WebM/IVF
/// refuses under [`LoopPolicy::Preserve`] unless the caller explicitly requests
/// `LoopPolicy::Set(LoopCount::once())`.
pub fn normalize_loop_for_target(
    source_loops: LoopCount,
    policy: LoopPolicy,
    target_container: &ContainerId,
) -> EngineResult<LoopCount> {
    let effective = match policy {
        LoopPolicy::Preserve => source_loops,
        LoopPolicy::Set(explicit) => explicit,
    };
    if container_supports_loops(target_container) {
        return Ok(effective);
    }
    match effective {
        LoopCount::Unspecified => Ok(LoopCount::once()),
        LoopCount::Plays(n) if n.get() == 1 => Ok(LoopCount::once()),
        LoopCount::Infinite | LoopCount::Plays(_) => Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::UnsupportedEffect,
            "target video container does not support repeat loops; set LoopPolicy::Set(LoopCount::once()) explicitly",
        ))),
    }
}

/// Quantized rational boundary result for one frame sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuantizedTiming {
    /// Quantized per-frame durations on `target_base`.
    pub durations: Vec<u64>,
    /// Maximum observed cumulative boundary error (numerator / denominator seconds).
    pub max_error_num: u128,
    /// Common denominator for `max_error_num`.
    pub max_error_den: u128,
}

/// Quantize a sequence of exact [`FrameDuration`]s onto `target_base` ticks using
/// cumulative-endpoint rounding (ties toward positive infinity), enforcing
/// [`TimingPolicy::Exact`] or `max_boundary_error` at EVERY frame boundary.
pub fn quantize_endpoints(
    durations: &[FrameDuration],
    target_base: TimeBase,
    policy: TimingPolicy,
) -> EngineResult<QuantizedTiming> {
    let tb_num = u128::from(target_base.numerator());
    let tb_den = u128::from(target_base.denominator());
    let mut out = Vec::with_capacity(durations.len());

    // Maintain exact rational cumulative time as (acc_num / acc_den) in seconds.
    let mut acc_num: u128 = 0;
    let mut acc_den: u128 = 1;
    let mut prev_ticks: u128 = 0;
    let mut max_err_num: u128 = 0;
    let mut max_err_den: u128 = 1;

    for &dur in durations {
        let d_num = u128::from(dur.numerator());
        let d_den = u128::from(dur.denominator());
        // Add d_num / d_den to acc_num / acc_den with GCD reduction to prevent overflow.
        let g = gcd_u128(acc_den, d_den);
        let lcm = (acc_den / g)
            .checked_mul(d_den)
            .ok_or_else(|| at!(EngineError::simple(Stage::Admit, Failure::ResourceLimit, "rational timeline denominator overflow")))?;
        let new_num = acc_num
            .checked_mul(d_den / g)
            .and_then(|a| d_num.checked_mul(acc_den / g).and_then(|b| a.checked_add(b)))
            .ok_or_else(|| at!(EngineError::simple(Stage::Admit, Failure::ResourceLimit, "rational timeline numerator overflow")))?;
        let rg = gcd_u128(new_num, lcm);
        acc_num = new_num / rg;
        acc_den = lcm / rg;

        // Exact tick position = (acc_num / acc_den) / (tb_num / tb_den) = (acc_num * tb_den) / (acc_den * tb_num).
        let num_ticks = acc_num
            .checked_mul(tb_den)
            .ok_or_else(|| at!(EngineError::simple(Stage::Admit, Failure::ResourceLimit, "tick numerator overflow")))?;
        let den_ticks = acc_den
            .checked_mul(tb_num)
            .ok_or_else(|| at!(EngineError::simple(Stage::Admit, Failure::ResourceLimit, "tick denominator overflow")))?;

        let rem = num_ticks % den_ticks;
        let q_floor = num_ticks / den_ticks;

        let target_ticks = match policy {
            TimingPolicy::Exact => {
                if rem != 0 {
                    return Err(at!(EngineError::simple(
                        Stage::Admit,
                        Failure::UnsupportedEffect,
                        "frame duration is not exactly representable on target clock under TimingPolicy::Exact",
                    )));
                }
                q_floor
            }
            TimingPolicy::QuantizeEndpoints { max_boundary_error } => {
                // Canonical ties-toward-positive-infinity: round up iff 2 * rem >= den_ticks.
                let q_ticks = if rem.saturating_mul(2) >= den_ticks {
                    q_floor.checked_add(1).ok_or_else(|| {
                        at!(EngineError::simple(Stage::Admit, Failure::ResourceLimit, "quantized tick overflow"))
                    })?
                } else {
                    q_floor
                };

                // Boundary error in seconds = |q_ticks * (tb_num/tb_den) - acc_num/acc_den|
                //   = |q_ticks * tb_num * acc_den - acc_num * tb_den| / (tb_den * acc_den).
                let lhs = q_ticks
                    .checked_mul(tb_num)
                    .and_then(|x| x.checked_mul(acc_den))
                    .ok_or_else(|| at!(EngineError::simple(Stage::Admit, Failure::ResourceLimit, "error bound overflow")))?;
                let rhs = acc_num
                    .checked_mul(tb_den)
                    .ok_or_else(|| at!(EngineError::simple(Stage::Admit, Failure::ResourceLimit, "error bound overflow")))?;
                let err_num = lhs.abs_diff(rhs);
                let err_den = tb_den
                    .checked_mul(acc_den)
                    .ok_or_else(|| at!(EngineError::simple(Stage::Admit, Failure::ResourceLimit, "error denom overflow")))?;

                // Compare err_num / err_den <= max_boundary_error (m_num / m_den) via exact 256-bit math!
                let m_num = u128::from(max_boundary_error.numerator());
                let m_den = u128::from(max_boundary_error.denominator());
                if crate::media::select::mul_u128_wide(err_num, m_den)
                    > crate::media::select::mul_u128_wide(m_num, err_den)
                {
                    return Err(at!(EngineError::simple(
                        Stage::Verify,
                        Failure::UnsupportedEffect,
                        "cumulative quantized frame boundary error exceeded max_boundary_error",
                    )));
                }
                if crate::media::select::mul_u128_wide(err_num, max_err_den)
                    > crate::media::select::mul_u128_wide(max_err_num, err_den)
                {
                    let eg = gcd_u128(err_num, err_den);
                    max_err_num = err_num / eg;
                    max_err_den = err_den / eg;
                }
                q_ticks
            }
        };

        let delta = target_ticks.saturating_sub(prev_ticks);
        let delta_u64 = u64::try_from(delta).map_err(|_| {
            at!(EngineError::simple(
                Stage::Admit,
                Failure::ResourceLimit,
                "quantized frame tick duration exceeds u64",
            ))
        })?;
        out.push(delta_u64);
        prev_ticks = target_ticks;
    }

    Ok(QuantizedTiming {
        durations: out,
        max_error_num: max_err_num,
        max_error_den: max_err_den,
    })
}

/// Collects presentation events emitted by [`PresentationMap`].
#[derive(Default)]
pub struct TimelineBridge {
    map: Option<PresentationMap>,
    ready: Vec<MappedPresentation>,
    /// Accepted packets still in flight (`(pts, optional discard)`), bounded so neither
    /// pending discard entries nor non-discard reorder entries can grow without limit.
    pending_packets: VecDeque<(Timestamp, Option<Discard>)>,
}

const MAX_PENDING_PACKETS: usize = 128;

/// Output item produced by [`TimelineBridge`] in presentation order.
pub enum MappedPresentation {
    /// A displayed video frame with mapped presentation timing.
    Frame(Frame),
    /// An explicit empty edit gap on the presentation timeline.
    Gap {
        /// Gap start timestamp.
        start: Timestamp,
        /// Gap duration.
        duration: FrameDuration,
    },
}

struct VecSink<'a>(&'a mut Vec<MappedPresentation>);

impl PresentationSink for VecSink<'_> {
    fn presentation(&mut self, frame: Frame) -> Result<(), TimelineError> {
        self.0.push(MappedPresentation::Frame(frame));
        Ok(())
    }

    fn gap(&mut self, start: Timestamp, duration: FrameDuration) -> Result<(), TimelineError> {
        self.0.push(MappedPresentation::Gap { start, duration });
        Ok(())
    }
}

impl TimelineBridge {
    /// Prepare a presentation timeline bridge for `track` and `evidence`.
    pub fn prepare(
        track: &TrackInfo,
        evidence: TimingEvidence,
        resources: &JobResources,
    ) -> EngineResult<Self> {
        let map = PresentationMap::prepare(track, evidence, resources).map_err(|e| {
            at!(EngineError::simple(
                Stage::Prepare,
                Failure::UnsupportedEffect,
                "could not prepare track PresentationMap",
            )
            .with_cause(e))
        })?;
        Ok(Self {
            map: Some(map),
            ready: Vec::new(),
            pending_packets: VecDeque::new(),
        })
    }

    /// Record an accepted packet's optional discard padding keyed by exact rational [`Timestamp`]
    /// in a bounded FIFO queue.
    ///
    /// Call ONLY after `VideoDecoder::send` returns `SendStatus::Accepted`.
    /// Refuses explicitly if any in-flight packet shares the same presentation instant with
    /// an entry carrying discard padding (in either order: hidden-then-visible or
    /// visible-then-hidden), preventing trim ambiguity from silently misapplying to a
    /// different picture.
    pub fn record_accepted_packet(
        &mut self,
        pts: Option<Timestamp>,
        discard: Option<Discard>,
    ) -> EngineResult<()> {
        let Some(p) = pts else {
            if discard.is_some() {
                return Err(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::UnsupportedEffect,
                    "packet carries discard padding without a PTS to correlate it with its decoded frame",
                )));
            }
            return Ok(());
        };
        let collides = self.pending_packets.iter().any(|(t, pd)| {
            crate::media::select::cmp_timestamp_exact(*t, p) == core::cmp::Ordering::Equal
                && (discard.is_some() || pd.is_some())
        });
        if collides {
            return Err(at!(EngineError::simple(
                Stage::Execute,
                Failure::UnsupportedEffect,
                "ambiguous packet discard: multiple accepted packets share identical PTS while a discard trim is present",
            )));
        }
        if discard.is_some() {
            let pending_discards = self
                .pending_packets
                .iter()
                .filter(|(_, d)| d.is_some())
                .count();
            if pending_discards >= MAX_PENDING_DISCARDS {
                return Err(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::ResourceLimit,
                    "in-flight packet discard queue exceeded 64-entry cap",
                )));
            }
        }
        if self.pending_packets.len() >= MAX_PENDING_PACKETS {
            // Never evict an unretired packet without proof it has left the decoder reorder queue!
            return Err(at!(EngineError::simple(
                Stage::Execute,
                Failure::ResourceLimit,
                "in-flight accepted packet queue exceeded 128-entry cap; cannot discard unretired provenance evidence",
            )));
        }
        self.pending_packets.push_back((p, discard));
        Ok(())
    }

    /// Convenience alias for recording an accepted packet that carries explicit discard padding.
    pub fn record_packet_discard(&mut self, pts: Timestamp, discard: Discard) -> EngineResult<()> {
        self.record_accepted_packet(Some(pts), Some(discard))
    }

    /// Feed a decoded frame into the presentation map and enqueue mapped items.
    pub fn push_frame(&mut self, frame: Frame) -> EngineResult<()> {
        let Some(map) = self.map.as_mut() else {
            return Err(at!(EngineError::simple(
                Stage::Execute,
                Failure::BackendFailure,
                "TimelineBridge already finished",
            )));
        };
        let discard = frame.timing().pts.and_then(|pts| {
            self.pending_packets
                .iter()
                .position(|(t, _)| {
                    crate::media::select::cmp_timestamp_exact(*t, pts)
                        == core::cmp::Ordering::Equal
                })
                .and_then(|idx| self.pending_packets.remove(idx))
                .and_then(|(_, d)| d)
        });
        let mut sink = VecSink(&mut self.ready);
        map.map_frame_with_discard(frame, discard, &mut sink)
            .map_err(|e| {
                at!(EngineError::simple(
                    Stage::Execute,
                    Failure::UnsupportedEffect,
                    "timeline frame mapping failed",
                )
                .with_cause(e))
            })
    }

    /// Flush terminal presentations at end-of-stream.
    pub fn finish(&mut self) -> EngineResult<()> {
        if let Some(mut map) = self.map.take() {
            let mut sink = VecSink(&mut self.ready);
            map.finish(&mut sink).map_err(|e| {
                at!(EngineError::simple(
                    Stage::Execute,
                    Failure::UnsupportedEffect,
                    "timeline finish failed",
                )
                .with_cause(e))
            })?;
        }
        Ok(())
    }

    /// Drain all currently queued presentation items in order.
    pub fn drain_ready(&mut self) -> impl Iterator<Item = MappedPresentation> + '_ {
        self.ready.drain(..)
    }
}

fn gcd_u128(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn quantize_endpoints_exact_and_bounded_tolerance() {
        let ms = TimeBase::new(1, 1000).unwrap();
        let durs = vec![
            FrameDuration::from_millis(40),
            FrameDuration::from_millis(75),
        ];
        let q = quantize_endpoints(&durs, ms, TimingPolicy::Exact).unwrap();
        assert_eq!(q.durations, vec![40, 75]);

        // 1/30s is not exact in ms (33.333... ms), so Exact refuses:
        let fps30 = vec![FrameDuration::new(1, 30).unwrap(); 3];
        assert!(quantize_endpoints(&fps30, ms, TimingPolicy::Exact).is_err());

        // With 1ms tolerance, cumulative endpoints [33.33, 66.67, 100.0] round to [33, 67, 100] -> [33, 34, 33]!
        let q30 = quantize_endpoints(
            &fps30,
            ms,
            TimingPolicy::QuantizeEndpoints {
                max_boundary_error: FrameDuration::from_millis(1),
            },
        )
        .unwrap();
        assert_eq!(q30.durations, vec![33, 34, 33]);
    }

    #[test]
    fn video_loop_normalization_refuses_implicit_drop_of_infinite_loop() {
        assert!(
            normalize_loop_for_target(
                LoopCount::Infinite,
                LoopPolicy::Preserve,
                &ContainerId::Mp4,
            )
            .is_err()
        );
        assert_eq!(
            normalize_loop_for_target(
                LoopCount::Infinite,
                LoopPolicy::Set(LoopCount::once()),
                &ContainerId::Mp4,
            )
            .unwrap(),
            LoopCount::once()
        );
    }
}
