# Media suite: a consumer of the codec contracts

Build on zenpipe's existing WASM editor, worker transport, comparison and export
UI. Zenimage is a retired sketch with a smaller worker-driven editor; reuse
useful interaction ideas, not its obsolete execution framework. This assessment
is source-based, not a rendered usability study.

The suite is a release-validation application for zencodec, zencodecs and
zenpixels. It must not grow another decoder, metadata resolver or rendering
engine. Keep the first screen simple: choose an example, manipulate it, compare,
export. Show advanced controls and the actual operation report when requested.
A polished shell must never masquerade as a qualified codec implementation.

## Vertical slices and release priority

1. **Contract shell (first landing):** task/asset identity, cancellation lifecycle,
   stale-result rejection, admission accounting, explicit evidence labels, and
   a deterministic generated image. Reuse the editor for image operations.
   Test orchestration with deterministic workers, visibly labeled as a model.
   No codec or memory-performance qualification follows from those tests.
2. **Native image slice:** load the actual WASM module; JPEG/PNG decode, resize,
   encode and readback. Assert backend identity, exact fixture pixels where
   lossless, metadata policy, cancellation and resource-owner cleanup. Browser
   canvas/mock fallback must fail native qualification, although a labeled
   browser preview may remain available for ordinary exploration.
3. **Collection slice:** native TIFF pages/SubIFDs and JXL frames/layers; inventory,
   raw selection and rendered view through existing zencodec collection/render
   APIs. This is a tree, not a flat animation-frame list. Preserve item identity,
   dependencies, orientation, canvas/crop and presentation time. Validate selected
   samples and rendered composition against existing independent native oracles.
4. **Media slice:** AVIF still/sequence, VP8 through its actual codec backend,
   MP4 demux/mux with explicitly supported tracks/codecs. MP4 is a container,
   not a decode capability. Add seek/scrub, pause, step, frame comparison and
   bounded range acquisition. Unsupported track/layout combinations refuse with
   a useful reason; do not silently fall back to unrelated browser codecs.
5. **Measured scheduling:** fixed concurrency/thread-count baselines, then an
   adaptive policy tested on held-out workloads. No invented optimum or cache
   benefit. This is not a prerequisite for releasing a correct fixed policy.

Only slices exposing a missing or incorrect shared contract block zencodec.
Editor polish, broad asset coverage and controller tuning can iterate separately.
Do not widen zencodec's API just to satisfy an unimplemented UI design.

## Minimum contract shell

The internal browser controller accepts a backend with submit/cancel/dispose
and events tagged by operation ID and source generation. It does not reproduce
Rust codec traits. The native adapter will translate existing operation receipts.

Required state transitions:

- queued -> admitted -> running -> completed/failed/cancelled;
- running -> cancelling -> cancelled only after a worker terminal receipt;
- aborting the UI request signals its operation-local control immediately;
- superseded results are retained for diagnostics but cannot replace the current
  preview; stale completions still release their own reservation exactly once;
- cancellation never releases the CPU/raw-state reservation before actual exit;
- failure and duplicate/late terminal events cannot double-release resources;
- queued cancellation causes no native effects; unrelated tasks remain live;
- teardown cancels owned tasks and reports unresolved workers honestly.

Test those transitions with a deterministic adapter before a native integration.
The first page must prominently show `model`, `browser` or `native-wasm`; a task
must not self-promote that status merely by returning plausible image bytes.

## Pulse, threads and cancellation

The current export pool can create multiple WASM instances, each initializing a
Rayon pool. The existing heuristic uses browser logical-core count, not a global
CPU/memory admission decision. Replace independent pool sizing only after a
shared budget is in place. Account for active codec threads across instances,
interactive work, background exports, source copies and retained decode caches.
Never synchronously wait for work queued behind the same exhausted worker pool.

A postMessage cancel handler cannot run while its worker is inside synchronous
WASM. Cooperative interruption needs either a shared control cell observed by
native Pulse checkpoints or resumable work that yields to the event loop. A
shared-cell bridge must define initialization on Rayon workers, alignment,
handle lifetime, generation and memory-growth behavior. Measure checkpoint
cost. Do not silently replace progress-aware Pulse with cancellation only.

Without that bridge, worker termination is an explicit fallback, labeled as
termination, with instance/cache recreation costs. Keep it distinct from a
native Cancelled receipt. COOP/COEP and SharedArrayBuffer are deployment facts:
show whether isolation and threads are actually available, with a supported
single-thread path. Never count cancellation-request acknowledgement as exit.

The existing native request-drop qualification is in zencodec's
[Tokio execution guide](https://github.com/imazen/zencodec/blob/main/docs/tokio-execution.md).
It proves native JPEG checkpoints on native threads, not this WASM bridge.

## Memory, estimation and publication

Display separate values: observed peak, proven upper bound, empirical estimate,
unknown, and reserved budget. Never label WASM linear-memory size as live heap
usage or claim that freeing an allocation shrinks linear memory. Account for
transferred buffers, current strips, raw intermediates, replay/spool storage,
compressed output, canvas/GPU copies and per-instance memory high-water marks.
Unknown is not zero. Strict admission refuses unknown required bounds before
preparation; ordinary mode can choose an explicit compatible alternative.

Buffered encoders should finish and release raw state before waiting on slow
publication, using independently budgeted compressed storage. Sequential live
output is allowed only for qualified configurations and pauseable upstreams.
Output timing is not a memory bound. Re-estimation after discovering dimensions,
passes or late metadata must occur before the newly required allocation/effect.
Do not deadlock by having every job retain partial admission while waiting for
more. Cancellation, timeout and HTTP partial publication remain distinct errors.

Begin with a fixed global budget. Adaptive policies must record workload class,
queue delay, compute time, network wait, native thread count, throughput, tail
latency, memory, context switches and cache counters. Preserve failed/interfered
runs. Compare against fixed limits on held-out sequences; predeclare acceptance.
Browser-only metrics cannot establish whole-process peak memory or cache churn.

## Curated assets and expected results

Commit an asset manifest, not large media binaries. Each real fixture needs a
stable ID, URL, hash, license/provenance, byte length, format/container, expected
inventory and an oracle. Resolve shared corpora through codec-corpus and retain
links. Missing assets are an explicit failing gate, not a skipped test.

Start with a deterministic generated RGBA image (known dimensions and samples),
then pinned baseline/progressive JPEG, alpha/16-bit PNG, AVIF 8/10/12-bit, VP8 and
MP4 short clips, TIFF multipage/SubIFD pyramids and JXL frame/layer cases. Include
orientation, oversized frame rectangles, alpha blend/disposal, unequal durations,
ICC/CICP and late metadata. Do not infer sample exactness from coded bit depth.
The manifest must distinguish raw-layer samples from fully rendered pixels.

The UI supports browsing the full collection tree, selecting a raw item or a
rendered view, comparing expected/actual pixels, and showing a timeline only when
presentation semantics exist. Do not flatten TIFF SubIFDs into animation or
assume every JXL frame is independently displayable.

## Acceptance and delegation

- Shell: deterministic tests for every transition above; keyboard-accessible
  example picker and controls; no horizontal overflow at narrow widths; model
  status visible; immutable task receipts and unknown-memory refusal controls.
- Native: same controls against actual WASM, worker cancellation/exit and exact
  lossless pixel/metadata assertions. Original correctness tests remain intact.
- Suite: each codec/configuration earns its own evidence row; browser preview,
  source review, native-thread validation and native-WASM validation stay separate.
- Devin loops own only the shell and deterministic tests, one lane at a time.
  No Rust/API design, broad repins, test weakening or independent heavy jobs.
  Root reviews the diff and test receipts before landing. Use the existing editor
  and worker abstractions; leave native integration visibly unqualified until run.
