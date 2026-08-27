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

## Known Bugs

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
  srotate/sflip/autorotate/frame/roundcorners/icc/hdr) — the generator
  only walks `#[kv]` params. Give adapter schemas synthetic ParamDescs or
  extend the generator, then regenerate.
- **`job::tests::e2e_jpeg::roundtrip_jpeg_no_nodes` fails (found 2026-07-22,
  pre-existing, unrelated to CMS/moxcms)**: minimal 8x8 JPEG in →
  `ImageJob` with `CmsMode::None` → out; the encoded output's first two
  bytes are `[0xFF, 0x0A]` instead of the JPEG SOI marker `[0xFF, 0xD8]`
  (`src/job.rs:2058`). Re-diagnosed 2026-08-27: `FF 0A` is the **JPEG XL
  codestream signature** — not a JPEG-encoder bug. `ImageJob` leaves
  `CodecIntent::format` as `None` when no output format/extension is
  given, and `zencodecs::select_format_from_intent` treats `None` like
  `FormatChoice::Auto` (`zencodecs/src/select.rs:400`), so the auto-selector
  picks JXL. The test expects "no format" to mean "keep the source format"
  (`FormatChoice::Keep`). Decide which default the job API wants, then fix
  `src/job.rs` (default `Keep`) or the test — not the encoder.
- **`export_querystring_keys_includes_kv_annotated_nodes` fails
  (`tests/regression_bridge_codec.rs:398`, `--features
  zennode,json-schema`)**: it expects `zenlayout.orient` in the querystring
  key registry "because it has `#[kv("srotate")]`", but the `Orient` node
  (`src/zennode_defs.rs:149`) carries no `#[kv]` attribute — `srotate` is
  handled by the hand-written RIAPI adapter (`src/zennode_defs.rs:1876`),
  which the `#[kv]`-only generator does not walk (same root cause as the
  "Generated docs regen pending" item above). Pre-existing on main; CI has
  not reached the test step since at least 2026-08-01 (it fails at
  dependency resolution first — see the zenavif note in the root
  `[patch.crates-io]`).
