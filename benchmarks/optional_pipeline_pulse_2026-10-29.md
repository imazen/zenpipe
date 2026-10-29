# Optional pipeline Pulse qualification — 2026-10-29

## Source and graph

- Pipeline/source/cache: `7c36d596`, closure-only Clippy correction `921a3f9f`.
- Animation: `2c0c89ed`, test path correction `ec39a532`, final public `decode`
  module paths included in `6469f4bc`.
- Lossless/filter implementation: `6469f4bc`; filter-test import only: `89bad4f7`.
- Shared core family: `feb09f9fd25e90e49a30d7161db8f3b558dfb2a6`.
- Pixels/converter: `283655c5424e0ad7ebc6d2acd94670fac1e29158`.

Tests ran from isolated immutable exports outside the sibling patch configuration,
using the tracked lock as the starting point. Default and all-feature metadata
resolve one shared core/pixels/how-far family. The first transition has 47 changed
package-name groups: explicit native/core pins, the selected PDF/SVG/TIFF dependency
chains, associated font/renderer/compression dependencies, and source-ID
normalization. The later filter dependency adds only the already-resolved how-far
edge to zenfilters; no package/source/version moves. The resulting root lock is
committed with this receipt. Other portable-root locks are not qualified here.

## Results

| Frozen run | Gates actually completed | Result |
|---|---|---|
| root01 / 7c36d596 | limits/effects: 29 tests | Pass; strict Clippy then failed one redundant closure |
| root02 / 921a3f9f | remaining strict Clippy; session cache: 25 tests | Pass |
| root03 / 2c0c89ed | animation compilation | Failed: test used private root type paths |
| root04 / ec39a532 | animation compilation | Failed: compiler-suggested module paths were also private |
| root05 / 6469f4bc | GIF animation: 11; retained job cancellation: 2; lossless JPEG: 11; animation and lossless strict Clippy | Pass; filter compilation then failed a missing test import |
| root06 / 89bad4f7 | filter pipeline: 8; strict library Clippy | Pass |

Commands are preserved in `just pipeline-pulse-check`, `just animation-pulse-check`
and `just processing-pulse-check`. Runtime checks use `--locked`; compilation and
Clippy retain their existing feature profiles and `-D warnings`.

New controls verify pre-cancelled execution cannot read input, cancellation during
a pull cannot deliver rows or finish the sink, materialization refuses before
inspecting source geometry/allocating the raster, and deprecated Stop signatures
still cancel. Native GIF wrappers record the exact optional Pulse identity at all
frame reads, writes and finish while delegating to the real codec; None/Some give
identical encoded bytes and frame readback. Already-stopped animation performs no
native frame call. Lossless identity-copy refusal and filter no-write refusal are
covered. Existing assertions were not weakened.

## Resource receipts

Every run used the serialized 3 GiB / two-job run-heavy wrapper and bounded commands.
These are build/test resource receipts, **not throughput, compile-time comparison,
or operation-wide memory bounds**.

| Run | Wrapper result | Seconds | Peak RSS GiB | Minimum available MiB | Peak load |
|---|---|---:|---:|---:|---:|
| root01 | 101 | 28 | 0.63 | 14373 | 0.80 |
| root02 | 0 | 10 | 0.64 | 14683 | 0.77 |
| root03 | 101 | 1 | 0.22 | 15135 | 0.11 |
| root04 | 101 | 1 | 0.22 | 15147 | 0.15 |
| root05 | 101 | 43 | 0.63 | 14321 | 0.83 |
| root06 | 0 | 5 | 0.35 | 14879 | 0.40 |

All original failures and full logs are retained privately, keyed by source/run.
Inactive Cargo executables/library caches were deleted for disk space, not archived.

## Remaining limits

Full publication/metadata-service qualification, HEIC/RAW optional-Pulse adapters,
all portable lock roots, generated APIs, browsers and required platforms remain
unqualified on this candidate. These changes do not solve the still-only
`zencodecs::transcode` animation-loss issue or PipeError's stop-reason loss.
Progress production inside native coefficient/filter kernels is not claimed.
GIF readback here is same-codec, not a new independent decoder oracle. Existing
native suites retain their prior oracles; no new full cross-codec parity claim.
