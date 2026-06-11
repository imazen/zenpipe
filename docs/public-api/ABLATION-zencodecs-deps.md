# ABLATION-zencodecs-deps.md — dependency-equivalence pass

**Date:** 2026-06-11
**Mode:** REFACTOR — each zencodecs function checked for an equivalent
implementation already present in its dependencies; equivalents delegated,
gaps recorded. Complements `ABLATION-zencodecs.md` (which audited the public
surface for leaks; this audits the *implementations* for duplication).
**Verification:** `cargo check -p zencodecs --features all,cms,std --all-targets`
clean; full `cargo test` run recorded in the commit series.
**External-usage scan:** every changed symbol grepped across `~/work`
(excluding zenpipe/zencodec) — zero external users; zencodecs is unpublished.

---

## Refactored — dependency equivalent existed

| zencodecs item | Was | Now delegates to | Commit |
|---|---|---|---|
| `gainmap::params_to_metadata` / `metadata_to_params` | identity `clone()` pair (ultrahdr-core 0.5 made `GainMapMetadata` = `zencodec::GainMapParams`, `types.rs:391`) | deleted; call sites use params directly | 4a0b261 |
| `decode::avif_gain_map_to_params` (~45 LOC) | rational→log2 conversion, **dead** (zenavif attaches `zencodec::gainmap::GainMapSource` with parsed params since codec.rs:2510) | deleted | 4a0b261 |
| `color::icc_profile_is_srgb` (~65 LOC FNV + 22-hash table) | exact-byte FNV-1a table | `zenpixels::icc::is_common_srgb` (normalized hash, web-corpus table — the exact replacement zencodec 0.1.16 deprecated *its* shim toward) | 0f5eb6c |
| `cms::CicpValues` | private Cicp clone (British field spelling) | `zencodec::Cicp` everywhere; `PngColorInfo.cicp: Option<Cicp>` | 0f5eb6c |
| `cms::synthesize_icc_from_cicp` (~52 LOC moxcms profile building, primaries {1,9,12} × transfer {1,6,13} only) | hand-built BT.2020/P3 profiles | `zenpixels_convert::icc_profiles::synthesize_icc_for_cicp` (bundled full assigned-H.273 grid, no CMS, moxcms byte-equality-tested) + explicit PQ/HLG guard | 0f5eb6c |
| `codecs/jpeg` probe orientation | `zenjpeg::lossless::parse_exif_orientation` (u8) + manual `Orientation::from_exif` | `zencodec::helpers::parse_exif_orientation` (returns `Orientation`) | 0f5eb6c |
| gray-collapse predicate ×2 (`decode::extract_jxl_gainmap`, `codecs/raw::extract_gainmap`) | hand-rolled R==G==B scans; the ProRAW copy sampled **only the first 100 px** then discarded chroma for the whole map | `zenpixels_convert::PixelSliceLoadBearingExt::determine_load_bearing().uses_chroma == Some(false)` (canonical full scan) | d08aa89 |

Deliberate semantics change in the `synthesize_icc_from_cicp` delegation
(documented in CHANGELOG): the sRGB/BT.709 default family (primaries 1/2 ×
transfer 1/2/13) answers `None` — the shared layer never fabricates an sRGB
profile, so a PNG `cICP (1,1)` no longer triggers a 709→sRGB micro-transform
(matches browser convention and the 2026-06-08 codec-sweep behavior), while
assigned-but-exotic code points (DCI-P3 etc.) now synthesize instead of
falling through to assumed-sRGB.

Drift repair done en route (4a0b261): the `jpeg-ultrahdr` /
`raw-decode-gainmap` / `avif-encode` feature targets had **not compiled** since
ultrahdr-core 0.5 (flat-field literals, `GainMapMetadata::new()`,
`serialize_iso21496`, `UhdrRawImage::from_data`, `zenjxl::container::*`, dyn
`decode_full_frame`, `zenwebp::Webp*Config` paths). Default-feature CI never
builds this code — see Recommendations.

---

## Kept — no dependency equivalent (with gap notes)

| Module / fn | Verdict | Why |
|---|---|---|
| `exif.rs` (2,068 LOC, typed `ExifData` extractor incl. DNG matrices) | KEEP, gap noted | `zencodec::exif::Exif` keeps entries private and exposes no typed make/model/exposure/GPS/DNG accessors, so `ExifData` cannot be rebuilt on it today. **Gap:** a public entry-iteration / typed-tag API in `zencodec::exif` would let this module shed ~1,200 LOC of duplicate IFD walking. Note: zero in-workspace production callers (fuzz target + pub API only); prior ablation cites zenpipe::sidecar/imageflow_compat as consumers — verify before any deeper cut. |
| `cms::srgb_icc_profile` | KEEP, gap noted | Used as the *destination* profile for ICC→sRGB transforms; `synthesize_icc_for_cicp` deliberately answers `NotNeeded` for sRGB. **Gap:** zenpixels-convert could export canonical sRGB profile bytes for destination use; the `cms` feature would then shrink further. |
| `cms::is_srgb_icc_structural` | KEEP | Structural primaries+TRC comparison over *arbitrary* profiles (vendor sRGB variants); zenpixels recognition is table-based — different tool, complementary. |
| `cms::parse_png_color_chunks` + `synthesize_icc_from_gama` | KEEP, gap noted | Raw-bytes gAMA/cHRM/sRGB/cICP scan without a decode; arbitrary-chromaticity ICC synthesis needs moxcms. **Gap:** zenpng does not surface gAMA/cHRM through `SourceColor`, forcing this re-parse. |
| `limits.rs` `Limits` | KEEP | Builder-friendly superset converting via `to_resource_limits` — wrapper by design, not duplication. |
| `depthmap.rs` (1,488 LOC) | KEEP | zencodec has only the `Supplements::depth_map` slot; this is the home for now. `DepthImage::resize` hand-rolls scaling (zenresize is not a dep) — acceptable for depth maps, noted. |
| `info.rs` | KEEP | Thin over `zencodec::ImageFormatRegistry::common().detect` + per-codec probes. `finalize_implicit_srgb` / `finalize_gain_map_presence` are policy, not duplication. |
| `quality / select / decision / intent / transcode / dispatch / dyn_dispatch / registry / policy / config / trace / format_set / codec_id / riapi_parse / zennode_defs` | KEEP | Orchestration — zencodecs' own domain. Pattern-swept (`from_be/le_bytes`, `powf`, `chunks_exact`) for byte-parsing/color-math duplication: clean. |
| `codecs/*` adapters | KEEP | Trait plumbing per codec. |

---

## Findings flagged, not fixed here

1. **`codecs/jpeg::encode_ultrahdr_rgb_f32` / `_rgba_f32` ignore their
   `_metadata` parameter and hardcode `UhdrColorGamut::Bt709`** — a P3/BT.2020
   HDR input is mistagged Bt709 on the UltraHDR encode path. Needs a decision
   (thread `SourceColor`→gamut), not a silent fix.
2. `tests/gainmap_integration.rs` `avif_seine_gainmap` contains a
   graceful-skip (`if !path.exists() { eprintln!("SKIP"); return; }`) —
   banned pattern; should become a codec-corpus-managed fixture.
3. `tests/stop_latency.rs` `cancel_instant` is dead code (pre-existing).
4. zcimg (CLI sub-crate) parses EXIF a third way (kamadak-exif) for display
   and pins `zencodec = "0.1.12"` — out of lib scope, worth a sweep of its own.

## Recommendations

- **Add a CI job building `-p zencodecs --features all,cms,std --all-targets`.**
  The entire gain-map surface rotted invisibly because default features never
  compile it.
- Upstream gaps worth filing: zencodec::exif entry iteration; zenpixels-convert
  canonical sRGB destination bytes; zenpng gAMA/cHRM in `SourceColor`.
