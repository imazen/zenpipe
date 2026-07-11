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

Confirmed at source level 2026-07-11 (details + citations in
`IMAGEFLOW-PARITY.md` §4–§6; all pre-0.1 blockers, workstreams W1/W7/W8):

- **`mode=max` / `mode=stretch` / `mode=none` rejected** by
  `parse_constraint_mode` (`src/bridge/parse.rs:56-75`) while the Constrain
  node doc (`src/zennode_defs.rs:521-529`) and `docs/querystring.md` promise
  them. The two most common legacy mode values error out on the native path.
- **Constrain default mode is `within`; imageflow RIAPI defaults to `pad`**
  when w+h are given (`imageflow_riapi/src/ir4/layout.rs:164-168`). Same URL,
  different canvas.
- **RIAPI `crop` units default to percent (100)** in the adapter
  (`src/zennode_defs.rs:1493,1499`); imageflow defaults to source-pixel units,
  supports negative (bottom-right-relative) coords, inverted-rect reset, and
  lenient parsing. Same URL, wildly different crop.
- **`srotate` kv is bound to the Orient node's EXIF-flag (1–8) param**
  (`src/zennode_defs.rs:153-157`); RIAPI `srotate` means degrees. `srotate=90`
  is an out-of-range orientation, not a rotation.
- **`s.roundcorners` is a pixel radius** (`zenpipe.round_corners`); imageflow
  treats it as percentage (`ir4/layout.rs:536-553`).
- **`sflip` is consumed as a synonym of post-`flip`**
  (`src/zennode_defs.rs:1133-1135`) — source-flip (pre-crop) placement lost;
  when both keys are present only one is consumed.
- **`f.sharpen_when` not accepted** — the kv is `sharpen_when`
  (`src/zennode_defs.rs:709`).
- **`expand_srcset` is unwired** (`src/srcset.rs:206` — no non-test caller):
  `srcset=`/`short=` URLs silently warn as unrecognized on the native path.
- **`ImageJob` limits under-enforced**: `Limits` gate only pre-decode checks
  (`src/job.rs:652-658`), are not threaded into `orchestrate::stream`
  (`ProcessConfig` has no limits field, `src/orchestrate.rs:63`);
  `AllocationTracker` has no call sites; no output-byte cap exists.
- **Animated input + processing steps → still image**: compat's animation
  passthrough correctly fires only for no-op jobs (`execute.rs:369`), but jobs
  *with* steps fall through to single-frame processing; `animation::transcode`
  (per-frame pipelines) is called only from tests. "Resize this GIF" loses the
  animation.
- **JPEG alpha flatten ignores the configured matte color** — hardcodes white
  (`src/job.rs:1012` TODO).
- **JPEG/WebP decoder downscale hints dropped**: `jpeg_downscale_hints` /
  `webp_decoder_hints` / `decoder.min_precise_scaling_ratio` have zero
  handling in `src/imageflow_compat/` (grep-verified); zenjpeg exposes no
  public scaled-IDCT knob yet.

Stale-doc queue (do not fix piecemeal; batch per repo rules):
`IMAGEFLOW-PARITY.md` §9.
