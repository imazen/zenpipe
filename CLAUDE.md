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

- Fixed in the optional Pulse candidate (2026-10-29): pipeline execution pulled
  a source before checking cancellation and could finish a sink after a cancelled
  empty pull. New effect controls require zero source reads for pre-cancellation
  and no sink delivery/finish after cancellation during a pull. The 29-test limits
  target and strict Clippy passed on 7c36d596/921a3f9f; session cache tests also pass.
  Native animation/processing migration is separately qualified.

- Fixed in source during publication qualification (2026-10-28): the browser
  fallback supported export but rejected `encode_preview`, leaving the preview
  hidden and its statistics empty. The worker now uses its existing canvas
  backend for previews and reports the actual encoded MIME. Two Node protocol
  controls fail on the old worker and pass on the repair; actual browser
  readback and the unchanged full suite remain unrun for this source change.
  See `docs/publication-qualification.md`.

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

### Windows session-cache benchmark stack overflow (2026-10-19)

Both Windows x64 and ARM64 full test jobs abort in the debug all-target
`session_cache` benchmark after JPEG generation and zenbench calibration,
with `STATUS_STACK_OVERFLOW`. This occurs at both 47695c06 and 5adaa204;
the overflowing stack frame has not been identified. Track the finding in
[issue 104](https://github.com/imazen/zenpipe/issues/104). No test, workload,
stack-size setting or expectation was changed to bypass it.


- 2026-10-19: the locked AVIF cohort loses Display-P3 primaries in `metadata_conformance::cicp_color_signaling_without_matrix` (12/13/0/full becomes 2/13/6/full). The unchanged test passes against the current-main encoder/decoder cohort with 12/13/6/full. The responsible dependency has not been isolated; [zenpipe issue 103](https://github.com/imazen/zenpipe/issues/103) tracks the pin refresh.
- 2026-10-19: floating-point HDR input to TIFF fails with the default integer predictor on both one-shot and streaming paths. The 264-cell streaming matrix retains this failing case; [zenextras issue 52](https://github.com/imazen/zenextras/issues/52) tracks the codec fix.

- Fixed during metadata integration: the operation catalog still called the legacy
  unsupported-timing checker for GIF after its adapter gained rational timing.
  It now asserts the advertised capability and checks exact 10/20 ms frame
  durations, pixel order and typed rejection of 1/3 second through both generic
  and erased encoders. The rejected frame must not mutate the animation.

- HDR rendering on PR100 required an explicit per-request policy and a common
  missing-peak fallback. The proposed `tonemap` followup uses
  `ToneMapPolicy::Bt2408` and the same metadata-or-1000-nit peak for whole
  images and strips. The feature alone retains the existing encode contract.
  Validation commands: `just test-tonemap-policy`, `just clippy-tonemap-policy`.

- Fixed: `transcode_matrix` explicitly chooses sRGB8 clipping for HDR-to-GIF in both paths, since RGBE reconstruction can exceed GIF's finite SDR range. All source/target cells and success/pixel-identity assertions remain. A white RGBE regression requires native refusal and clipped white output; `just test-hdr-gif` and `just clippy-hdr-gif` pass. `just test-hdr-gif-oracle` checks both outputs' pixels with ImageMagick. Encoder defaults and public APIs are unchanged.

- Dispatched native row encoders could accept repeated declarations or one-shot encoding after begin, and empty WebP/AVIF inputs could panic. The private row guard validates declarations, counts emitted bytes, poisons failed pushes, and matches begin/encode empty-canvas acceptance with the same typed validation error or identical finished bytes. `zencodecs/tests/streamed_bytes.rs` covers the enabled encoders. Rotated buffered encodes apply dimension limits to the output canvas.

- Fixed: strict one-shot encoding checked color policy after pixel-format negotiation, which could convert Display P3 to sRGB and make an otherwise forbidden fit look harmless. The check now runs against the original description first. `strict_refuses_conversion_when_descriptor_matches_source_color` reproduces the earlier successful 146-byte PNM and requires rejection in both paths.

- Fixed in the streaming audit: the old PNG pin stored linear-float input as sRGB bytes while retaining a linear CICP tag. `zencodecs/tests/color_adapt.rs` measured a maximum byte error of 70; the current PNG pin includes its transfer-tag correction. This test passes against the committed graph.

- Fixed in the streaming audit: `EncodeRequest::encode` rebuilt a validated row view through an adapter that required final-row padding. Passing the validated `PixelSlice` into dispatch preserves short final rows. `zencodecs/tests/strided_encode.rs` compares packed, padded and short-final-row inputs.

- Streaming output regressions fixed in the post-landing review: encoder/writer errors could still be finalized or retried, and buffered row fallback did not enforce protocol state or admission limits. Regression coverage is in `tests/sink_output_contract.rs`, `zencodecs/tests/transcode_output_contract.rs`, and `zencodecs/tests/buffered_encode_contract.rs`. The dispatch byte matrix is separate from pixel-equivalence coverage.

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
- **zenavif is pinned to main `5e493d39`** (rav1d-safe `dfd01e32`) since
  2026-10-15. The earlier hold at `11033c95` / rav1d-safe `140f9145` was for
  an aarch64 film-grain panic (rav1d-safe#526, closed 2026-09-06); the command
  above passes at the new pin on aarch64. Keep every zenavif rev in the repo
  equal (`just check-pins`), and re-run the command when moving it.

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
  jpeg/webp/gif/png/avif boxes; JXL/TIFF boxes don't exist yet).
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

## Known Bugs: single-codec subset warnings (2026-10-29)

zenpipe `std,nodes-jxl` builds zencodecs with only `zennode,jxl-decode`; that
subset had 13 strict-Clippy warnings (encoder-only helpers and the owned
streaming shim compiled without a user). Main integration exposed the same
class of warnings for AVIF/HEIC/JXL decode-only nodes. Native encoder helpers
now compile only with an encoder feature; no lint allowance is added.
`just clippy-codec-subsets` lints every `nodes-<codec>` subset; pass NODE=CODEC
pairs to check a few. Qualification is recorded in docs/current-main-integration.md.

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

## Known Bugs: AVIF decode-only test helpers (fixed 2026-10-29)

Seven strict-Clippy errors arose because encode-only helpers/imports in
`zencodecs/tests/avif_capability.rs` compiled without `avif-encode`. Matching
the helpers to their consuming tests preserves all tests and assertions.
`just clippy-codec-subsets nodes-avif=avif-decode` passes at b7f8ff79; see
benchmarks/optional_pipeline_pulse_2026-10-29.md.

## Known Bugs: portable serializer source split (2026-10-29)

The decoder-pin gate fails at3df0dd88 even after root lock qualification:
zenavif94116d26 selects zenravif aac04acc, which explicitly selects
zenavif-serialize98c8a501 from the canonical zenavif git repository. Root
metadata confirms that sole serializer parent. This is a transitive explicit git
edge, so adding a registry patch or merely refreshing the root lock cannot align
it with94116d26. Preserve the pin gate. The portable repair needs a coordinated
zenravif dependency commit, the consuming zenavif pin, and all zenpipe graph roots
(including the direct parser edge), followed by native encode/readback controls.
No pixel defect is established by this source-identity finding alone. Existing
zenavif issue53 concerns historical serializer behavior, not proof of this fix.

Resolution on the optional Pulse branch: zenravif `52bb0243` makes the serializer
a registry edge; zenavif `f0a3919e` and all five application-root serializer
patches now agree. Both source/family validators pass on `96972529`; the prior
failure remains recorded above. Native consumer controls remain a separate gate.
See `benchmarks/portable_serializer_2026-10-29.md`.

Native gate `avif-consumer-contract-check` on `4fd1d7ec` resolves successfully
but fails 3 orientation controls (8 pass, 4 pre-existing ignores). Two existing
metadata tests require carrier preservation while native still encoding bakes
orientation; separate diagnostic witnesses confirm that bake. The third failure
exposes zencodecs dropping `OrientationHint` before AVIF decode. Do not weaken
these assertions to declare the graph release-qualified.

## Fixed: full-profile RAW gain-map controls (2026-10-29)

Paired HEIC/RAW adapters compile under the full generated-API feature profile
on3ae78770. A nested RAW JPEG gain-map decode still passed None to an owned
Pulse argument; it now retains the operation's Pulse and limits and propagates
decode errors instead of reporting absence. Full-profile strict Clippy and
real JPEG timeout/limit/malformed-input controls pass. Complete Apple DNG
container coverage and aggregate memory accounting are not established by
that focused control. See `benchmarks/full_profile_2026-10-29.md`.

### Comparing unpublished API surfaces

`just semver-against <reviewed-source-export>` builds stable-toolchain rustdoc JSON inside the actual workspaces and compares the resulting API graphs at patch level. This preserves root git patches for unpublished dependencies that a synthetic outer workspace loses. `just test-streaming-seam`, `just test-dispatched-output`, and `just streaming-clippy-gate` exercise the dispatched row and transport contract.

## ImageJob optional Pulse integration (2026-10-29)

Fuzz compilation exposed stale with_stop calls in the feature-gated ImageJob
decode/fallback/encode paths, plus a duplicate animation stop block. Default
zenpipe tests do not compile the job feature. Those paths now use the existing
shared control helper; animation retains its already-attached shared Pulse.
The dedicated integration-job-check recipe covers job and gain-map features.
The combined job/gain-map/node/Tokio/schema runtime and strict Clippy pass.
Original deadline/cancellation assertions are retained; see
`docs/current-main-integration.md` for evidence and the five HDR placeholder gaps.

## Known Bugs: demo deployment source drift (2026-10-29)

Deployment still rewrote dependencies to sibling heads and deleted the demo
lock, producing a different graph from portable qualification. It now keeps
the committed graph and builds locked. Ordinary demo/size-shim WASM checks
pass; threaded release/browser qualification remains separate. See
`docs/current-main-integration.md`.
