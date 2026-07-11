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

Confirmed at source level 2026-07-11; the 2026-07-11 fix wave (commits
009c7938..f7d1900d) closed most of the original list. Remaining:

- **Animated input + processing steps → still image**: compat's animation
  passthrough correctly fires only for no-op jobs (`execute.rs`), but jobs
  *with* steps fall through to single-frame processing; `animation::transcode`
  (per-frame pipelines) is called only from tests. "Resize this GIF" loses the
  animation. (Parity workstream W8.)
- **JPEG alpha flatten ignores the configured matte color** — hardcodes white
  (`src/job.rs` `needs_alpha_removal` TODO); pending zenresize matte support.
  The fused geometry path drops `matte_color` for the same reason.
- **JPEG/WebP decoder downscale hints dropped**: `jpeg_downscale_hints` /
  `webp_decoder_hints` / `decoder.min_precise_scaling_ratio` have zero
  handling in `src/imageflow_compat/`; zenjpeg exposes no public scaled-IDCT
  knob yet; the generic decode node's `min_size` hint is unwired. (W6.)
- **`AllocationTracker` still has no call sites** (the estimate-gate +
  codec-limit enforcement added 2026-07-11 covers the practical budget; the
  tracker itself remains API-only — wire or delete, W7 remainder).
- **compat `ExecutionSecurity`**: `max_total_file_pixels`, `max_threads`,
  `max_json_bytes`, `mem_budget_policy` still unenforced
  (`max_input_file_bytes` and the three size limits are enforced).
- **`png.quantization_speed` / `jpeg.turbo` / `jpeg.li`** remain
  compat-engine-only: the zen node→encoder config path has no consumer for
  them (W10 encode-config audit).
- **`larger_than` constraint mode unsupported** (`zenlayout` has no
  equivalent; the bridge errors on it).
- **mode×scale approximations**: `(crop, scale=canvas)` maps to WithinCrop
  (imageflow does a partwise crop + virtual canvas), `(stretch, canvas)` maps
  to plain Distort. Cross-axis `width`+`maxheight` bounding still needs
  source aspect at the preprocess layer. (Documented in
  `src/bridge/geometry.rs` comments; W9 behavioral suite should quantify.)

Fixed 2026-07-11 (regression tests in `tests/riapi_keys.rs`,
`src/bridge/geometry.rs`, `src/riapi.rs`, `tests/gainmap_roundtrip.rs`):
mode aliases + pad default + mode×scale composition; crop units/negative
coords/no-op; srotate/sflip semantics + PostRotate/PostFlip ordering;
s.roundcorners percentage + 4-value; f.sharpen_when; named/bare-hex bgcolor;
anchor=x,y + IR4 anchor spellings; srcset wiring + legacy pairs + IR4
default-mode rule; Constrain gravity/canvas_color/zoom/up_filter/sharpen
actually reaching execution; quality_intent profile-default bug (dead
`quality=`, format flipped to auto); ignoreicc/ignore_icc_errors;
webp.lossless; Limits→codec-request threading + pre-flight estimate gate +
max_output_bytes + deadline; hdr=/gainmap= directives (preserve/strip/
reconstruct); imageflow-compat CmsMode drift (feature didn't compile).

Stale-doc queue (do not fix piecemeal; batch per repo rules):
`IMAGEFLOW-PARITY.md` §9 — plus, after this wave: parity doc §3/§4 status
cells for the fixed keys, and `docs/querystring.md` regeneration (new
adapters aren't in the generated docs).
