# zenpipe JSON Job Spec (envelope v1) — design

Status: **design, pre-0.1**. This document defines the long-term JSON surface for
zenpipe jobs: the envelope, the step syntax, the encode-intent model, and the
compatibility/lifecycle policy that lets the format absorb feature additions and
removals for a decade without breaking clients. It builds on machinery that already
exists and is tested: the zennode registry (one-key node JSON, schemas, versioning
rules), `zencodecs::CodecIntent`/`QualityProfile`, and the schema exporters in
`src/schema_export.rs`.

Related docs: [`IMAGEFLOW-PARITY.md`](IMAGEFLOW-PARITY.md) (gap analysis this spec
must be able to cover), [`docs/querystring.md`](docs/querystring.md) (the RIAPI surface,
generated from the same node schemas), `ORDERING-DESIGN.md` (ordering and
fusion semantics).

---

## 1. Design principles

1. **One vocabulary, three syntaxes.** RIAPI querystrings, JSON jobs, and the Rust
   builder all resolve to the same zennode instances. A JSON step's params are the
   node's schema params; a querystring key is an alias for one of them
   (`ParamDesc.kv_keys`). Nothing is expressible in one syntax that lacks a name in
   the others (terse kv aliases aside).
2. **The registry is the schema.** Node ids, param names, JSON names/aliases,
   ranges, defaults, versions all live in `NodeSchema`/`ParamDesc`
   (zennode). JSON Schema 2020-12 + OpenAPI 3.1 are *exported* from the registry
   (`src/schema_export.rs`), never hand-maintained.
3. **Readable first.** JSON names are full snake_case words (`width`, not `w`).
   Terse forms stay in the querystring layer. Enums serialize as snake_case
   strings, never integers.
4. **Steps are a list the user controls.** Declaration order is execution order;
   the engine fuses adjacent compatible nodes but never reorders unless the job
   opts into `optimize` (see §6 and ORDERING-DESIGN.md).
5. **Additive forever, breaking never (within a major).** The zennode versioning
   rules (SPEC.md "Versioning Rules — NEVER BREAK") extend to the envelope:
   node ids/json keys are permanent, params are additive with `since`, enum
   variants and kv keys are never removed, defaults are frozen.
6. **Removals are loud, never silent.** A removed capability keeps its name
   reserved forever and produces a structured `removed_feature` error naming the
   replacement. Silently skipping a step the server no longer understands would
   change pixels; that is never acceptable.
7. **Strict JSON, tolerant querystrings.** Unknown node params in JSON are
   errors (typos in programmatic input are bugs). Unknown querystring keys are
   warnings (URLs carry cache-busters and third-party junk; RIAPI tradition).
   Both surfaces report through the same warnings channel.
8. **Intent over mechanism.** The primary encode step expresses *intent*
   (format choice/auto, quality profile, lossless preference, allow-list,
   per-codec hints) and lets the engine resolve mechanism. Explicit per-codec
   encoder nodes remain available for exact control.
9. **Capability discovery over version sniffing.** Clients ask the engine what it
   supports (exported schemas + codec list) instead of parsing version numbers.
10. **imageflow is a dialect, not the design.** imageflow v1 JSON
    (`Build001`/`Execute001`/`framewise`) stays supported via the
    `imageflow-compat` translator. The zenpipe envelope is free to be better; the
    translator carries the legacy.

## 2. The envelope

A job is one JSON object. The `zenpipe` field is both the format marker and the
envelope major version — a dispatcher can accept zenpipe-native and imageflow
`Build001` payloads on the same endpoint and route by shape (`"zenpipe"` vs
`"framewise"`).

```json
{
  "zenpipe": 1,
  "inputs":  { "main": {}, "logo": { "file": "watermark.png" } },
  "outputs": { "web": {} },
  "limits":  { "max_pixels": 120000000, "max_memory_mb": 512, "deadline_ms": 10000 },
  "color":   { "icc": "apply", "on_profile_error": "error" },
  "metadata": "web",
  "optimize": "none",
  "steps": [
    { "decode": { "input": "main" } },
    { "constrain": { "mode": "within", "width": 1600 } },
    { "exposure": { "stops": 0.3 } },
    { "watermark": { "input": "logo", "gravity": "bottom_right", "opacity": 0.7 } },
    { "encode": { "output": "web", "format": "auto", "quality_profile": "good",
                   "allow": ["webp", "avif"] } }
  ]
}
```

Top-level keys (all optional except `zenpipe` and `steps`):

| Key | Type | Meaning |
|---|---|---|
| `zenpipe` | int | Envelope major version. This document defines `1`. |
| `steps` | array | Ordered one-key node objects (§4). Required. |
| `inputs` | object | Named input slots (§3). Optional for host-bound single-input jobs. |
| `outputs` | object | Named output slots (§3). Optional for host-bound single-output jobs. |
| `limits` | object | Resource limits (§9). |
| `color` | object | Color-management policy (§8). |
| `metadata` | string \| object | Metadata retention policy (§8). |
| `optimize` | string | `"none"` (default) \| `"lossless"` \| `"speed"` — reorder permission (ORDERING-DESIGN.md). |

Unknown **top-level** keys are errors (they are semantic — a job asking for
something the engine cannot see must not run). New top-level keys may be added in
minor revisions; clients discover them via capabilities (§11) before use.

`"graph"` is **reserved** for a future explicit-topology form. Envelope v1
rejects it with a structured error; steps + named branches (§5) cover DAG-shaped
jobs, including everything imageflow's `framewise.graph` expresses.

## 3. Inputs and outputs (io slots)

Slots are named with strings. Names are arbitrary (`"main"`, `"logo"`, `"web"`);
imageflow's integer `io_id`s translate to their decimal strings (`"0"`, `"1"`).

```json
"inputs": {
  "main": {},                          // bound by the host (ABI buffer, WASM, HTTP body)
  "logo": { "file": "logo.png" },      // host-gated: file access must be enabled
  "mask": { "base64": "iVBORw0K..." }  // inline bytes
},
"outputs": {
  "web":   {},                         // returned in the response / host buffer
  "thumb": { "file": "out/thumb.jpg" } // host-gated file write
}
```

Slot descriptor keys: `{}` (host-bound, the default), `base64`, `file`. Hosts may
reject descriptors they do not allow (`file` on a public server) with
`action_forbidden`. Additional descriptors (e.g. `url`) are future additive keys,
host-gated the same way.

Steps reference slots by name: `decode.input`, `encode.output`, `watermark.input`.
**Ergonomic defaults:** a job with exactly one input may omit `inputs` and the
`decode` step's `input`; a job with exactly one output may omit `outputs` and the
encode step's `output`. If `steps[0]` is not a decode-role node, `{"decode": {}}`
is implied; this matches `ImageJob`'s probe → decode → pipeline → encode chain and
keeps RIAPI-translated jobs tiny.

## 4. Steps: one-key node objects

Each step is an object with exactly one key — the node's `json_key` — whose value
is the parameter object:

```json
{ "constrain": { "mode": "fit_crop", "width": 800, "height": 600,
                 "gravity": { "x": 50, "y": 30 } } }
```

This is the serialization the zennode registry already implements
(`NodeRegistry::node_from_json` / `pipeline_from_json`): lookup by `json_key`
(falling back to the permanent node id), params matched by `json_name`, then
param `name`, then `json_aliases`; `null` clears to identity.

Two current-state facts the 0.1 audit must resolve:

- **No node has a short `json_key` yet** — every schema's `json_key` is empty,
  so the effective key is the full id (`"zenresize.constrain"`), which is what
  the CLI `JobDef` uses today. The audit assigns the short keys shown in this
  spec (`constrain`, `crop`, `encode_jpeg`, …); full ids remain accepted
  forever as the fallback spelling.
- **`deny_unknown_fields` is opt-in in the derive and set almost nowhere.** The
  envelope parser therefore enforces strictness itself: unknown params are
  errors at the job layer regardless of per-node flags (per-node opt-outs are
  not honored from the envelope; see §12).

Why one-key objects instead of `{"op": "constrain", ...}`:

- It is what zennode implements and what imageflow v1 shipped for a decade —
  migration reads naturally.
- Params live in their own namespace; step-level concerns can never collide with
  param names.
- It follows that **everything is a node** — branching, frame selection, and
  future step-level features are nodes with schemas, not envelope syntax. The
  envelope grammar never grows.

Ordering semantics: user order is preserved and executed left-to-right; the
bridge fuses adjacent geometry into one `LayoutPlan` and adjacent per-pixel
filters into one fused pass (never reordering). `optimize: "lossless" | "speed"`
opts into schema-driven reordering per ORDERING-DESIGN.md. RIAPI-translated jobs
always arrive canonically ordered (querystring keys have no order).

Node naming conventions (enforced by a registry audit test before 0.1):

- `json_key`: short snake_case operation name (`constrain`, `crop`,
  `crop_whitespace`, `encode_jpeg`, `exposure`).
- Param `json_name`: full words (`width`, `height`, `threshold`,
  `percent_padding`); terse forms (`w`, `h`) allowed only as `json_aliases` and
  kv keys.
- Colors: `"transparent"`, CSS named colors, or `#RGB/#RGBA/#RRGGBB/#RRGGBBAA`
  strings, in every color-valued param.
- Percentages are 0–100 and named `*_percent` or documented as percent;
  unit-interval values (opacity, strengths) are 0–1.

## 5. Branching and multi-output jobs

Fan-out (srcset renditions, multiple formats) and multi-input nodes (watermark,
composite) are expressed with two tiny pipeline nodes plus named slot refs — the
steps array stays flat and readable:

```json
{
  "zenpipe": 1,
  "inputs": { "main": {}, "logo": {} },
  "outputs": { "large": {}, "small": {}, "avif": {} },
  "steps": [
    { "decode": { "input": "main" } },
    { "constrain": { "mode": "within", "width": 1600 } },
    { "checkpoint": { "name": "base" } },

    { "encode": { "output": "large", "format": "webp", "quality": 82 } },

    { "resume": { "from": "base" } },
    { "constrain": { "mode": "within", "width": 400 } },
    { "watermark": { "input": "logo", "gravity": "bottom_right", "opacity": 0.7 } },
    { "encode": { "output": "small", "format": "webp", "quality": 78 } },

    { "resume": { "from": "base" } },
    { "encode": { "output": "avif", "format": "avif", "quality": 60 } }
  ]
}
```

- `{"checkpoint": {"name": ...}}` names the current pipeline position. The engine
  materializes at a checkpoint only when a later `resume` actually rewinds to it
  (single-consumer chains stay streaming; `src/cache.rs` + `Session` already
  implement subtree caching and reuse).
- `{"resume": {"from": ...}}` continues from a checkpoint, starting a new chain.
- Multi-input nodes (`watermark`, `composite`, `overlay`) take their secondary
  input from an input slot **or** a checkpoint name — one namespace, slots and
  checkpoints may not share names.

Node-availability note: `composite` and `overlay` exist today
(`zenpipe.composite`/`zenpipe.overlay`); `watermark` — imageflow's rich
gravity/fit_box/fit_mode/min_canvas semantics as a first-class node — is a 0.1
work item. The semantics are already implemented in
`src/imageflow_compat/watermark.rs` (491 lines, translating
`s::Node::Watermark` onto overlay); the work is promoting that into a
registered node so native JSON jobs get it without the compat feature.

This is DAG-complete: any imageflow `framewise.graph` (nodes + input/canvas
edges) linearizes into steps + checkpoints via topological order, which is how
`imageflow-compat` translates graph jobs.

## 6. The encode intent step

`{"encode": {...}}` is the primary encode step. It serializes
`zencodecs::CodecIntent` — the same engine behind `format=`/`qp=`/`accept.*`
querystring keys and imageflow's `EncoderPreset::Auto|Format`:

```json
{ "encode": {
    "output": "web",
    "format": "auto",
    "allow": ["webp", "avif", "jxl"],
    "quality_profile": "good",
    "quality": 80,
    "dpr": 2.0,
    "lossless": "keep",
    "matte": "#ffffff",
    "jpeg": { "progressive": true, "subsampling": "420" },
    "webp": { "effort": 6, "sharp_yuv": true },
    "avif": { "speed": 6 }
} }
```

| Param | Type | Meaning |
|---|---|---|
| `output` | string | Output slot. Optional when the job has one output. |
| `format` | string | `"auto"` (select from `allow`), `"keep"` (match source), or a specific format: `jpeg`, `png`, `webp`, `gif`, `avif`, `jxl`, `tiff`, `bmp`. Default: `keep`, or `auto` when `quality_profile` is set (imageflow-compatible). |
| `allow` | array | Formats/features permitted for `auto` (and gating for explicit formats). Members: format names plus feature flags `jpeg_progressive`, `jpeg_xyb`, `color_profiles`, and set names `web_safe`, `modern_web_safe`, `all`. Default: `web_safe`. |
| `quality_profile` | string \| number | Named profile `lowest`/`low`/`medium_low`/`medium`/`good`/`high`/`highest`/`lossless` or numeric 0–100 (`QualityProfile`). |
| `quality` | number | Generic 0–100 fallback when `quality_profile` is absent (`CodecIntent::effective_quality` precedence: profile > quality > default 73). |
| `dpr` | number | Device-pixel-ratio quality adjustment (baseline 3.0). |
| `lossless` | bool \| `"keep"` | Tri-state (`BoolKeep`): force lossless, force lossy, or match source losslessness. |
| `matte` | color | Composite onto this color when the chosen format lacks alpha. |
| `jpeg`/`png`/`webp`/`gif`/`avif`/`jxl`/`tiff`/`bmp` | object | Per-codec hints, applied only when that codec is chosen. Each object's params are **exactly the corresponding `encode_*` node's schema params** — one vocabulary, one implementation (see §7). Unknown params inside them are errors, like any node. |

Two levels, explicitly:

- **Intent level (recommended, future-proof):** `encode` + `allow` — new codecs
  and features slot into auto-selection without job changes.
- **Mechanism level (exact control):** explicit codec nodes `encode_jpeg`,
  `encode_mozjpeg`, `encode_png`, `encode_webp_lossy`, `encode_webp_lossless`,
  `encode_gif`, `encode_avif`, `encode_jxl`, `encode_tiff`, `encode_bmp` — the
  16 zencodecs-owned codec nodes. These bypass selection entirely.

Today's implementation of the intent level is the `zencodecs.quality_intent`
node (params `profile`/`quality_fallback`/`format`/`dpr`/`lossless`/
`allow_webp`/`allow_avif`/`allow_jxl`/`allow_color_profiles`, resolved through
`to_codec_intent()`). The 0.1 naming audit gives it json key `encode` and adds
the `output`, `allow` (array form), `matte`, and per-codec-object params —
all additive.

Reserved for an early minor revision (and the worked example of the additive
policy, §12): `"target": {"metric": "zensim" | "ssim2" | "butteraugli",
"value": 80}` — metric-targeted encoding. The machinery is already live below
the intent layer: `zencodec::Fidelity::{ssim2, butteraugli, codec_quality}`
(shared vocabulary), zenjpeg's closed-loop `Quality::Zq` SSIM2 target,
zenwebp's `target_zensim`/`target_size`/`target_psnr`, and
`zencodecs::transcode_to_quality` for JPEG/JXL. When wiring is uniform,
`target` lands as a new optional param with an identity default,
`since`-versioned, discoverable via capabilities. Older engines reject jobs
that use it with `unknown_param` — never silently different bytes than asked.

## 7. Per-codec option vocabulary

The per-codec objects (in `encode.jpeg`, or as explicit `encode_jpeg` params) are
defined by the codec crates' node schemas and rendered to
[`docs/querystring.md`](docs/querystring.md) + [`docs/nodes/`](docs/nodes/) by the doc generator.
Snapshot of the current surface (authoritative source: the node schemas):

| Codec | Params (current schema names¹) |
|---|---|
| `jpeg` | `quality` (0–100, def 85), `effort` (0–2), `color_space` (ycbcr/xyb/grayscale), `subsampling` (def quarter=4:2:0), `chroma_downsampling`, `scan_mode` (baseline/progressive…, def progressive), `quant_tables` (def jpegli), `deringing` (def on), `aq` (adaptive quantization, def on) |
| `mozjpeg` (node only) | `quality` (1–100), `effort`, `subsampling` |
| `png` | `png_quality`¹ (0–100), `min_quality`, `effort` (0–12 → zenpng `Compression` tiers, def 5), `lossless` (def true), `max_deflate` |
| `webp` (lossy) | `quality`, `effort` (0–10 → method 0–6, def 5), `preset`, `sharp_yuv`, `alpha_quality`, `target_size`, `target_psnr`, `segments` (1–4), `sns_strength`, `filter_strength`, `filter_sharpness` (0–7) |
| `webp` (lossless) | `effort`, `method` (0–6), `near_lossless` (0–100), `exact`, `alpha_quality`, `target_size` |
| `gif` | `quality` (1–100, def 80), `dithering` (0–1), `lossy_tolerance` (0–255), `quantizer` (def auto), `shared_palette` (def true), `palette_error_threshold` (0–50), `loop_count` (def infinite), `use_transparency` (def true) |
| `avif` | `quality` (1–100, def 75), `effort` (0–10, def 6; inverse of speed), `speed` (1–10 rav1e-native), `alpha_quality`, `bit_depth` (8/10/auto), `color_model` (ycbcr/rgb), `alpha_color_mode` (clean/dirty/premultiplied), `lossless` |
| `jxl` | `jxl_quality`¹ (0–100, def 75), `distance` (0–25 butteraugli, def 1.0), `lossless`, `effort` (0–10, def 7), `noise` |
| `tiff` | `compression` (lzw/deflate/packbits/none), `predictor` (def horizontal), `big_tiff` |
| `bmp` | `bits` (1–32, def 24) |

¹ Field names verified against `zencodecs/src/zennode_defs.rs` at this
revision. The prefixed names (`png_quality`, `jxl_quality`) shed their prefixes
via `json_name = "quality"` in the 0.1 naming audit — the struct field stays,
the JSON reads clean, the old spelling remains a `json_alias`.

In the intent step, `webp` merges the lossy/lossless node vocabularies; the
engine routes params to whichever encoder the lossless tri-state selects, and
errors on params that only exist for the other mode when that mode is forced.

Decode-side nodes carry codec decode options the same way: `decode_jpeg`
(`strictness`, `auto_orient`, `max_megapixels`), `decode_webp` (`upsampling`,
`dithering_strength`), `decode_jxl` (`adjust_orientation`,
`intensity_target`), `decode_heic` (`extract_gain_map`, `extract_depth`,
`extract_mattes`, `decode_thumbnail`), plus the generic `decode` node's
`hdr_mode` (`sdr_only`/`hdr_reconstruct`/`preserve`), `color_intent`, and
`min_size` (scaled-decode hint — wiring it to actual scaled decode is parity
workstream W6). Frame selection for animated/multi-page sources is today's
`zenpipe.riapi.frame` adapter (kv `frame`/`page`); the naming audit gives it
JSON key `frame_select`.

## 8. Color management and metadata

```json
"color": {
  "icc": "apply",              // "apply" (default) | "discard"  — imageflow: DiscardColorProfile
  "on_profile_error": "error", // "error" (default) | "ignore"   — imageflow: IgnoreColorProfileErrors
  "output": "srgb"             // "srgb" (default) | "keep"      — riapi: accept.color_profiles
},
"metadata": "web"              // "preserve" (default) | "web" | "strip" | {"keep": ["icc", "copyright"]}
```

- `color` is job-level policy; the engine inserts the CMS transform at decode
  (moxcms) and tags output color. Scaling colorspace (linear vs sRGB resampling
  math) is per-constrain (`down.colorspace` param), not job policy — it changes
  pixels, so it stays with the step that owns them.
- `metadata` maps to `zencodec::MetadataPolicy`: `preserve` = `PreserveExact`
  (verbatim + EXIF-orientation reconciliation), `web` = strips
  GPS/camera/timestamps/XMP, keeps orientation/rights/color. `strip` and the
  keep-list object form are planned additive variants — capability-discoverable,
  rejected with `unknown_variant` by engines that predate them.
- EXIF orientation is applied at decode by default (`autorotate` semantics);
  disable per-decode via the decode node.

## 9. Limits and security

Job-level `limits` merges into the host-configured `zenpipe::Limits` — a job may
tighten host limits, never loosen them (the effective limit is the minimum).

```json
"limits": {
  "max_pixels": 120000000,
  "max_memory_mb": 512,
  "deadline_ms": 10000,
  "max_frames": 200
}
```

imageflow's `ExecutionSecurity` (`max_decode_size`/`max_frame_size`/
`max_encode_size` as `{w,h,megapixels}`, `max_input_file_bytes`,
`max_json_bytes`, `max_total_file_pixels`, `max_threads`, `mem_budget_policy`)
translates onto this in `imageflow-compat`; the parity doc tracks field-by-field
coverage. The pre-flight estimator (`graph.estimate(...)` → check against
limits before executing) is the zenpipe-native answer to imageflow's
`v1/estimate` + `mem_budget_policy`.

## 10. Response envelope, warnings, errors

```json
{
  "ok": true,
  "outputs": [
    { "name": "web", "format": "webp", "mime": "image/webp",
      "extension": "webp", "width": 800, "height": 533, "byte_length": 14812 }
  ],
  "source": { "width": 4000, "height": 3000, "format": "jpeg",
              "has_alpha": false, "animated": false },
  "warnings": [
    { "kind": "deprecated", "subject": "jpeg.turbo", "message": "…use jpeg.mimic…" }
  ]
}
```

Encoded bytes travel out-of-band (host buffers/files) or inline as `"base64"`
per the output slot descriptor. Errors are structured and locate the failure:

```json
{
  "ok": false,
  "error": {
    "kind": "unknown_param",
    "message": "node 'constrain' has no param 'widht' (did you mean 'width'?)",
    "step": 1, "node": "constrain", "param": "widht"
  },
  "warnings": []
}
```

Error kinds are a closed, documented set (extended additively): `invalid_json`,
`unknown_node`, `unknown_param`, `invalid_value`, `out_of_range`,
`unknown_variant`, `missing_input`, `unknown_slot`, `image_malformed`,
`format_unsupported`, `format_disabled`, `limit_exceeded`, `cancelled`, `oom`,
`removed_feature`, `action_forbidden`, `internal`. Each carries an HTTP-ish
status for server hosts (mapping table lives with the error type — imageflow's
`ErrorCategory::http_status_code()` is the model). Warning kinds mirror the
zennode `KvWarningKind` set (`unrecognized_key`, `invalid_value`,
`deprecated`, `duplicate_key`) plus `ignored_key` for the querystring surface.

## 11. Capability discovery

One call returns everything a client needs to feature-detect (all pieces exist in
`src/schema_export.rs` + `src/codec_info.rs` today; this bundles them):

```json
{
  "zenpipe_envelope": 1,
  "revision": "0.1.0",
  "nodes":  { "...": "JSON Schema 2020-12 with $defs per node, x-zennode-* extensions" },
  "querystring": { "...": "key registry grouped by node" },
  "codecs": { "decode": ["jpeg", "png", "webp", "gif", "avif", "jxl", "tiff", "bmp", "heic"],
               "encode": ["jpeg", "png", "webp", "gif", "avif", "jxl", "tiff", "bmp"] },
  "features": ["animation", "gain_maps", "smart_crop", "imageflow_compat"]
}
```

Client contract: **check, or try-and-handle-structured-error.** Both are safe —
nothing degrades silently. This is also the SDK-generation source (the C#
generator in `src/codegen_csharp.rs` consumes the same export).

## 12. Compatibility & lifecycle policy

The envelope inherits zennode's versioning rules and adds envelope-level ones.
The full contract, stated once:

**Permanent (never renamed, never reused):** node ids, node `json_key`s, param
`name`s/`json_name`s/`json_aliases`, kv keys, enum variant names, top-level
envelope keys, error/warning kinds, slot descriptor keys.

**Additive-only within envelope major 1:** new nodes; new params (identity
default + `since` version); new enum variants; new top-level keys; new
capability entries; new warning kinds. An old job always means the same pixels
on a new engine — defaults are frozen per schema version.

**Deprecation (the feature-removal path):**

1. Mark: schema gains `deprecated_since` + replacement pointer; every use emits a
   `deprecated` warning; capabilities and generated docs flag it. Behavior
   unchanged.
2. Sunset (≥ one minor later, and only with real cause — security, correctness,
   unmaintainable dependency): the node/param keeps parsing but returns
   `removed_feature` with the replacement named. **The name stays reserved
   forever.** No job ever gets *different pixels* than it asked for — it gets a
   clear refusal.
3. Envelope major 2 (avoid indefinitely): the only point where wire shape may
   change; v1 payloads remain accepted via translation, exactly as imageflow v1
   is via `imageflow-compat`.

**Unknown-input policy (asymmetric by design, §1.7):**

| Surface | Unknown node/param/variant | Rationale |
|---|---|---|
| JSON envelope | structured error | programmatic input; typos must not ship wrong pixels |
| Querystring | `unrecognized_key`/`ignored_key` warning; key ignored | URLs carry foreign keys; RIAPI behavior for 15+ years |

**What "supporting removals" does *not* mean:** per-step "optional/skip if
unsupported" flags. A skipped resize or filter silently changes output — that is
a shipping bug by this repo's standards, not graceful degradation. Clients that
want fallback behavior implement it client-side with capabilities.

## 13. RIAPI and imageflow dialects

Both legacy surfaces are translations into this envelope:

- **RIAPI**: `NodeRegistry::from_querystring` builds node instances from kv keys
  (the path the CLI `--qs` uses today); the job layer wraps them in an implied
  envelope (single input/output, canonical phase ordering). A
  `{"riapi": {"query": "w=800&h=600&mode=crop"}}` step — the imageflow
  `command_string` equivalent for embedding querystrings in JSON jobs — is a
  0.1 work item (§15), not yet a node. Note the querystring path has open
  semantic divergences vs imageflow and an unwired `srcset=` expander; see
  [`IMAGEFLOW-PARITY.md`](IMAGEFLOW-PARITY.md) §4 and workstream W1 — the
  divergence fixes land there, not in this spec.
- **imageflow v1** (`imageflow-compat` feature): `Build001`/`Execute001`
  envelopes, `framewise.steps|graph`, `io` arrays, `security`, decoder commands,
  and `EncoderPreset`s translate node-by-node; the parity matrix in
  [`IMAGEFLOW-PARITY.md`](IMAGEFLOW-PARITY.md) is the authoritative coverage
  statement.

## 14. Worked examples

**Minimal — RIAPI equivalent of `?w=800&format=webp&quality=80`:**

```json
{ "zenpipe": 1,
  "steps": [
    { "constrain": { "mode": "within", "width": 800 } },
    { "encode": { "format": "webp", "quality": 80 } }
  ] }
```

(Implied: single host-bound input and output, implied decode.)

**Photo edit + srcset fan-out:** see §5.

**Exact-control encode (bypasses selection):**

```json
{ "zenpipe": 1,
  "steps": [
    { "decode": {} },
    { "constrain": { "mode": "fit_crop", "width": 1200, "height": 630,
                     "gravity": { "x": 50, "y": 25 } } },
    { "encode_jpeg": { "quality": 84, "progressive": "progressive",
                        "subsampling": "420", "deringing": true } }
  ] }
```

**Animated GIF → animated WebP, resized:**

```json
{ "zenpipe": 1,
  "steps": [
    { "decode": {} },
    { "constrain": { "mode": "within", "width": 480 } },
    { "encode": { "format": "webp", "quality": 75 } }
  ] }
```

(Animation is a property of the source, not the job: animated inputs stream
frame-by-frame through the same per-frame pipeline; `frame_select` collapses to
a single frame when stills are wanted.)

## 15. Path to 0.1 (envelope work items)

1. **Registry naming audit** — assign short `json_key`s (all are empty today →
   full ids), review every param `json_name`/alias and enum variant against §4
   conventions, de-prefix `png_quality`/`jxl_quality`, rename
   `quality_intent`→`encode`, reserve `checkpoint`/`resume`/`watermark`/
   `frame_select`/`riapi`/`graph`. Renames are free now and forbidden after
   0.1. Land the audit as a unit test (conventions + collision detection +
   reserved-word list).
2. **Envelope parser** — `Job::from_json` implementing §2–§5 over the existing
   registry/pipeline/ImageJob machinery; checkpoint/resume nodes; slot
   resolution; job-layer strictness (§4). Retires the CLI-only `JobDef`
   (`zenpipe-cmd/src/job_json.rs`) — same CLI flag, new envelope underneath,
   per-output format from the job (not the file extension).
3. **Response/warning/error types** — §10 structs, serialization, HTTP mapping.
4. **Capabilities bundle** — §11 (compose existing exporters + codec_info).
5. **Golden tests** — envelope JSON fixtures → output digests; schema snapshot
   test (export drift breaks CI, like imageflow's `openapi_schema_v1.json` hash
   guard); fuzz the envelope parser (serde-arbitrary + structured corpus).
6. **Schema `deprecated_since`** — add to zennode `NodeSchema`/`ParamDesc`
   (additive), wire the `deprecated` warning kind.
7. **imageflow-compat + querystring parity** — the divergence fixes and
   coverage workstreams live in [`IMAGEFLOW-PARITY.md`](IMAGEFLOW-PARITY.md)
   (W1–W10); W1 (querystring semantic fixes), W2 (delete dead parsers, wire
   `srcset`), W7 (Limits threading), and W9 (parity test harness) gate 0.1
   alongside this envelope.
8. **First-class `watermark` node** — promote the compat watermark semantics
   into the registry (§5 note).
