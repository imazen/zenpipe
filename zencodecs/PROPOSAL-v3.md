# zencodecs v3 Proposal: Right-Sized Codec Orchestration

## Problem

zencodecs v2 tries to be everything: codec dispatch, format selection, quality mapping,
gain map extraction, depth map extraction, streaming encode/decode, animation, pixel
conversion, and metadata handling. This makes it:

1. **Hard to stabilize** — too many responsibilities changing at different rates
2. **Hard to depend on** — downstream crates get all the complexity even if they need one thing
3. **Confused about its boundary with zenpipe** — both want to own encode/decode orchestration
4. **Missing its zennode integration** — QualityIntent is currently in zennode with hardcoded
   boolean fields instead of using zencodecs' own FormatSet

And critically:

5. **Full-frame materialization is the path of least resistance** — the easy APIs
   (`decode() → DecodeOutput`, `encode_rgb8()`) materialize entire images, and once
   materialization slips into a system it's nearly impossible to remove. A 4K RGBA image
   is 36MB. A transcode pipeline that materializes on both sides is 72MB per image.
   JPEG can stream through in 192KB via MCU rows. The API shape must make streaming
   the default, not the advanced option.

## Proposed Role

**zencodecs is the complete codec I/O layer: oracle, streaming dispatch, and faithful
container roundtrip.**

Two audiences, one crate:

1. **zenpipe** uses zencodecs at its graph edges — `push_decode` feeds the pipeline
   source, `streaming_encoder` drains the pipeline sink. zencodecs handles codec-level
   streaming; zenpipe handles multi-stage pixel processing between them.

2. **CLI tools** (a `cjpegli`, `cwebp`, `cavif` equivalent) use zencodecs directly
   for format conversion without losing anything — metadata, animation frames, gain
   maps, depth maps, supplementary images all roundtrip faithfully. No zenpipe dependency
   needed for tools that don't process pixels.

The dividing line: **zencodecs owns everything in the container. zenpipe owns
everything that transforms pixels.** Extraction, embedding, and passthrough of
container-level data is zencodecs' job. Resizing, filtering, tone mapping, and
color management are zenpipe's job.

## Streaming-First Design

### Why streaming must be the default

If full-frame is the easy path, everyone takes it:
- CLI tools call `decode()` and get a 36MB buffer they didn't need
- Batch processors hold N images in memory simultaneously
- Transcode pipelines materialize twice (decode side + encode side)
- The "just get it working" path becomes the production path

If streaming is the only path (with full-frame as a convenience wrapper):
- The default behavior is memory-efficient
- Callers who actually need full-frame pixels opt in explicitly
- Transcode goes through `TranscodeSink` — per-strip, ~192KB peak for JPEG
- The path of least resistance is the performant path

### Core streaming API

```rust
/// PRIMARY API: Push-decode through a sink. Codec pushes rows into the sink —
/// no full-image buffer allocated. This is how you decode.
pub fn push_decode(
    data: &[u8],
    sink: &mut dyn DecodeRowSink,
    registry: &CodecRegistry,
) -> Result<OutputInfo, CodecError>;

/// PRIMARY API: Build a streaming encoder. Returns a DynEncoder that accepts
/// rows via push_rows() and produces encoded bytes on finish().
pub fn streaming_encoder(
    format: ImageFormat,
    intent: &ResolvedIntent,
    width: u32,
    height: u32,
    metadata: Option<&ImageMetadata<'_>>,
    registry: &CodecRegistry,
) -> Result<Box<dyn DynEncoder + '_>, CodecError>;
```

### Transcode API (zero-materialization)

```rust
/// TRANSCODE: Decode → encode with no full-image materialization.
/// Internally wires push_decode → TranscodeSink → DynEncoder.
/// Per-strip pixel format conversion via adapt_for_encode().
pub fn transcode(
    data: &[u8],
    decision: &FormatDecision,
    opts: &TranscodeOptions<'_>,
    registry: &CodecRegistry,
) -> Result<TranscodeOutput, CodecError>;

/// Controls what gets preserved during transcode.
pub struct TranscodeOptions<'a> {
    /// Metadata to embed (EXIF, ICC, XMP). None = extract from source and roundtrip.
    pub metadata: Option<&'a ImageMetadata<'a>>,
    /// Supplement handling policy.
    pub supplements: SupplementPolicy,
    /// Matte color for alpha compositing when encoding to a format without alpha
    /// (e.g., RGBA source → JPEG output). None = white.
    pub matte: Option<[u8; 3]>,
}

/// What to do with container supplements (gain maps, depth maps, etc.)
/// during transcode.
#[derive(Clone, Copy, Debug, Default)]
pub enum SupplementPolicy {
    /// Roundtrip all supplements the target format supports.
    /// Gain maps, depth maps, and auxiliary images are extracted from the
    /// source container and re-embedded in the output container.
    /// Supplements that the target format can't represent are silently dropped.
    #[default]
    Preserve,
    /// Strip all supplements. Output contains only the primary image + metadata.
    Strip,
    /// Preserve only specific supplement types.
    Only(SupplementSet),
}

/// Bitflag set of supplement types.
#[derive(Clone, Copy, Debug)]
pub struct SupplementSet(u32);
impl SupplementSet {
    pub const GAIN_MAP: Self = Self(1);
    pub const DEPTH_MAP: Self = Self(2);
    pub const THUMBNAIL: Self = Self(4);
    // Future: STEREO_PAIR, SEMANTIC_MASK, etc.
}
```

### Full-frame convenience wrappers

```rust
/// CONVENIENCE: Full-frame decode. Built on push_decode with a collecting sink.
/// Use this when you genuinely need all pixels in memory (e.g., for resize,
/// analysis, or non-streaming consumers). Not the default path.
pub fn decode_full_frame(
    data: &[u8],
    registry: &CodecRegistry,
) -> Result<DecodeOutput, CodecError>;

/// CONVENIENCE: Full-frame encode from a pixel buffer.
/// Built on streaming_encoder with push_rows from the buffer.
pub fn encode_full_frame(
    pixels: PixelSlice<'_>,
    format: ImageFormat,
    decision: &FormatDecision,
    metadata: Option<&ImageMetadata<'_>>,
    registry: &CodecRegistry,
) -> Result<EncodeOutput, CodecError>;
```

The naming signals intent: `push_decode` and `streaming_encoder` are the primary
verbs. `decode_full_frame` and `encode_full_frame` are explicitly marked as the
full-materialization path — you have to ask for it by name.

### TranscodeSink: the zero-materialization bridge

Already exists in v2. Implements `DecodeRowSink`, forwards decoded strips directly
to `DynEncoder::push_rows()` with per-strip `adapt_for_encode()` conversion.
Peak memory for JPEG→WebP transcode: ~192KB (MCU strip buffer + conversion buffer).

### Streaming encode lifetime fix

`build_streaming_encoder()` is currently stubbed due to a lifetime GAT issue:
`EncoderConfig::job()` borrows config, but the encoder lifetime can't escape
the function scope when config is constructed inside the closure.

**Fix path:** Have each codec produce a `Box<dyn DynEncoder + 'static>` directly
via a factory method that takes owned config, bypassing the GAT borrow chain.
The individual codec crates (zenjpeg, zenwebp, etc.) each provide this.

### Animation: streaming frames

Animation decode is already streaming — `DynAnimationFrameDecoder::render_next_frame_to_sink`
pushes each frame through a `DecodeRowSink`. Animation encode uses
`DynAnimationFrameEncoder::push_frame()`. Both avoid materializing all frames simultaneously.

## Container Supplements

### The principle

**Container data** = zencodecs' job. Anything that lives in the file container
alongside the primary image: metadata, gain maps, depth maps, thumbnails,
auxiliary images, MPF segments, AVIF auxiliary items, JXL container boxes.

**Pixel transformation** = zenpipe's job. Anything that changes pixel values:
tone mapping a gain map onto the base image, resizing a depth map to match a
resized base, color management, filtering.

zencodecs extracts, embeds, and passes through container data.
zenpipe transforms pixel data using that container data as input.

### What qualifies as a supplement

| Supplement | Source formats | Container mechanism |
|---|---|---|
| Gain map | JPEG (UltraHDR MPF), AVIF (tmap), JXL (jhgm), DNG (MPF) | Secondary image + metadata |
| Depth map | JPEG (MPF disparity), HEIC (auxl) | Auxiliary image |
| Thumbnail | JPEG (JFIF APP0), HEIC (thmb), JXL (preview) | Embedded reduced image |
| Portrait matte | HEIC (auxl, Apple URNs) | Auxiliary alpha plane |
| Stereo pair | JPEG (MPF), HEIC (stereo) | Secondary image |

### Supplement passthrough during transcode

When `SupplementPolicy::Preserve` is active (the default), `transcode()`:

1. Probes the source container for supplements
2. Extracts any supplements the target format can represent
3. Streams the primary image pixels through `TranscodeSink` (no materialization)
4. Re-embeds extracted supplements into the output container
5. Silently drops supplements the target format can't represent

JPEG→JPEG transcode preserves UltraHDR gain maps. JPEG→PNG drops them (PNG
has no gain map container). This is automatic.

### What zenpipe does with supplements

When zenpipe processes an image (resize, filter, etc.), it needs to transform
supplements to match. zenpipe calls zencodecs to extract supplements, transforms
them as sidecar streams in the pipeline graph, then calls zencodecs to re-embed
them in the output. Extraction/embedding = zencodecs; geometric tracking and
pixel transformation = zenpipe.

## Codec Intent and RIAPI Integration

### Design precedent

imageflow already has these abstractions in various stages of development:

- **`CodecIntent`** — parsed from routed query keys: format choice, quality profile,
  quality fallback, DPR, lossless (BoolKeep), allowed formats, per-codec hints
- **`FormatChoice`** — `Specific(ImageFormat)` / `Auto` / `Keep`
- **`BoolKeep`** — `True` / `False` / `Keep` (preserve source losslessness)
- **`PerCodecHints`** — `BTreeMap<String, String>` per codec (untyped, extensible)
- **`FormatDecision`** — format + quality intent + per-codec hints + selection trace
- **`QualityProfile`** — 8 named presets + `Percent(f32)`
- **`QualityIntent`** — resolved quality with DPR adjustment, per-codec calibration
  tables (`mozjpeg_quality()`, `jxl_distance()`, `avif_quality()`, etc.)
- **Calibration tables** — SSIM-equivalence derived, piecewise-linear interpolation

zencodecs should be the authoritative home for these types. imageflow consumes them.

### CodecIntent: the parsed user intent

```rust
/// Parsed codec-related user intent from querystring parameters.
/// Constructed from RIAPI codec keys (format, qp, quality, accept.*, codec.*).
#[derive(Debug, Clone, Default)]
pub struct CodecIntent {
    /// Explicit format choice. None = context-dependent default.
    pub format: Option<FormatChoice>,
    /// Quality profile from `qp=`.
    pub quality_profile: Option<QualityProfile>,
    /// Fallback quality from `quality=` (0–100). Used when qp is absent.
    pub quality_fallback: Option<f32>,
    /// DPR adjustment for quality from `qp.dpr=`.
    pub quality_dpr: Option<f32>,
    /// Global lossless preference from `lossless=`.
    pub lossless: Option<BoolKeep>,
    /// Allowed formats from `accept.*` keys.
    pub allowed: FormatSet,
    /// Per-codec hints (raw key-value pairs for downstream config builders).
    pub hints: PerCodecHints,
    /// Matte color for alpha compositing. From `bgcolor=` when encoding.
    pub matte: Option<[u8; 3]>,
}

/// Explicit format choice from `format=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatChoice {
    /// A specific format: `format=jpeg`, `format=webp`, etc.
    Specific(ImageFormat),
    /// `format=auto` — let the selector decide.
    Auto,
    /// `format=keep` — match source format.
    Keep,
}

/// Tri-state for lossless: true, false, or keep (match source).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolKeep {
    True,
    False,
    /// Preserve source losslessness. If source was lossless PNG, encode lossless.
    /// If source was lossy JPEG, encode lossy.
    Keep,
}

/// Per-codec encoder hints as raw key-value pairs.
/// Untyped and extensible — adding a new codec hint is a parsing change,
/// not a struct change. The codec adapter interprets the strings downstream.
#[derive(Debug, Clone, Default)]
pub struct PerCodecHints {
    pub jpeg: BTreeMap<String, String>,
    pub png: BTreeMap<String, String>,
    pub webp: BTreeMap<String, String>,
    pub avif: BTreeMap<String, String>,
    pub jxl: BTreeMap<String, String>,
    pub gif: BTreeMap<String, String>,
}
```

### Per-codec hints: the override mechanism

Per-codec hints carry raw `BTreeMap<String, String>` entries parsed from
`jpeg.quality=75`, `webp.lossless=true`, `avif.speed=4`, etc. They override
the profile-derived quality for the selected format:

```rust
impl FormatDecision {
    /// Per-codec hints for the selected format.
    /// Empty if no hints were specified for this format.
    pub fn hints_for_format(&self) -> &BTreeMap<String, String> { ... }
}
```

The codec adapter reads the hints and applies them. For example, the JPEG adapter:
- `hints.get("quality")` → override mozjpeg_quality()
- `hints.get("progressive")` → override progressive default
- `hints.get("li")` → select jpegli encoder

This approach is better than typed `EncoderHints` structs
because adding a new hint for a new codec never changes the struct. The strings
are validated at the codec adapter, not at parse time.

### FormatDecision: the resolved output

```rust
/// The result of codec selection: what format, what quality, why.
#[derive(Debug, Clone)]
pub struct FormatDecision {
    /// The selected output format.
    pub format: ImageFormat,
    /// Resolved quality intent with per-codec calibration.
    pub quality: QualityIntent,
    /// Global lossless preference (resolved from BoolKeep + source facts).
    pub lossless: bool,
    /// Per-codec hints for the selected format.
    pub hints: BTreeMap<String, String>,
    /// Matte color for alpha compositing (RGBA → opaque format).
    pub matte: Option<[u8; 3]>,
    /// Explanation trace for debugging/auditing.
    pub trace: Vec<SelectionStep>,
}
```

### Quality resolution: profile → per-codec values

```rust
/// Resolved quality with DPR adjustment. Per-codec calibration via lookup methods.
#[derive(Debug, Clone, Copy)]
pub struct QualityIntent {
    /// Codec-agnostic quality 0–100, already DPR-adjusted.
    pub generic_quality: f32,
    /// The original profile before DPR adjustment, if any.
    pub profile: Option<QualityProfile>,
    /// The DPR value used for adjustment.
    pub dpr: Option<f32>,
}

impl QualityIntent {
    pub fn mozjpeg_quality(&self) -> u8 { ... }   // calibration table
    pub fn libwebp_quality(&self) -> f32 { ... }   // calibration table
    pub fn jxl_distance(&self) -> f32 { ... }      // calibration table (inverse)
    pub fn avif_quality(&self) -> f32 { ... }      // calibration table
    pub fn png_quality_range(&self) -> (u8, u8) { ... }
    pub fn is_lossless(&self) -> bool { ... }
}
```

Per-codec hints override these. If `hints.get("quality")` returns `Some("75")`,
the adapter uses 75 instead of `mozjpeg_quality()`. The calibration table is the
default; hints are the override.

### Implicit format default: `qp` triggers auto

When no `format` key is present:
- If `qp` is set → `FormatChoice::Auto` (modern behavior)
- If `qp` is absent → `FormatChoice::Keep` (legacy behavior)

This matches imageflow's existing semantics. The presence of `qp` signals
"I'm using the quality profile system, auto-select the best format."

## Versioned RIAPI Key Parsing

### The versioning problem

imageflow needs to know when a query uses "v3 features" so it can route through
zencodecs' selection/streaming engine vs its legacy path. The presence of certain
keys signals the query's feature level.

### Approach: two-level feature detection

zencodecs provides a parser for codec-related RIAPI keys that reports whether
the query uses modern codec features (requiring zencodecs) or only legacy keys
(handleable by imageflow's existing encoder path).

```rust
/// Whether a query uses zencodecs' codec engine or can be handled by legacy imageflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecEngine {
    /// Only `quality=` and `format=<specific>`. No profile system, no auto-selection.
    /// Handleable by imageflow's existing monolithic encoder path.
    Legacy,
    /// Any modern codec key: `qp=`, `accept.*`, `lossless=`, per-codec hints,
    /// `format=auto`, supplements, matte. Requires zencodecs.
    Modern,
}

/// Parse codec-related RIAPI keys and detect which engine is needed.
///
/// Accepts the codec partition from a key router (a BTreeMap of canonicalized
/// codec keys) and returns a CodecIntent plus the detected engine requirement.
pub fn parse_codec_keys(keys: &BTreeMap<String, String>) -> (CodecIntent, CodecEngine) {
    let intent = CodecIntent::from_keys(keys);
    let engine = detect_engine(keys);
    (intent, engine)
}
```

### Detection rules

```rust
fn detect_engine(keys: &BTreeMap<String, String>) -> CodecEngine {
    // Any of these keys requires zencodecs
    let modern_keys = [
        "qp", "qp.dpr", "lossless",
        "accept.webp", "accept.avif", "accept.jxl",
        "supplements", "supplements.only", "matte",
    ];
    for key in &modern_keys {
        if keys.contains_key(*key) {
            return CodecEngine::Modern;
        }
    }

    // Per-codec hints require zencodecs
    if keys.keys().any(|k| {
        k.starts_with("jpeg.") || k.starts_with("png.") || k.starts_with("webp.")
            || k.starts_with("avif.") || k.starts_with("jxl.")
    }) {
        return CodecEngine::Modern;
    }

    // format=auto triggers the selection engine
    if keys.get("format").map(|v| v == "auto") == Some(true) {
        return CodecEngine::Modern;
    }

    // Formats only zencodecs handles
    if let Some(fmt) = keys.get("format") {
        if matches!(fmt.to_ascii_lowercase().as_str(), "heic") {
            return CodecEngine::Modern;
        }
    }

    CodecEngine::Legacy
}
```

### How imageflow uses this

```rust
// In imageflow's RIAPI handler:
let routed = key_router::route_querystring(&querystring);
let (codec_intent, engine) = zencodecs::parse_codec_keys(&routed.codec);

match engine {
    CodecEngine::Legacy => {
        // Existing imageflow_core encoder path
        let preset = calculate_encoder_preset_legacy(&instructions);
        // ...
    }
    CodecEngine::Modern => {
        // zencodecs selection + streaming
        let facts = ImageFacts::from_decoded(&decoded_info);
        let decision = zencodecs::select_format(&codec_intent, &facts);
        // decision.format, decision.quality, decision.hints → configure encoder
        // ...
    }
}
```

### Key vocabulary owned by zencodecs

zencodecs' parser recognizes these codec keys (behind `feature = "riapi"`):

| Key | Type | Engine | Description |
|-----|------|--------|-------------|
| `format` | FormatChoice | Legacy* | Output format (jpeg/png/gif/webp/avif/jxl/auto/keep) |
| `quality` | f32 | Legacy | Legacy quality 0-100, fallback for `qp` |
| `qp` | QualityProfile | Modern | Named quality preset or numeric 0-100 |
| `qp.dpr` / `qp.dppx` | f32 | Modern | Device pixel ratio for quality adjustment |
| `lossless` | BoolKeep | Modern | Lossless preference (true/false/keep) |
| `accept.webp` | bool | Modern | Allow WebP in auto-selection |
| `accept.avif` | bool | Modern | Allow AVIF in auto-selection |
| `accept.jxl` | bool | Modern | Allow JXL in auto-selection |
| `jpeg.*` | String | Modern | JPEG hints (quality, progressive, li) |
| `png.*` | String | Modern | PNG hints (quality, lossless, min_quality, etc.) |
| `webp.*` | String | Modern | WebP hints (quality, lossless) |
| `avif.*` | String | Modern | AVIF hints (quality, speed) |
| `jxl.*` | String | Modern | JXL hints (quality, distance, effort, lossless) |
| `supplements` | SupplementPolicy | Modern | Supplement handling (preserve/strip) |
| `supplements.only` | SupplementSet | Modern | Selective supplement preservation |
| `matte` | Color | Modern | Alpha matte color for opaque format encode |

*`format=auto` and `format=heic` trigger Modern; `format=jpeg` etc. are Legacy alone.

Keys not in this table (layout, filter, decode, compose) are NOT zencodecs'
concern — they belong to zenlayout, zenfilters, and zenpipe respectively.
The key router (in imageflow) partitions keys before zencodecs sees them.

`accept.color_profiles` is NOT a codec key — it controls color management
policy, which is zenpipe's responsibility. The key router should route it
to the CMS/decode partition, not to zencodecs.

## QualityIntent Node (zennode integration)

When the `zennode` feature is enabled, zencodecs exposes a zennode node that wraps
`CodecIntent` for use in node graphs.

```rust
// In zencodecs, behind feature = "zennode"

/// Format selection and quality profile for encoding.
///
/// This is the zennode-native wrapper around CodecIntent. The node's parameters
/// map directly to RIAPI codec keys. At resolve time, the node produces a
/// FormatDecision that the pipeline sink uses to configure the encoder.
#[derive(Node, Clone, Debug)]
#[node(id = "zencodecs.quality_intent", group = Encode, phase = Encode)]
#[node(tags("quality", "auto", "format", "encode"))]
pub struct QualityIntentNode {
    #[param(default = "high")]
    #[param(section = "Main", label = "Quality Profile")]
    #[kv("qp")]
    pub profile: String,

    #[param(default = "")]
    #[param(section = "Main", label = "Output Format")]
    #[kv("format")]
    pub format: String,

    #[param(range(0.5..=10.0), default = 1.0, identity = 1.0, step = 0.5)]
    #[param(unit = "×", section = "Main")]
    #[kv("qp.dpr", "qp.dppx")]
    pub dpr: f32,

    #[param(default = "")]
    #[param(section = "Main")]
    #[kv("lossless")]
    pub lossless: String, // "" | "true" | "false" | "keep" → parsed as Option<BoolKeep>

    #[param(default = "")]
    #[param(section = "Allowed Formats", label = "Accepted Formats")]
    #[kv("accept")]
    pub accepted_formats: String, // "webp,avif,jxl" → parsed to FormatSet
}

impl QualityIntentNode {
    /// Convert to a CodecIntent for resolution.
    pub fn to_codec_intent(&self) -> CodecIntent {
        CodecIntent {
            format: parse_format_choice(&self.format),
            quality_profile: QualityProfile::parse(&self.profile),
            quality_dpr: if self.dpr != 1.0 { Some(self.dpr) } else { None },
            lossless: parse_bool_keep(&self.lossless),
            allowed: parse_format_set(&self.accepted_formats),
            ..Default::default()
        }
    }

    /// Resolve to a FormatDecision given image facts.
    pub fn resolve(&self, facts: &ImageFacts) -> FormatDecision {
        select_format(&self.to_codec_intent(), facts)
    }
}
```

**RIAPI compatibility:** The `from_kv` implementation maps individual `accept.webp=true`
keys into the comma-separated `accepted_formats` string. Adding a new format requires
only a new key mapping, not a struct field. Per-codec hints (`jpeg.*`, etc.) can be
passed separately via the node graph's encode-phase context.

## Relationship Diagram

```
                    zennode (infrastructure)
                   /          |            \
                  /           |             \
    codec crates            zenpipe          zencodecs
   (zenjpeg, etc.)      (pixel pipeline)   (codec orchestration + I/O)
   Implement encode/   Multi-stage graph    RIAPI codec key parsing
   decode traits       Op fusion            Format selection / quality
                       Resize → filter →    Calibration tables
                       CMS → encode chain   CodecRegistry / Policy
                       Supplement tracking  Streaming decode / encode
                       Color management     Supplement extract / embed
                       (geometric xforms)   Transcode w/ passthrough
                            |               Animation frame streaming
                            |               Config re-exports
                            |                    |
                       imageflow (aggregator)
                       Key routing (key_router)
                       Layout (zenlayout bridge)
                       Composes both. Owns IO.
                       RIAPI + JSON API.

                       CLI tools (zen-transcode, zen-probe, etc.)
                       Use zencodecs directly. No zenpipe needed.
```

### Boundary: zencodecs vs zenpipe

**zencodecs** owns codec-level I/O and container fidelity:
- Rows in/out of a single codec (streaming)
- Container supplement extraction and embedding
- Metadata roundtrip (EXIF, ICC, XMP)
- Animation frame iteration and re-encoding
- Format selection, quality calibration, per-codec hints
- RIAPI codec key parsing and feature level detection

**zenpipe** owns multi-stage pixel transformation:
- Graph fusion (decode → resize → filter → CMS → encode)
- Supplement geometric tracking (resize gain map to match resized base)
- Gain map application (tone mapping HDR)
- Depth map processing (normalization, resolution matching)
- Color management (moxcms profile conversion, `accept.color_profiles`)

### Boundary: zencodecs vs imageflow key_router

The **key router** (in imageflow) partitions ALL query keys by subsystem:
layout → zenlayout, codec → zencodecs, filter → zenpipe, decode → decoder config,
compose → composition engine, meta → informational.

zencodecs only sees the **codec partition** — already filtered to format/quality/
accept/per-codec keys. It doesn't need to know about `w=`, `mode=`, `f.sharpen=`,
or other non-codec keys.

The key router is imageflow's responsibility. zencodecs provides the vocabulary
(which keys it recognizes) but doesn't do the routing itself.

### Dependency graph

```
zencodec (traits) ← all codec crates
zennode ← all codec crates (optional), zencodecs (optional), zenpipe (optional)
zencodecs → zencodec, zennode (optional)
zenpipe → zencodec, zennode (optional), zenresize, zenlayout
imageflow → zencodecs, zenpipe, zennode, all codec crates
CLI tools → zencodecs only
```

**zencodecs and zenpipe are peers, not layered.** Neither depends on the other.

### Encode path: imageflow (full pipeline)

```
1. imageflow key_router partitions RIAPI query into {layout, codec, filter, ...}
2. zencodecs::parse_codec_keys(&routed.codec) → (CodecIntent, Modern)
3. zenpipe bridge builds streaming pixel pipeline from layout + filter keys
4. After decode, build ImageFacts from decoded info
5. zencodecs::select_format(&codec_intent, &facts) → FormatDecision
6. FormatDecision: format=WebP, quality=76 (from calibration), hints={quality: "70"} (override)
7. Bridge calls zencodecs::streaming_encoder() with decision
8. Pipeline streams rows: decode → resize → filter → encoder (no materialization)
9. zencodecs::finish() → encoded bytes, supplements re-embedded
```

### Encode path: CLI tool (no pipeline)

```
1. Read input file
2. Parse CLI args into CodecIntent (or use zencodecs::parse_codec_keys for RIAPI compat)
3. Probe source: zencodecs::probe() → ImageFacts
4. zencodecs::select_format(&intent, &facts) → FormatDecision
5. zencodecs::transcode(&data, &decision, &opts, &registry)
   → push_decode streams rows through TranscodeSink to encoder
   → supplements extracted from source, re-embedded in output
   → metadata roundtripped (EXIF, ICC, XMP)
6. Write output file
```

## CLI Tool Use Case

zencodecs should be sufficient to build complete codec CLI tools without zenpipe:

```
zen-transcode input.jpg -f webp -q high -o output.webp
zen-transcode input.jpg -f jxl -q medium --strip-supplements -o output.jxl
zen-transcode animated.gif -f webp -q high -o animated.webp
zen-probe input.jpg  # format, dimensions, supplements, metadata summary
```

Implementation using zencodecs alone:

```rust
// Full-fidelity transcode: JPEG → JPEG, preserving gain map + metadata
let intent = CodecIntent {
    format: Some(FormatChoice::Specific(ImageFormat::Jpeg)),
    quality_profile: Some(QualityProfile::High),
    ..Default::default()
};
let facts = zencodecs::probe(&input_data, &registry)?.to_image_facts();
let decision = zencodecs::select_format(&intent, &facts);
let opts = TranscodeOptions::default(); // Preserve supplements, roundtrip metadata
let output = zencodecs::transcode(&input_data, &decision, &opts, &registry)?;

// Format=keep with lossless=keep: re-encode at same format/losslessness
let intent = CodecIntent {
    format: Some(FormatChoice::Keep),
    quality_profile: Some(QualityProfile::High),
    lossless: Some(BoolKeep::Keep),
    ..Default::default()
};
```

No zenpipe, no zennode, no graph — just codec I/O with full container fidelity.

## What stays in zencodecs

| Responsibility | Why it belongs here |
|---|---|
| `CodecRegistry` | Compile-time + runtime codec enable/disable |
| `FormatSet` | Compact bitflag set of formats |
| `CodecPolicy` | Per-request codec filtering (killbits, allowlist, preferences) |
| `QualityProfile` | Named quality presets (lowest→lossless) |
| `QualityIntent` | Resolved per-codec quality with calibration tables + DPR adjustment |
| `CodecIntent` | Parsed user intent from RIAPI codec keys |
| `FormatDecision` | Resolved format + quality + hints + trace |
| `FormatChoice` | Specific / Auto / Keep |
| `BoolKeep` | True / False / Keep (preserve source) |
| `PerCodecHints` | Raw `BTreeMap<String, String>` per codec — override mechanism |
| `select_format()` | Format auto-selection from CodecIntent + ImageFacts |
| `SelectionTrace` | Audit trail for selection decisions |
| `ImageFacts` | Source image properties driving selection |
| `CodecConfig` | Per-codec config re-exports behind feature gates |
| `CodecId` | Identifies specific codec implementations for policy targeting |
| `parse_codec_keys()` | RIAPI codec key parser with feature level detection |
| `CodecEngine` | Legacy / Modern — lets imageflow route to the right engine |
| Format detection | `ImageFormat::detect()` from magic bytes (zencodec + registry awareness) |
| Streaming decode | `push_decode(data, sink)` — primary decode API |
| Streaming encode | `streaming_encoder()` → `DynEncoder` with `push_rows()` |
| Transcode | `transcode()` with supplement passthrough and matte |
| Full-frame decode/encode | Convenience wrappers built on streaming |
| Animation decode/encode | Per-frame sink streaming |
| Supplement extraction/embedding | Gain maps, depth maps, thumbnails — container I/O |
| Probe | `probe()` → ImageInfo + supplement inventory |
| Calibration tables | SSIM-equivalence derived per-codec quality mapping |

## What moves OUT of zencodecs

| Responsibility | Where it goes | Why |
|---|---|---|
| Gain map *application* (tone mapping) | zenpipe | Pixel transformation |
| Depth map *processing* (resize, normalize) | zenpipe | Pixel transformation |
| Supplement *geometric tracking* | zenpipe | Pipeline-level concern |
| Pixel conversion (PixelData enum) | Drop — use zenpixels directly | Redundant wrapper |
| Color management (moxcms) | zenpipe | Pipeline-level concern |
| `accept.color_profiles` | zenpipe / decode config | CMS policy, not format selection |

## What moves INTO zencodecs

| Responsibility | From where | Why |
|---|---|---|
| `CodecIntent` + parsing | imageflow (scattered) | Codec selection IS zencodecs' job |
| `QualityIntent` + calibration tables | imageflow (scattered) | Quality mapping IS zencodecs' job |
| `FormatChoice`, `BoolKeep`, `PerCodecHints` | imageflow (scattered) | Types that support codec intent |
| `QualityIntentNode` (zennode) | zennode/nodes.rs | Node wrapper for CodecIntent |
| `Encode` zennode node | New | "Encode to io_id with these settings" |
| `CodecEngine` detection | New | Legacy/Modern engine routing |

## Migration Plan

### Phase 1: Build codec oracle in zencodecs

1. Implement `QualityProfile`, `QualityIntent`, calibration tables in zencodecs
   (porting the proven designs from imageflow's encoder logic)
2. Implement `CodecIntent`, `FormatChoice`, `BoolKeep`, `PerCodecHints`
3. Implement `select_format()`, `ImageFacts`, `FormatDecision`, `SelectionStep`
4. Add `parse_codec_keys()` and `CodecEngine` detection
5. Feature-gate RIAPI parsing behind `feature = "riapi"` (no_std compatible without it)

### Phase 2: Streaming-first API + supplement passthrough

1. Fix streaming encoder lifetime issue (per-codec owned-config factories)
2. Promote `push_decode` + `streaming_encoder` + `transcode` to top-level API
3. Rename existing `decode()` → `decode_full_frame()`, `encode_rgb8()` → `encode_full_frame()`
4. Add `TranscodeOptions` + `SupplementPolicy` + `SupplementSet` + matte
5. Wire supplement extraction/embedding into `transcode()`
6. Remove `PixelData` wrapper (use zenpixels directly)
7. Remove color management stubs (moves to zenpipe)

### Phase 3: Add zennode integration

1. Add `zennode` optional dependency
2. Define `zencodecs.quality_intent` node wrapping CodecIntent
3. Remove `zennode.quality_intent` from zennode
4. Add `Encode` node (format-agnostic: io_id + quality_intent reference)

### Phase 4: Wire into zenpipe + imageflow

1. zenpipe source/sink nodes use zencodecs streaming APIs
2. imageflow key_router partitions keys, passes codec partition to zencodecs
3. imageflow uses `CodecEngine` to route Legacy queries to existing path, Modern to zencodecs
4. Full pipeline streams rows end-to-end without materialization

### Phase 5: CLI tools

1. Build `zen-transcode` binary using zencodecs directly
2. Build `zen-probe` binary for format/supplement/metadata inspection
3. Validate the "zencodecs without zenpipe" use case end-to-end

## What zencodecs is NOT

- NOT a pipeline graph (that's zenpipe)
- NOT a pixel processor (that's zenfilters/zenresize)
- NOT a codec implementation (that's zenjpeg/zenpng/etc.)
- NOT an I/O manager (that's the aggregating crate)
- NOT a pixel format converter (that's zenpixels-convert, though TranscodeSink uses it per-strip)
- NOT a color manager (that's zenpipe + moxcms)
- NOT a supplement *processor* (that's zenpipe — resize, normalize, tone map)
- NOT a query string router (that's imageflow's key_router — zencodecs only parses the codec partition)

It IS:
- The codec **oracle** (format selection, quality calibration, per-codec hints)
- **Streaming codec dispatch** (rows in/out without materialization)
- **Container-faithful I/O** (supplements, metadata, animation — nothing lost in roundtrip)
- **RIAPI codec vocabulary** (parse codec keys, detect feature level, resolve intent)

## Resolved Questions

1. **Format detection:** Lives in zencodec (trait crate) as pure magic-byte matching.
   zencodecs adds registry-aware filtering on top. Both stay.

2. **Codec config re-exports:** Stay. Removes need for direct codec deps.

3. **ImageInfo / probe:** Lives in zencodec. zencodecs adds `probe()` convenience.

4. **Animation:** zencodecs owns per-frame streaming. zenpipe composes into
   multi-stage animated pipelines.

5. **Streaming vs full-frame:** Streaming is the primary API. Full-frame is
   explicitly named (`decode_full_frame`, `encode_full_frame`).

6. **Supplements:** Extraction/embedding = zencodecs. Transformation = zenpipe.
   CLI tools get faithful roundtrip without zenpipe.

7. **Per-codec overrides:** `PerCodecHints` with `BTreeMap<String, String>` per
   codec. Untyped, extensible. Override profile-derived quality.

8. **`format=keep`:** `FormatChoice::Keep` — resolve to source format at
   `select_format()` time using `ImageFacts.source_format`.

9. **`lossless=keep`:** `BoolKeep::Keep` — resolve to source losslessness at
   `select_format()` time using `ImageFacts.source_lossless`.

10. **Matte color:** On `TranscodeOptions` and `FormatDecision`. Applied at encode
    time when compositing RGBA → opaque format.

11. **`accept.color_profiles`:** Not a codec concern. Routes to zenpipe/CMS config.

12. **RIAPI versioning:** `CodecEngine::Legacy/Modern` detected from key presence.
    imageflow uses this to route to existing encoder path vs zencodecs engine.
    Any modern codec key (`qp`, `accept.*`, per-codec hints, `format=auto`,
    supplements, matte) triggers Modern.

13. **`quality` vs `qp`:** `quality=80` → `CodecIntent.quality_fallback`. `qp=high`
    → `CodecIntent.quality_profile`. Profile takes precedence. When neither is set,
    `FormatChoice::Keep` is the default (no auto-selection). When `qp` is set but
    `format` is absent, `FormatChoice::Auto` is implied.
