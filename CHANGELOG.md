# Changelog

All notable changes to the zenpipe workspace are documented here, per crate.
(Started 2026-06-11; earlier history lives in git log.)

## Workspace

### [Unreleased]

#### Fixed (orphaned member lockfiles / Dependabot, 2026-08-29)

- **Deleted `zeneditor/Cargo.lock`, `zenfilters/Cargo.lock` and
  `zenpipe-cmd/Cargo.lock` — cargo never read them, but GitHub did.** All three
  directories are `[workspace] members`, so cargo resolves them against the ROOT
  `Cargo.lock`; a lockfile inside a member is dead weight. Proven rather than
  assumed: `cargo locate-project --workspace` run from each of the three returns
  the root `/Cargo.toml`, and `cargo update -p rand` invoked inside `zeneditor/`
  reports the ROOT lock's `rand@0.9.4` / `rand@0.10.1` — not the `rand 0.9.2`
  sitting in `zeneditor/Cargo.lock`. Nothing in CI, the justfile, or any script
  referenced them. `demo/crate/Cargo.lock` and `wasm-size-shim/Cargo.lock` are
  **kept**: those two are in `exclude`, `locate-project` resolves each to itself,
  so their locks are live.
- **This was the repo's only open security alert, and it was a phantom.**
  GHSA-cq8v-f236-94qc (`rand >= 0.9.0, < 0.9.3`, low) was reported against
  `zeneditor/Cargo.lock` — the dead file pinning `rand 0.9.2`. The real dependency
  graph has been on `rand 0.9.4` (and `0.10.1`), both patched, the whole time. The
  alert could never be closed by updating anything real, because nothing real was
  vulnerable; deleting the stale file is the fix.
- **It is also why `Dependabot Updates` went red on 2026-04-22 and 2026-05-07.**
  Dependabot treats each lockfile as its own update directory, which is why the run
  titles name `/zeneditor`, `/zenfilters` and `/demo/crate` separately. For a
  directory that is a workspace *member*, the update it computes cannot agree with
  the root resolution that actually governs the build, so the job errors instead of
  opening a PR. Removing the three orphans leaves Dependabot only directories whose
  lockfiles cargo genuinely owns.

#### Changed (zencodec/zenpixels version ranges, 2026-08-29)

- **Every `zencodec` / `zenpixels` / `zenpixels-convert` requirement across the
  workspace now spans the published minor AND the next one** — 15 requirement
  lines in the root `[workspace.dependencies]`, `zencodecs`, `zencodecs/fuzz`,
  `zencodecs/zcimg`, `zenfilters`, `demo/crate` and `wasm-size-shim`. `zencodec`
  becomes `>=0.1.26, <0.3.0`; `zenpixels` / `zenpixels-convert` become
  `>=0.2.10, <0.4.0` (`>=0.2.15, <0.4.0` where zenfilters already required
  0.2.15). For a `0.x` crate Cargo treats the minor as the major, so a plain
  `"0.1.26"` meant `^0.1.26` = `>=0.1.26, <0.2.0` and a `zencodec 0.2.0` release
  would have been invisible until all seven manifests were hand-edited — which
  is precisely the coordinated wave the `zencodec 0.1.26` rollout cost this
  workspace. Floors are unchanged (each crate keeps its own minimum) and nothing
  newer is published, so resolution is identical: `cargo metadata` still yields
  exactly one `zencodec 0.1.26`, one `zenpixels`, one `zenpixels-convert`.
  **Why uniformity matters here specifically:** this workspace has repeatedly
  hit the two-copies failure — a graph carrying two versions of one `0.x` crate,
  whose types do not unify, so a trait impl from one copy silently fails to apply
  to the other. Widening only some consumers is how that second copy gets in, so
  the sweep covered every manifest at once. All `[patch.crates-io]` entries and
  the `path =` / `git =` bindings are untouched — a patch replaces the source
  regardless of the requirement; the range removes the need for a future *edit*,
  not the present patch. The standing current-plus-next rule (re-derive the
  ceiling at each release) is documented in the zencodec repo's `CLAUDE.md`.

#### Fixed (dependency resolution + CI coverage, 2026-08-29)

- **The AVIF decoder is pinned instead of floating, and the pin is now
  checked** (`scripts/check-decoder-pins.py`, `just check-pins`, CI job
  "Decoder pin agreement"). This repo decodes AVIF through
  `zenavif → rav1d-safe`, and every reference to `imazen/zenavif` was a git
  dep or patch entry **with no `rev`** — so each fresh resolve re-picked
  whatever `main` was at that moment, and the decoder underneath moved with
  no edit to any manifest and nothing in any output recording it. No CI job
  here enables `avif-decode`, so nothing else would have noticed either.
  - Measured before-state, from the committed lockfiles: **three different
    zenavif revisions in one repo.** Root `Cargo.lock` at `11033c95`
    (rav1d-safe `140f9145`), `fuzz/Cargo.lock` at `7d950f1c`, and
    `demo/crate/Cargo.lock` fallen all the way back to the **registry** at
    zenavif 0.1.6 / rav1d-safe 0.5.7 — its `[patch.crates-io]` entry
    declared but not reflected in the resolved graph at all.
  - Pinned at the level that actually controls the decoder: **`zenavif`, not
    `rav1d-safe`.** A `[patch.crates-io] rav1d-safe` here would substitute
    nothing — patch replaces registry sources only, and rav1d-safe is
    reached through a git-rev dep on zenavif's own dep line. All four
    references now carry the same rev (root patch, `zencodecs`'s
    `zenavif-parse` dep line, and the mirrored tables in `fuzz/` and
    `demo/crate/`). They must stay equal: cargo treats `git+URL` and
    `git+URL?rev=X` as different sources, so a mismatch puts two copies of
    the zenavif workspace in one graph and its `At<Error>` stops unifying.
- **The pin is held at `zenavif 11033c95` / `rav1d-safe 140f9145`, NOT the
  workspace-wide `66f58fa6`** — deliberately, because moving forward was
  measured to break AVIF decode here. zenavif, ravif and zenmetrics are all
  on `66f58fa6`; this repo is the exception until rav1d-safe#526 closes.
  ```
  cargo test -p zencodecs --no-default-features \
    --features std,cms,avif-decode,jpeg,webp,png,gif,gif-zenquant,png-zenquant \
    --test corpus -- --ignored avif
  ```
  | zenavif | rav1d-safe | `corpus_avif_decode_valid` |
  |---|---|---|
  | `11033c95` | `140f9145` | **ok**, 60–66 s |
  | `e4b3820` | `66f58fa6` | **FAILED**, 2 of 2 runs |

  The failure is a panic in rav1d-safe's own bounds-map guard on aarch64 —
  `src/safe_simd/filmgrain_arm.rs:1628:41 took a 122880 B picture-plane
  reservation while tile threading is active; the measured ceiling for that
  file is 3840 B` — on every worker thread, followed by
  `Option::unwrap()` on `None` at `src/thread_task.rs:534` during unwind.
  `PIC_EXTENT_CEILINGS` is absent at `140f9145` and present at `66f58fa6`
  and no commit in the range touches `filmgrain_arm.rs`, so the guard was
  *added* inside the range: the over-wide reservation is likely older than
  the guard rather than a new regression. Filed as **rav1d-safe#526**.
  - `140f9145` is still on the correct side of the aarch64 NEON conformance
    campaign of 2026-08-07/08 (2026-08-11 vs 08-07/08), which took
    rav1d-safe from **302/766 to 766/766** against dav1d's published MD5
    vectors. What this pin gives up versus `66f58fa6` is
    `Settings::strictness` defaulting to `Strict` (rav1d-safe@2e0f7e8) and
    rav1d-safe#524's x86_64 loop-filter fix. ravif measured decoded pixels
    **identical** across `140f9145 → 66f58fa6` (0 of 400 cells moved), so
    holding here costs no decode correctness on aarch64.
- **`just check-pins` / CI job "Decoder pin agreement"** fails on three
  things, and self-tests itself first so a check that has stopped detecting
  anything fails loudly rather than passing vacuously:
  1. **FLOAT** — a tracked git dep or patch entry with no `rev`.
  2. **DISAGREE** — a `rev` that differs between manifests, or a lockfile
     that resolved something else (including a silent fallback to the
     registry, which is what `demo/crate` had done).
  3. **DEAD PATCH** — a `[patch]` entry cargo did not use. This is the one
     that is otherwise invisible: a `[patch.crates-io]` can only replace a
     package required *from the registry*, so pointing one at a crate
     reached through a git dep leaves it inert, recorded only as
     `[[patch.unused]]` in the lock while the graph resolves something else.
     That is how zenmetrics' patch entry sat dead, and how zentone's
     shootout `rav1d-safe` patch controlled nothing.

  It distinguishes real defects from three things that merely look like
  them, reporting each as a note instead of failing: a patch unused because
  the feature that would pull it in is off (`fuzz/` has one legitimately); a
  stale `Cargo.lock` beside a workspace *member*, which cargo never reads
  (`zeneditor/`, `zenfilters/`, `zenpipe-cmd/`); and a rev inherited through
  a **path** dep into a sibling checkout, which no manifest here can pin
  (`zencodecs/fuzz` reaches zenavif that way and therefore follows whatever
  that checkout is on — currently `66f58fa6`, compile-only in CI).
  `--root` plus `--expect URL=REV` audits a sibling repo; zentone and ravif
  both pass under their own expected revs.

- **The Pages deploy resolves again: carry the sibling's `zenanalyze`
  patch.** zenjpeg `147444fe` moved its own `zenanalyze` dep from a git rev
  pin to a crates.io VERSION (`0.2.0`) resolved through a
  `[patch.crates-io]` at the *zenjpeg* workspace root — one patch there
  collapses the `zenanalyze_api::Offer` contract to a single type. A
  dependency's patch table is invisible from outside its own workspace, so
  every manifest here started asking crates.io for `zenanalyze ^0.2.0`,
  which does not and will not exist (registry frozen at `0.1.0` by the
  crate's 0.1.x API freeze; 0.2.x lives only on git). "Deploy Demo to
  GitHub Pages" deletes both lockfiles and re-resolves, so it failed first
  (runs 33229080325 / 33230408160) while committed locks — still pinning
  the pre-`147444fe` zenjpeg — hid it locally. Fixed by adding the entry to
  all five independently-resolving manifests (`7040aa6a`); same shape and
  reason as zenjxl `1ae0da79`.
- **`demo/crate`'s patch table had lost `zenavif`** — registry 0.1.7 is
  yanked and `zencodecs/avif-decode` requires `^0.1.7`, so a standalone
  resolve of the demo died on "version 0.1.7 is yanked". The Pages job
  overwrites that whole section with the root's (which carries zenavif), so
  only local demo builds saw it (`c36f8c5e`).
- **`wasm-size-shim` resolves again.** The WASM Benchmark workflow had been
  red since 2026-08-27 on `failed to select a version for the requirement
  zenjpeg = "^0.9.0"` — `zenpipe = { path = ".." }` puts zenpipe's registry
  requirements into that graph and several are unpublished (zenjpeg 0.9.0,
  zenwebp 0.5.0, zenpng 0.2.0, zengif 0.8.0), while its committed lockfile
  predated the bumps and hid it. Patch table completed, with the form chosen
  per entry: path for the four crates it also path-deps directly (a git
  source would double the instance and break zencodec type unification),
  git for zenanalyze (absent from the workflow's sibling clone list), and
  path for zenpixels-convert (the git form put two zenpixels in the graph —
  ten E0308s on `PixelDescriptor`/`PixelSlice`). Lockfile regenerated
  (`31a305b8`).
- **Both fuzz sub-workspaces resolve and compile again.** `fuzz/` and
  `zencodecs/fuzz/` are each their own `[workspace]` and inherit nothing
  from the root patch table. `fuzz/` stopped at `zengif = "^0.8.0"`
  (registry tops out at 0.7.3); its table now mirrors the root in full,
  git-form throughout since nothing there binds a local checkout
  (`4fd592d0`). `zencodecs/fuzz/` failed differently — a stale lock pinning
  archmage 0.9.26 against the `^0.9.27` current zenanalyze needs — and was
  regenerated to 0.9.28 (`6d43e25c`). With resolution unblocked, eight fuzz
  targets turned out to have bit-rotted against `Limits`: the new
  `max_output_bytes` field, `with_max_memory_bytes` →
  `with_max_memory`, and u64→u32 width/height builders (`57f3c9f9`). The
  rot accumulated silently because this repo has no Fuzz CI workflow.

#### Added (CI coverage, 2026-08-29)

- **macOS Intel and i686 lanes** (`a3ec316e`). The matrix was Linux x64/arm
  + Windows x64/arm only. macOS Intel comes back via the reusable
  workflow's documented `platforms` opt-in (`macos-26-intel` — zenpipe
  ships no binaries), covering both its clippy and test jobs.
  `i686-unknown-linux-gnu` is a new `cross` job covering the workspace root
  `--no-default-features`, zenlayout `--all-targets`, and zencodecs
  `--no-default-features` — 32-bit is where `usize` narrowing shows up and
  is the closest proxy CI has for the wasm32 pointer width the demo ships
  on. That job deliberately skips zen-workspace's setup action: `ci-clone
  --add-paths` points deps at sibling checkouts above the repo, which
  `cross` does not bind-mount into its container; a plain checkout resolves
  the same graph through the repo's own git-form `[patch.crates-io]`.
- **`turbojpeg` no longer builds on every CI job** (`ee215ac6`). It was an
  unconditional dev-dependency of zencodecs, and cargo compiles every
  dev-dep for `--all-targets` whether a target uses it or not — so the
  `required-features = ["calibrate"]` already on its only consumer, the
  `quality_calibrate` example, did nothing. Every job on every platform was
  building libjpeg-turbo from source through cmake with `REQUIRE_SIMD=ON`,
  which needs an assembler; that broke both new lanes (*"No
  CMAKE_ASM_NASM_COMPILER could be found"* — the `cross` i686 container has
  no nasm, and neither does GitHub's Intel macOS image, where the reusable
  workflow gives a caller no way to inject an install step). Now an optional
  regular dependency pulled in by `calibrate`, the same feature that gates
  the example. A `Cross.toml` that installed nasm into the container
  (`c948aaa0`) went in first and is removed again by the same commit: with
  turbojpeg out of the graph, no i686 lane graph contains an
  assembler-needing crate at all.
- **`quality_calibrate` compiles again** (`05d94908`) — pre-existing rot
  found while confirming the gate: `zenwebp::WebpEncoderConfig` moved into
  `zenwebp::zencodec`, and `encode_full_frame_srgba8_imgref` is now
  `encode_srgba8_imgref`. Renames only, no behaviour change.
- **A Fuzz build gate** (`4b82b6dd`), the missing piece the entry above
  names: `.github/workflows/fuzz.yml` compiles every fuzz target on every
  push, Linux only. `cargo check --all-targets` per fuzz workspace rather
  than `cargo fuzz build` — cargo-fuzz needs nightly plus
  `-Zsanitizer=address` and fully codegens the codec graph with
  sanitizer-coverage instrumentation per target, while every rot actually
  found was an E0063/E0308/E0599 or a resolution failure that `cargo check`
  catches on stable for a fraction of the cost. Real fuzzing stays
  hand-run/nightly. The `zencodecs/fuzz` cell clones the five siblings it
  path-patches into `$GITHUB_WORKSPACE/..` plus a blobless sparse checkout
  of `codec-corpus/crate` (~2 MB, ~1s, against ~700 MB for the full repo);
  `cargo metadata` confirms those six are the whole out-of-repo closure. It
  deliberately skips zen-workspace's setup action for the same reason the
  i686 job does, with one addition: `ci-clone --add-paths` rewrites
  manifests, and a stale fuzz patch table is precisely what this gate must
  see.
- **Committed crash seeds are replayed on stable**
  (`zencodecs/tests/fuzz_regression.rs`, `4b82b6dd`).
  `zencodecs/fuzz/regression/` held five minimized POCs that nothing ran.
  The harness walks the directory and puts every seed through every entry
  point the fuzz targets drive — `decode_full_frame`,
  `animation_frame_decoder`, `decode_gain_map` (behind `jpeg-ultrahdr`) and
  `push_decode` — under the targets' own tight `Limits`. Every seed on
  every entry point, not just the one that found it: they are
  format-detected bytes, and `fuzz_depthmap/` has had no target since
  depth-map decode was removed on 2026-06-25 yet its bytes still exercise
  detection and dispatch. Two assertions keep it from becoming a gate that
  does not gate: the seed directory must exist and must be non-empty. It
  also rides along on the existing `cargo test -p zencodecs ...
  --all-targets` lines, so it covers all five platforms, not just the new
  job's Linux runner. Verified green on its first real run
  (33245135943, ~2 min per cell cold).
- **`just fuzz-check` / `just fuzz-regression`, both wired into `just ci`**
  (`eac1d5e8`) — the same two commands the workflow runs, so `just ci` no
  longer claims to run all CI checks locally while skipping the only gate
  that compiles the fuzz targets. Running them needs the five siblings
  checked out next to the repo, as the recipe comment says.
- **The fuzz gate clones its siblings before `rust-cache`, not after**
  (`4ab400ed`). rust-cache runs `cargo metadata` on the workspace it caches
  to build its key; with the siblings still missing it failed on run
  33245135943 (*"failed to load manifest for workspace member
  .../zencodecs/fuzz/."*). Non-fatal — the right paths were cached and the
  save succeeded — but the action could not enumerate workspace crates, so
  its target-dir cleaning ran blind. Same ordering, same reason, that
  zen-workspace's setup action already documents.

#### Fixed (fuzz-tooling rot, 2026-08-29)

- **`zencodecs`'s `fuzz-ci` / `fuzz-smoke` / `fuzz-deep` recipes ran no
  fuzzing at all** (`ee827ea4`). All three invoked `fuzz_exif` and
  `fuzz_depthmap`, whose targets were deleted along with the hand-rolled exif
  module (`85ecbb5`) and depth-map support (`6cfc21b`) on 2026-06-25;
  `cargo fuzz run` on a name with no `[[bin]]` fails, so the recipes had been
  unrunnable for two months. `zencodecs/CLAUDE.md` still advertised 11 targets
  and named both. Recipes and docs now list the nine that exist, say which two
  went and when, and record that compiling the targets no longer needs nightly.
  Found while wiring the Fuzz gate — same rot class, one layer up from the
  targets themselves.

#### Fixed (CI green on rustc 1.98, 2026-08-27)

- **`cargo clippy -- -D warnings` on 1.98.0** — the new
  `clippy::chunks_exact_to_as_chunks` and `manual_slice_fill` lints plus a
  handful of `collapsible_if` / `too_many_arguments` /
  `field_reassign_with_default` / unused-import sites had turned every
  Clippy job red; fixed per crate (`ce1c1d10` zenfilters, `1a475127`
  zenlayout, `2fc057ec` zencodecs, `88e3654b` zenpipe). `zeneditor` and
  `zcimg` keep `chunks_exact` on purpose — their MSRV (1.85) predates
  `as_chunks`, and clippy's msrv gate does not fire there.
- **`Public API snapshots` job** — `docs/public-api/*.txt` were stale
  against a long run of API growth; regenerated (`2ec122aa`).

#### Fixed (imageflow graph compatibility, 2026-07-16)

- **Multi-input imageflow graphs execute (W5)** — the graph decomposer
  rejected any node with >1 input edge, so `draw_image_exact` /
  `copy_rect_to_canvas` compositing graphs (watermarks, canvas paste-ups)
  failed outright. Canvas edges now form the executable spine and each
  input edge is recursively executed to a bitmap spliced in as a new
  `imageflow.exact_overlay` node (exact placement, optional resize,
  compose or overwrite blend, linear-space source-over matching the
  watermark path). Nested composites recurse (cap 16); >2 predecessors
  or duplicate edge kinds error loudly. Conformance: the graph suite in
  `tests/v2_json_jobs.rs` now executes imageflow's own canonical
  `Framewise::example_graph()` plus pixel-asserted draw/copy/nested
  cases. JSON-JOB-SPEC.md records the permanence commitment: the
  imageflow graph dialect is supported forever, and any future native
  `graph` form must reuse imageflow's nodes+edges shape.

#### Fixed (RIAPI parity wave 2, 2026-07-11)

- **"Resize this GIF" returned a still image** — animated inputs now run
  their node pipeline per frame through a shared helper (timing/loop
  carry over; limits + deadline enforced) in both ImageJob and the
  imageflow-compat executor; static targets keep first-frame semantics
  (c75ca304).
- **Alpha flatten hardcoded white** — now composites onto the requested
  matte via MatteFlattenOp, fed from RIAPI `matte=`/`s.matte=`; also
  fixed dimensionless Constrain nodes erroring as 0×0 targets (c75ca304).
- **`?jpeg.quality=` (and every per-codec encode key) was a silent no-op
  on ImageJob** — encode nodes and QualityIntentNode now fold into the
  codec intent (format forcing + decision-level hints; generic-scale
  caveat documented) (b0e3a822).
- **maxwidth/maxheight were plain w/h aliases** — now imageflow
  `get_wh_from_all` bounding (same-axis min, cross-axis clamps the
  derived axis, maxes-alone become mode=max targets) (5f26bbfc).
- **`mode=larger_than` rejected** — zenlayout's LargerThan was already
  implemented and imageflow-exact (larger_than IS Max+UpscaleOnly);
  the bridge now accepts it and maps max+scale=up onto it (5f26bbfc).
- **`scale=canvas` padded to the raw target** instead of imageflow's
  aspect-correct inner box; RiapiCrop missed the layout_plan coalesce
  group so crop+resize combinations silently dropped the crop —
  both caught by the new suite (975fb69d).
- **compat enforces `ExecutionSecurity::max_total_file_pixels`**
  (frame_count × w × h at probe) (this commit).

#### Added (RIAPI parity wave 2, 2026-07-11)

- **Two-engine parity suite** (`tests/riapi_two_engine_parity.rs`,
  975fb69d): 43 querystrings run through BOTH the legacy imageflow_riapi
  engine and the zen-native path, asserting output geometry within ±1 px
  — the W9 verification layer; it caught three real divergences on its
  first run.

#### Fixed (RIAPI parity wave, 2026-07-11)

- **Native querystring path now matches ImageResizer/imageflow geometry
  semantics** (f378fe02): crop units default to source pixels with
  negative-coordinate + inverted-window handling (was percent, and the
  bridge silently no-opped ALL percent crops due to an x1/y1-vs-x/y/w/h
  param mismatch); `srotate` is degrees again (was misbound to the EXIF
  flag); `rotate`/`flip` are post-resize ops via new PostRotate/PostFlip
  nodes; `mode=max/stretch/aspectcrop/none/carve` parse; mode×scale
  composes per the imageflow 4×4 matrix with dimension gating; bare
  `crop`/`pad` resolve downscale-only (the old aliases upscaled);
  `w=&h=` without mode letterboxes (IR4 pad default, max for
  maxwidth/maxheight); srcset expansion is wired; Constrain
  gravity/anchor (incl. IR4 spellings + `x,y`), bgcolor, zoom/dpr,
  up-vs-down filter selection, unsharp/sharpen_when/scaling colorspace/
  kernel shaping now actually reach execution in fused runs.
- **`?quality=` was dead and flipped `?format=` to auto-selection**
  whenever any quality key appeared — the quality_intent profile default
  ("high") always outranked the fallback (44020afa).
- **s.roundcorners percentage semantics + 4-value form; ignoreicc /
  ignore_icc_errors; CSS named colors + bare hex for bgcolor;
  f.sharpen_when; webp.lossless** (44020afa).
- **`imageflow-compat` did not compile** against either the locked or
  current imageflow_types (`CmsMode` added-then-removed upstream); the
  compat path now owns `CompatCmsMode` and v2_json_jobs' 3-arg callers
  were repaired (009c7938).

#### Added (RIAPI parity wave, 2026-07-11)

- **Limits enforcement end-to-end** (9cb07998): `Limits::to_codec_limits()`
  feeds every decode/encode request; orchestrate runs the compiled graph's
  resource estimate against limits BEFORE execution; new
  `Limits::max_output_bytes` caps encoded output; `max_duration` drives the
  pipeline through `execute_with_stop`; compat enforces
  `ExecutionSecurity::max_input_file_bytes`.
- **HDR / gain-map querystring surface** (f7d1900d): `hdr=preserve|strip|
  reconstruct` (+ `hdr.headroom`, `gainmap=` alias) via the new
  HdrDirectives node — preserve rides the existing resize-safe sidecar
  path, strip drops the gain map, reconstruct produces HDR pixels at
  decode (job-ultrahdr; JPEG UltraHDR today; loud errors otherwise).
- **`zenpipe::riapi` module** (f378fe02): `preprocess_querystring` (srcset,
  legacy pairs, value coercions, IR4 default-mode rule, same-axis maxwidth
  reconciliation) + `riapi_order` (IR4 phase ordering — querystring keys
  have no inherent order).

#### Added

- **`IMAGEFLOW-PARITY.md`** — verified imageflow gap analysis (imageflow @
  0ba1c9ea vs zenpipe @ da8d8da): full RIAPI key parity matrix, six confirmed
  semantic divergences on the native querystring path, imageflow JSON
  node/preset/envelope coverage, limits+animation under-wiring, and
  workstreams W1–W10 to full coverage (a8a7144d).
- **`JSON-JOB-SPEC.md`** — the zenpipe-native JSON job envelope v1 design:
  one-key zennode steps, checkpoint/resume branching, encode-intent step over
  `CodecIntent`/`QualityProfile` with per-codec option objects, response/
  warning/error envelope, capability discovery, and the additive-forever /
  removals-are-loud lifecycle policy; includes the path-to-0.1 checklist
  (b0b712cc).
- **Root `CLAUDE.md`** with the Known Bugs log — twelve source-verified pre-0.1
  defects (mode-alias rejection, crop-unit and srotate semantics, unwired
  srcset, under-threaded limits, single-frame animation fallthrough, dropped
  decoder downscale hints, …) and the four-RIAPI-parsers hazard note
  (a8a7144d).

#### Fixed

- **zengif and zenwebp both made `zencodec` a required, always-on dependency
  (adopting the zencodec Pattern B `At<CodecError>` error boundary) and dropped
  their `zencodec` cargo feature, which broke every dependency declaration in
  this repo still requesting it** — "failed to select a version" on every CI
  platform, twice in a row as each landed (zengif: imazen/zengif#13, d8610292;
  zenwebp: zenwebp#69, 9a38d46e). Removed `zencodec` from every affected
  feature list (root Cargo.toml ×2 each, `zencodecs/Cargo.toml`,
  `wasm-size-shim/Cargo.toml`); added `std` back explicitly for zengif (its
  std-only codec glue used to be gated behind the removed feature, which
  always implied `std` — zenwebp's codec module isn't std-gated, so no
  replacement was needed there). Both needed a `[patch.crates-io]` entry for
  `zencodec` itself pointing at its own git main (zencodec#99's unreleased
  `CodecError`/`CategorizedError`/`ErrorCategory` taxonomy, additive on top of
  the published 0.1.25 — safe for every other zencodec consumer in this
  graph). Bumped the zengif/zenwebp version requirement strings to match what
  they now declare (0.7.3, 0.5.0) so the patch can actually resolve.

- **PNM decode-bomb OOM in `zencodecs::fuzz_push_decode` bounded (zenpipe#50).**
  Bumped the `zenbitmaps` dependency `0.1.3`/`0.1.5` → `0.2.0` (the
  `At<BitmapError>` error wrapper + a 120 MP default pixel cap + the 16-bit
  ASCII PNM roundtrip fix, fuzz zenbitmaps#7/#10) and added a git
  `[patch.crates-io]` entry for it (path patch in `zencodecs/fuzz`) since 0.2.0
  is unpublished. zencodecs' PNM adapter now rejects oversized headers at the
  pre-allocation dimension check: a crafted `P2`/`P3`/`P5` bomb returns
  `LimitExceeded` instead of allocating gigabytes — verified end-to-end through
  `DecodeRequest` (a 2 000 000-wide header → `Err`, 0.76 GiB peak RSS, no
  OOM/panic). The bump also lets the fuzz farm's superwork sibling-clone of
  `zenbitmaps`-main satisfy the requirement (was pinned `^0.1.5`, which 0.2.0
  couldn't match). The RAW/TIFF facet of #50 was already gated (961acad1).
  Follow-up: publish `zenbitmaps` 0.2.0 to crates.io before any zencodecs
  crates.io release (the git patch covers bare-clone / farm builds until then).

- **`wasm-size-shim` builds again; "WASM Benchmark" workflow un-redded**
  (red since 2026-06-01): excluded the shim from the root workspace (it
  keeps its own lockfile, same rationale as `demo/crate`), ported it off
  the removed zenresize `layout` feature to `zenlayout::ConstraintMode`,
  aligned zenresize/zenblend to zenpipe's registry specs (path copies made
  a second crate instance whose `Filter`/`BlendMode` no longer unified),
  adapted `Constrain{w,h}` to `Option<u32>`, dropped the removed zenjpeg
  `yuv` feature, and refreshed its Cargo.lock. Verified locally on both
  workflow targets (wasm32-unknown-unknown cdylib + wasm32-wasip1
  wasm-demo). The fuzz/ manifest got the same treatment (zencodecs path,
  stale local-path patch table → root-mirroring git patches, lock refresh).

- **Resolves from a bare `git clone` / as a git dependency** (#37): every
  cross-repo dependency in the workspace manifests (root, zencodecs,
  zenfilters) is now a plain registry spec — the `path = "../…"` keys that
  only resolved inside the local superworkspace are gone. Crates whose
  required versions/content are not yet on crates.io (zenjpeg 0.8.7, zenjxl
  `jpeg-lossy`, jxl-encoder 0.3.2, zenjxl-decoder/zennode/zenpng/zenwebp
  mains, zensim 0.3.0, zensim-regress 0.4.0, imageflow_types/_riapi,
  zensally/-zentract) resolve via `[patch.crates-io]` git entries at the
  workspace root. CI is unchanged (superwork ci-clone deletes the patch
  section per the new `delete_sections` and paths every dep to its sibling
  clone); local sibling-lockstep development is opt-in via
  `cargo superwork patch`/`unpatch`. Full CI test matrix verified green
  under the registry+git resolution.

#### Changed

- **deps: zenavif re-pinned to git main (0.1.7); the dead `zenavif/zencodec`
  feature request is gone — CI resolves again.** Since 2026-08-01 every CI
  job died at `cargo test`'s resolve step with "package `zencodecs` depends on
  `zenavif` with feature `zencodec` but `zenavif` does not have that feature":
  CI paths zenavif to a sibling clone of main, where `zencodec` became a
  required dep (no feature). Both zenavif dep lines (root, zencodecs) now
  request 0.1.7 without the feature; zencodecs' `zenavif-parse` moves to
  0.7.0 (parser results are `whereat::At<Error>`) as a git dep on the
  imazen/zenavif workspace itself — a registry spec plus root patch collided
  in CI with zenavif's in-repo member ("package collision in the lockfile",
  because superwork's crate map still sends `zenavif-parse` to the archived
  standalone repo); zenavif is supplied by a new `[patch.crates-io]` entry
  from the same repo. The encode chain that used to block
  this (zenavif → zenravif 0.2.0 → path-only zenrav1e, plus zenravif's
  registry requirement on the unpublished zenavif-serialize 0.2.0 that only
  cavif-rs's own `[patch]` supplied) is git-rev pinned end to end upstream as
  of 2026-08-27 (cavif-rs `09a0dba3` + `f6c883b6`, zenavif `e971bd5e` +
  `11033c95`), so zenravif / zenrav1e / zenavif-serialize resolve from git
  with no patch entries — which is what CI needs, since it deletes this
  workspace's patch table and paths everything to sibling clones (a first
  attempt that patched zenavif-serialize here still failed there). Cost to
  note: cargo fetches zenavif's corpus submodules (~600 MB) per rev for git
  consumers — CI is unaffected (shallow sibling clones).

- **deps: zencodec 0.1.26 rollout — drop the `bea2f94c` flat-taxonomy git
  patch, re-pin every codec to a generation built on 0.1.26.** Every `zencodec`
  dependency in the workspace and the standalone manifests (zcimg, demo/crate,
  wasm-size-shim, zencodecs/fuzz) now requires the published `0.1.26` (the
  two-level `ErrorCategory` taxonomy); the `[patch.crates-io] zencodec = { git
  … }` entry (locked to the 0.1.25-era flat taxonomy at `bea2f94c`) is gone and
  `Cargo.lock` carries exactly one zencodec, from the registry. Codec git
  patches re-locked to their post-migration mains: zenwebp `017c1414` →
  `47c562b1` (0.5.0), zentiff `fe244fbb` → `f3b191a7` (zenextras), zenjpeg
  `9835cf5e` → `e7c53d2e` (0.8.7 → **0.9.0**; the no-op `decoder`/`trellis`
  features were dropped upstream, so the dep lines no longer request them),
  zengif `935304ca` → `510e7a88` (0.7.3 → **0.8.0**), zenpng `a453d82b` →
  `f7167b79` (0.1.4 → **0.2.0**), zenjxl `0180bdc6` → `9226d3a3` (0.2.1 →
  **0.3.0**), zenjxl-decoder `e7e077d8` → `f1faec70` (0.3.10 → **0.4.0**),
  jxl-encoder `8a185c2d` → `cd9a7325`, heic `3fe1110c` → `8d40c94e`,
  zenbitmaps `7021f5d8` → `bedb3035`. New patches: `zenraw` (imazen/zenraw
  `a6b78e93`) and `zenpdf` (imazen/zenextras `f3b191a7`) — registry 0.2.0 of
  both predates `CategorizedError` entirely. `ultrahdr-core` stays pinned at
  `031d4a20` (0.5.0 + `full_reconstruction_boost`) for heic's `^0.5`, while
  zencodecs/zenjpeg 0.9.0 moved to registry `0.6.0` (documented in the patch
  table). No source fallout from the taxonomy in zenpipe's own code (nothing
  here matched the flat variants). **Not moved:** zenavif stays at registry
  0.1.6 / zenavif-parse 0.6.2 — zenavif main's `encode` chain (zenravif 0.2.0
  → path-only `zenrav1e`) is unpublished and cannot resolve from git; and
  zensim-regress (dev-only) still wants `zenpng ^0.1.4`, so a registry zenpng
  0.1.4 sits beside the git 0.2.0 in dev builds only.
- **README overhaul + crates.io README split (zenpipe + zencodecs/zenfilters/zenlayout).**
  Standardized every published crate's badge row to the flat-square set (CI,
  crates.io, lib.rs, docs.rs, MSRV, license), with the CI badge pointing at the
  monorepo `ci.yml` and no `branch=` param — zenpipe was on `for-the-badge` and
  missing the crates.io/lib.rs/docs.rs badges. Added `## Quick start` sections,
  regenerated the shared crosslink footer from the crate registry, and split each
  crate's crates.io README into a generated `README.crates.md` (CI-badge-only,
  absolute links) wired via `readme = "README.crates.md"`. Repointed the
  `repository` field of zencodecs/zenfilters/zenlayout from their archived
  standalone repos to `https://github.com/imazen/zenpipe`, and aligned the
  `zencodecs` license field (`AGPL-3.0-or-later` →
  `AGPL-3.0-only OR LicenseRef-Imazen-Commercial`) with its siblings, the bundled
  LICENSE-COMMERCIAL, and its own README. Fixed stale README claims:
  `zenpipe::transcode` → `zenpipe::animation::transcode` (real signature),
  `NodeOp::Resize`'s `filter`/`sharpen_percent` are `Option`s over
  `zenresize::Filter`, the zenfilters film-look-gallery URL → imazen.github.io/zenpipe,
  and the zencodecs default-feature list (drops the non-default `heic-decode`).
- **deps: migrate to published `zencodec 0.1.24`; drop the git-rev patch.**
  Bumped the workspace `zencodec` dependency `0.1.16` → `0.1.24` and removed the
  `[patch.crates-io] zencodec = { git = … }` entry now that `zencodec 0.1.24` is
  on crates.io. Every edge — including the `heic` → zencodec one the patch was
  added to lock — now resolves zencodec from the registry (`source =
  registry+…` in `Cargo.lock`). `zenpixels` / `zenpixels-convert` stay
  git-patched (their mains aren't published yet) and the rest of the patch
  table is unchanged. No code changes: zenpipe's own `ResourceEstimate`
  (`src/graph.rs`) is an independent local type, and nothing in the workspace
  reads zencodec's `estimate` / `ResourceEstimate` API. `cargo test
  --all-targets`, the broad `zencodecs` member test, and the `job-ultrahdr`
  gain-map round-trip suite all pass.

## zeneditor

### [Unreleased]

#### Added (demo)

- **Export worker pool + srcset generator** (zenpipe#22, `demo/`):
  `js/worker-pool.js` runs full-resolution encodes on 2–4 extra
  `worker.js` instances, each with its own Editor decoded from a kept
  copy of the source bytes (`state.sourceBytes`, re-`upgrade`d when the
  primary did), so interactive overview/detail renders never queue behind
  an export; the pool is discarded on every new image (`state.imageEpoch`)
  and cancel terminates in-flight workers. The export modal gained a
  Srcset section (`js/srcset.js`): width presets (thumbnail / mobile /
  desktop / retina / all) or a custom list — capped to the source width,
  no upscaling — × format checkboxes → one job per cell on the pool, a
  progress bar with Cancel, output as one stored zip (`js/zip.js`) or
  individual downloads, and a `<picture>`/`<img srcset>` snippet. The
  plain Export button also runs on the pool now. `tests/srcset.spec.js`
  (playwright, mock backend): zip entries = widths × formats with valid
  CRCs, snippet contents, ≥2 workers used, cancel leaves no workers and
  no download then respawns, pool reset on image change. Verified on
  both backends: the OffscreenCanvas mock locally, and the deployed
  Pages WASM build (`d326fe3b`, mirrored to a local server) where the
  same 4 tests pass with real JPEG/WebP/AVIF encodes (4 pool workers
  finished 117 of 200 encodes in 0.8 s before Cancel landed). Found
  while doing so: on that WASM build every JXL export ends in
  `unreachable` at 100/160/200 px while JPEG/WebP/AVIF/PNG encode fine
  (the PNG skip note in `features.spec.js` is stale) — a codec-in-WASM
  issue outside this feature.

#### Fixed

- **zeneditor compiles again**: `pipeline.rs` built an
  `zennode_defs::ExpandCanvas` without the `fill` field that the canvas
  fill-mode work (16dd7a2) added, so `zeneditor` (and therefore
  `cargo test --workspace`) failed with E0063. The editor's padding is a
  solid color, so it passes `fill: "solid"`.

- **Pages demo WASM packaging** (#43): the threaded build now exports
  `__heap_base` (wasm-bindgen's threading transform needs it to inject
  thread ids), and wasm-bindgen-cli is installed at the exact version the
  crate pins (`=0.2.123`) — the bindgen schema requires crate == CLI and
  the deploy deletes Cargo.lock, so a floating crate version drifted past
  the pinned CLI.

- **`decode` builds again** (#43): the feature list dropped the zencodecs
  stubs (`heic-decode`, `bitmaps-qoi`/`-tga`/`-hdr`) that have never
  compiled — `zeneditor/decode` was uncompilable since April and blocked
  the Pages demo deploy. Native WASM decode formats are now jxl/avif/bmp
  (plus browser-native); the stubs return when their backends land. CI now
  tests and lints zeneditor on every push (it was previously built by no
  job), including new clippy fixes in zeneditor and zenpipe's
  `json-schema`-gated codegen.

## zenpipe

### [Unreleased]

#### Changed (tile pyramid profile + fixes, 2026-08-28, #24)

- **The sink's own pass is 23–29 % faster** with byte-identical output
  (`5b3c8101`; matched back-to-back A/B, `--repeat 25` — the commit message's
  "26–33 %" paired a single-shot baseline against a warmed run and is
  optimistic, see the benchmarks doc). `shrink_rows` spent 62 % of that pass
  on three integer divides per output pixel in the alpha-weighted average;
  fully opaque row pairs now take the plain path, which is provably identical
  there (`(255·S + 510)/1020` reduces exactly to `(S + 2)/4`), and the
  remaining paths index fixed-size arrays. `shrink_rows` fell from 62 % to
  54 % of the pass.
- **Allocation churn down 25 % / 22 %** (`5b3c8101`): every even row used to
  be cloned into `Level::pending` so it could pair one level down; the row
  queue now keeps the newest row and `push_row` reads the pair in place.
  Peak memory is unchanged — the clone was transient.
- **`ZipStore` is 2.8× faster** (`2fe50306`): its byte-at-a-time CRC-32 was
  the entire gap to `FsStore` (0.105 s vs 0.024 s for 229 tiles). Now
  slicing-by-8; zip's overhead over `FsStore` fell 6.8× (81 ms → 12 ms).
- **`examples/tile_pyramid_profile.rs`** (`275c5dbb`) — counting global
  allocator (peak live heap + allocation churn) plus
  `--layout/--store/--threads/--encode/--source` axes. `--source` measures
  the four input classes: perfect stream, real streaming codec, full-frame
  decode, and `TempFileSource` spool. Validated against `heaptrack` on
  x86_64 Linux (agreement within 0.1 %).
- **`benchmarks/tile_pyramid_profile_2026-08-28.{md,tsv}`** — the full grid,
  the `sample` time profile, and an honest per-input-class answer: only
  JPEG/PNG/WebP/GIF stream today, so a JXL or TIFF pyramid pays a whole
  decoded frame (280.4 MB at 64 MP vs 25.0 MB streaming). `zenjxl-decoder`
  exposes no region/group accessor and `zentiff` has no strip/tile accessor
  on main — both blockers live outside this repo.

#### Fixed (`Session` incremental cache, 2026-08-28, refs #3)

- **`Session::stream` ignored `config.limits`** — both the prefix and suffix
  segments were compiled with `limits: None`, so a job that
  `orchestrate::stream` would reject (max_pixels, max_memory_bytes, …) ran
  unbounded through a `Session`. Limits now gate every executed segment
  (`session_enforces_limits_on_miss_and_hit`).
- **Cache key omitted `hdr_mode`** (and the source's alpha/HDR/gain-map
  flags): the entry stores the processed gain-map sidecar, whose presence
  depends on `hdr_mode`, so an `sdr_only` run could hand its (absent) sidecar
  to a `preserve` run of the same nodes. All of these are hashed into the
  chain root (`session_hdr_mode_is_part_of_the_key`).
- **An entry larger than the whole budget was still inserted** after
  evicting everything else, leaving `current_bytes > memory_budget`. It is
  now run uncached (`session_skips_entry_larger_than_budget`).

#### Changed (`Session`, refs #3)

- **Merkle-chain prefix hashing + longest-prefix lookup**: node lists are
  hashed as `chain[i] = subtree_hash(nodes[i-1], [chain[i-1]])` from a
  source-identity root (one forward pass, `cache::prefix_chain`). Lookup
  takes the longest cached prefix up to the geometry/filter split, so
  appending a geometry node re-runs only the new node from the cached pixels
  instead of the full decode + geometry
  (`session_partial_prefix_hit_resumes_from_cache`). Partial hits fall back
  to the full path when a gain-map sidecar must be derived from the original
  source dimensions. No public-API change; `prefix_hash`/`subtree_hash`
  keep their signatures.

#### Fixed (2026-08-27)

- **`ImageJob` with no output format re-encoded JPEG as JPEG XL**
  (`986a0b37`): `CodecIntent::format == None` reached the selector, which
  treats it as `Auto`. The job now applies the JSON-JOB-SPEC contract
  before selection — default `keep` (match source), `auto` only when a
  `quality_profile` is set. Regression: `job::tests::default_format_*` +
  `e2e_jpeg::roundtrip_jpeg_no_nodes`.
- **`zenlayout.orient` had no querystring key** (`d944e51c`): the EXIF
  value is now `?orientation=1..8` (`#[kv("orientation")]`); `srotate`
  (degrees) and `autorotate` stay with their adapters. Restores
  `export_querystring_keys_includes_kv_annotated_nodes`
  (`--features zennode,json-schema`).
- rustc 1.98 clippy wall (`88e3654b`) — see Workspace. `resolve_riapi_crop`
  takes a `RiapiCropWindow`; `get_io_bytes` lifetimes elided (no signature
  change). `job` feature: `ColorManagement` deprecation and
  `new_without_default` kept behind targeted allows (both real fixes are
  public-API changes: `IccTransformSource::from_transform` takes
  `Box<dyn RowTransform>`; a `Default` impl is new surface).
- `tests/animation.rs::imagejob_animation` gated on `nodes-gif` +
  `nodes-png` (`9bb3c160`) — it needs those codecs and failed at probe
  time under `job` alone.

#### Added

- **Tile pyramid sink, first chunk** (#24): `tiles::TilePyramidSink`
  generates every Deep Zoom (DZI) level in one top-to-bottom pass with RAM
  bounded by image width — per level a queue of `tile_size + 2·overlap`
  rows, tile rows cut as soon as their rows arrive, 2×2 box shrink
  cascading to the 1×1 apex (alpha-weighted for RGBA, last row/column
  replicated for odd sizes; 8-bit-channel formats). Tiles go to a
  pluggable `TileWriter`: `MemoryTileWriter` and a `DziFsWriter` (`std`)
  that writes `{name}.dzi` + `{name}_files/{level}/{col}_{row}.{ext}` with
  a caller-supplied per-tile encoder. `buffer_bytes_estimate()` is the
  formula, not a measurement. Verified against a full-image reference
  shrink chain (6 geometries incl. odd sizes, 3-row strips, overlap 0/1/2)
  and a filesystem layout test.

- **Tile pyramid, second chunk — layouts, stores, parallel encode** (#24):
  `tiles::PyramidWriter<L, S>` replaces `DziFsWriter` (added a day
  earlier, unreleased): a `TileLayout` names tiles and writes the
  descriptor — `DziLayout` (`.dzi` + `_files/{level}/{col}_{row}`),
  `Iiif3Layout` (`{id}/{x},{y},{w},{h}/{tw},{th}/0/default.{ext}` +
  `info.json`, full-resolution regions like libvips `--layout iiif3`),
  `GoogleMapsLayout` (`{z}/{y}/{x}`, image padded top-left into a
  `256·2^k` square so every tile is complete) and `ZoomifyLayout`
  (`TileGroup{n}/{level}-{col}-{row}` + `ImageProperties.xml`, tiles
  numbered sequentially from the apex, 256 per group) — and a
  `TileStore` persists them: `FsStore`, `MemoryStore`, and `ZipStore`
  (stored entries, streaming to any `Write`, ZIP64 records past 65 535
  entries / 4 GiB; entries over 4 GiB rejected). `TilePyramidConfig` gained
  a `PyramidGeometry` (`ToOnePixel` / `ToOneTile` / `PaddedSquare`) with
  `dzi()` / `iiif()` / `zoomify()` / `google_maps(bg)` presets; each
  layout rejects a mismatched geometry in `begin`. The sink now hands the
  writer whole tile rows (`TileWriter::write_tile_row`, one row-of-tiles
  scratch, still bounded by width); `PyramidWriter::with_threads(n)`
  encodes a row on `n` scoped threads with store writes kept in raster
  order, `with_skip_blanks(bg, threshold)` drops near-background tiles
  (`TileRef::is_blank`). Tests: every geometry against the full-image
  reference chain (incl. padded canvases), filesystem layout tests per
  format, zip == memory store byte-for-byte with an independent CRC, a
  70 000-entry ZIP64 archive read back, threads=3 == threads=1, and an
  encode error surfacing from a worker thread; mutation-verified
  (col-major Zoomify numbering, reversed parallel results, and dropped
  ZIP64 records each fail their test). Still not done: tiled-TIFF / mmap
  input (`MmapTiledSource`, needs zentiff tiled access), column-parallel
  execution, PMTiles.

- **Tile pyramid memory, measured** (#24): `examples/tile_pyramid_mem.rs`
  streams a synthetic image (rows generated on the fly) through a
  counting writer; `/usr/bin/time -l` max RSS on an Apple M4 Pro, release,
  RGBA8 / DZI 254/1: 10 000×1000 → 38.3 MB (formula 31.1), 40 000×1000 →
  124.8 MB (123.7), 100 000×600 → 298.1 MB (308.9); runtime baseline
  1.8 MB. Height does not enter (≤ `tile + 2·overlap + 1` rows per level
  are held), so the issue's "100 K px wide under 1 GB" holds with room.
  Recorded in the `tiles` module docs.

- **`sources::TempFileSource` — decode once, replay from disk** (#24, the
  "analysis barriers without full materialization" step): drains a
  source into a spool file (`ZENPIPE_SPOOL_DIR` or the OS temp dir,
  removed on drop) and replays it as strips on every `rewind()`, holding
  one strip of RAM; the OS page cache owns residency. Two-pass streaming
  operations (statistics pass, then the real pass) can run on gigapixel
  input without a frame in memory. Not an mmap-backed
  `MaterializedSource`: every mmap crate maps through an `unsafe fn` and
  zenpipe forbids `unsafe`, so random-access analysis (`Analyze`,
  `CropWhitespace`, `EffectSource`) still materializes. Unit-tested:
  byte-exact replay across passes from a materialized and a
  row-generator source, exhaustion until rewind, short upstream rejected
  with the spool removed.

- **Auto-deskew, first chunk** (#27): `EffectSource` now resolves
  content-adaptive effects (`DimensionEffect::forward() == None`) against
  the materialized frame via the new `DimensionEffect::analyze` hook
  (Rec.709 luma composited over white), recomputes their output dims,
  and exposes what they resolved to through `EffectSource::effects()`.
  With `zenlayout::AutoDeskewEffect` this straightens skewed scans/rules
  end to end (`tests/auto_deskew.rs`: pipeline-rotated rulings at ±3–7.5°
  come back within 0.2°).

- **Auto-deskew, second chunk — node, RIAPI key, planner re-plan, Hough**
  (#27): `zenlayout.auto_deskew` node (`max_angle`, `mode`
  inscribed/expand/original, `method` projection/hough/gradient) and the
  `autodeskew=` RIAPI key (`true|1` → 10° budget, a number in (1, 45] →
  that budget, `false|0` off; ordered after crop, before the resize).
  Geometry fusion turns it into a `Command::Effect` analysis barrier, and
  the graph now **re-plans** the fused layout once `EffectSource` resolves
  the barrier: `LayoutPlan::replan` keeps the command pipeline when it
  holds a barrier, `Pipeline::resolve_effect` swaps in the concrete
  rotation, and the constraint is recomputed against the real post-deskew
  dims (a 45° expand barrier on 320×240 with `w=200` now yields 200×200,
  not the placeholder 200×150). The pre-flight estimate counts the two
  full frames `EffectSource` materializes. `AutoDeskewMethod::Hough {
  min_confidence }` (see zenlayout). Measured (release, 4000×3000, see
  `zenlayout/examples/deskew_timing.rs`): projection variance 34 ms,
  Hough 17.7 ms, gradient moment 1.3 ms — all under the issue's 50 ms
  budget. Tests: `?autodeskew=1&w=200` through registry → IR4 order →
  fusion → graph → straight output (`tests/auto_deskew.rs`), the re-plan
  with a controllable barrier, RIAPI key parsing (`tests/riapi_keys.rs`);
  mutation-verified (restoring the planner's `pre_effects.clear()`,
  disabling the re-plan, and zeroing the Hough confidence each fail).
  Not yet: `docs/querystring.md` regen for the new key (see CLAUDE.md
  "Generated docs regen pending"), and a real scanned-document corpus —
  accuracy is proven on synthetic anti-aliased rulings only.

- **Canvas extend fill modes: replicate / mirror / repeat** (#23):
  `sources::CanvasFill { Solid, Replicate, Mirror, Repeat }` (sharp
  `extendWith` / vips `embed` extend semantics; mirror repeats the edge
  pixel, `abc|cba|abc`) via `ExpandCanvasSource::with_fill`, the new
  additive `NodeOp::ExtendCanvas { .., fill }` graph op, and a `fill`
  param on the `zenlayout.expand_canvas` node (`solid` default keeps the
  old op). Streaming with bounded buffering: replicate 1 row each side,
  mirror `min(top,H)` leading + `min(bottom,H)` trailing rows, repeat
  buffers the whole visible content only when `top > 0`. The solid path
  is untouched. Pixel-exact tests in `tests/canvas_fill.rs` (unique-pixel
  fixture, 3-row strips, padding wider than the content).

- **Execution-layer tracing: per-strip events, execution finalization,
  phase timing** (#8): `TraceConfig::strip_events` /
  `with_strip_events()` (on in `full()`) makes every `TracingSource`
  record a `StripEvent { strip_num, rows, duration, bytes }` into
  `NodeTiming::strips`; `PipelineTrace::compile_duration` captures the
  `compile_traced` phase; `FullPipelineTrace::finish_execution()` derives
  the previously never-populated `ExecutionTrace` (output-node total /
  strip count, `phases: [(ExecutionPhase, Duration)]`, `slowest_strip`)
  from the drained graph; `strip_timing()` renders an ASCII per-strip
  chart; `to_text()` prints phases + slowest strip; `to_json()` gains
  per-node `timing` / `strip_events` and a top-level `execution` object.

- **Memory timeline + `JobResult::trace`** (#8, closes the two items the
  entry above left open): `TraceConfig::memory_timeline` /
  `with_memory_timeline()` (on in `full()`) attaches a `MemoryLedger` to
  the `PipelineTrace`; every `TracingSource` charges the buffers the
  engine knows it allocates — the full input frame a materializing node
  (`Orient`, `CropWhitespace`, `Analyze`, `FillRect`, `Materialize`)
  drains into, plus its output strip buffer — at its first pull and
  releases them at its EOF pull or when the pipeline drops, each as a
  `MemorySnapshot { allocated_bytes, allocation_count, timestamp, event }`
  stamped from the start of `compile_traced`. `ExecutionTrace` gains
  `peak_memory_bytes` + `memory`; `FullPipelineTrace::memory_timeline()`
  renders the ASCII bar chart, `to_text()` prints the peak, `to_json()`
  emits `execution.memory`. This is an *accounting of the graph's own
  buffer plan against wall-clock*, not a heap measurement (no allocator
  hook; codec-internal and resize-kernel buffers are not counted) — the
  doc on `MemoryLedger` says exactly what is in and out; `AllocationTracker`
  stays unwired per root CLAUDE.md. `ImageJob::with_trace` now surfaces
  the trace on `JobResult::trace` (new field), finalized after the encode
  drained the pipeline. Tests: materialize/strip charge + release + zero
  at the end on an Orient→Resize graph, off-by-default, and the job path
  (`tests/trace_execution.rs`); mutation-verified (skipping the
  materialize charge and skipping `finish_execution` each fail).

#### Fixed

- **`Limits::to_codec_limits` forwards `max_total_pixels`** (#18): the
  cumulative animation budget was dropped on the way to the codec layer,
  so only the probe-time `width × height × frame_count` check applied
  (and only when the frame count was known). Now reaches
  `zencodec::ResourceLimits` and zencodecs' per-frame guard.
- **`zen_get_image_info` reports display-oriented dimensions** (#16):
  EXIF/container orientation is applied before returning, so orientations
  5–8 report swapped `width`/`height` (matching imageflow's
  `v1/get_image_info` `swap_dimensions_by_exif`) and `orientation` is
  `Identity`. Regression test: `tests/imageflow_compat_info.rs`.
- **Gain-map sidecars resample in encoded space** (#41, b938a2b0): sidecar
  pixels are log2-quantized gain values, not color — Skia's SkGainmapShader
  and libultrahdr interpolate them raw. The decode path now labels sidecar
  strips `TransferFunction::Linear`; `NodeOp::Layout` derives its working
  format from the source transfer (`RGBA8_LINEAR` for Linear sources, so
  zenresize does raw u8↔f32 with no gamma round-trip); `ResizeSource`
  carries the working format instead of hardcoding `RGBA8_SRGB`. Previously
  resized Preserve jobs bounced gain values through the sRGB EOTF/OETF.
- **Gain-map re-embed repacks the materialized sidecar** (#41, b938a2b0):
  tight 1-/3-channel packing driven by ISO 21496-1
  `GainMapParams::is_single_channel()` (metadata-driven, not pixel
  inspection), fixing the latent corruption where a resized RGBA sidecar
  was re-embedded as raw RGBA bytes labeled `channels: 3`.

## zencodecs-cli

### [Unreleased]

#### Added

- **`--lossless-if-cheaper [FACTOR]`** (#68): the last unchecked CLI item —
  encodes lossless and lossy, keeps lossless when it is at most FACTOR×
  (default 1.5) the lossy size. Rejects non-positive factors and formats
  without a lossless mode. Binary-level tests in `tests/cli.rs`.
- **`--speed`** preset flag (#28).

#### Fixed

- **README synced with the shipped CLI** (#68): HEIC decode and `--hdr`
  were still listed as "tracked"; the flag table now covers
  `--lossless-if-cheaper`, `--speed`, `--hdr`, `--keep-orientation`, and
  the `convert-hdr-corpus.sh` example; states the PQ-only HDR output.

## zenlayout

### [Unreleased]

#### Fixed

- **Pre-resize dimension effects were dropped from the plan** (zenpipe#27):
  `compute_layout_sequential` cleared `pre_effects` on every `Constrain`,
  so a `rotate_angle` / `AutoDeskewEffect` placed before a resize never
  reached `IdealLayout::effects` and the engine silently skipped it. They
  now stay in the plan (post-ops still reset per constrain).

#### Added

- `deskew::detect_skew_hough` / `detect_skew_hough_with_confidence`
  (zenpipe#27): gradient-magnitude-weighted Hough over Sobel edges, 1°
  sweep + 0.1° refinement, with a `1 − mean / peak` confidence taken
  against the refined peak (the coarse peak under-reports fine-pitched
  content on subsampled scans: 0.06 vs 0.73 at step 4).
  `AutoDeskewMethod::Hough { min_confidence }`. Ignores flat tonal
  regions; within 0.2° on rulings.
- `Pipeline::has_analysis_barrier` / `Pipeline::resolve_effect` and
  `LayoutPlan::replan` (non-exhaustive struct, additive) so an engine can
  re-plan after resolving a content-adaptive effect. `Pipeline` is now
  `Clone`.
- `detect_skew_projection_variance` runs its 1° sweep and a 0.25° pass on
  a 2× coarser sample grid and only the last 0.1° pass on the full grid:
  101 ms → 34 ms on a 4000×3000 ruling (release), same 0.1° resolution.
- `examples/deskew_timing.rs` — the measurement behind those numbers.

#### Fixed (2026-08-27)

- rustc 1.98 clippy: `riapi/parse.rs` `chunks_exact(4)` → `as_chunks`
  (`1a475127`).

#### Added

- **`deskew` module + `AutoDeskewEffect`** (#27): `deskew::
  detect_skew_projection_variance` (perpendicular-projection histogram
  variance, 1° sweep + 0.1° refinement; within 0.2° of the ground-truth
  angle on anti-aliased rulings, 9 angles tested) and
  `detect_skew_gradient_moment` (structure tensor, `O(N)`, coarse — about
  ±10–15% of the angle on thin rulings, documented and tested as such).
  `AutoDeskewEffect { mode, max_angle_deg, method }` is an analysis-barrier
  `DimensionEffect` (`forward`/`inverse` are `None`) that `analyze`s into a
  `RotateEffect::from_degrees(-skew, mode)`. New defaulted trait methods
  `DimensionEffect::analyze` and `rotation_angle_rad` (additive).
  Angle convention matches `RotateEffect` (a horizontal line rotated by
  `a` is detected as `a`).

## zenfilters

### [Unreleased]

#### Changed

- **`quality_validation` vs libvips re-measured on a second machine**
  (zenpipe#44): with Homebrew libvips 8.18.6 on macOS arm64 and the CID22
  corpus, all 21 corpus + vips tests pass; `saturation_boost_vs_vips`
  scores min zensim 88.8 / avg 91.8 (1028637.png: zensim 88.8,
  mean_diff 0.4) against the workstation's recorded failure of 43.0 /
  mean_diff 3.0 on the same file — so the workstation number comes from
  its vips reference render, not from zenfilters (zenfilters output is
  identical across dependency resolutions per the issue). Not
  reproduced here: `dt_contrast_full_corpus` — `darktable-cli` is
  unavailable on this platform (the Homebrew cask is deprecated for
  failing Gatekeeper and is disabled 2026-09-01). Thresholds unchanged.
  New `just test-zenfilters-quality-vips` runs the corpus + vips subset
  without darktable.

#### Fixed (2026-08-27)

- rustc 1.98 clippy wall (`ce1c1d10`): `convenience.rs` and the
  tests/examples use `as_chunks::<N>()`; 121 constant-fill test helper
  loops use `slice::fill` (loops whose RHS reads the element are unchanged).

#### Added

- `Pipeline::apply_with_stop` — cooperative cancellation via
  `&dyn enough::Stop`, checked at scatter/gather strip and between-filter
  boundaries (outer loops only, never per-pixel); `apply()` delegates with
  `enough::Unstoppable` so the uncancellable path is byte-identical.
  Companion `Pipeline::apply_planar_with_stop` for the manual scatter/gather
  path. New `PipelineError::Cancelled(enough::StopReason)` variant
  (non-breaking — the enum is already `#[non_exhaustive]`). `enough` promoted
  to a normal dependency.

#### Fixed

- **`tests/quality_validation.rs` no longer self-skips** (#44): the binary
  used to return early with `SKIP: …` when the CID22 corpus / `vips` /
  `darktable-cli` were absent, so every one of its 32 tests passed on CI
  without testing anything. It is now gated on `required-features =
  ["local-fixtures"]` (corpus), with the vips and darktable groups behind
  `local-vips` / `local-darktable`; a missing prerequisite panics with the
  path/tool it needs. Run via `just test-zenfilters-quality`. Also adds
  the workspace-sibling `../../codec-corpus` candidate (the old
  `zenfilters/../codec-corpus` never matched the monorepo layout) and
  fixes the env override name to `ZENFILTERS_CORPUS_DIR`. The two
  threshold failures the skip was hiding (`saturation_boost_vs_vips`,
  `dt_contrast_full_corpus`) remain open — they need a workstation with
  vips/darktable to investigate.

## zencodecs

### [Unreleased]

#### Changed

- **Metadata conformance: AVIF CICP re-verified, `cicp` split into two
  strengths** (zenpipe#36 gap 3): with zenavif git-main (11033c95)
  `Metadata::cicp` does reach the nclx `colr` box and comes back —
  primaries 12 / transfer 13 / full range for a Display P3 request — so
  the old gap note ("not wired through encode/decode") was wrong. What
  cannot round-trip exactly is `matrix_coefficients`: AVIF signals the
  matrix it *coded* with (BT.601 = 6), not the RGB identity (0) the
  caller wrote — a coding decision (`zenavif` `color_model`), not
  pass-through metadata. `tests/metadata_conformance.rs` now checks
  `cicp_color` (primaries + transfer + range; AVIF/PNG/JXL `Ok`) next to
  the exact-triple `cicp` (AVIF stays `Gap`, note rewritten to say the
  matrix is codec-owned by design). Remaining #36 items all need codec
  crates: JXL codestream orientation write + EXIF-orientation
  normalization (jxl-encoder has no public orientation setter at
  cd9a7325; zenjxl's adapter has no orientation plumbing).

#### Fixed (2026-08-27)

- rustc 1.98 clippy (`2fc057ec`): tests/examples use `as_chunks`;
  `decode_info_format` no longer warns on `codec_config` without `jpeg`;
  `push_dec!` is `allow(unused_macros)` for feature sets without the
  jpeg/bitmaps/raw/svg arms.
- **Fuzz manifest resolves again** (`0396941a0a2d`): `zencodecs/fuzz/Cargo.toml`
  pointed `zenavif-parse` at a deleted sibling and `codec-corpus` one
  directory too high; the unpublished mains the root patch table pins are
  mirrored into the fuzz workspace. Compilation of the fuzz targets is
  still blocked on their own `Limits` API rot (`with_max_memory_bytes`,
  `with_max_width(u64)`).

#### Added

- **`svg` feature is live** (fixes zenpipe#1): the `compile_error!` stub is
  gone and `svg = ["dep:zensvg"]` wires the zensvg rasterizer (resvg, a
  zencodec decode adapter, unpublished — git dep on imazen/zenextras next to
  zentiff). Detection, probe, one-shot/push decode, dyn dispatch, `CodecId`,
  the resource estimator and the `AllowedFormats` custom-format registry
  (`"svg"` bit, compiled in with the feature) all route SVG/SVGZ as zensvg's
  `ImageFormat::Custom("svg")`. zencodec 0.1.26's common registry detects
  SVG as the built-in `ImageFormat::Svg`; `detect_format` normalizes that to
  zensvg's definition when the feature is on, so every svg arm sees one
  representation. Integration test `tests/svg_decode.rs` pins detect → probe
  (64×32) → rasterize (red top-left pixel) and that malformed SVG errors
  instead of panicking.
- **`EncodeSpeed` presets** (#28): `Fastest` / `Realtime` / `Offline` /
  `OfflineMax` map to a per-codec generic effort (`EncodeSpeed::
  generic_effort(format)`, a policy table on the 0–10 `with_generic_effort`
  scale — JPEG 0/1/2/2, WebP 0/4/7/10 (= method 0/2/4/6), JXL 1/3/7/9,
  AVIF 0/2/6/10, PNG 0/3/6/10) and a threading policy (`Fastest` forces
  `Sequential`; the others never widen a caller's explicit limit). Attach
  with `EncodeRequest::with_speed`; explicit `with_effort` wins. Also
  `EncodeSpeed::from_name`/`name` and the CLI's `--speed`. The table is
  unmeasured — no timing sweep backs the specific numbers.
- **JPEG honors generic effort**: `codecs::jpeg::build_encoding` dropped
  `effort` entirely (never called `with_generic_effort`), so
  `EncodeRequest::with_effort` was a silent no-op for JPEG; now forwarded
  (zenjpeg clamps to its 0–2 scale). Surfaced by #28.
- **Stub features fail loudly** (#43): enabling `heic-decode`,
  `bitmaps-qoi`/`-tga`/`-hdr`, `tiff`, `svg` (since wired — see above), or `jp2-decode` now produces
  one clear `compile_error!` naming the missing backend instead of a flood
  of unresolved-item errors. Delete the guard when the backend lands.

- **CI now compiles and tests the gain-map/UltraHDR/raw surface** (#38):
  new workflow run + `just test-gainmap-surface` covering
  `jpeg-ultrahdr`/`raw-decode-gainmap` and the avif-less codec set —
  these targets sat uncompilable for weeks with nothing on CI building
  them. Widen to `all,cms,std` once the zencodec↔zenavif drift settles.
- `local-fixtures` feature: caller-controlled gate for tests reading
  dev-workstation-only fixtures (`icc_srgb` reads sibling jpegli-cpp ICC
  profile trees); CI never enables it, `just test-local-fixtures` does.

#### Changed

- **Encode-side pixel negotiation adopts zenpixels' `PixelCow` API** (#78):
  the five `adapt_for_encode` sites (`encode.rs`, `dispatch.rs`,
  `transcode.rs`, `avif_enc.rs`, `jxl_enc.rs`) and the doc example now call
  `adapt_for_encode_cow` and hand the encoder `PixelCow::as_slice()`
  directly — the recompute-stride + `PixelSlice::new` boilerplate is gone,
  and hand-rolled `width * bpp` strides use the saturating
  `PixelDescriptor::aligned_stride`. Requires the git-pinned zenpixels /
  zenpixels-convert at `3dbf246` (0.2.16), where the non-`_cow` names and
  the packed `Adapted` type are deprecated (the issue's `as_pixel_slice`
  route was superseded upstream). The `Cicp::from_bytes` item is not
  available at the pinned zencodec rev and was left as-is.
- Feature-conditional test hygiene so every CI feature combination runs
  green instead of failing on tests for codecs that are compiled out:
  `regress` requires `all` (its checksum baselines are recorded under the
  full set per its docs), `stop_and_limits` requires `jpeg,webp,gif` with
  avif legs cfg-gated, selection/encode unit tests gate on the codec
  corpus they exercise, and the avif trace test gates on `nodes-avif`.
- `metadata_conformance`: PNG `orient_from_exif` promoted Gap → Ok —
  zenpng now normalizes the eXIf orientation tag into `info.orientation`
  on decode (stricter pin; regression-guarded both directions).
- `png_capability::png_cicp_chunk_round_trips`: expect
  `matrix_coefficients = 0` after the cICP round-trip — PNG-3 §11.3.2.6
  requires matrix 0 (RGB storage); echoing the source's matrix 9
  verbatim, as the test originally pinned, was a spec violation. The
  encoded chunk is `[9, 16, 0, 1]` (verified at the byte level).
- `icc_srgb`: the 8 expectations stale against the zenpixels-convert
  0.2.13 normalized hash DB flipped to `false` with measured ground truth
  (#42, via zenpixels `icc-gen --bin probe42`): e-sRGB and the v4
  LUT/preference profiles have →sRGB identity errors of ~9–112 u8 steps
  (real transforms the old recognizer silently skipped); the v5/iccMAX
  trio doesn't parse under the production CMS at all. The 0.2.13
  narrowing was correct on every count.

#### Fixed

- **`Metadata::orientation` reaches JPEG / PNG / WebP** (#36, gap 1): those
  formats' only orientation carrier is EXIF, and a request that set the
  field without an EXIF blob silently lost it. `EncodeRequest` now folds
  the field into the blob at the codec boundary
  (`dispatch::fold_orientation_into_exif`): authors a minimal TIFF with the
  Orientation tag when there is no blob, inserts the tag into a blob that
  lacks it, leaves a blob that already has one to the policy's
  reconciliation. AVIF (irot/imir) and JXL (codestream orientation;
  EXIF not authoritative) are untouched. Conformance table promotes
  `orient_from_field` Gap→Ok for jpeg/png/webp; cross-codec orientation
  transfer now holds between jpeg/png/webp/avif. Still open from #36:
  JXL orientation (codestream write + EXIF normalization), AVIF/JXL
  `Metadata::cicp`, AVIF raw EXIF blob — all in the codec crates.
- **`stream_dec!` macro cfg-gated to gif / avif-decode / heic-decode**: a
  build enabling none of those (zenfilters' dev-dep set) failed
  `clippy -D warnings` with `unused macro definition`.
- **`max_total_pixels` enforced cumulatively while animation frames are
  produced** (#18): `DecodeRequest::animation_frame_decoder()` now wraps
  the codec decoder in a guard that charges every rendered frame (owned
  and to-sink paths) against `Limits::max_total_pixels` and fails with
  `LimitExceeded::TotalPixels` once the running total crosses it —
  codec-independent, so GIF/APNG streams whose frame count is unknown at
  probe time are bounded too. Tests: `stop_and_limits.rs`
  `limits_animation_total_pixels_*`.
- **UltraHDR encode derives the color gamut from CICP metadata** (#40):
  `encode_ultrahdr_rgb_f32` / `encode_ultrahdr_rgba_f32` previously ignored
  their metadata parameter and hardcoded BT.709. CICP color primaries 1/2 →
  BT.709, 12 → Display P3, 9 → BT.2100; an explicit code outside the three
  UltraHDR gamuts is an encode error (`UnsupportedOperation`) rather than a
  silent BT.709 fallback, which would compute wrong gain-map luma.
