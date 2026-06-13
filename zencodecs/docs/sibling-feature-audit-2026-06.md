# zencodecs sibling-feature audit — June 2026

What landed in the zen* codec crates this month that `zencodecs` (the dispatch
registry) should adopt, wire, or be aware of. Compiled 2026-06-13 from a per-crate
survey of `git log --since=2026-06-01` + each crate's `CHANGELOG.md [Unreleased]`.

Most items map to **zenpipe#43** (the "stub features awaiting backend wiring" tracker).
Nothing here is applied yet — this is the backlog, prioritized.

Legend: ✅ consumable from crates.io now · 🚆 blocked on a sibling publish · ⚠️ behavior/pixel change on bump · 🔒 expands public API (needs sign-off per API-stability rule).

---

## TL;DR — priority order

1. **Wire HEIC for real** ✅🔒 — `heic` 0.2.0 is published; the adapter is already written and correct. Delete the stale commented dep + the `compile_error!` stub, add the crates.io dep with `backend-rust`, and add an `ultrahdr-core` git patch at the workspace root. (User-requested: probing.)
2. **Un-stub `bitmaps-qoi` / `bitmaps-tga` / `bitmaps-hdr`** ✅🔒 — zenbitmaps 0.1.5 (published) already ships all six configs; adapters already reference them. Three feature-line edits + delete three `compile_error!`s.
3. **Un-stub `tiff`** ✅🔒 — zentiff 0.1.2 (published) adapter is complete and current. One dep line + feature edit + delete the `compile_error!`. Needs zenpixels-convert ≥ 0.2.13 (covered by #4).
4. **Bump the additive, already-published pins** ✅ — zenpixels/convert 0.2.10→0.2.14, linear-srgb 0.6.3→0.6.12, zenwebp 0.4.4→0.4.5, zenjpeg declared floor →0.8.7. All additive, all verified symbol-compatible.
5. **Fix workspace-root pin drift** ✅ — `[workspace.dependencies]` still lists `zencodec 0.1.16`, `zenavif 0.1.3`, `zenjxl 0.2.0` — stale vs the registry's real pins.
6. **Watch the AVIF + JXL publish train** 🚆⚠️ — real correctness fixes (AVIF pixel corruption, JXL transcode) exist only in git; the registry keeps the buggy 0.1.6/0.2.1 behavior until they publish.

---

## 1. HEIC — wire it (user-requested) ✅🔒

The adapter at `src/codecs/heic.rs` already calls the **current** API
(`heic::HeicDecoderConfig::new().job()` → `.probe(data)` / `.decoder(Cow::Borrowed(data), &[]).decode()`).
It is dead code today: `mod.rs:30` gates `mod heic` behind `heic-decode`, which is a
`compile_error!` stub (`lib.rs:120-123`), and the dep is commented out (`Cargo.toml:42`,
stale path `../../heic-decoder-rs`). The real crate is **`heic` 0.2.0, published on
crates.io** (`/home/lilith/work/zen/heic`, multi-backend, `heic-core` + 5 HW backends).

**Probe works without a backend** — `ImageInfo::from_bytes` only parses the HEIF
container + HEVC SPS/`hvcC`/`ispe`/`av1C`; it never instantiates a decoder. But the
crate's `compile_error!` gate still requires *a* backend feature to compile, so
`backend-rust` must be enabled regardless. `PROBE_BYTES = 4096`. Probe surface includes
dims (orientation-applied), `has_alpha`, `bit_depth`, `chroma_format`, CICP
(primaries/transfer/matrix/full-range), `has_icc_profile`, `has_exif`/`has_xmp`,
`has_depth`, `has_gain_map`, plus raw `exif`/`xmp`/`icc_profile`.

**Backend allowlist (0.2.0's breaking change — but additive for the registry):**
`DecoderConfig::{with_backend, with_backends, recommended_backends}` + error
`HeicError::NoBackendSelected` (fires only on an *empty* allowlist). `HeicDecoderConfig::new()`
already installs `recommended_backends()`, which pushes `Backend::Rust` last whenever
`backend-rust` is compiled — so the list is never empty and **no adapter-code change is
needed**. Do **not** add a manual `.recommended_backends()` call. Native backends
(videotoolbox/mediafoundation/mediacodec/vaapi/d3d11va) are `std`+FFI, platform-gated, and
patent-pool relevant (HEVC/Access Advance — Imazen grants copyright only, decode-only); they
aren't plumbed into decode dispatch yet, so enabling them buys nothing functional for a
portable build. **`backend-rust` is the correct sole default** for the no_std+alloc/wasm posture.

**Detection:** the `heic` crate exports no sniffer — keep zencodecs' own ftyp-brand
detector. Route brands `heic / heix / hevc / hevx / mif1 / msf1 / mif2 / mif3` → HEIC;
route `avif / avis` → AVIF (do not send them to the HEIC adapter).

**Gain map / HDR (June ISO 21496-1 `tmap` work, additive):** `tmap` payload (iOS 18+
"Adaptive HDR", Samsung) is now authoritative, EXIF MakerNote headroom is the legacy-Apple
fallback. `GainMapRender::{BaseOnly, Components, ReconstructHdr{target_headroom}}`;
`reconstructs_hdr() == true`; probe reports `GainMapPresence::Available`. The registry's
format-agnostic `src/gainmap.rs` has **no HEIC arm** (only Jpeg/Avif/Jxl) — route HEIC
through the `Decode` trait with `GainMapRender::Components` (or the existing
`extract_gain_map=true` already threaded through `codecs/heic.rs:29`). There is no
standalone `decode_gain_map` free fn.

**Depth — the stub is already implemented, not a stub:** `extract_heic_depth`
(`src/decode.rs:837`) already calls `heic::DecoderConfig::decode_depth` and maps
`DepthRepresentationType` → `DepthFormat`/`DepthUnits` with `DepthSource::AppleHeic`. It
only needs the feature to compile. (The CLAUDE.md "HEIC depth not implemented" note is **stale**.)

### Actions
- [ ] `Cargo.toml:42` — delete the commented `heic-decoder` line; add:
  `heic = { version = "0.2.0", optional = true, default-features = false, features = ["zencodec", "backend-rust"] }`
- [ ] `Cargo.toml:102` — replace stub with `heic-decode = ["dep:heic"]`; add `heic-decode` to `all`.
- [ ] `lib.rs:120-123` — delete the `heic-decode` `compile_error!` block.
- [ ] **Workspace root** `[patch.crates-io]` (`zenpipe/Cargo.toml:214`) — add
  `ultrahdr-core = { git = "https://github.com/imazen/ultrahdr", rev = "3ac20f9…" }`.
  Verified absent today. `heic`'s *internal* patch does not propagate to consumers, and
  crates.io ultrahdr-core 0.5.0 predates the Apple MakerNote HDR parser the gain-map path uses.
- [ ] `src/codecs/heic.rs` — **no change** (already current).
- [ ] After wiring: `cargo update`, confirm `heic` resolves to 0.2.0 from crates.io (not a path), update the stale depth note in CLAUDE.md.

---

## 2. Un-stub bitmaps QOI / TGA / HDR — ready against published 0.1.5 ✅🔒

zenbitmaps 0.1.5 (published, tag) already ships `QoiEncoderConfig`/`QoiDecoderConfig`,
`TgaEncoderConfig`/`TgaDecoderConfig`, `HdrEncoderConfig`/`HdrDecoderConfig` (each a full
`EncoderConfig`/`EncodeJob`/`Encoder` + `DecoderConfig`/`DecodeJob`/`Decode`(+`StreamingDecode`)
stack, mirroring `Bmp*Config`). The zencodecs adapters `src/codecs/{qoi,tga,hdr}.rs` already
call these exact names. Only the `compile_error!`s + missing feature-passthrough block them.
The configs are gated behind zenbitmaps' `qoi`/`tga`/`hdr` *format* features, so the
passthrough must enable them (exactly like `bitmaps-bmp` enables `zenbitmaps/bmp`).

### Actions
- [ ] `Cargo.toml:108-110`:
  `bitmaps-qoi = ["bitmaps", "zenbitmaps/qoi"]` · `bitmaps-tga = ["bitmaps", "zenbitmaps/tga"]` · `bitmaps-hdr = ["bitmaps", "zenbitmaps/hdr"]`
- [ ] `lib.rs:124-135` — delete the three bitmaps `compile_error!` blocks.
- [ ] (Optional) publish zenbitmaps 0.1.6 later for the load-bearing-narrowing optimization (`d18de98`, additive). Not required to un-stub.

## 3. Un-stub `tiff` — adapter complete against published zentiff 0.1.2 ✅🔒

`zentiff` 0.1.2 (published, `/home/lilith/work/zen/zenextras/zentiff`) exposes exactly
the API `src/codecs/tiff.rs` already calls: `TiffDecoderCodecConfig::new().job().probe()`,
`.job().decoder(Cow, &[])`, `TiffEncoderCodecConfig`. Its zencodec floor is 0.1.22 = the
registry's. June work closed the gaps: OrientationHint (`efcc2af`/`97da4d7`), EXIF
embed/decompose into native IFDs (`50955e6`, was metadata-dropping), 6-OS CI (`c0d2406`).
Supports ICC/EXIF/XMP/IPTC/resolution/orientation.

### Actions
- [ ] `Cargo.toml` deps — add
  `zentiff = { version = "0.1.2", optional = true, default-features = false, features = ["std", "deflate", "lzw", "zencodec"] }`
  (use `zentiff/all-codecs` instead of `deflate,lzw` if fax/jpeg/webp/zstd-compressed TIFF input matters).
- [ ] `Cargo.toml:103` — `tiff = ["dep:zentiff"]`; add `tiff` to `all`.
- [ ] `lib.rs:136-139` — delete the `tiff` `compile_error!`.
- [ ] Requires zenpixels-convert ≥ 0.2.13 in the graph (zentiff pulls it) — satisfied by §4's 0.2.14 bump.

> `svg` and `jp2-decode` stay stubbed — no backend exists yet.

---

## 4. Version-pin bumps — additive, published, safe now ✅

All symbol-compatibility verified against each crate's public-API snapshot.

| Dep | Pinned | Bump to | Why |
|---|---|---|---|
| `zenpixels` | 0.2.10 | **0.2.14** | F16 formats, BT2100 PQ/HLG presets, ICC-recognition table growth (helps `color.rs`), overflow guards. Keep `default-features = false, features = ["imgref"]`. |
| `zenpixels-convert` | 0.2.10 | **0.2.14** | `synthesize_icc_for_cicp` (already called in `cms.rs`), `load_bearing` module, signal-range correctness. **Keep default features ON** so `icc-db` stays — disabling it silently shrinks ICC emission. |
| `linear-srgb` | 0.6.3 | **0.6.12** | Not imported directly; additive + a cross-path slice-tail polynomial pixel-divergence fix. (Task said 0.6.8 — actual latest is 0.6.12.) |
| `zenwebp` | 0.4.4 | **0.4.5** (released) | #57 L8/La8 lossless through full VP8L (~20× size fix) + stop-token fix. Empty public-surface diff. |
| `zenjpeg` | 0.8.4 | **0.8.7** | MPF parse fix (`3684ebb2`) repairs the registry's *own* `ultrahdr_sample.jpg` fixture; XMP hdrgm element-form fix (`689c5686`) repairs gain-map params; decode `ImageInfo` now carries bit_depth/channel_count. Additive (the June breaks were all on expert/internal surfaces the registry never imports). Confirm 0.8.7 is on crates.io vs git-pinned. |
| `zenjxl-decoder` | 0.3.8 | **0.3.9** floor | Honesty: 0.3.9 is the floor that actually contains `reconstruct_jpeg` + JBRD fixes. Git patch overrides resolution regardless; no code change. |
| `zenraw` | 0.2.0 | hold (0.2.1 later) | Probe-crop fix + OrientationHint adapter land in 0.2.1 (unpublished). Pin compiles as-is. |
| `ultrahdr-core` | 0.5.0 | hold (0.5.1 later) | Gain-map math API unchanged. New Apple MakerNote parser is additive + not yet consumed. |
| `zencodec` | 0.1.22 | hold | Already current. HEAD is +12 unreleased (only the Fidelity API). |

> After bumps, `cargo update` to pull matching `archmage`/`magetypes` 0.9.26 + `lz4_flex`.

---

## 5. Workspace-root pin drift — fix ✅

`zenpipe/Cargo.toml` `[workspace.dependencies]` under-pins vs the registry's real deps:
- `zencodec = "0.1.16"` (line 191) — 6 patches stale vs the registry's 0.1.22. Load-bearing: any member resolving `zencodec.workspace = true` gets a floor that under-specifies the 0.1.21+ metadata/color-emit/gain-map APIs. **Bump to 0.1.22.**
- `zenavif = "0.1.3"` (line 131), `zenjxl = "0.2.0"` (line 132) — staler than the registry's 0.1.6 / 0.2.1. Align upward.

All caret-compatible, so they co-resolve today; the fix makes the floors honest and blocks a future accidental `--precise` downgrade.

---

## 6. Publish-train blockers — git-only, not yet consumable 🚆⚠️

The registry already builds against unreleased git via workspace `[patch.crates-io]`
(zenjxl, jxl-encoder, zenjxl-decoder). These cannot be pinned to a release until published,
and some carry **correctness fixes the registry currently lacks**:

- **AVIF stack** 🚆⚠️ — *nothing June is on crates.io.* zenavif 0.1.6→**0.1.8**, zenavif-parse 0.6.2→**0.6.3**, zenrav1e→0.1.5, rav1d-safe→0.5.8. Until they ship, the registry keeps:
  - `a074b89` **pixel corruption**: identity-MC (MC=0) decoded through BT.601 (every pixel wrong); 16-bit encode plane order `[r,g,b]` where AV1 wants G,B,R; H.273 P3/BT.2020/FCC silently guessed BT.601.
  - `f3c9f04` (zenavif-parse): `size=0` (extends-to-EOF) boxes — **gating fix** for real Apple HDR gain-map files; without it the registry's AVIF `decode_gain_map` rejects them outright.
  - `c3567081` (zenrav1e): AVIF "lossless" (q=0) was actually lossy with ±2 error on 7–28% of pixels.
  - Additive once shipped: `GainMapRender` modes + applying `ReconstructHdr`, native Gray8/Gray16 decode, OrientationHint, `ColorContext` on decoded buffers.
- **zenjxl** 🚆 — `transcode.rs` imports `zenjxl::jpeg_lossy::*`, which exists only on git main. Cut **zenjxl 0.2.2** (jxl-encoder 0.3.2 first), then pin. Behavior to flag on adopt: `eb0711ee` makes `transcode_jpeg_to_jxl_lossless` cleanly reject >2 chroma-factor JPEGs (was broken output); `780d45e` makes lossy `Reencode` preserve source ICC.
- **zenjxl-decoder** 🚆 — crates.io 0.3.10 ≠ git 0.3.10 (older content, same version). Cut **0.3.11** (orientation `cf97249` + `reject_progressive`) to drop the git patch. Needed by zenjxl's adapter, not by zencodecs directly.
- **zenpng 0.1.5 / zengif 0.7.4** 🚆 — see §7/§8.

---

## 7. New capabilities the registry could wire — additive, future 🔒

**Status (2026-06-13):** the user asked to "do the future stuff, skip the fidelity api." Outcome below — ✅ landed / ⏳ verified-but-publish-blocked / ⏭️ skipped.

- ✅ **HDR reconstruction (JPEG)** — **landed** (commit `025ffb01`). `DecodeRequest::reconstruct_hdr(target_headroom)` / `with_gain_map_render(GainMapRender)` drive zencodec's `ReconstructHdr` through the decode trait; JPEG UltraHDR (`0e7b46f8`) emits linear-float HDR + CLL/mastering. Non-supporting formats return `UnsupportedOperation` (honesty guard) rather than a silent SDR buffer. Also repaired a pre-existing `ultrahdr-core` workspace-patch drift that was breaking clean `jpeg-ultrahdr` builds.
  - ⏳ **AVIF/JXL/HEIC HDR reconstruction** — publish-blocked. Their *locked* decoders report `reconstructs_hdr() = false` (the applying-`ReconstructHdr` work is in unpublished zenavif 0.1.8 `c02dd6e` / newer zenjxl / heic 0.2.0). The honesty guard correctly errors for them today. **Wire-when-published**: unify the avif/jxl/heic adapters' `extract_gain_map` bool into a `GainMapRender` arg + extend the guard to a per-codec `reconstructs_hdr()` capability check — deferred deliberately so it's written and tested against the real dep, not blind.
- ✅ **zenjpeg `supplements.gain_map`** (`94fb6ec6`) — already wired in the JPEG probe path (`codecs/jpeg.rs:62`); no work needed.
- ⏳ **AVIF native Gray8/Gray16 decode** (`d01cca6`) — **verified: zencodecs needs NO code change.** The avif adapter (`codecs/avif_dec.rs:48`) already passes empty preferred-descriptors (`&[]`) and returns zenavif's native output, so gray surfaces automatically. Purely publish-blocked on zenavif 0.1.8 (see §6).
- ✅ **pdf-decode** — **landed** (commit `ec08e5e9`). zenpdf 0.2.0 (hayro) wired as a decode-only `ImageFormat::Custom`; opt-in `pdf-decode` feature; `%PDF-` detection; renders page 0; 3 tests.
- ⏳ **zengif `quantizer_preference(series)` + `QuantizerBackend`** (`853574a`) — additive encode knob; publish-blocked on zengif 0.7.4.
- ⏭️ **zencodec Fidelity API** (`f0f9527`, unreleased) — **skipped per user.** When a 0.1.23 ships, route Distance/Metric/TargetBytes/NearLossless through `try_target_fidelity` + `EncodeCapabilities::{near_lossless, supports_distance, supports_metric_target, supports_size_target}`.

> Gain-map *encode* embedding is still deferred upstream — no `with_gain_map` encode carrier in zencodec (the registry's `with_gain_map` builder remains decode-extract only).

---

## 8. Behavior/pixel changes to expect on bump ⚠️, and stale notes to clear

**Behavior changes (correctness wins, but observable output moves) when pins advance:**
- zenjpeg `d5d8ae11` — EXIF-rotated 4:2:0 auto-orient was producing wrong pixels; now lossless.
- zenwebp `503bff5` — L8/La8 lossless output ~20× smaller.
- linear-srgb — slice-tail now matches the SIMD polynomial (unaligned-slice pixel divergence fix).
- AVIF/JXL — see §6 (gated on publish).

**Stale notes to clear in `zencodecs/CLAUDE.md`:**
- "HEIC auxiliary depth image extraction (requires heic-decoder auxid support)" under *What's NOT implemented* — `extract_heic_depth` is written; clears once HEIC compiles.
- zenpipe#36 — the zenpng decode-orientation sweep that trips `orientation_from_exif_tag` /
  `transcode_orientation_transfer_matches_carrier_support` **landed committed** (`560e793`).
  Re-derive the metadata_conformance verdict table once zenpng 0.1.5 publishes.

---

## Appendix — per-crate June status

| Crate | June commits | Current | Registry pin | Consumable? |
|---|--:|---|---|---|
| heic | 18 | 0.2.0 | (commented out) | ✅ crates.io — wire it |
| zenjpeg | 84 | 0.8.7 | 0.8.4 | ✅ (confirm 0.8.7 published) |
| jxl-encoder | 134 | 0.3.2 | (via zenjxl) | 🚆 publish 0.3.2 |
| zenjxl | 25 | 0.2.1 | 0.2.1 (git-patched) | 🚆 publish 0.2.2 |
| zenjxl-decoder | 39 | 0.3.10(git) | 0.3.8 (git-patched) | 🚆 publish 0.3.11 |
| zenavif | 39 | 0.1.7 | 0.1.6 | 🚆 publish 0.1.8 ⚠️ |
| zenavif-parse | 7 | 0.6.3-prep | 0.6.2 | 🚆 publish 0.6.3 |
| zenrav1e | 12 | 0.1.4 | (via zenavif) | 🚆 publish 0.1.5 ⚠️ |
| rav1d-safe | 10 | 0.5.7 | (via zenavif) | 🚆 publish 0.5.8 |
| zenpixels | 76 | 0.2.14 | 0.2.10 | ✅ bump |
| zenpixels-convert | — | 0.2.14 | 0.2.10 | ✅ bump |
| linear-srgb | 12 | 0.6.12 | 0.6.3 | ✅ bump |
| zencodec | 41 | 0.1.22 | 0.1.22 | ✅ current |
| zenwebp | 19 | 0.4.5 | 0.4.4 | ✅ bump |
| zenpng | 15 | 0.1.4(+unrel) | 0.1.4 | 🚆 publish 0.1.5 |
| zengif | 14 | 0.7.3(+unrel) | 0.7.3 | 🚆 publish 0.7.4 |
| zenbitmaps | 7 | 0.1.5 | 0.1.5 | ✅ un-stub qoi/tga/hdr |
| zentiff | 7 | 0.1.2 | (stub) | ✅ un-stub tiff |
| zenraw | 8 | 0.2.0 | 0.2.0 | ✅ (0.2.1 later) |
| ultrahdr-core | 17 | 0.5.0 | 0.5.0 | ✅ (0.5.1 later) |
| zenpdf | 7 | 0.2.0 | ✅ wired | `pdf-decode` landed (ec08e5e9) |
| zentone | 4 | 0.1.0 | — | not consumed |
| fax | 0 | — | — | dormant |
