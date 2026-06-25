# zencodecs → transcoding-core reshaping plan (2026-06)

## Thesis

zencodecs has accreted into a kitchen-sink dispatch + selection + metadata + supplement
layer. Its **unique, irreducible value** is two things neither `zencodec` (the trait crate)
nor imageflow's own inline dispatch provide:

1. **The format-selection oracle** — intent → `FormatDecision`, the MLP picker, quality
   calibration, selection trace.
2. **The transcode engine** — decode→re-encode with metadata/supplement carry-through,
   quality-targeted recompression, lossless JPEG↔JXL.

Everything else is either (a) duplicated by `zencodec`, (b) duplicated by imageflow's own
`codecs/zen_encoder.rs`+`zen_decoder.rs`, (c) movable wholesale upstream, or (d) a separable
integration concern.

### Load-bearing reality check (verified 2026-06-25)

- **imageflow does NOT use zencodecs.** `imageflow_core` depends on `zencodec` (as `zc`) +
  the raw codec crates, with its own dispatch in `src/codecs/zen_{encoder,decoder}.rs`.
  `zencodecs::` appears 0× in imageflow_core/src. So reshaping zencodecs cannot break the
  product. zencodecs' real consumers are the zenpipe-side tooling: `zenpipe`,
  `zencodecs-cli`, `zenfilters`, `zeneditor`, `zcimg`, `zentone`, `codec-eval`,
  `zensquoosh-codecs`.
- **Much of "adopt existing zencodec" is already done.** `color.rs` already delegates the
  ICC hash to `zenpixels::icc`; `estimate.rs` re-exports zencodec's estimate types;
  `info.rs::detect_format` delegates to `zencodec::ImageFormatRegistry::common().detect()`;
  `limits.rs` already forwards `AllocPreference`/`ThreadingPolicy`. The crate is further
  along this path than a static LOC inventory suggests.

## LOC baseline (2026-06-25)

19,704 total = **13,768 production + 5,936 test**. Target after reshaping: roughly a
40–50% production-LOC reduction, with the remainder being genuine transcoding/selection value.

## Module disposition

| Module | LOC (prod) | Disposition |
|---|---|---|
| select / intent / policy / decision / quality / trace / codec_id | ~2,600 | **KEEP** — the oracle. zencodec deliberately doesn't pick formats. |
| transcode | ~740 | **KEEP** — the transcode engine. The crate's reason to exist. |
| decode / encode (request builders) | ~2,000 | **KEEP core**, shrink dispatch (see below). |
| dispatch / dyn_dispatch / info / registry | ~1,600 | **KEEP**, collapsible only via a zencodec codec-registry (deferred, design tradeoff). |
| **exif.rs** | 1,001 | **SHED** — fuzz-only-consumed rich IFD extraction; duplicates zencodec's `Exif` walker. Biggest clean in-crate win. |
| **depthmap.rs** | 703 | **UPSTREAM** — 100% format-agnostic; move to `zencodec::depthmap`, re-export. Needs upstream PR. |
| gain-map special-casing (jpeg/avif/jxl/raw adapters) | ~400–480 | **UPSTREAM** — route `with_gain_map` through `zencodec::EncodeJob`; deletes the 4× hand-assembly. Needs upstream + codec PRs. |
| format_set.rs | 150 | **UPSTREAM** (optional) — `zencodec::FormatSet` over `ImageFormat` (non_exhaustive + Custom design cost). |
| limits.rs | 235 | **DEFENSIBLE DUP** — `ResourceLimits` is `#[non_exhaustive]`, so the ergonomic struct-literal `Limits` is a justified shim. Adopting upstream = ~130 LOC delete + ~20 test-site builder conversions. Medium priority. |
| color.rs | 55 | **DONE-ish** — already delegates; the only deletable bit (`SourceColorExt`) needs an upstream `SourceColor::is_srgb()`. |
| cms.rs | 429 | **KEEP** — moxcms transform-decision layer; zencodec stays moxcms-free by policy. |
| zennode_defs.rs | 829 | **KEEP / maybe separate crate** — the zennode integration schema. |
| riapi_parse.rs | 266 | **RELOCATE up** (optional) — belongs in imageflow_riapi, not down in zencodec. |
| config / error / pixel | ~390 | **KEEP** — aggregating glue with nothing upstream to delegate to. |

## Ordered work plan

Each chunk: land with green tests, commit, push, before the next.

- [x] **0. Plan doc** (this file).
- [ ] **1. Bump `zencodec` 0.1.22 → 0.1.25** (Cargo.lock already resolves 0.1.25). Formalizes
  relying on the 0.1.25 surface. Trivial; verify build.
- [x] **2. exif.rs reduction** (DONE 85ecbb5, −2085 LOC) — delete the rich `ExifData` extraction parser (parse_exif +
  IFD walker + typed getters), keeping only what's actually consumed internally
  (orientation already via `zencodec::helpers::parse_exif_orientation`; the zenraw
  `from_raw_metadata` bridge if still needed). Retire the `fuzz_exif` target or repoint it at
  `zencodec::exif::Exif`. **~800–1000 LOC.** Biggest in-crate win.
- [x] **3. limits.rs** (DONE, −331 LOC) — alias `Limits = zencodec::ResourceLimits`, convert the ~20
  struct-literal sites to builders, drop `to_resource_limits` + conversion tests. ~130 LOC.
  Only if the ergonomic regression is acceptable.
- [ ] **4. UPSTREAM PRs** (need zencodec/codec changes — file as zenpipe issues):
  - `zencodec::depthmap` ← move depthmap.rs (~700 LOC).
  - `EncodeJob::with_gain_map_source` ← collapse the 4× gain-map special-casing (~450 LOC).
  - `zencodec::FormatSet` ← format_set.rs (~120 LOC).
  - `SourceColor::is_srgb()` ← color.rs `SourceColorExt` (~30 LOC).
  - `ExifData`-style typed extraction on `zencodec::exif` (if extraction is wanted back).

## Notes

- `docs/` is package-excluded; this file does not ship.
- Verification: iterate on `--features jpeg,png,webp,gif,std` (fast, covers doctests);
  full `just test` (`--all-features`) before any push.
