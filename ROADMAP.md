# zenpipe roadmap (2026-10-25)

Where zenpipe stands after the June–October work, what changed beneath it, and
the order of work to make it a production media pipeline with an interactive
workbench. Baseline for "since May": `f70d4118` (2026-05-31). Evidence for each
claim is a command, file or issue named next to it.

## 1. What changed in zenpipe since May

342 commits, 407 files, +57,375 / −15,327 lines (`git diff --shortstat f70d4118 HEAD`).

Landed:
- Colour and HDR correctness: `EncodePolicy::strict` checks before pixel
  negotiation (#97), an explicit per-request HDR rendering policy (#102),
  EXIF ColorSpace kept in step with the written colour (#98), gain-map re-embed
  on transcode (#46), the linear-float PNG transfer tag fix.
- Streaming output: `EncoderSink`/`TranscodeSink` emit bytes as the encoder
  produces them (#95); the streaming preference reaches encode requests (#101).
  Row contracts are enforced across dispatched encoders.
- Cancellation: one job deadline reaches every decode/encode request (#83).
- Metadata dispatch through zenmeta (#90); an encoded-image inventory CLI.
- Test infrastructure: a codec operation matrix read back by independent
  decoders (CI job), a 264-cell streaming transcode matrix, metadata
  conformance, a reader-compatibility study (`docs/reader-compat.md`), 23
  fuzz crash issues closed since June, a 43-case two-engine RIAPI parity suite.
- Seams: AVIF auto-tune consumer, tile pyramids, `Session` prefix caching.

Not moved since May:
- JSON job envelope v1 (`JSON-JOB-SPEC.md`, workstream W3) is still a design;
  `zenpipe-cmd` still parses its own `JobDef`.
- The dead RIAPI parsers (`zenlayout::riapi`, `zencodecs::riapi_parse`, W2)
  are still in the tree.
- Scaled JPEG decode (W6) is still blocked: zenjpeg main has no scaled-IDCT
  knob and zencodec has no decode scale hint.
- The editor: `zeneditor` and `demo/` are unchanged in substance; the six-phase
  end-to-end plan in `demo/SPEC.md` §1.3 has not started.
- imageflow does not use zenpipe; its bridge PR (imageflow #703) has been idle
  since April.
- Neither `zenpipe` nor `zencodecs` has ever been published.

Costs that grew:
- **Dependency choreography.** 70 of 342 commits are pin, lock or graph fixes.
  The root `[patch.crates-io]` carries 29 git entries and the repo holds 7
  lockfiles. Most codec mains are 3–6 months ahead of crates.io (zenpng 0.2.0
  vs 0.1.4, zengif 0.8.0 vs 0.7.3, zenavif 0.2.0 vs 0.1.6, zencodec 0.1.27 vs
  0.1.26).
- **Recipe and doc sprawl.** `justfile` has about 80 recipes, many specific to
  one PR; eleven top-level design docs; `CONTEXT-HANDOFF.md` describes the
  editor demo as of March.
- **CI on main is red on Windows** (x64 and ARM64) since 2026-10-19: stack
  overflow in the `session_cache` bench (#104).
- **CI tests sibling mains, not the lockfile.** The shared `rust` job clones
  sibling repos at main (`cargo superwork ci-clone --add-paths`), so a sibling
  commit can turn zenpipe main red with no zenpipe change. On 2026-10-25 the
  pre-change commit failed on re-run exactly as the new one did: zencodec's
  new default `Encoder::begin` refuses row-level WebP encode (zencodec #164),
  and rav1d-safe main does not compile off x86_64 (rav1d-safe #536). Only the
  "Committed dependency graph" job checks what the lockfile ships.

## 2. New abstractions beneath zenpipe, and adoption

| Sibling capability | State | zenpipe use today |
|---|---|---|
| zencodec streaming encode output (`OutputTiming`, `pending_output`, `take_output`) | merged | adopted (#95, #101) |
| `DecodePolicy`/`EncodePolicy`, colour authority | merged | adopted |
| zenmeta (filter, rewrite, audit) | merged; C2PA audit in zencodec #157 | partial (#90); open stack #86 → #84 → #87 |
| `zencodec-io`: sans-I/O drivers over slice, `ReadAt`, `Read`, tokio; `transcode_async` | merged, unpublished | **none**: inputs are whole `Vec<u8>` |
| `zencodec-coroutine` (incremental decoders as async code) | merged | n/a (codec-side) |
| `zencodec::time` (`TimeBase`, `Timestamp`, `Rounding`) | merged | **none** |
| `zencodec-media`: packets, tracks, refcounted frames, sessions, demux/mux, animation over video | merged, unpublished | **none**; its design assigns the session pump, registry, `Video::open` and CLI to zencodecs |
| zencodec `Fidelity` targets (ssim2, butteraugli, codec quality) | merged | **none** in the job surface |
| `how-far` (`&dyn Pulse`: progress + cancellation in one handle) | 0.1.0 published | **none**; zenraw adopted; zenresize draft #16 |
| zenpixels 0.2.17: `SampleEncoding` (10/12-bit), `PixelBuffer` parts, `RowConverter::prepare`, checked CICP; legacy `planar` deprecated | main; 0.3 PRs open | partial (`RowConverter`) |
| zenresize: resampling conventions (#18), NoHalo/LoHalo (#22), `try_resize*` stop variants (#15) | open PRs | n/a until merged |
| zentone 0.2 (BT.2408 policy) | main | admitted; tone-map PR #100 open |
| zencodec-testkit contract checks; zencodec-media-testkit | merged | used by codec repos, not by zencodecs' own tests |

Video and animation codecs:
- AV1 sessions in zenavif (#69, merged): rav1d-safe decode with zero-copy
  frames; zenrav1e encode (8/10/12-bit, all subsamplings, lookahead); zenav1-svt
  encode is all-intra only while its inter frames are corrupt (zenav1-svt #26,
  open). zenav1-aom has no video session (its key-frame encoder writes a
  still-picture header). Small-frame and lossless-inter fixes wait in zenrav1e #44.
- Containers: `zenmp4` (zenavif #70, merged): sans-I/O MP4 demux and mux,
  progressive and fragmented, seeking, FFmpeg oracle. `zenwebm` and `zenivf`
  live in zenextras.
- Qualification research (zenextras branch `research/media-codec-qualification`):
  a safe-Rust VP8 decoder byte-exact against libvpx; no H.264 or audio backend
  qualifies yet.

## 3. Priorities

### P0. Release train: publish the cohort, delete the patch table

Every later item is cheaper once zenpipe builds from crates.io. Publish in
dependency order, each after README review and green CI on every required
platform: zencodec 0.1.27 + zencodec-io + zenmeta → zenpixels 0.2.17 →
ultrahdr → the codecs (zenjpeg, zenpng, zenwebp, zengif, zenavif, zenjxl,
zenjxl-decoder, jxl-encoder, heic, zentiff, zenraw, zenbitmaps) → zennode →
zensim 0.3 → zenanalyze/zenpredict/zenpicker → zencodecs 0.1 → zenpipe 0.1.

Acceptance: the root `[patch.crates-io]` table is empty; fuzz, demo and
metadata-services lockfiles resolve from the registry; `just check-pins` has
nothing left to check.

Before it: fix Windows CI (#104); land or close the open stack #86 → #84 → #87
and #100; delete the dead parsers (W2).

### P1. End-to-end coverage for zencodecs

Today: 40 integration test files, the operation matrix with independent
readers, the streaming transcode matrix, metadata conformance. Gaps: real-image
corpus tests are `#[ignore]`d and need the network; AVIF decode has no CI leg;
no transcode asserts perceptual quality; wasm32 is not run end to end; the
all-pairs model in zencodec `docs/codec-operation-matrix.md` is a proposal.

Build one test registry (a `publish = false` crate) that generates both the
execution and the coverage table: fixture × decoder configuration × encoder
configuration × path (one-shot, streaming, erased dispatch, wasm32) × policy.
Each cell records admission or typed refusal, geometry and orientation, colour
authority, metadata carriers, encoded bytes, time, and zensim against the
source; every encoded file is read back by an independent decoder.

- **PR tier** (minutes): committed fixtures from codec-corpus spanning tiny,
  small and ~1 MP sizes, alpha, 16-bit, HDR/gain-map, animation; all format
  pairs; every feature leg CI already builds; x86-64, aarch64, i686, wasm32
  under wasmtime. Expected outcomes are explicit: accept with named
  observations, reject with a named cause, or unavailable in this build.
- **Corpus tier** (opt-in through an env var set by a `just` recipe and a
  manual workflow; never a silent skip): a stratified imazen-26 subset (k-means
  on zenanalyze features, every category, the 76 gain-map images) over q5–q95
  plus lossless. Results go to a committed `benchmarks/` table with commit,
  host and grid; a later run fails when zensim or bytes regress beyond the
  recorded spread.
- Run each adapter's `zencodec-testkit` checks from zencodecs too, so the
  dispatch layer is held to the same contracts as the codecs.

### Test wall time (measured 2026-10-25)

Measured on a 20-thread machine, 2,308 tests over libs, bins, tests and
examples:
- Dependencies at opt-level 0 made test execution 99 s under nextest (177 s
  sequential under `cargo test`), 97 s of it one test (`transcode_matrix`).
  Dependencies now build at opt-level 2 with checks unchanged: 8.5 s, same
  results. One-time cost: 143 s.
- `cargo test --all-targets` also runs the four zenbench benches as full
  debug benchmarks: 53 s, 95 s, 136 s, and `kernel_tiers` from 126 s to over
  1,200 s depending on load. They are most of the suite's wall time, their
  debug numbers mean nothing, and `session_cache` is the Windows stack
  overflow (#104).
- nextest cannot list them: zenbench's `main!` ignores `--list --format terse`
  and runs the benchmark, so `just nextest` excludes benches.

Next: give zenbench a test mode, as criterion has. Under `cargo test`
(no `--bench`), run each benchmark body once; answer `--list`/`--exact` in
libtest's format so nextest runs each body as a test. Coverage stays the same
(every body still executes), and the minutes go away. Combining the 84
integration test binaries into a few would then cut link time; measure link
time per binary with `cargo build --timings` before doing it.

### P2. Adopt the new abstractions

In order of user-visible payoff:
1. **Inputs from `zencodec-io`.** Accept `ReadAt`, `Read` and async sources in
   `ImageJob`, so probe and inventory read only the bytes they need (HTTP
   range reads), memory stays bounded on a server, and `transcode_async`
   streams from socket to socket.
2. **`how-far` progress.** Take `&dyn Pulse` where execution takes `&dyn Stop`
   today; report per-node and per-frame work. This is what a live workbench
   and a server progress stream consume. Coordinate with zenresize #16,
   zenrav1e #43 and zenav1-aom #19.
3. **Video through `zencodec-media`.** Implement the zencodecs session pump,
   registry entries and `Video::open` the media design assigns here, behind
   default-off features. First deliverable: animation ⇄ video (GIF, APNG,
   animated WebP ⇄ AV1 in MP4/WebM), plus poster-frame extraction. Blocked on
   publishing zencodec-media; AV1 encode uses zenrav1e until zenav1-svt #26
   closes.
4. **Quality targets.** Map zencodec `Fidelity` targets to the JSON `target`
   parameter of the envelope (zensim, ssim2, butteraugli).
5. **Scaled decode (W6).** Propose a zencodec decode scale hint and a zenjpeg
   scaled-IDCT path; the largest single speedup for thumbnail workloads.
6. 10/12-bit frames through `SampleEncoding`; `RowConverter::prepare` to hoist
   per-job setup; resampling conventions and halo samplers once merged.

### P3. One interface, three syntaxes

Implement envelope v1 (W3): `Job::from_json`/`to_json` that round-trips, a
capabilities document exported from the zennode registry, structured warnings
and errors with HTTP mapping, and a trace event stream (per-node timing,
memory, progress). `src/trace.rs` already records DAG snapshots, per-node
timing, strip events and a memory timeline after a run; the change is to emit
them while the job runs. Replace `zenpipe-cmd`'s `JobDef` with it and fold
`zencodecs-cli` into one CLI. Then the RIAPI querystring, the JSON job, the
Rust builder and the workbench graph are four views of one structure.

### P4. Workbench

One job document runs on interchangeable executors:
- **Browser executor**: zenpipe + zencodecs on wasm32 in a Web Worker pool
  (`demo/worker-pool.js` exists), threaded through wasm-bindgen-rayon where
  cross-origin isolation is available (#21; `demo/coi-serviceworker.js` exists).
- **Server executor**: `zenpipe serve` (axum): `POST /v1/jobs` with uploaded
  or URL inputs, a server-sent event stream of progress, trace and preview
  thumbnails, `GET /v1/capabilities`, and a caching proxy for corpus images.
- **Parity mode** runs a job on both and diffs the outputs (bytes for
  deterministic encoders, zensim otherwise), which doubles as the wasm
  end-to-end test.

The UI:
- Media sets: imazen-26 over its public R2 index (originals and the PNG-v3
  SDR/HDR renders; URLs come from the measured index in `ACCESS.md`, never
  from swapped extensions), codec-corpus, local folders. The 32
  redistribution-unverified screenshots are excluded by default.
- A step editor generated from the registry's JSON Schema (no hand-written
  forms), with querystring ⇄ JSON ⇄ graph views kept in sync.
- Live runs: per-node progress, strip timing and memory from the trace,
  intermediate previews from `Session` taps, cancel-and-rerun on edit with
  prefix caching.
- Batch grids over a set: bytes, bits per pixel, encode time, zensim/ssim2,
  failures; swipe, flicker and heat-map diff views; quality sweeps drawn as
  rate-distortion curves; executor A vs B and codec A vs B.
- Video and animation: a frame scrubber once P2.3 lands.

`zeneditor` (the `Command`/`ViewUpdate` protocol, history, sessions) stays the
single-image editing core inside it.

## 4. Milestones

| | Deliverable | Depends on |
|---|---|---|
| M0 | Windows CI green; open PR stack landed; W2 done; cohort published; patch table empty | user release approvals |
| M1 | zencodecs e2e registry, PR tier in CI on four targets | M0 for wasm32 graph |
| M2 | Envelope v1, capabilities, trace events; one CLI | — |
| M3 | Server and browser executors running the same job; parity mode | M2 |
| M4 | Workbench over imazen-26: sets, step editor, live runs, batch grids | M3 |
| M5 | Animation ⇄ AV1 video in MP4/WebM | zencodec-media publish, zenrav1e #44 |
| M6 | `ReadAt`/async inputs, `how-far` progress, scaled JPEG decode | zencodec, zenjpeg API PRs |
