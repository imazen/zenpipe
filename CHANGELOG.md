# Changelog

All notable changes to the zenpipe workspace are documented here, per crate.
(Started 2026-06-11; earlier history lives in git log.)

## Workspace

### [Unreleased]

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
  and a filesystem layout test. Not yet: IIIF / Google Maps / Zoomify
  layouts, parallel tile encoding, blank-tile skipping, zip output,
  tiled-TIFF input, heaptrack-measured memory.

- **Auto-deskew, first chunk** (#27): `EffectSource` now resolves
  content-adaptive effects (`DimensionEffect::forward() == None`) against
  the materialized frame via the new `DimensionEffect::analyze` hook
  (Rec.709 luma composited over white), recomputes their output dims,
  and exposes what they resolved to through `EffectSource::effects()`.
  With `zenlayout::AutoDeskewEffect` this straightens skewed scans/rules
  end to end (`tests/auto_deskew.rs`: pipeline-rotated rulings at ±3–7.5°
  come back within 0.2°). Not yet: planner integration (`Command::Effect`
  with a barrier), the `autodeskew=` RIAPI key / zennode param, and a
  timing budget measurement.

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
  Not included: the memory timeline / `MemorySnapshot` — there is no data
  source until `AllocationTracker` is wired (see root CLAUDE.md), and
  `ImageJob` still does not surface its trace (only `orchestrate::process_*`,
  `build_pipeline_traced`, and `compile_traced` do).

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
