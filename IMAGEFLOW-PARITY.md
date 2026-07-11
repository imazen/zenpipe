# imageflow parity & gap analysis

Status: **verified survey, 2026-07-11.** What imageflow implements (RIAPI
querystrings + v1 JSON API), what zenpipe covers today, where semantics diverge,
and the workstreams to full coverage. Every claim below was verified against
source at these revisions:

- imageflow @ `0ba1c9ea` — `imageflow_riapi/src/ir4/{parsing,layout,encoder,srcset}.rs`,
  `imageflow_types/src/lib.rs`, `imageflow_core/src/json/**`
- zenpipe @ `da8d8da` — `src/zennode_defs.rs`, `src/imageflow_compat/**`,
  `src/bridge/**`, `src/job.rs`, `src/limits.rs`, `zencodecs/**`

Companion doc: [`JSON-JOB-SPEC.md`](JSON-JOB-SPEC.md) — the zenpipe-native JSON
envelope this analysis motivates.

**Status legend** used throughout:

- ✅ **native** — handled by the zen-native registry path (`#[kv]` params or
  `from_kv` adapters via `full_registry().from_querystring`; the path the CLI
  `--qs` and `expand_zen` use)
- 🔁 **compat-only** — correct behavior only through the legacy engine
  (`imageflow-compat` feature → `imageflow_riapi::Ir4Expand`)
- ⚠️ **divergent** — present on the native path but with different semantics
  than imageflow (wrong pixels or wrong acceptance for the same URL)
- ❌ **missing** — no native handling (unrecognized-key warning)
- ➕ **exceeds** — zenpipe capability imageflow does not have

---

## 1. Executive summary

- **Two engines exist and both work, but they are not the same engine.** The
  legacy engine (`imageflow_riapi` crate driven from
  `src/imageflow_compat/`) is full-fidelity IR4. The zen-native registry path
  covers most keys but has **6 confirmed semantic divergences** (§4) — three of
  which produce *different pixels* for common legacy URLs (`mode=max`,
  `crop=...` without units, `srotate=90`).
- **imageflow v1 JSON**: all 28 public node variants translate
  (`translate.rs`), all 10 encoder presets map (`preset_map.rs`), watermark
  translation is structurally complete (491-line module with fit_box/fit_mode/
  min_canvas/opacity). The big holes: **no envelope acceptance** (`Build001`/
  `Execute001`/`io`/response JSON — callers must pre-parse into Rust types),
  **graph jobs with multi-input nodes are rejected** (no canvas-edge
  compositing through the graph decomposer), and **no
  `get_image_info`/`estimate`/`tell_decoder`/schema endpoints**.
- **Four RIAPI parser implementations exist; two are dead code**
  (`zenlayout::riapi`, `zencodecs::riapi_parse`) and one is implemented but
  unwired (`src/srcset.rs` — `expand_srcset` has no non-test caller). This
  fracture, not missing functionality, is the main source of divergence risk.
- **No zen-native JSON job envelope exists in the library.** `ImageJob` has no
  serde; the only JSON job today is the CLI-side `JobDef`
  (`zenpipe-cmd/src/job_json.rs`) which keys nodes by full id and takes output
  format from the file extension. [`JSON-JOB-SPEC.md`](JSON-JOB-SPEC.md) is the
  design that replaces it.
- **Limits are under-wired on the native path**: `ImageJob`'s `Limits` gate
  only pre-decode checks (`job.rs:652-658`); they are not threaded into
  `orchestrate::stream` (`ProcessConfig` has no limits field,
  `orchestrate.rs:63`), `AllocationTracker` has no call sites, and there is no
  output-byte cap. The compat path enforces imageflow's `ExecutionSecurity`
  size limits but not its newer fields (`max_input_file_bytes`,
  `max_json_bytes`, `max_total_file_pixels`, `max_threads`,
  `mem_budget_policy`).

## 2. Architecture: who parses what today

| Parser | Location | Wired into | Notes |
|---|---|---|---|
| zennode registry (`#[kv]` + `from_kv` adapters) | node defs in `src/zennode_defs.rs`, `zencodecs/src/zennode_defs.rs`, `zenfilters/src/zennode_defs.rs`; entry `NodeRegistry::from_querystring` | CLI `--qs` (`zenpipe-cmd/src/convert.rs:144-152`), `expand_zen` (`src/imageflow_compat/riapi.rs:132`) | **The zen-native path.** 98 registered nodes (21 zenpipe + 16 zencodecs + 61 zenfilters, `src/node_registry.rs:50`), incl. 5 RIAPI adapter defs (`zennode_defs.rs:1563-1568`) |
| `imageflow_riapi::ir4` (external crate) | `Ir4Expand`/`Ir4Translate` driven from `src/imageflow_compat/{execute,riapi}.rs` | `execute_framewise` CommandString path (`execute.rs:1291`), `expand_legacy` (`riapi.rs:54`) | Full IR4: 100-key recognizer, 68 supported keys, layout engine, warnings |
| `zenlayout::riapi` (`Instructions`) | `zenlayout/src/riapi/` | **nothing** (tests only) | Doc comments in `riapi.rs:9,112` still claim it is used — they are wrong |
| `zencodecs::riapi_parse` | `zencodecs/src/riapi_parse.rs` | **nothing** (tests only) | Exported at `lib.rs:213`, never called |
| `src/srcset.rs` (`expand_srcset`) | zenpipe | **nothing** (tests only) | The `srcset=`/`short=` micro-syntax expander exists but no caller wires it in |

imageflow side, for reference: one parser (`ir4/parsing.rs`, keys pre-lowercased,
100-entry `IR4_KEYS` recognizer at `parsing.rs:242-258`), one layout engine
(`ir4/layout.rs` — emission order at `add_steps`, `:473-647`), one preset
selector (`ir4/encoder.rs`). Nothing in the parser hard-errors; invalid values
warn (`ValueInvalid`) and drop.

## 3. RIAPI querystring parity matrix

imageflow semantics cited from `imageflow_riapi/src/ir4/parsing.rs` (parse) and
`ir4/layout.rs` (behavior); zenpipe citations per row.

### 3.1 Sizing & geometry

| Key(s) | imageflow behavior | zenpipe status |
|---|---|---|
| `width`/`w`, `height`/`h` | i32 targets (`parsing.rs:489-490`) | ✅ Constrain `#[kv("w","width","maxwidth")]` etc. (`zennode_defs.rs:507,515`) |
| `maxwidth`, `maxheight` | reconciled against w/h: both given → smaller wins; cross terms bound by aspect (`layout.rs:63-91`) | ⚠️ plain aliases of `w`/`h` — no smaller-wins reconciliation when both are present |
| `mode` | `none/max/pad/crop/carve/stretch/aspectcrop` (`parsing.rs:90-105`); default **Pad** (Max when neither w nor h given) (`layout.rs:162-168`); `carve` warns → Stretch | ⚠️ **two bugs**: (a) bridge accepts `distort/fit/within/fit_crop\|crop/within_crop/fit_pad\|pad/within_pad/pad_within/aspect_crop` only (`bridge/parse.rs:56-75`) — **`max`, `stretch`, `aspectcrop`, `none` are documented on the node (`zennode_defs.rs:521-529`) but rejected**; (b) default mode is `within` (`zennode_defs.rs:531`) vs imageflow's **Pad** — `?w=800&h=600` letterboxes in imageflow, plain-fits in zenpipe |
| `scale` | `down`(default)/`up`/`both`/`canvas`; full 4×4 mode×scale matrix (`layout.rs:170-280`) | ✅ param exists (`zennode_defs.rs:546`); composed behavior vs the 4×4 matrix needs the W9 behavioral suite before claiming equivalence |
| `zoom`/`dpr`/`dppx` | f32, trailing `x` ok, clamp 0.00008..80000 (`parsing.rs:875-880`, `layout.rs:112-116`) | ⚠️ Constrain kv (`zennode_defs.rs:555`) but param range 0.1..=10 (`:553`) — narrower clamp; also doubles as `qp.dpr` input on quality_intent (`zencodecs/zennode_defs.rs:647`) |
| `stretch=fill`, `crop=auto` | legacy mode shortcuts (`parsing.rs:511-524`) | ❌ native · 🔁 compat |
| `anchor` | 9 names **or** `x,y` percentages (`parsing.rs:1079-1102`) | ⚠️ 9 names only (`bridge/parse.rs:77-90`); `x,y` form unhandled (gravity_x/gravity_y params exist but have no kv, `zennode_defs.rs:579,587`) |
| `crop`, `c`, `cropxunits`, `cropyunits` | units default = **source pixel dimensions** when unset/0 (`layout.rs:736-745`); negative coords are bottom-right-relative; inverted rect resets to full image; lenient paren-stripping parse (`parsing.rs:793-809`) | ⚠️ adapter (`zennode_defs.rs:1411-1518`) consumes all four keys but **units default to 100 (percent)** (`:1493,1499`), no negative-coordinate handling, no inverted-rect reset, strict parse only. `?crop=10,10,300,300` means pixels in imageflow, percent×3 in zenpipe |
| `c.gravity` | 2 floats 0..100 crop gravity (`parsing.rs:898-908`) | ✅ special-cased in `expand_zen` (`imageflow_compat/riapi.rs:136-139`) |
| `c.focus`, `c.zoom`, `c.finalmode` | — (not in imageflow) | ➕ smart-crop surface (`riapi.rs:479,548,565`; README §smart crop) |

### 3.2 Rotate / flip / orientation

| Key(s) | imageflow | zenpipe |
|---|---|---|
| `rotate` | degrees snapped to nearest 90 (warn on non-multiples) post-resize (`parsing.rs:962-974`, `layout.rs:638`) | ✅ adapter, same 90-snap (`zennode_defs.rs:1206-1230`) |
| `srotate` | **degrees**, pre-crop source rotate (`parsing.rs:499`) | ⚠️ **bug** — kv is bound to the Orient node's **EXIF flag 1–8** param (`zennode_defs.rs:153-157`): `srotate=90` is an out-of-range orientation, not a 90° rotation |
| `flip` | post flip `none/h/x/v/y/both/xy` (`parsing.rs:497`) | ✅ adapter (`zennode_defs.rs:1128-1150`); `both` → Rotate180 (pixel-equivalent) |
| `sflip` | **source** flip, applies **pre-crop** (`layout.rs:479`) | ⚠️ adapter consumes `sflip` as a synonym of `flip` (`zennode_defs.rs:1133-1135`): source-vs-post distinction lost; when both keys are present only the first is consumed |
| `autorotate` | honor EXIF (`parsing.rs:501`) | ✅ adapter → Orient sentinel `orientation=0` = "use EXIF at decode" (`zennode_defs.rs:1289-1297`) |

### 3.3 Decoder & color management

| Key(s) | imageflow | zenpipe |
|---|---|---|
| `ignoreicc` | → `DecoderCommand::DiscardColorProfile` (`ir4/mod.rs:180-182`) | ❌ native · 🔁 compat |
| `ignore_icc_errors` | → `IgnoreColorProfileErrors` (`mod.rs:183-185`) | ❌ native · 🔁 compat |
| `down.colorspace`, `up.colorspace` | independent down/up working spaces `srgb/linear/gamma` (`parsing.rs:531-532`) | ⚠️ both keys alias **one** param (`zennode_defs.rs:643`) — cannot set down≠up |
| `decoder.min_precise_scaling_ratio` | IDCT/preshrink threshold, default 2.1 (`mod.rs:164`) | ❌ native (generic `decode` node's `min_size` param has no kv) · 🔁 compat parses it, but see §6 — the hints are dropped downstream |
| `frame`/`page` | `SelectFrame` (`mod.rs:201-203`) | ✅ adapter (`zennode_defs.rs:1391-1398`) |
| `jpeg.strictness`, `jpeg.orient`, `jpeg.max_megapixels`, `webp.upsampling`, `webp.dithering`, `jxl.orient`, `jxl.nits`, `heic.*` | — | ➕ per-codec decode knobs (querystring.md) |

### 3.4 Resize quality & sharpening

| Key(s) | imageflow | zenpipe |
|---|---|---|
| `f.sharpen` | sharpen percent (`parsing.rs:578`) | ✅ `#[kv("f.sharpen","unsharp")]` (`zennode_defs.rs:656`) |
| `f.sharpen_when` | `downscaling/sizediffers/always` (`parsing.rs:579`) | ⚠️ kv is **`sharpen_when`** (`zennode_defs.rs:709`) — the imageflow spelling `f.sharpen_when` is not accepted; value set also differs (adds `upscaling`, uses `size_differs`) |
| `down.filter`, `up.filter` | 33-name filter enum (`parsing.rs:159-193`) | ✅ separate params (`zennode_defs.rs:624,632`); name overlap near-complete (zenpipe adds raw_lanczos/fast variants, drops none) |
| — | — | ➕ `lobe_ratio`/`kernel_lobe_ratio`, `resample_when`, kernel-shape controls (querystring.md:208-210) |

### 3.5 Whitespace trim, corners, effects

| Key(s) | imageflow | zenpipe |
|---|---|---|
| `trim.threshold`, `trim.percentpadding` | CropWhitespace (`parsing.rs:536-537`) | ✅ (`zennode_defs.rs:803-818`) |
| `s.roundcorners` | 1 or 4 values, **percentage** semantics → `RoundCornersMode::Percentage(Custom)` (`layout.rs:536-553`) | ⚠️ 1 or 4 values but **pixel radius** by default (querystring.md:172, node `zenpipe.round_corners`) — `s.roundcorners=50` = half-circle in imageflow, 50 px in zenpipe |
| `s.grayscale` | `ntsc/true/y/ry/flat/bt709` → sRGB-weight grayscale (`layout.rs:577-586`) | ⚠️ values `oklab/ntsc/bt709/flat/ry` (querystring.md:75) — `true`/`y` aliases missing; native default is Oklab-space (different luma than sRGB NTSC). Compat path preserves byte parity via `imageflow.color_matrix_srgb` (`translate.rs:617-748`) |
| `s.sepia` | **bool** (`parsing.rs:562`) | ⚠️ number 0–1 strength (querystring.md:93) — `s.sepia=true` doesn't parse natively |
| `s.alpha`, `s.contrast`, `s.saturation` | f32 → sRGB ColorFilter (`parsing.rs:558-560`) | ✅ kv on zenfilters nodes; semantics are Oklab-space natively (compat uses sRGB matrices for parity) |
| `s.brightness` | f32 -1..1 sRGB offset | ⚠️ kv bound to Exposure (photographic stops) — scale mismatch documented in querystring.md:69 itself |
| `s.invert` | **ignored** (in `IR4_KEYS`, never consumed) | ➕ native (`zenfilters` invert, querystring.md:81) |
| `a.balancewhite` | `WhiteBalanceHistogramAreaThresholdSrgb` (`parsing.rs:563-575`) | ❌ native kv (the node `imageflow.white_balance_srgb` exists but only in the compat registry, `translate.rs:756`) · 🔁 compat |
| `bgcolor` | hex 3/4/6/8 + ~148 CSS named colors (`imageflow_helpers/src/colors.rs:65-75,199-354`); default white for jpeg output (`layout.rs:493-503`) | ⚠️ Constrain kv accepts `transparent`/`white`/`black`/hex only (`bridge/parse.rs:92-135`) — named-color table missing; jpeg-default-white behavior unverified on native path |

### 3.6 Format & quality selection

| Key(s) | imageflow | zenpipe |
|---|---|---|
| `format`/`thumbnail` | jpeg aliases ×7, png, gif, webp, avif, jxl/jpegxl, auto, keep (`parsing.rs:50-66`) | ✅ quality_intent kv (`zencodecs/zennode_defs.rs:638`); jpeg alias list shorter (jpg/jpeg) |
| `quality` | legacy jpeg + webp fallback; drives Auto profile → **High** when `qp` absent and quality absent (`encoder.rs:44-54`) | ⚠️ quality_intent default profile is `high` (`zencodecs/zennode_defs.rs:612-681`) but `CodecIntent::effective_quality()` defaults to **Good/73** when neither is set (`zencodecs/src/intent.rs:142-155`) — same-URL default-quality difference needs W9 confirmation |
| `qp`, `qp.dpr`/`qp.dppx` | named profiles + 0-100; dpr adjust (`parsing.rs:625-626`) | ✅ (`zencodecs/zennode_defs.rs:612-681`) |
| `lossless` | BoolKeep true/false/keep (`parsing.rs:618`) | ✅ quality_intent `lossless` |
| `accept.webp/avif/jxl/color_profiles` | AllowedFormats gating; base = web_safe (`encoder.rs:5-29`) | ✅ `allow_*` kv (`zencodecs/zennode_defs.rs`) |
| `jpeg.quality/progressive` | EncoderHints (`parsing.rs:589,593`) | ✅ EncodeJpeg kv |
| `jpeg.turbo`, `jpeg.li` | encoder mimic selection (`encoder.rs:75-83`) | ❌ native kv (zenpipe expresses this via `jpeg.tables`/`jpeg.effort`/mozjpeg node instead) · 🔁 compat |
| `webp.quality` | ✅ | ✅ |
| `webp.lossless` | BoolKeep (`parsing.rs:597`) | ⚠️ no first-class kv — lossless choice rides the global `lossless` key / node selection (EncodeWebpLossless kv are `webp.effort/method/near_lossless/...`) |
| `png.quality/min_quality/lossless/max_deflate` | PngEncoderHints | ✅ EncodePng kv |
| `png.quantization_speed`, `png.libpng` | quantizer speed; libpng mimic (`parsing.rs:602-603`) | ❌ native (zenquant `quant.quality` tiers replace speed; no libpng mimic) · 🔁 compat |
| `avif.quality/speed` | preset passthrough (`parsing.rs:605-606`) | ✅ + ➕ `avif.effort/alpha_quality/depth/color_model/alpha_color_mode/lossless` |
| `jxl.quality/effort/distance/lossless` | preset passthrough (`parsing.rs:607-610`) | ✅ + ➕ `jxl.noise` |
| `subsampling` | parsed-and-ignored, deprecated (`parsing.rs:1311-1313`) | ≈ parity: bare key unrecognized natively; `jpeg.subsampling`/`jpeg.ss` is the real knob ➕ |
| `srcset`/`short` | full micro-syntax (`srcset.rs`, forces mode=max base) | ⚠️ implementation exists **unwired** (`src/srcset.rs:206` — no non-test caller) |
| `watermark_red_dot` | test marker node | 🔁 compat (`translate.rs` RedDotNode) |
| `mozjpeg.*`, `gif.*`, `tiff.*`, `bmp.bits`, `quant.*` | — | ➕ zenpipe-only encode surfaces |

### 3.7 Keys imageflow recognizes but ignores (22)

`cache, process, colors, 404, paddingcolor, bordercolor, preset, floatspace,
jpeg_idct_downscale_linear, watermark, s.invert, a.blur, a.sharpen,
a.removenoise, dither, encoder, decoder, builder, paddingwidth, paddingheight,
margin, borderwidth` — each in `IR4_KEYS` but never consumed → `IgnoredKey`
warning (`parsing.rs:317-319`, list verified against `delete_from_map`
`:481-635`).

zenpipe policy: mostly unrecognized-key warnings (parity in effect), with two
deliberate upgrades — ➕ `paddingwidth/paddingheight/margin/borderwidth(+colors)`
are *revived* via `parse_expand_shorthand` → ExpandCanvas
(`imageflow_compat/riapi.rs:273-330`), and ➕ `s.invert` works. Recommended: keep
the imageflow ignore-list as an explicit warn-list in the native path so these
keys warn `ignored` rather than `unrecognized` (tiny UX difference, big
migration-debugging difference).

## 4. Confirmed semantic divergences (native path) — fix before 0.1

> **STATUS 2026-07-11:** items 1–6 below (plus the crop no-op from §1) were
> **fixed** in the 2026-07-11 wave (`f378fe02` geometry core, `44020afa`
> remaining keys, `9cb07998` limits, `f7d1900d` hdr), with regression tests in
> `tests/riapi_keys.rs`, `src/bridge/geometry.rs`, and `src/riapi.rs`. The §3
> matrix retains the pre-fix status cells as the historical survey; consult
> `CLAUDE.md` Known Bugs for what remains open (animation W8, decode hints W6,
> matte, AllocationTracker, encode-config audit W10, and the documented
> mode×scale approximations). The W9 two-engine behavioral suite is still the
> missing verification layer.

Ranked by blast radius; all verified at source level, each needs a regression
test in the W9 suite. These are wrong-pixels or wrong-acceptance bugs under this
repo's zero-tolerance rule, not cosmetics.

1. **`mode=max` / `mode=stretch` rejected.** Doc comment promises them
   (`zennode_defs.rs:521-529`); `parse_constraint_mode` doesn't accept them
   (`bridge/parse.rs:56-75`). The two most common legacy mode values error out.
   Also missing: `none`, `aspectcrop` spelling, `carve`→stretch fallback.
2. **Default mode `within` vs imageflow `pad`** (`zennode_defs.rs:531` vs
   `layout.rs:164-168`). Bare `?w=&h=` URLs produce different canvases.
3. **`crop` units default percent vs pixels** (`zennode_defs.rs:1493,1499` vs
   `layout.rs:736-745`) + missing negative-coord / inverted-rect / lenient
   parse behaviors.
4. **`srotate` parsed as EXIF flag 1–8, not degrees** (`zennode_defs.rs:153-157`).
5. **`s.roundcorners` pixels vs percent** (querystring.md:172 vs
   `layout.rs:536-553`).
6. **`sflip` loses source-flip placement; `f.sharpen_when` spelling not
   accepted** (`zennode_defs.rs:1133-1135`; `:709`).

Secondary (acceptance/quality, not geometry): named bgcolors missing, `anchor=x,y`
missing, `s.sepia=true` unparsed, `s.grayscale=true|y` unparsed, single
scaling-colorspace param, zoom clamp 0.1..10, default-quality Good-vs-High
check, `webp.lossless` kv.

## 5. imageflow v1 JSON node parity

`s::Node` has 28 JSON-visible variants (`imageflow_types/src/lib.rs:1276-1355`;
`CaptureBitmapKey` is `#[serde(skip)]`). Translation status
(`src/imageflow_compat/translate.rs:92-500`):

| s::Node (JSON tag) | zenpipe translation | Status |
|---|---|---|
| `decode` / `encode` | io config + preset map | ✅ (all 10 presets, `preset_map.rs:28-283`) |
| `flip_v`/`flip_h`/`rotate_90/180/270`/`transpose` | zenlayout orient nodes (transpose = rotate90+flip_h) | ✅ |
| `apply_orientation` | `zenlayout.orient` | ✅ |
| `crop`/`region`/`region_percent`/`expand_canvas` | zenlayout equivalents | ✅ |
| `crop_whitespace` | `zenpipe.crop_whitespace` | ✅ |
| `constrain` (9 modes, gravity, hints, canvas_color) | `zenresize.constrain` (+remove_alpha for opaque matte) | ✅ except `larger_than` mode → bridge error (`bridge/parse.rs:67-69`) |
| `resample_2d` | `zenresize.resize` | ✅ (1:1 no-ops elided, `execute.rs:238,1158`) |
| `round_image_corners` | `zenpipe.round_corners` | ✅ |
| `fill_rect`/`create_canvas` | fill_rect / canvas source | ✅ (canvas capped 120 MP, `execute.rs:1071`) |
| `copy_rect_to_canvas` | crop + composite | ✅ |
| `draw_image_exact` | `zenpipe.composite` | ⚠️ its `w`/`h` are ignored — no foreground resize (`translate.rs:348`) |
| `watermark` | `zenpipe.overlay` via `watermark.rs` (fit_box ×4, fit_mode, gravity, min_canvas_w/h, opacity, resize) | ✅ structurally; pixel-parity untested (W9) |
| `watermark_red_dot` | RedDotNode | ✅ |
| `color_filter_srgb` (10 sub-ops) / `color_matrix_srgb` | `imageflow.color_matrix_srgb` (5×5 sRGB matrices) | ✅ byte-parity-oriented |
| `white_balance_histogram_area_threshold_srgb` | `imageflow.white_balance_srgb` | ✅ (compat registry only) |
| `command_string` (ir4) | expanded via `Ir4Expand` then translated | ✅ (+ trim retry logic, `execute.rs:1331-1340`) |
| any future variant | catch-all `Err(Unsupported)` (`translate.rs:498`) | ✅ fails loud |

**Framewise handling** (`execute.rs`): `steps` ✅ linear; `graph` ⚠️ — DAGs are
decomposed into per-encode linear branches by edge back-tracing (`:516`), and
**any node with >1 input edge is rejected** ("no multi-input compositing",
`:575-581`). imageflow graph jobs that composite via `canvas` edges
(`copy_rect_to_canvas`, `draw_image_exact` fan-in) do not run. Cycles detected
(`:557`).

**Animation:** compat's `encode_animation_passthrough` (`execute.rs:698`) fires
only for **no-op jobs** — `has_encode && pipeline.nodes.is_empty() &&
!has_select_frame` (`execute.rs:369`) — so processing steps are never silently
skipped. The gap is the other branch: animated input **with** steps falls
through to the standard path and processes a single frame, i.e. "resize this
GIF" yields a still. The primitive that does per-frame processing
(`animation::transcode`, `src/animation.rs:406`) is called only from tests;
`ImageJob` likewise processes only the first frame (`job.rs` `decode_source`).
imageflow itself re-encodes GIF animations; zenpipe must route animated inputs
through per-frame pipelines (or refuse loudly) before parity is claimable.

## 6. Envelopes, endpoints, decoder commands, security

| imageflow surface | Definition | zenpipe status |
|---|---|---|
| `Build001` (io array + framewise + security), `Execute001` | `imageflow_types/src/lib.rs:1577-1581,1652-1658` | ❌ — `execute_framewise(&Framewise, io_buffers, &ExecutionSecurity, &JobOptions)` takes pre-parsed Rust types (`execute.rs:177`); no envelope JSON, no `IoEnum` (bytes_hex/base_64/byte_array/file/output_buffer/placeholder), no `JsonAnswer{code,success,message,data}` response |
| `v1/get_image_info`, `get_scaled_image_info` | `v1.rs:36-45` | ⚠️ `zen_get_image_info` exists (`imageflow_compat/mod.rs:27`) but no envelope/response shape |
| `v1/tell_decoder` | `v1.rs:51-55` | ❌ |
| `v1/estimate` (+`MemBudgetPolicy`) | `v1.rs:46-50` | ❌ as endpoint; ➕ zenpipe's `graph.estimate()` is the richer native equivalent (streaming vs materialization split) |
| `v1/schema/riapi/*`, `v1/schema/openapi/*`, OpenAPI hash guard | `v1.rs:65-104,571-624` | ⚠️ exporters exist (`src/schema_export.rs`) with no endpoint/bundle; no snapshot guard test |
| `brew_coffee` → 418 | `v1.rs:99` | ❌ (carry it — it's free and it's tradition) |
| Error taxonomy: 22 `ErrorCategory` values with HTTP mapping | `imageflow_core/src/errors.rs:779-901` | ❌ no `PipeError` → category/HTTP mapping |
| `DecoderCommand::jpeg_downscale_hints` (IDCT downscale + spatial-luma options) | `types:1873-1878,1891-1920` | ❌ **dropped** — zero references in `src/imageflow_compat/` (grep verified); zenjpeg exposes no public scaled-IDCT knob (only MCU-aligned `with_crop_hint`); the generic decode node's `min_size` hint is the intended replacement, unwired |
| `webp_decoder_hints` | `types:1883-1886` | ❌ same |
| `discard_color_profile`, `ignore_color_profile_errors` | `types:1896-1899` | 🔁 honored in compat CMS path (`ir4/mod.rs:180-185` → `cms.rs`); no native kv/JSON |
| `select_frame` | `types:1918-1919` | ✅ frame adapter (native) + compat |
| `ExecutionSecurity` max_decode/frame/encode_size | `types:1195-1223` | ✅ compat enforces at decode/canvas/encode (`execute.rs:187,270,434,1074`) |
| `ExecutionSecurity` max_input_file_bytes / max_json_bytes / max_total_file_pixels / max_threads / mem_budget_policy | `types:1204-1222` | ❌ not enforced in compat; native `Limits` has kindred fields (`max_total_pixels`, `max_frames`) but is itself under-wired (§1) |

## 7. Codec-level state relevant to parity (zencodecs @ this rev)

- Wired: JPEG, PNG, WebP, GIF, AVIF (enc+dec), JXL (dec default, enc
  feature-gated), TIFF, HEIC (dec), RAW/DNG (dec), PDF (dec, page 0), BMP/PNM/
  Farbfeld. Stubbed with `compile_error!`: QOI, TGA, HDR-radiance, SVG, JP2
  (`zencodecs/src/lib.rs:123-140`).
- Animation encode dispatch: GIF, WebP, APNG only
  (`zencodecs/src/dyn_dispatch.rs:476-500`); AVIF animation types exist in
  zenavif but aren't dispatched.
- `CodecConfig` native-config boxes exist for jpeg/webp/gif/png/avif(3 scalars)/
  raw — **no JXL/TIFF/HEIC boxes**, and the TIFF adapter threads no knobs
  (`zencodecs/src/config.rs:116-169`, `codecs/tiff.rs`).
- HDR `reconstruct_hdr` honored end-to-end only for JPEG today.
- Metric-targeted encoding exists beneath the intent layer: zencodec
  `Fidelity::{ssim2,butteraugli,codec_quality}` (`zencodec/src/fidelity.rs`),
  zenjpeg `Quality::Zq` closed-loop, zenwebp `target_zensim`/`target_size`/
  `target_psnr` — the JSON `target` param in json-job-spec.md §6 has real
  machinery to land on.

## 8. Workstreams to full imageflow coverage

Ordered for 0.1; sizes are S (≤1 day), M (2-4 days), L (≥1 week). Every ⚠️/❌
above maps to exactly one workstream.

- **W1 — Native RIAPI closure (M).** Fix §4 items 1-6; add missing kv:
  `ignoreicc`, `ignore_icc_errors`, `a.balancewhite` (move white_balance node
  into `full_registry`), `jpeg.turbo`/`jpeg.li` (as mimic aliases),
  `webp.lossless`, `png.quantization_speed`; accept `anchor=x,y`, named
  bgcolors (port the CSS table), `stretch=fill`/`crop=auto` shortcuts; wire
  `expand_srcset` into `from_querystring` preprocessing; adopt the imageflow
  ignore-list as a warn-list. Acceptance: the W9 querystring corpus passes both
  engines with identical geometry.
- **W2 — Delete or wire the dead parsers (S).** `zenlayout::riapi` and
  `zencodecs::riapi_parse` either become the implementation under W1 or get
  removed; fix the `expand_zen` doc comment (`riapi.rs:9,112`) that
  misdescribes its own code. Divergence risk lives in this duplication.
- **W3 — JSON envelope v1 (M).** Implement
  [`JSON-JOB-SPEC.md`](JSON-JOB-SPEC.md): `Job::from_json`, checkpoint/resume,
  slot resolution, response/warning/error types with HTTP mapping, capabilities
  bundle, naming audit (json_keys are currently all-empty → full ids). Replaces
  CLI `JobDef`.
- **W4 — imageflow envelope acceptance (M).** `Build001`/`Execute001`/io-objects
  → translate onto W3 internals; `JsonAnswer` responses; `get_image_info`/
  `tell_decoder`/`estimate` equivalents; `ErrorCategory` mapping; OpenAPI
  snapshot guard. Fixtures: imageflow's own doc examples
  (`docs/src/json/*.md`) and `Framewise::example_*`.
- **W5 — Graph compositing in compat (M).** Support multi-input nodes in the
  graph decomposer (canvas edges, `copy_rect_to_canvas`, `draw_image_exact`
  with fg resize) — zenpipe's own graph executor already handles fan-in
  (`tests/fanout.rs`, composite nodes); the gap is only in the imageflow-graph
  → steps decomposition.
- **W6 — Decode hints (M, cross-crate).** Public scaled-decode knob in zenjpeg
  (or wire `min_size` → internal DCT scaling), WebP size hints, honor
  `jpeg_downscale_hints`/`webp_decoder_hints`/`decoder.min_precise_scaling_ratio`
  in both engines.
- **W7 — Limits unification (M).** Thread `Limits` through
  `orchestrate::stream`; wire or delete `AllocationTracker`; add output-byte
  caps; enforce/translate the five newer `ExecutionSecurity` fields; single
  security model documented in json-job-spec.md §9.
- **W8 — Animation correctness (M).** Route ImageJob/compat through
  `animation::transcode` with per-frame pipelines; make geometry+animated-input
  either work or fail loud (never silent passthrough); AVIF animation dispatch
  decision.
- **W9 — Parity test harness (M, enables everything).** Golden corpus of
  querystrings + v1 JSON jobs run through imageflow and zenpipe; assert
  geometry/canvas/format identically and pixels within threshold; wire into CI.
  Without this, every ⚠️ above stays "believed fixed."
- **W10 — Codec option wiring (M).** JXL/TIFF/HEIC/AVIF-full `CodecConfig`
  boxes; TIFF adapter knobs; stub-format decisions (QOI/TGA/HDR at minimum —
  native configs already exist in zenbitmaps).

## 9. Doc corrections queued (do not fix silently — batch per repo rules)

- `docs/formats.md` — AVIF/JXL encode marked "—" but both encode nodes exist.
- README node counts (6/16/43) vs actual registry (21/16/61 = 98).
- `imageflow_compat/riapi.rs:9,112` doc comments claim zenlayout::riapi is used.
- `IMAGEFLOW-MAGIC-SPEC.md` and CLI-SPEC.md `magic` subcommand — design-only,
  unbuilt (mark status headers).
- `WASM-AUDIT.md` — self-declared partially stale.
- querystring.md omits the five riapi adapter keys (crop/flip/rotate/
  autorotate/frame) because the generator only walks `#[kv]` params, not
  `from_kv` adapters — generator fix.
