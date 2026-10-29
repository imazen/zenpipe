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

Standalone metadata-service qualification, HEIC/RAW optional-Pulse adapters,
all portable lock roots, generated APIs, browsers and required platforms remain
unqualified on this candidate. These changes do not solve the still-only
`zencodecs::transcode` animation-loss issue or PipeError's stop-reason loss.
Progress production inside native coefficient/filter kernels is not claimed.
GIF readback here is same-codec, not a new independent decoder oracle. Existing
native suites retain their prior oracles; no new full cross-codec parity claim.

## Published revision mapping

An inherited empty commit needed a description before pushing the review branch.
JJ rewrote descendant identities; every corresponding Git tree was checked equal.
The frozen original run IDs above remain unchanged in the retained receipts.

| Frozen source | Published equivalent |
|---|---|
| `5835fc20cfa2b7b32ac85b0ef68af0ada93cf891` | `c640c634ec0b7c23de0413d0227531f2edadfdd3` |
| `89bad4f756607fceb66d9a839737589c3a7e3558` | `9a77969feb9c37ca424dbb4bd14252874ae0839f` |
| `6469f4bc2a63b1689e43d6fb5d906a6545364e7c` | `ab3c59f8bf8b5429f3ea9ebcedc83283ad3b6a41` |
| `ec39a5327245257d8c32051362d6dce97a66600b` | `7d239dd59bfdd6b29e27bd1439b343945d61a17f` |
| `2c0c89edc205b7b68a05a546e17cfefabea29002` | `14f8cb6e263fe1ac27920ab3ecc31bf3a6cb3da0` |
| `921a3f9f731fe77f425bc3bec69b238b6a740b80` | `a321b6b6d1b6d363f10c9ff2ba51b0fe411d7a77` |
| `7c36d596c1b4bc0701948d0554b00b6691e0f27b` | `21173bbaddf74d42c1e0e2fe4b8f43d56dbe2d01` |

## Checked publication follow-through

Frozen published source `a0b34b33f9b8` (root07) passed locked metadata,
`just checked-publication-check` and the JPEG/PNG metadata-scrub target:
16 checked-publication unit tests, 35 native integration tests and 22 metadata
scrub tests. The existing recipe also passes strict Clippy on the checked library,
integration target and both examples, then the codec-free checked-transcode build.
No feature/assertion exclusions or lock changes. Original publication cancellation,
partial-write, retained-owner, metadata inventory and callback controls remain.

Wrapper: rc0, 33 seconds, peak RSS 0.68 GiB, minimum available 14262 MiB,
peak load 1.42; 3 GiB/two-job cap. This extends the focused Linux qualification;
standalone metadata-services, full native profile/API, other locks and browser/
platform gates remain unqualified. Earlier failed receipts remain unchanged.
