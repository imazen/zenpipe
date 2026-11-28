//! Presentation selection (`PresentationIndex`, `PresentationTime`, `SeekMode`)
//! and bounded packet/decoded-presentation indexes (`PacketIndexV1`, `PresentationIndex`).

use alloc::vec::Vec;
use whereat::at;
use zencodec::time::Timestamp;

use crate::engine::{EngineError, EngineResult, Failure, Stage};

/// Directional time-matching mode for timestamp-based frame/poster extraction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SeekMode {
    /// Last displayed presentation with `pts <= target`.
    #[default]
    AtOrBefore,
    /// First displayed presentation with `pts >= target`.
    AtOrAfter,
    /// Presentation minimizing `|pts - target|` (ties prefer the earlier presentation).
    Nearest,
}

/// Which presentations to extract from a media stream, animation, or multi-page document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeSelection {
    /// Yield all presentations in display order.
    #[default]
    All,
    /// Select a single displayed presentation by zero-based presentation ordinal
    /// (counted AFTER the admitted `PresentationMap`, never raw packet ordinal).
    PresentationIndex {
        /// Optional track override (`None` = selected primary video track).
        track: Option<u32>,
        /// Zero-based displayed frame ordinal.
        index: u64,
    },
    /// Select a single displayed presentation by presented timestamp.
    PresentationTime {
        /// Optional track override (`None` = selected primary video track).
        track: Option<u32>,
        /// Target timestamp on the presentation clock.
        time: Timestamp,
        /// Directional match rule (`AtOrBefore`, `AtOrAfter`, or `Nearest`).
        mode: SeekMode,
    },
    /// Select a single page from a multi-page document (PDF/TIFF).
    Page(u32),
}

/// Single packet-level index entry (version 1 wire-compatible format).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PacketIndexEntry {
    /// Track index.
    pub track: u32,
    /// Packet ordinal on the track.
    pub ordinal: u64,
    /// Packet presentation timestamp (if present).
    pub pts: Option<Timestamp>,
    /// Container byte offset (if known).
    pub byte_offset: Option<u64>,
    /// Whether this entry is a container-confirmed random-access point.
    pub container_keyframe: bool,
    /// Configuration epoch active for this packet.
    pub config_epoch: u32,
}

/// Bounded version-1 packet index cacheable across repeat random extractions on immutable sources.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PacketIndexV1 {
    /// Entries in container packet order.
    pub entries: Vec<PacketIndexEntry>,
    /// Maximum entry cap enforced during construction.
    pub max_entries: usize,
}

impl PacketIndexV1 {
    /// Create a bounded packet index capped at `max_entries`.
    #[must_use]
    pub fn with_capacity_cap(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries,
        }
    }

    /// Append an entry, failing cleanly if `max_entries` would be exceeded.
    pub fn push(&mut self, entry: PacketIndexEntry) -> EngineResult<()> {
        if self.entries.len() >= self.max_entries {
            return Err(at!(EngineError::simple(
                Stage::Inspect,
                Failure::ResourceLimit,
                "PacketIndexV1 entry count exceeds configured cap",
            )));
        }
        self.entries.push(entry);
        Ok(())
    }
}

/// Single decoded-presentation index entry built from displayed frames after `PresentationMap`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentationEntry {
    /// Zero-based displayed presentation ordinal.
    pub presentation_index: u64,
    /// Actual presentation timestamp on the stream clock.
    pub pts: Timestamp,
    /// Whether this presentation is a decoded keyframe / random-access sync point.
    ///
    /// Never conflated with container-confirmed keyframe flags (e.g. IVF has no container key flag).
    pub decoded_keyframe: bool,
    /// Active configuration epoch.
    pub config_epoch: u32,
}

/// Bounded decoded-presentation index supporting deterministic time/index lookup.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PresentationIndex {
    /// Displayed presentations in presentation order.
    pub entries: Vec<PresentationEntry>,
    /// Whether construction required decoding the stream (always true for codec-derived indexes).
    pub decoded_stream: bool,
}

/// Build a [`PresentationIndex`] from a sequence of observed displayed timestamps and flags.
pub fn build_presentation_index(
    entries: Vec<PresentationEntry>,
    max_entries: usize,
) -> EngineResult<PresentationIndex> {
    if entries.len() > max_entries {
        return Err(at!(EngineError::simple(
            Stage::Inspect,
            Failure::ResourceLimit,
            "PresentationIndex exceeds configured cap",
        )));
    }
    Ok(PresentationIndex {
        entries,
        decoded_stream: true,
    })
}

/// Exact 256-bit unsigned product `(hi, lo)` of two `u128` integers (zero saturation/overflow).
#[inline]
pub(crate) fn mul_u128_wide(a: u128, b: u128) -> (u128, u128) {
    let a_lo = (a as u64) as u128;
    let a_hi = a >> 64;
    let b_lo = (b as u64) as u128;
    let b_hi = b >> 64;

    let lo_lo = a_lo * b_lo;
    let lo_hi = a_lo * b_hi;
    let hi_lo = a_hi * b_lo;
    let hi_hi = a_hi * b_hi;

    let (mid, c1) = (lo_lo >> 64).overflowing_add(lo_hi);
    let (mid, c2) = mid.overflowing_add(hi_lo);
    let lo = (lo_lo & ((1u128 << 64) - 1)) | (mid << 64);
    let hi = hi_hi + (mid >> 64) + ((u128::from(c1) + u128::from(c2)) << 64);
    (hi, lo)
}

/// Compare two timestamps in exact cross-multiplied `i128` seconds (`t.ticks * t.num / t.den`).
pub(crate) fn cmp_timestamp_exact(a: Timestamp, b: Timestamp) -> core::cmp::Ordering {
    let a_num = i128::from(a.ticks()) * i128::from(a.base().numerator());
    let a_den = i128::from(a.base().denominator());
    let b_num = i128::from(b.ticks()) * i128::from(b.base().numerator());
    let b_den = i128::from(b.base().denominator());
    (a_num * b_den).cmp(&(b_num * a_den))
}

/// Returns `true` iff `cand` is strictly closer to `target` than `best` (exact 256-bit
/// cross-clock comparison; ties return `false` to preserve the earlier presentation).
pub(crate) fn is_strictly_closer_to_target(
    cand: Timestamp,
    best: Timestamp,
    target: Timestamp,
) -> bool {
    let t_num = i128::from(target.ticks()) * i128::from(target.base().numerator());
    let t_den = i128::from(target.base().denominator());

    let c_num = i128::from(cand.ticks()) * i128::from(cand.base().numerator());
    let c_den = i128::from(cand.base().denominator());
    let b_num = i128::from(best.ticks()) * i128::from(best.base().numerator());
    let b_den = i128::from(best.base().denominator());

    let dc = (c_num * t_den).abs_diff(t_num * c_den);
    let db = (b_num * t_den).abs_diff(t_num * b_den);
    // Exact 256-bit comparison: dc / (c_den * t_den) < db / (b_den * t_den) <=> dc * b_den < db * c_den.
    mul_u128_wide(dc, b_den as u128) < mul_u128_wide(db, c_den as u128)
}

/// Compute exact scaled distance `|a - b| * a_den * b_den` for deterministic `Nearest` tiebreaks.
fn dist_timestamp_scaled(a: Timestamp, b: Timestamp) -> u128 {
    let a_num = i128::from(a.ticks()) * i128::from(a.base().numerator());
    let a_den = i128::from(a.base().denominator());
    let b_num = i128::from(b.ticks()) * i128::from(b.base().numerator());
    let b_den = i128::from(b.base().denominator());
    (a_num * b_den).abs_diff(b_num * a_den)
}

/// Deterministically resolve which entry in `index` matches `selection`.
///
/// Guarantees exact parity between indexed seek and sequential scan selection.
pub fn select_presentation_index(
    index: &PresentationIndex,
    selection: DecodeSelection,
) -> EngineResult<usize> {
    if index.entries.is_empty() {
        return Err(at!(EngineError::simple(
            Stage::Execute,
            Failure::InvalidData,
            "cannot select presentation from empty stream",
        )));
    }
    match selection {
        DecodeSelection::All => Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::InvalidRequest,
            "single-presentation selection requires PresentationIndex or PresentationTime (not All)",
        ))),
        DecodeSelection::Page(_) => Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::InvalidRequest,
            "DecodeSelection::Page is not valid for video presentations",
        ))),
        DecodeSelection::PresentationIndex { index: target, .. } => {
            let idx = usize::try_from(target).map_err(|_| {
                at!(EngineError::simple(
                    Stage::Admit,
                    Failure::InvalidRequest,
                    "presentation index out of range",
                ))
            })?;
            if idx >= index.entries.len() {
                return Err(at!(EngineError::simple(
                    Stage::Execute,
                    Failure::InvalidRequest,
                    "requested presentation index exceeds stream length",
                )));
            }
            Ok(idx)
        }
        DecodeSelection::PresentationTime { time, mode, .. } => match mode {
            SeekMode::AtOrBefore => {
                let mut best = None;
                for (i, e) in index.entries.iter().enumerate() {
                    if cmp_timestamp_exact(e.pts, time) != core::cmp::Ordering::Greater {
                        best = Some(i);
                    }
                }
                best.ok_or_else(|| {
                    at!(EngineError::simple(
                        Stage::Execute,
                        Failure::InvalidRequest,
                        "no presentation exists at or before requested timestamp",
                    ))
                })
            }
            SeekMode::AtOrAfter => index
                .entries
                .iter()
                .position(|e| cmp_timestamp_exact(e.pts, time) != core::cmp::Ordering::Less)
                .ok_or_else(|| {
                    at!(EngineError::simple(
                        Stage::Execute,
                        Failure::InvalidRequest,
                        "no presentation exists at or after requested timestamp",
                    ))
                }),
            SeekMode::Nearest => {
                let mut best_i = 0usize;
                let mut best_d = dist_timestamp_scaled(index.entries[0].pts, time);
                for (i, e) in index.entries.iter().enumerate().skip(1) {
                    let d = dist_timestamp_scaled(e.pts, time);
                    // Strict `<` breaks ties in favor of the earlier presentation deterministically.
                    if d < best_d {
                        best_d = d;
                        best_i = i;
                    }
                }
                Ok(best_i)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use zencodec::time::TimeBase;

    #[test]
    fn select_presentation_time_modes_and_exact_cross_clock_comparison() {
        let tb30 = TimeBase::new(1, 30).unwrap();
        let tb1000 = TimeBase::new(1, 1000).unwrap();
        let idx = build_presentation_index(
            vec![
                PresentationEntry {
                    presentation_index: 0,
                    pts: Timestamp::new(0, tb30),
                    decoded_keyframe: true,
                    config_epoch: 0,
                },
                PresentationEntry {
                    presentation_index: 1,
                    pts: Timestamp::new(15, tb30), // 0.500s
                    decoded_keyframe: false,
                    config_epoch: 0,
                },
                PresentationEntry {
                    presentation_index: 2,
                    pts: Timestamp::new(30, tb30), // 1.000s
                    decoded_keyframe: true,
                    config_epoch: 0,
                },
            ],
            16,
        )
        .unwrap();

        // 750ms is exact midpoint between 0.500s and 1.000s
        let t750 = Timestamp::new(750, tb1000);
        assert_eq!(
            select_presentation_index(
                &idx,
                DecodeSelection::PresentationTime {
                    track: None,
                    time: t750,
                    mode: SeekMode::AtOrBefore,
                },
            )
            .unwrap(),
            1
        );
        assert_eq!(
            select_presentation_index(
                &idx,
                DecodeSelection::PresentationTime {
                    track: None,
                    time: t750,
                    mode: SeekMode::AtOrAfter,
                },
            )
            .unwrap(),
            2
        );
        // Tie prefers earlier presentation deterministically
        assert_eq!(
            select_presentation_index(
                &idx,
                DecodeSelection::PresentationTime {
                    track: None,
                    time: t750,
                    mode: SeekMode::Nearest,
                },
            )
            .unwrap(),
            1
        );
    }
}
