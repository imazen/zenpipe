# zenpipe

Streaming pixel pipeline monorepo (zenpipe + zencodecs + zenfilters + zenlayout
+ zenpipe-cmd + zeneditor). See README.md for architecture; zencodecs has its
own `zencodecs/CLAUDE.md`.

**Design docs (read before touching the job/JSON/RIAPI surface):**

- `JSON-JOB-SPEC.md` — the zenpipe-native JSON job envelope v1 (design,
  pre-0.1) and the compatibility/lifecycle policy.
- `IMAGEFLOW-PARITY.md` — verified imageflow gap analysis: RIAPI key
  matrix, JSON node matrix, envelope/endpoint coverage, divergence log,
  workstreams W1–W10. Provenance: zenpipe @ da8d8da, imageflow @ 0ba1c9ea.
- `ORDERING-DESIGN.md` — node ordering/coalescing/fusion semantics.

**Architectural hazard:** there are FOUR RIAPI parser implementations in this
repo. Wired: the zennode registry (`#[kv]`/`from_kv` via
`full_registry().from_querystring` — CLI `--qs`, `expand_zen`) and the legacy
`imageflow_riapi` engine (`imageflow-compat` feature). Dead/unwired:
`zenlayout::riapi`, `zencodecs::riapi_parse`, and `src/srcset.rs`
(`expand_srcset`). Do not add a fifth; parity workstream W2 deletes or wires
the dead ones. The doc comments in `src/imageflow_compat/riapi.rs:9,112`
claiming `zenlayout::riapi` is used are wrong.

**Decoder pin:** this repo decodes AVIF through `zenavif → rav1d-safe`, and the
rev that decides *which decoder* is the **zenavif** one — rav1d-safe hangs off
zenavif's own dep line, so a `[patch.crates-io] rav1d-safe` here substitutes
nothing. Four manifests reference `imazen/zenavif` (root patch table,
`zencodecs`'s `zenavif-parse` dep line, `fuzz/`, `demo/crate/`) and they must
all carry the *same* rev: cargo treats `git+URL` and `git+URL?rev=X` as
different sources, so a mismatch puts two copies of the zenavif workspace in one
graph and `At<Error>` stops unifying. `just check-pins` enforces it (and
self-tests itself first). Note also that a `[patch."<git-url>"]` cannot re-pin a
git dep to another rev of the *same* repo at all — cargo rejects it outright:
`patch for X points to the same source, but patches must point to different
sources`.

## Known Bugs

- Fixed during native PNG qualification (2026-10-25): a 97x65 source could
  deliver a 65-row native batch to an admitted 64-row transcode strip and fail
  with `StripGeometry`. The sink now advertises its capacity; shared coroutine
  delivery splits batches. 97x65/257 RGB8 and RGBA16 retain exact independent
  samples with 1/3/64-row buffers (`just checked-transcode-async-check`).

- Fixed in the checked-transcode review (2026-10-25): default PNG color emission
  could add derived cICP beside a retained Display P3 ICC, changing authority.
  The adapter now supplies checked verbatim defaults without overriding required
  policy fields. Packed PNG palette index width also no longer masquerades as
  decoded channel precision. The native regression compares expanded pixels
  against an independent PNG reader; `just checked-transcode-async-check`.

- ARM adapter audit 2026-09-06: the stale PDF estimator regression now compares
  dispatch with the actual backend estimate. The corrected expanded suite has
  335 passed / zero failed / 79 existing ignored tests. Native CI explicitly
  enables `std,pdf-decode` for the PDF regression. Records are in
  `benchmarks/arm_audit_2026-09-06/`.

- **AVIF decode is not covered by CI here.** No job in `ci.yml` enables
  `avif-decode` (the widest feature list, in the `zencodecs` leg, stops at
  `raw-decode-gainmap`), and every real AVIF decode test is `#[ignore]`d —
  `corpus_avif_decode_valid` / `corpus_avif_invalid_no_panic` need
  `codec-corpus` (network on first run), and the three
  `avif_hdr_fixture_*` tests need `/mnt/v/input/`. So a decoder change lands
  here silently. Run them by hand:
  ```
  cargo test -p zencodecs --no-default-features \
    --features std,cms,avif-decode,jpeg,webp,png,gif,gif-zenquant,png-zenquant \
    --test corpus -- --ignored avif
  ```
- **The coordinated media dependency pin advances to `rav1d-safe dfd01e32`**
  via `zenavif b777fa12`. The former aarch64 film-grain reservation blocker
  **rav1d-safe#526** closed with **#527**, commit `83fa5d3edd5137f1b0b1d981625c0ba28ae79a49`,
  an ancestor of this pin. Keep all root/demo/fuzz manifests and authoritative
  locks consistent; the pin checker rejects drift. Re-run the corpus command
  above for decoder changes; x86 validation does not replace the upstream ARM
  conformance evidence.

Two 2026-07-11 fix waves (009c7938..f7d1900d, then c75ca304..) closed the
original list — animation per-frame routing, matte flatten, the two-engine
parity suite (43 cases), maxwidth/maxheight bounding, larger_than,
scale=canvas inner-box, encode-node/quality-intent folding, limits
threading, hdr directives, compat security. Verified remaining:

- **Decoder downscale hints (W6, blocked on siblings)**:
  `jpeg_downscale_hints` / `webp_decoder_hints` /
  `decoder.min_precise_scaling_ratio` are parsed but produce no scaled
  decode — zenjpeg exposes no public scaled-IDCT knob and zencodec's
  DecodeJob has no scale hint; the generic decode node's `min_size` is
  unwired. Perf-only (output pixels are correct, just slower). Needs
  sibling-crate API work (zenjpeg/zencodec).
- **W10 remainder — exact native-unit per-codec encode params**: per-codec
  `quality`/`effort` keys now apply via decision hints on the GENERIC
  calibrated scale; exact native units plus the non-decision params
  (progressive, subsampling, quant tables, sharp_yuv, …) need per-codec
  `CodecConfig` boxes wired from encode nodes (zencodecs `config.rs` has
  jpeg/webp/gif/png/avif boxes; JXL/TIFF/HEIC boxes don't exist yet).
- **`AllocationTracker` is API-only by decision**: superseded by the
  orchestrate estimate gate + codec-level `ResourceLimits`; scheduled for
  removal in the pre-0.1 public-API sweep rather than wiring a duplicate
  accounting path.
- **compat `ExecutionSecurity` partial**: `max_threads` (zencodec
  `ThreadingPolicy` can only express sequential-vs-parallel),
  `max_json_bytes` (compat takes pre-parsed types — enforce where JSON
  actually enters), and `mem_budget_policy` (needs an estimate hook on the
  compat path) remain unenforced; sizes, input bytes, and total file
  pixels are enforced.
- **mode×scale approximations (documented, parity-suite-visible)**:
  `(crop, scale=canvas)` → WithinCrop (imageflow: partwise crop + virtual
  canvas); `(stretch, canvas)` → Distort without canvas pad.
- **Generated docs regen pending**: `docs/querystring.md` / `docs/nodes/`
  don't list the hand-written RIAPI adapter keys (crop/flip/rotate/
  srotate/sflip/autorotate/autodeskew/frame/roundcorners/icc/hdr) — the generator
  only walks `#[kv]` params. Give adapter schemas synthetic ParamDescs or
  extend the generator, then regenerate. (`docs/querystring.md:159` /
  `docs/nodes/zenlayout-orient.md` still show `Orient` bound to `srotate`;
  since 2026-08-27 its key is `orientation` — regen will fix that too.)

## Known Bugs: animated routes and qualification gaps (2026-10-28)

- Fixed (`3c688575`): an animated transcode whose decoder renders frames in a
  layout other than the encoder's (zenwebp renders an opaque lossy animation as
  RGB8) panicked in `FrameSink`. Each frame now converts to the encoder's
  format. Evidence: `benchmarks/pulse_animation_qualify_2026-10-28.md`.
- Open (#111): `zencodecs::transcode()` is still-only. Given a multi-frame
  source, it encodes the first frame and drops the rest without reporting it.
  Nothing changes until an API proposal is reviewed.
- Open: `imageflow-compat` (`std,imageflow-compat,nodes-jpeg,nodes-png` lib)
  fails strict Clippy with 65 pre-existing lints, all in
  `src/imageflow_compat/*`
  (`scripts/qualify/imageflow-compat-clippy-baseline-60127aaf.tsv`).
- Open: four `zencodecs/tests/stop_and_limits.rs` tests are ignored with the
  reason "PNG codec adapter not yet wired into zencodecs", yet pass with
  `--ignored`. The attributes stay until reviewed.
- Open: `PipeError::Cancelled` carries no stop reason. A caller's `TimedOut`
  and a codec's typed stop error are lost.
- Unmeasured: stop latency. Every `zencodecs/tests/stop_latency.rs` test is an
  `#[ignore]` benchmark (`just stop-latency` runs them). They used to count any
  error as a stop; they now report only a stop as one, with its reason.
- Open: four natives predate the zencodec 0.2 Pulse contract, and their features
  do not build against the core: zenavif, heic, zenextras (zenpdf, zentiff) and
  zenraw. No compatible head existed on 2026-10-28
  (`benchmarks/native_pulse_heads_2026-10-28.md`).
- CI blind spot: each extras step stops at its first failure. At `363f7c50` the
  `pdf_` and `zeneditor` failures kept the Test extras for zeneditor, the
  raw-decode set, `metadata_scrub`, `tools/metadata-services` and zenfilters,
  and the Clippy extra for zenfilters, from running.

## Known Bugs: single-codec subset warnings (fixed 2026-10-28)

zenpipe `std,nodes-jxl` builds zencodecs with only `zennode,jxl-decode`; that
subset had 13 strict-Clippy warnings (encoder-only helpers and the owned
streaming shim compiled without a user). `just clippy-codec-subsets` lints every
`nodes-<codec>` subset; pass NODE=CODEC pairs to check a few.

## Known Bugs: optional decoder limits warning (fixed 2026-10-25)

The JPEG/PNG/CMS-only configuration exposed an unused decoder-factory limits
parameter when WebP was disabled. The parameter remains forwarded in the WebP
arm; `just checked-transcode-baseline` verifies this smaller configuration.

### Session benchmark main-thread stack (2026-10-25)

Windows x64 and ARM64 CI reached `benches/session_cache.rs` after the ordinary
correctness tests, then the standalone benchmark executable overflowed its main
thread stack before reporting a sample. The benchmark now enters through a named
worker with an explicit 8 MiB stack reservation on every platform; all benchmark
bodies, assertions and zenbench process options are retained. This is a reservation,
not a measured live-memory bound for the codecs. `just session-stack-check` runs
the nested decode/crop/resize/filter path; Windows execution remains a CI gate.
