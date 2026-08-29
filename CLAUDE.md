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
- **The rav1d-safe pin is held back at `140f9145`** (via `zenavif 11033c95`)
  while zenavif, ravif and zenmetrics are all on `66f58fa6`. Not neglect —
  measured: at `66f58fa6` the command above fails 2 of 2 runs with a panic in
  rav1d-safe's bounds-map guard on aarch64 (`filmgrain_arm.rs:1628` reserves
  122880 B against a 3840 B ceiling under tile threading), then
  `Option::unwrap()` on `None` at `thread_task.rs:534`. Filed as
  **rav1d-safe#526**. Move the pin when that closes, and re-run the command
  rather than assuming.

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
