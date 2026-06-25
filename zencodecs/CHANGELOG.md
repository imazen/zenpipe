# Changelog

All notable changes to `zencodecs` are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/); this crate is pre-1.0, so any
0.x bump may break the API.

## [Unreleased]

### Removed
- **Hand-rolled `exif` module deleted** (`exif.rs`, 2,085 LOC incl. tests; ~1,000
  production): the rich EXIF/TIFF extraction parser (`parse_exif` → `ExifData` with
  GPS coordinates, exposure rationals, and DNG color-science tags) and its
  `fuzz_exif` target are gone. It was consumed only by that fuzz target — never by
  the decode/encode/transcode paths, which use `zencodec::helpers::parse_exif_orientation`
  and the maintained no_std `zencodec::exif::Exif` parser. EXIF parsing now lives
  entirely in `zencodec` (fuzzed there by `exif_parse`/`exif_filter`/`exif_author`/
  `exif_roundtrip`). This aligns zencodecs with its transcoding/selection core;
  rich field extraction, if wanted, belongs upstream as typed getters on
  `zencodec::exif::Exif`. **BREAKING: the `zencodecs::exif` module is removed.**

### Changed
- Bumped the `zencodec` dependency floor `0.1.22` → `0.1.25` (the lockfile already
  resolved 0.1.25; this formalizes relying on its surface).

### Added
- **Content-aware format picker** (`picker` / `picker-api` features, both off by
  default so the publishable core stays dependency-light):
  - `MlpFormatPicker` — a [`FormatPicker`] backed by a zenpicker meta-model (an MLP
    over zenanalyze features). It only re-ranks the already-valid candidates
    auto-selection produced; it can never widen the allowed set (7e0e298, 7d889dc).
  - `MlpFormatPicker::pick_with_budget` — budget-aware family selection: a
    per-candidate additive penalty (the degradation cost of running a format at a
    cheaper effort than the model was trained at) folds into the MLP's argmin via
    zenpredict's `argmin_masked_with_scorer`, so a feasible-but-degraded format can
    lose to a rival at full effort. Reduces exactly to `pick` at zero penalties.
  - `select_format_with_budget_picker` + `FormatPicker::pick_with_penalties` (a
    trait default method that `MlpFormatPicker` overrides) — thread budget through
    the auto-selection seam with one `Fn(ImageFormat) -> Option<f32>` closure:
    `None` drops an infeasible format (heuristic head included), `Some(p)` is its
    degradation penalty. Honors format limits (registry/policy/lossless) and
    encode-resource limits in one pass.
  - `MlpFormatPicker::pick_from_offer` + `OfferPick` (`picker-api` feature) —
    negotiate feature *reuse* against a `zenanalyze_api::Offer`, so one zenanalyze
    pass feeds the meta-picker and every per-codec picker. Tri-state result:
    `Picked` / `NoCandidate` / `NeedsAnalysis` (the last when the offer can't
    satisfy the model — drift, a missing column, or a pre-`name@hash` bake).
  - New optional deps `zenpicker` / `zenpredict` (with `advanced`) / `zenanalyze-api`,
    all git-sourced from `imazen/zenanalyze` so the `zenanalyze_api` contract types
    resolve to one crate instance across the graph.
- **Metadata retention policy** (zencodec 0.1.21 adoption):
  `EncodeRequest::with_metadata_policy(MetadataPolicy)` and
  `TranscodeOptions::metadata_policy`. Defaults to `MetadataPolicy::PreserveExact`
  (verbatim embed, with a stale EXIF orientation tag reconciled — the
  double-rotation guard); set `MetadataPolicy::Web` to strip
  GPS/camera/timestamps/XMP for web publishing. `MetadataPolicy` is re-exported at
  the crate root.
- **Color-emission policy** convenience:
  `EncodeRequest::with_color_emit_policy(ColorEmitPolicy)` — merges into the encode
  policy's `color` field; codecs resolve it via `EncodePolicy::resolve_color` +
  `resolve_color_emit`. `ColorEmitPolicy` re-exported.
- **Quality-targeted transcode**: `transcode_to_quality(data, target, target_zq, ..)`
  — coefficient-domain, no pixel re-encode. JPEG→JPEG via `zenjpeg::recompress`;
  JPEG→JXL lossy via `zenjxl::jpeg_lossy` driven by a zensim Profile-A scorer
  (feature `transcode-iqa`).
- **Lossless byte-exact JPEG↔JXL transcode** (JBRD / brunsli-parity):
  - `transcode_jpeg_to_jxl_lossless(jpeg, effort)` — JPEG→JXL, byte-exactly
    recoverable (feature `jpeg-jxl-transcode`, via
    `zenjxl::LosslessConfig::encode_jpeg_transcode`).
  - `reconstruct_jpeg_from_jxl(jxl)` — the inverse JXL→JPEG reconstruction (feature
    `jxl-jpeg-reconstruct`, via `zenjxl_decoder::reconstruct_jpeg`; decode-side only,
    needs neither zenjpeg nor the JXL encoder).
  - New optional dependency `zenjxl-decoder` (with its `jpeg` feature for the JBRD box).

### Changed
- Migrated the encode dispatch (`dispatch.rs`, `dyn_dispatch.rs`) off the
  now-deprecated `EncodeJob::with_metadata` to `EncodeJob::with_metadata_policy` —
  eliminates 4 deprecation warnings and routes the retention policy to the codec
  boundary.
- `TranscodeOptions` gained a hand-written `Default` (was derived) because
  `MetadataPolicy` has no `Default` by design (metadata retention is an explicit
  privacy choice).
- **Dependency-equivalence ablation** (see
  `docs/public-api/ABLATION-zencodecs-deps.md`) — local implementations replaced
  by their canonical homes in dependencies (0f5eb6c, d08aa89):
  - `icc_profile_is_srgb` delegates to `zenpixels::icc::is_common_srgb`
    (normalized-hash, web-corpus table) — drops the local 22-entry exact-byte
    FNV table; detection is now robust to timestamp/padding-only profile
    variants (strict superset).
  - `cms::synthesize_icc_from_cicp` takes `zencodec::Cicp` and delegates to
    `zenpixels_convert::icc_profiles::synthesize_icc_for_cicp` (bundled full
    H.273 grid, no CMS). sRGB-family CICP now answers `None` (never fabricates
    an sRGB profile, matching the codec-sweep convention; callers already
    treat `None` as no-transform), BT.709-transfer inputs no longer get a
    709→sRGB micro-transform, and assigned-but-exotic code points (e.g.
    DCI-P3) now synthesize instead of silently falling through to
    assumed-sRGB. PQ/HLG still never produce a naive ICC→sRGB transform.
  - JPEG probe orientation parses via `zencodec::helpers::parse_exif_orientation`
    (was a zenjpeg::lossless reach-in + manual `Orientation::from_exif`).
  - Both gain-map RGB→gray collapses (JXL extras, Apple ProRAW) decide via
    `zenpixels_convert::PixelSliceLoadBearingExt::determine_load_bearing`;
    the ProRAW path previously sampled only the first 100 pixels before
    discarding chroma for the whole map.

### Removed
- `cms::CicpValues` — use `zencodec::Cicp` (`PngColorInfo.cicp` changed type
  accordingly; crate is unpublished, no external users) (0f5eb6c).
- `gainmap::params_to_metadata` / `gainmap::metadata_to_params` — identity
  clones since ultrahdr-core 0.5 made `GainMapMetadata` an alias for
  `zencodec::GainMapParams`; use the params directly (4a0b261).
- Dead `decode::avif_gain_map_to_params` — unused since zenavif started
  attaching `zencodec::gainmap::GainMapSource` with parsed params (4a0b261).

### Fixed
- `tests/metadata_conformance.rs`: bind the parsed `Exif` before calling
  `copyright()`, which now returns a borrowing `Cow` in zencodec 0.1.21 (was a
  borrow-after-move, E0515).
- **ultrahdr-core 0.5 drift across the `jpeg-ultrahdr` / `raw-decode-gainmap` /
  `avif-encode` feature-gated targets** — these had stopped compiling entirely
  (default-feature CI never builds them): flat `gain_map_max`/`gain_map_min`/
  `gamma`/offset field literals and `GainMapMetadata::new()` against the
  channels-based `#[non_exhaustive]` `zencodec::GainMapParams`;
  `serialize_iso21496` → `serialize_iso21496_fmt(.., Iso21496Format::JxlJhgm)`;
  `UhdrRawImage::from_data` → `ultrahdr_core::pixel_buffer_from_vec`;
  `apply_gainmap` returning a `PixelBuffer` (was `.data`);
  `zenjxl::container::is_container` → `zenjxl::is_container`; dyn decoder
  `decode_full_frame()` → `decode()`; zenwebp configs under
  `zenwebp::zencodec::` (4a0b261).
