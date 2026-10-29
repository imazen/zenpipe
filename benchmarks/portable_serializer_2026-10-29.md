# Portable serializer graph repair

Source `96972529`, five independent graph roots resolved outside workspace
path overrides with `just checked-publication-locks`. Both unchanged validators
pass: one core family at `feb09f9f`, and AVIF/parser/serializer at `f0a3919e`
with rav1d-safe `b19aec65`. Optional AVIF patches remain unused in the two roots
that do not enable AVIF; this does not qualify those features.

The serializer edge in zenravif `52bb0243` is now a registry requirement.
Application-root patches select the matching serializer; no cyclic git repinning
or source-check exception is needed. All five roots carry the patch.

Lock audit: root changes only zenavif, parser, serializer and zenravif sources.
Other roots catch up to already-committed core/Pulse codec pins: Ultra HDR,
JPEG/YUV, PNG/zenflate, WebP and bitmaps. Demo and codec-fuzz additionally move
rav1d to the already-selected b19 revision. Its manifest introduces
rav1d-disjoint-mut and likely_stable; the latter's lock dependencies introduce
rustc_version → semver. No unrelated registry version upgrades were observed.
The precise per-root package deltas are retained in the adjacent JSON ledger.

Receipt: `run-heavy: done rc=0 7s | peak-RSS 0.29GiB | min-avail 15124MiB | peak-load 0.57`.
This proves graph resolution and source agreement, not native behavior, API
compilation or browser qualification. Those remain separate required gates.

## Native consumer qualification

The unmodified AVIF suite on `4fd1d7ec` failed: 8 passed, 3 failed, 4 existing
ignores. Two retained assertions require explicit/EXIF orientation tags to survive
encoding; native still encoding instead bakes the orientation. That contract
conflict remains unresolved; neither assertion has been weakened.

The third failure identified a dropped consumer request: AVIF decoding never
received `DecodeRequest::with_orientation`. Fix `0a26163f` forwards it to the
native decode job. All three orientation pixel witnesses now pass unchanged,
including exact Preserve/Correct samples, rectangular dimensions and container
carriers. Strict library/test Clippy passes. Full-suite success is NOT claimed.

Receipts: failing full suite `run-heavy: done rc=101 22s | peak-RSS 0.95GiB | min-avail 13809MiB | peak-load 0.87`; focused repair `run-heavy: done rc=0 5s | peak-RSS 0.39GiB | min-avail 14738MiB | peak-load 0.39`.
