//! Named encode-speed presets (zenpipe#28).
//!
//! The codec-agnostic effort knob (`with_generic_effort`, 0–10) means
//! different things per codec: effort 5 is a mild JPEG trellis knob but a
//! multi-second AVIF speed setting. [`EncodeSpeed`] names the four
//! operating points callers actually reason about and maps each to a
//! per-codec generic effort plus a [`ThreadingPolicy`], so a server can say
//! "realtime" once and get sane behavior for whatever format the selector
//! picks.
//!
//! Attach with [`EncodeRequest::with_speed`](crate::EncodeRequest::with_speed).
//! An explicit `with_effort()` always wins over the preset's effort.

use crate::ImageFormat;
use zencodec::{ResourceLimits, ThreadingPolicy};

/// Encode speed preset — resolves to a per-codec generic effort and a
/// threading policy. See the [module docs](self) for the mapping rationale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EncodeSpeed {
    /// Lowest effort, single-threaded. Latency-critical paths (thumbnails
    /// on the request path, previews).
    Fastest,
    /// Balanced effort, parallel. The server default.
    Realtime,
    /// High effort, parallel. Batch / build-time processing.
    Offline,
    /// Maximum effort, parallel. Archival, one-off optimization.
    OfflineMax,
}

impl EncodeSpeed {
    /// Every preset, fastest first.
    pub const ALL: [EncodeSpeed; 4] = [
        EncodeSpeed::Fastest,
        EncodeSpeed::Realtime,
        EncodeSpeed::Offline,
        EncodeSpeed::OfflineMax,
    ];

    /// Generic effort (the `with_generic_effort` 0–10 scale) this preset
    /// requests for `format`.
    ///
    /// Policy table, not a calibration: each codec's `with_generic_effort`
    /// maps the value onto its native scale (zenjpeg clamps to 0–2, zenwebp
    /// derives `method = effort * 6 / 10`, zenjxl clamps to 1–10, zenpng maps
    /// to a compression level). The per-format rows exist so the slow codecs
    /// (AVIF, JXL) sit one notch lower at the same preset than the cheap ones.
    pub fn generic_effort(self, format: ImageFormat) -> u32 {
        use EncodeSpeed::*;
        match format {
            ImageFormat::Jpeg => match self {
                Fastest => 0,
                Realtime => 1,
                Offline => 2,
                OfflineMax => 2,
            },
            // zenwebp: method = effort*6/10 → 0 / 2 / 4 / 6.
            ImageFormat::WebP => match self {
                Fastest => 0,
                Realtime => 4,
                Offline => 7,
                OfflineMax => 10,
            },
            ImageFormat::Jxl => match self {
                Fastest => 1,
                Realtime => 3,
                Offline => 7,
                OfflineMax => 9,
            },
            ImageFormat::Avif => match self {
                Fastest => 0,
                Realtime => 2,
                Offline => 6,
                OfflineMax => 10,
            },
            ImageFormat::Png => match self {
                Fastest => 0,
                Realtime => 3,
                Offline => 6,
                OfflineMax => 10,
            },
            _ => match self {
                Fastest => 0,
                Realtime => 3,
                Offline => 7,
                OfflineMax => 10,
            },
        }
    }

    /// Threading policy for this preset: [`Fastest`](Self::Fastest) is
    /// sequential (no pool hand-off latency), everything else parallel.
    pub fn threading(self) -> ThreadingPolicy {
        match self {
            EncodeSpeed::Fastest => ThreadingPolicy::Sequential,
            _ => ThreadingPolicy::Parallel,
        }
    }

    /// Apply this preset's threading to `limits`.
    ///
    /// Only [`Fastest`](Self::Fastest) overrides (to `Sequential`); the other
    /// presets keep whatever the caller's limits say — `Parallel` is already
    /// the `ResourceLimits` default, and a caller who set `Sequential`
    /// explicitly (WASM, a pinned thread budget) must not be widened by a
    /// preset.
    pub fn apply_to_limits(self, limits: ResourceLimits) -> ResourceLimits {
        match self {
            EncodeSpeed::Fastest => limits.with_threading(ThreadingPolicy::Sequential),
            _ => limits,
        }
    }

    /// Stable lowercase name (`fastest` / `realtime` / `offline` / `offline-max`).
    pub fn name(self) -> &'static str {
        match self {
            EncodeSpeed::Fastest => "fastest",
            EncodeSpeed::Realtime => "realtime",
            EncodeSpeed::Offline => "offline",
            EncodeSpeed::OfflineMax => "offline-max",
        }
    }

    /// Parse a preset name (case-insensitive; accepts `offline-max`,
    /// `offline_max`, `offlinemax`, and `max`).
    pub fn from_name(s: &str) -> Option<Self> {
        let mut buf = [0u8; 16];
        let bytes = s.as_bytes();
        if bytes.len() > buf.len() {
            return None;
        }
        for (d, b) in buf.iter_mut().zip(bytes) {
            *d = b.to_ascii_lowercase();
        }
        match &buf[..bytes.len()] {
            b"fastest" => Some(EncodeSpeed::Fastest),
            b"realtime" => Some(EncodeSpeed::Realtime),
            b"offline" => Some(EncodeSpeed::Offline),
            b"offline-max" | b"offline_max" | b"offlinemax" | b"max" => {
                Some(EncodeSpeed::OfflineMax)
            }
            _ => None,
        }
    }
}

impl core::fmt::Display for EncodeSpeed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMATS: [ImageFormat; 6] = [
        ImageFormat::Jpeg,
        ImageFormat::WebP,
        ImageFormat::Jxl,
        ImageFormat::Avif,
        ImageFormat::Png,
        ImageFormat::Gif,
    ];

    #[test]
    fn effort_is_monotonic_and_in_generic_range() {
        for fmt in FORMATS {
            let mut prev = None;
            for s in EncodeSpeed::ALL {
                let e = s.generic_effort(fmt);
                assert!(e <= 10, "{s} {fmt:?} effort {e} outside 0..=10");
                if let Some(p) = prev {
                    assert!(e >= p, "{s} {fmt:?}: effort {e} < previous {p}");
                }
                prev = Some(e);
            }
            assert!(
                EncodeSpeed::OfflineMax.generic_effort(fmt)
                    > EncodeSpeed::Fastest.generic_effort(fmt),
                "{fmt:?}: presets must span a non-trivial effort range"
            );
        }
    }

    #[test]
    fn only_fastest_forces_sequential() {
        assert_eq!(
            EncodeSpeed::Fastest.threading(),
            ThreadingPolicy::Sequential
        );
        for s in [
            EncodeSpeed::Realtime,
            EncodeSpeed::Offline,
            EncodeSpeed::OfflineMax,
        ] {
            assert_eq!(s.threading(), ThreadingPolicy::Parallel);
        }

        let seq = ResourceLimits::none().with_threading(ThreadingPolicy::Sequential);
        // Non-Fastest presets never widen an explicit Sequential.
        assert_eq!(
            EncodeSpeed::Offline.apply_to_limits(seq).threading,
            ThreadingPolicy::Sequential
        );
        assert_eq!(
            EncodeSpeed::Fastest
                .apply_to_limits(ResourceLimits::none())
                .threading,
            ThreadingPolicy::Sequential
        );
        assert_eq!(
            EncodeSpeed::Realtime
                .apply_to_limits(ResourceLimits::none())
                .threading,
            ThreadingPolicy::Parallel
        );
    }

    #[test]
    fn names_round_trip() {
        for s in EncodeSpeed::ALL {
            assert_eq!(EncodeSpeed::from_name(s.name()), Some(s));
            assert_eq!(
                EncodeSpeed::from_name(&s.name().to_ascii_uppercase()),
                Some(s)
            );
        }
        assert_eq!(EncodeSpeed::from_name("max"), Some(EncodeSpeed::OfflineMax));
        assert_eq!(
            EncodeSpeed::from_name("offline_max"),
            Some(EncodeSpeed::OfflineMax)
        );
        assert_eq!(EncodeSpeed::from_name("warp"), None);
        assert_eq!(EncodeSpeed::from_name("this-name-is-far-too-long"), None);
    }
}
