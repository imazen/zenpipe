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

### Comparing unpublished API surfaces

`just semver-against <reviewed-source-export>` builds stable-toolchain rustdoc JSON inside the actual workspaces and compares the resulting API graphs at patch level. This preserves root git patches for unpublished dependencies that a synthetic outer workspace loses. `just test-streaming-seam`, `just test-dispatched-output`, and `just streaming-clippy-gate` exercise the dispatched row and transport contract.
