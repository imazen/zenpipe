# Changelog

All notable changes to the zenpipe workspace are documented here, per crate.
(Started 2026-06-11; earlier history lives in git log.)

## Workspace

### [Unreleased]

#### Fixed

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

## zeneditor

### [Unreleased]

#### Fixed

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

#### Fixed

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

## zencodecs

### [Unreleased]

#### Added

- **Stub features fail loudly** (#43): enabling `heic-decode`,
  `bitmaps-qoi`/`-tga`/`-hdr`, `tiff`, `svg`, or `jp2-decode` now produces
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

- **UltraHDR encode derives the color gamut from CICP metadata** (#40):
  `encode_ultrahdr_rgb_f32` / `encode_ultrahdr_rgba_f32` previously ignored
  their metadata parameter and hardcoded BT.709. CICP color primaries 1/2 →
  BT.709, 12 → Display P3, 9 → BT.2100; an explicit code outside the three
  UltraHDR gamuts is an encode error (`UnsupportedOperation`) rather than a
  silent BT.709 fallback, which would compute wrong gain-map luma.
