# zencodecs v2 Design

Registry + policy + quality mapping + format selection + streaming dispatch over zencodec traits.

Replaces ~3,950 lines in zenimage. No new trait system — dispatches to `DynEncoderConfig` / `DynDecoderConfig` from zencodec.

## What Changes

| Current | v2 |
|---------|-----|
| `CodecRegistry` = bitflags (enable/disable) | `CodecRegistry` = entries with ID, priority, capabilities, factories |
| One decoder/encoder per format | Multiple per format, priority-ordered |
| `CodecConfig` bundles codec-specific options | Quality mapping + extensions on job |
| `dispatch.rs` `BuiltEncoder` closure | Registry lookup → factory → `DynEncoderConfig` → job |
| `auto_select_format` (12 lines) | Format preference engine with image facts + policy |
| No policy | `CodecPolicy` with killbits, allowlist, preferences |
| No quality profiles | `QualityIntent` with profiles, DPR adjustment, calibration tables |
| No streaming exposure | `DecodeRequest::streaming_decoder()`, `push_decoder()` |
| No animation exposure | `DecodeRequest::full_frame_decoder()`, `EncodeRequest::full_frame_encoder()` |
| No fallback | Ordered fallback chain on decode error |
| No decision tracing | `SelectionTrace` with steps |

## What Stays

- `ImageFormat` (from zencodec)
- `CodecError` (extended, not replaced)
- `DecodeRequest` / `EncodeRequest` builder pattern (internals change)
- `EncodeOutput` / `DecodeOutput` (from zencodec)
- Feature-gated codec knowledge (adapters become registration functions)
- `AnyEncoder` trait (kept for simple one-shot use)
- `Limits` struct and `Stop` re-export

---

## 1. Codec Identity

```rust
/// Identifies a specific codec implementation.
///
/// Each format may have multiple implementations (e.g., zenjpeg vs zune-jpeg
/// for JPEG decode). Policy targets these IDs.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CodecId {
    // JPEG
    ZenjpegDecode,
    ZenjpegEncode,

    // WebP
    ZenwebpDecode,
    ZenwebpEncode,

    // GIF
    ZengifDecode,
    ZengifEncode,

    // PNG
    PngDecode,       // png crate
    PngEncode,

    // AVIF
    ZenavifDecode,
    RavifEncode,

    // JXL
    ZenjxlDecode,
    JxlEncoderEncode,

    // HEIC
    HeicDecode,

    // Bitmaps
    PnmDecode,
    PnmEncode,
    BmpDecode,
    BmpEncode,
    FarbfeldDecode,
    FarbfeldEncode,

    /// Third-party codec. String must be unique.
    Custom(&'static str),
}

impl CodecId {
    pub fn format(&self) -> ImageFormat;
    pub fn is_decoder(&self) -> bool;
    pub fn is_encoder(&self) -> bool;
    pub fn name(&self) -> &'static str;  // "zenjpeg (decode)"
}
```

Minimal — only codecs we actually ship. `Custom` for anything else.

---

## 2. Codec Entries

### Encoder Entry

```rust
/// A registered encoder implementation.
pub struct EncoderEntry {
    pub id: CodecId,
    pub format: ImageFormat,
    pub priority: i32,                              // higher = preferred
    pub capabilities: EncodeCapabilities,            // from zencodec
    pub supported_descriptors: &'static [PixelDescriptor],
    factory: EncoderFactory,
}

/// Creates a configured DynEncoderConfig from generic encoding parameters.
///
/// The factory encapsulates codec-specific quality mapping. Given a generic
/// quality 0-100, it produces a config with the correct codec-native setting.
type EncoderFactory = Box<dyn Fn(&EncodeIntent) -> Result<Box<dyn DynEncoderConfig>>
    + Send + Sync>;
```

The factory is the bridge between generic parameters and codec-specific configs. Each codec's registration function builds a factory closure that:
1. Takes `EncodeIntent` (quality, effort, lossless)
2. Maps generic quality to codec-native quality via calibration table
3. Constructs the codec's `EncoderConfig` with mapped parameters
4. Returns it as `Box<dyn DynEncoderConfig>`

### Decoder Entry

```rust
/// A registered decoder implementation.
pub struct DecoderEntry {
    pub id: CodecId,
    pub format: ImageFormat,
    pub priority: i32,
    pub capabilities: DecodeCapabilities,           // from zencodec
    pub supported_descriptors: &'static [PixelDescriptor],
    factory: DecoderFactory,
}

/// Creates a DynDecoderConfig. Decoders have fewer tuning knobs than
/// encoders — most configuration is on the job, not the config.
type DecoderFactory = Box<dyn Fn() -> Box<dyn DynDecoderConfig> + Send + Sync>;
```

---

## 3. Registry

```rust
/// Runtime codec registry. Built at startup, shared immutably.
///
/// Holds encoder and decoder entries sorted by priority within each format.
/// Feature gates control which codecs are *compiled in*. The registry
/// controls which are *available*. Policy controls which are *used*.
#[derive(Clone)]
pub struct CodecRegistry {
    decoders: Vec<DecoderEntry>,   // sorted: format, then descending priority
    encoders: Vec<EncoderEntry>,   // sorted: format, then descending priority
}

impl CodecRegistry {
    /// All compiled-in codecs, default priorities.
    pub fn defaults() -> Self;

    /// Empty — caller registers everything manually.
    pub fn empty() -> Self;

    /// Register an encoder.
    pub fn register_encoder(&mut self, entry: EncoderEntry);

    /// Register a decoder.
    pub fn register_decoder(&mut self, entry: DecoderEntry);

    // --- Query ---

    /// All decoder entries for a format, descending priority.
    pub fn decoders_for(&self, format: ImageFormat) -> &[DecoderEntry];

    /// All encoder entries for a format, descending priority.
    pub fn encoders_for(&self, format: ImageFormat) -> &[EncoderEntry];

    /// Best decoder for format (highest priority, no policy filter).
    pub fn best_decoder(&self, format: ImageFormat) -> Option<&DecoderEntry>;

    /// Best encoder for format.
    pub fn best_encoder(&self, format: ImageFormat) -> Option<&EncoderEntry>;

    /// Formats with at least one decoder.
    pub fn decodable_formats(&self) -> impl Iterator<Item = ImageFormat>;

    /// Formats with at least one encoder.
    pub fn encodable_formats(&self) -> impl Iterator<Item = ImageFormat>;

    /// Can any registered decoder handle this format?
    pub fn can_decode(&self, format: ImageFormat) -> bool;

    /// Can any registered encoder handle this format?
    pub fn can_encode(&self, format: ImageFormat) -> bool;

    /// Does any registered decoder for this format support streaming?
    pub fn streaming_decode_available(&self, format: ImageFormat) -> bool;

    /// Does any registered encoder for this format support animation?
    pub fn animation_encode_available(&self, format: ImageFormat) -> bool;

    /// Detect format from magic bytes.
    pub fn detect_format(&self, data: &[u8]) -> Option<ImageFormat>;
}
```

### Default Registration

Each codec feature gate provides a registration function:

```rust
// In codecs/jpeg.rs (feature = "jpeg")
pub(crate) fn register(registry: &mut CodecRegistry) {
    registry.register_decoder(DecoderEntry {
        id: CodecId::ZenjpegDecode,
        format: ImageFormat::Jpeg,
        priority: 100,
        capabilities: zenjpeg::DecoderConfig::default().capabilities(),
        supported_descriptors: zenjpeg::DecoderConfig::supported_descriptors(),
        factory: Box::new(|| Box::new(zenjpeg::DecoderConfig::default())),
    });

    registry.register_encoder(EncoderEntry {
        id: CodecId::ZenjpegEncode,
        format: ImageFormat::Jpeg,
        priority: 100,
        capabilities: zenjpeg::EncoderConfig::default().capabilities(),
        supported_descriptors: zenjpeg::EncoderConfig::supported_descriptors(),
        factory: Box::new(|intent| {
            let quality = intent.jpeg_quality(); // calibration table lookup
            let mut config = zenjpeg::EncoderConfig::new()
                .with_quality(quality);
            if let Some(effort) = intent.effort {
                config = config.with_optimize_coding(effort >= 5);
            }
            Ok(Box::new(config))
        }),
    });
}
```

`CodecRegistry::defaults()` calls all feature-gated `register()` functions.

---

## 4. Policy

```rust
/// Per-request codec filtering. Composes with the registry to control
/// which codecs are available for a specific operation.
#[derive(Clone, Debug, Default)]
pub struct CodecPolicy {
    /// Explicitly disabled codec IDs. Checked first — kills override allows.
    disabled: SmallVec<[CodecId; 4]>,

    /// If Some, only these IDs are allowed. If None, all non-disabled are allowed.
    allowed: Option<SmallVec<[CodecId; 8]>>,

    /// Per-format preference order. Position 0 = +1000 bonus, 1 = +900, etc.
    /// Applied on top of base priority.
    preferences: SmallVec<[(ImageFormat, SmallVec<[CodecId; 2]>); 4]>,

    /// Allow fallback to next decoder on error? Default: true.
    fallback_on_error: bool,

    /// Allowed output formats for auto-selection. None = all registered.
    allowed_formats: Option<FormatSet>,
}

impl CodecPolicy {
    pub fn new() -> Self;

    // --- Builder ---
    pub fn with_disabled(self, id: CodecId) -> Self;
    pub fn with_allowed(self, ids: &[CodecId]) -> Self;
    pub fn with_preference(self, format: ImageFormat, order: &[CodecId]) -> Self;
    pub fn with_fallback(self, enabled: bool) -> Self;
    pub fn with_allowed_formats(self, formats: FormatSet) -> Self;

    // --- Presets ---
    /// Only pure-Rust codecs (no C/C++ dependencies).
    pub fn pure_rust() -> Self;
    /// Only codecs safe for wasm32 (no threading, no C).
    pub fn wasm_compatible() -> Self;
    /// Web-safe output formats only (JPEG, PNG, GIF).
    pub fn web_safe_output() -> Self;
    /// Modern web output (JPEG, PNG, GIF, WebP, AVIF).
    pub fn modern_web_output() -> Self;

    // --- Query ---
    pub fn is_allowed(&self, id: CodecId) -> bool;
    pub fn effective_priority(&self, entry_id: CodecId, base_priority: i32, format: ImageFormat) -> i32;
    pub fn is_format_allowed(&self, format: ImageFormat) -> bool;

    // --- Composition ---
    /// Merge two policies. Disabled = union. Allowed = intersect.
    /// Preferences = other overrides self. Fallback = disabled if either disables.
    pub fn merge(self, other: CodecPolicy) -> Self;
}

/// Bitflag set of ImageFormat values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatSet(u16);

impl FormatSet {
    pub fn empty() -> Self;
    pub fn all() -> Self;
    pub fn web_safe() -> Self;      // JPEG, PNG, GIF
    pub fn modern_web() -> Self;    // + WebP, AVIF, JXL
    pub fn insert(&mut self, format: ImageFormat);
    pub fn remove(&mut self, format: ImageFormat);
    pub fn contains(&self, format: ImageFormat) -> bool;
    pub fn iter(&self) -> impl Iterator<Item = ImageFormat>;
}
```

### Policy-Filtered Selection

The registry + policy produce a filtered, priority-sorted list:

```rust
impl CodecRegistry {
    /// Decoders for format, filtered and re-sorted by policy.
    pub fn select_decoders(
        &self,
        format: ImageFormat,
        policy: &CodecPolicy,
    ) -> SmallVec<[&DecoderEntry; 4]>;

    /// Best encoder for format after policy.
    pub fn select_encoder(
        &self,
        format: ImageFormat,
        policy: &CodecPolicy,
    ) -> Option<&EncoderEntry>;

    /// Encodable formats after policy filtering.
    pub fn available_encode_formats(
        &self,
        policy: &CodecPolicy,
    ) -> impl Iterator<Item = ImageFormat>;
}
```

---

## 5. Quality Mapping

### Quality Intent

```rust
/// Generic encoding quality parameters. Codec-agnostic.
#[derive(Clone, Debug)]
pub struct QualityIntent {
    /// Generic quality 0.0-100.0. Mapped to per-codec native scales.
    pub quality: f32,
    /// Speed/quality tradeoff. Higher = slower + better. Codec-mapped.
    pub effort: Option<u32>,
    /// Force lossless encoding.
    pub lossless: bool,
}
```

### Quality Profiles

```rust
/// Named quality presets with tuned per-codec mappings.
/// Values from imageflow's perceptual calibration.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum QualityProfile {
    Lowest,      // ~15
    Low,         // ~20
    MediumLow,   // ~34
    Medium,      // ~55
    Good,        // ~73 (default)
    High,        // ~91
    Highest,     // ~96
    Lossless,    // 100
}

impl QualityProfile {
    /// Generic quality value for this profile.
    pub fn generic_quality(&self) -> f32;

    /// Convert to QualityIntent.
    pub fn to_intent(&self) -> QualityIntent;

    /// Apply DPR (device pixel ratio) adjustment.
    /// Baseline: 3.0. Lower DPR → higher quality (upscaled → artifacts visible).
    pub fn to_intent_with_dpr(&self, dpr: f32) -> QualityIntent;
}
```

### DPR Adjustment

```rust
/// Adjust quality for device pixel ratio.
///
/// At DPR 1.0, the browser displays each source pixel at 3x3 screen pixels
/// (relative to baseline 3.0), magnifying artifacts. Quality increases.
/// At DPR 6.0, each source pixel is 0.5x0.5 screen pixels. Quality decreases.
///
/// Adjustment is perceptual, not linear:
///   factor = 3.0 / dpr.clamp(0.1, 12.0)
///   adjusted = 100.0 - (100.0 - base) / factor
pub fn adjust_quality_for_dpr(base_quality: f32, dpr: f32) -> f32;
```

### Encode Intent (Internal)

```rust
/// Resolved encoding parameters, used by encoder factories.
/// Contains both generic quality and pre-computed per-codec values.
pub(crate) struct EncodeIntent {
    pub quality: f32,          // generic 0-100
    pub effort: Option<u32>,
    pub lossless: bool,

    // Pre-computed per-codec values from calibration tables.
    // Factories call these instead of doing their own mapping.
}

impl EncodeIntent {
    pub fn from_quality(quality: f32) -> Self;
    pub fn from_profile(profile: QualityProfile) -> Self;
    pub fn from_profile_dpr(profile: QualityProfile, dpr: f32) -> Self;

    // --- Per-codec quality lookups (interpolated from calibration tables) ---
    pub fn jpeg_quality(&self) -> u8;        // 0-100
    pub fn webp_quality(&self) -> f32;       // 0-100
    pub fn webp_method(&self) -> u8;         // 0-6
    pub fn avif_quality(&self) -> f32;       // 0-100
    pub fn avif_speed(&self) -> u8;          // 0-10
    pub fn jxl_distance(&self) -> f32;       // butteraugli, 0-25
    pub fn jxl_effort(&self) -> u8;          // 1-9
    pub fn png_quality_range(&self) -> (u8, u8);  // (min, max) for quantization
    pub fn gif_quality(&self) -> u8;         // quantization quality
}
```

### Calibration Tables

Ported from imageflow's `codec_decisions.rs`. Anchor points with linear interpolation:

```
Profile     Generic  JPEG  WebP  WebP_m  AVIF  AVIF_s  JXL_d   JXL_e  PNG_min PNG_max
Lowest      15.0     15    15    0       10    10      15.0    1      0       30
Low         20.0     20    20    1       12    9       12.0    2      5       40
MediumLow   34.0     34    34    3       22    8       7.0     3      15      60
Medium      55.0     57    53    5       45    6       4.0     4      30      80
Good        73.0     73    76    6       55    6       2.58    5      50      100
High        91.0     91    93    6       78    4       1.0     7      80      100
Highest     96.0     96    96    6       90    3       0.3     8      90      100
Lossless    100.0    100   100   6       100   3       0.0     9      100     100
```

`Percent(n)` quality interpolates linearly between the two nearest anchor rows.

---

## 6. Format Auto-Selection

```rust
/// Facts about the image being encoded. Drives format selection.
#[derive(Clone, Debug)]
pub struct ImageFacts {
    pub has_alpha: bool,
    pub has_animation: bool,
    pub is_lossless_source: bool,   // source was PNG/GIF/lossless WebP
    pub pixel_count: u64,           // width * height
    pub is_hdr: bool,               // PQ/HLG/linear >1.0
}

impl ImageFacts {
    /// Derive from DecodeOutput metadata.
    pub fn from_decode_output(output: &DecodeOutput) -> Self;
    /// Derive from ImageInfo.
    pub fn from_image_info(info: &ImageInfo) -> Self;
}
```

### Selection Algorithm

```rust
impl CodecRegistry {
    /// Auto-select the best output format.
    ///
    /// Considers: image facts, policy restrictions, registered encoders,
    /// lossless preference. Returns format + trace of the decision.
    pub fn select_format(
        &self,
        facts: &ImageFacts,
        intent: &QualityIntent,
        policy: &CodecPolicy,
    ) -> Result<FormatSelection>;
}

pub struct FormatSelection {
    pub format: ImageFormat,
    pub trace: SelectionTrace,
}
```

**Algorithm** (from imageflow, adapted):

```
1. Collect candidate formats = registered encoders ∩ policy.allowed_formats

2. If intent.lossless:
   - JXL (if available) → WebP → PNG → AVIF
   - Skip formats that don't support lossless

3. If facts.has_animation:
   - Lossless: WebP → GIF
   - Lossy: AVIF → WebP → GIF
   - Skip formats that don't support animation

4. If facts.has_alpha:
   - JXL → AVIF → WebP → PNG
   - Skip JPEG (no alpha)

5. If facts.pixel_count < 3_000_000 (small):
   - JXL → AVIF → JPEG → WebP → PNG

6. Else (large):
   - JXL → JPEG → AVIF → WebP → PNG
   - (AVIF demoted: slower for large images)

7. First candidate that passes all filters wins.
   If none: return CodecError::NoSuitableEncoder.
```

Each step emits `SelectionStep` entries for tracing.

---

## 7. Decision Tracing

```rust
/// Audit trail for codec/format selection decisions.
#[derive(Clone, Debug, Default)]
pub struct SelectionTrace {
    steps: SmallVec<[SelectionStep; 8]>,
}

#[derive(Clone, Debug)]
pub enum SelectionStep {
    FormatChosen { format: ImageFormat, reason: &'static str },
    FormatSkipped { format: ImageFormat, reason: &'static str },
    EncoderChosen { id: CodecId, priority: i32, reason: &'static str },
    EncoderSkipped { id: CodecId, reason: &'static str },
    DecoderChosen { id: CodecId, priority: i32, reason: &'static str },
    DecoderSkipped { id: CodecId, reason: &'static str },
    DecoderFailed { id: CodecId, error: String },
    FallbackAttempt { from: CodecId, to: CodecId },
    Info { message: &'static str },
}

impl SelectionTrace {
    pub fn steps(&self) -> &[SelectionStep];
    pub fn chosen_format(&self) -> Option<ImageFormat>;
    pub fn chosen_encoder(&self) -> Option<CodecId>;
    pub fn chosen_decoder(&self) -> Option<CodecId>;
}
```

---

## 8. Decode API

### One-Shot (Current, Enhanced)

```rust
pub struct DecodeRequest<'a> {
    data: &'a [u8],
    format: Option<ImageFormat>,
    limits: Option<&'a Limits>,
    stop: Option<&'a dyn Stop>,
    registry: &'a CodecRegistry,        // required (was optional)
    policy: CodecPolicy,                // NEW
    preferred: SmallVec<[PixelDescriptor; 4]>,  // NEW: pixel format preference
    crop_hint: Option<(u32, u32, u32, u32)>,    // NEW
    orientation: Option<OrientationHint>,         // NEW
}

impl<'a> DecodeRequest<'a> {
    pub fn new(data: &'a [u8], registry: &'a CodecRegistry) -> Self;

    // --- Builders (existing + new) ---
    pub fn with_format(self, format: ImageFormat) -> Self;
    pub fn with_limits(self, limits: &'a Limits) -> Self;
    pub fn with_stop(self, stop: &'a dyn Stop) -> Self;
    pub fn with_policy(self, policy: CodecPolicy) -> Self;       // NEW
    pub fn with_preferred(self, desc: &[PixelDescriptor]) -> Self; // NEW
    pub fn with_crop_hint(self, x: u32, y: u32, w: u32, h: u32) -> Self; // NEW
    pub fn with_orientation(self, hint: OrientationHint) -> Self; // NEW

    // --- One-shot decode (existing) ---
    pub fn decode(self) -> Result<DecodeOutput>;

    // --- Typed convenience (existing) ---
    pub fn decode_into_rgb8(self, dst: ImgRefMut<Rgb<u8>>) -> Result<ImageInfo>;
    pub fn decode_into_rgba8(self, dst: ImgRefMut<Rgba<u8>>) -> Result<ImageInfo>;
    // ... etc

    // --- NEW: Streaming decode ---
    /// Returns a streaming decoder that yields pixel batches.
    /// Falls back to buffered one-shot for codecs without native streaming.
    pub fn streaming_decoder(self) -> Result<Box<dyn DynStreamingDecoder + 'a>>;

    /// Push-based decode: decoder writes rows into the sink.
    pub fn push_decode(self, sink: &mut dyn DecodeRowSink) -> Result<OutputInfo>;

    // --- NEW: Animation ---
    /// Returns a full-frame decoder for animated images.
    /// For single-frame images, yields one frame then None.
    pub fn full_frame_decoder(self) -> Result<Box<dyn DynFullFrameDecoder>>;

    // --- NEW: Probe ---
    /// Probe without decoding. Cheaper than decode().
    pub fn probe(&self) -> Result<ImageInfo>;

    // --- NEW: Trace ---
    /// Probe + return which decoder would be selected.
    pub fn probe_with_trace(&self) -> Result<(ImageInfo, SelectionTrace)>;
}
```

### Fallback Chain

When `policy.fallback_on_error` is true (default), decode tries decoders in priority order:

```
1. Detect format (or use explicit)
2. Get decoder list: registry.select_decoders(format, &policy)
3. For each decoder in list:
   a. Create config via factory
   b. Create job, configure (limits, stop, crop, orientation)
   c. Attempt decode
   d. On success: return result + trace
   e. On error: record in trace, try next
4. If all fail: return last error + full trace
```

For streaming/push/animation, no fallback — the first decoder is used. Fallback
is only practical for one-shot decode where retrying is cheap.

---

## 9. Encode API

### One-Shot (Current, Enhanced)

```rust
pub struct EncodeRequest<'a> {
    format: Option<ImageFormat>,        // None = auto-select
    quality: Option<f32>,               // generic 0-100
    quality_profile: Option<QualityProfile>,  // NEW: named profile
    dpr: Option<f32>,                   // NEW: device pixel ratio
    effort: Option<u32>,
    lossless: bool,
    limits: Option<&'a Limits>,
    stop: Option<&'a dyn Stop>,
    metadata: Option<&'a Metadata>,
    registry: &'a CodecRegistry,        // required (was optional)
    policy: CodecPolicy,                // NEW
    image_facts: Option<ImageFacts>,    // NEW: for auto-selection
}

impl<'a> EncodeRequest<'a> {
    /// Encode to a specific format.
    pub fn new(format: ImageFormat, registry: &'a CodecRegistry) -> Self;

    /// Auto-select best format based on image facts + policy.
    pub fn auto(registry: &'a CodecRegistry) -> Self;

    // --- Builders (existing + new) ---
    pub fn with_quality(self, quality: f32) -> Self;
    pub fn with_quality_profile(self, profile: QualityProfile) -> Self;  // NEW
    pub fn with_dpr(self, dpr: f32) -> Self;                              // NEW
    pub fn with_effort(self, effort: u32) -> Self;
    pub fn with_lossless(self, lossless: bool) -> Self;
    pub fn with_limits(self, limits: &'a Limits) -> Self;
    pub fn with_stop(self, stop: &'a dyn Stop) -> Self;
    pub fn with_metadata(self, meta: &'a Metadata) -> Self;
    pub fn with_policy(self, policy: CodecPolicy) -> Self;               // NEW
    pub fn with_image_facts(self, facts: ImageFacts) -> Self;            // NEW

    // --- One-shot encode (existing) ---
    pub fn encode_rgb8(self, img: ImgRef<Rgb<u8>>) -> Result<EncodeOutput>;
    pub fn encode_rgba8(self, img: ImgRef<Rgba<u8>>) -> Result<EncodeOutput>;
    pub fn encode_srgba8_imgref(self, img: ImgRef<Rgba<u8>>, ignore_alpha: bool) -> Result<EncodeOutput>;
    // ... etc for other pixel types

    // --- NEW: Animation ---
    /// Create a full-frame encoder for multi-frame output.
    pub fn full_frame_encoder(self, width: u32, height: u32)
        -> Result<Box<dyn DynFullFrameEncoder>>;

    // --- NEW: Trace ---
    /// Encode + return selection trace.
    pub fn encode_rgb8_traced(self, img: ImgRef<Rgb<u8>>)
        -> Result<(EncodeOutput, SelectionTrace)>;
}
```

### Encode Flow

```
1. Resolve quality:
   - quality_profile + dpr → QualityIntent (via calibration tables)
   - OR quality (raw f32) → QualityIntent
   - OR default (Good profile) → QualityIntent

2. Resolve format:
   - Explicit → use it
   - Auto → registry.select_format(facts, intent, policy)
     - facts derived from pixels if not provided

3. Select encoder:
   - registry.select_encoder(format, &policy)
   - Error if none available

4. Create config:
   - entry.factory(&intent) → Box<dyn DynEncoderConfig>

5. Create job:
   - config.dyn_job()
   - job.set_stop(stop)
   - job.set_metadata(metadata)
   - job.set_limits(limits)

6. Negotiate pixel format:
   - Check entry.supported_descriptors vs input
   - adapt_for_encode() if needed (existing logic)

7. Encode:
   - job.into_encoder().encode(pixels)
   - OR job.into_full_frame_encoder() for animation
```

---

## 10. Streaming

### Streaming Decode

Passes through to zencodec's `StreamingDecode` trait:

```rust
// Usage:
let registry = CodecRegistry::defaults();
let mut decoder = DecodeRequest::new(&jpeg_data, &registry)
    .streaming_decoder()?;

// Pull strips on demand:
while let Some((y, pixels)) = decoder.next_batch()? {
    pipeline.feed(y, &pixels);
}
```

Internally:
1. Select decoder via registry + policy
2. Create config → job
3. Call `job.into_streaming_decoder(data, preferred)`
4. Return the `Box<dyn DynStreamingDecoder>`

For codecs without native streaming (WebP, AVIF), zencodec's job implementation
already handles buffering internally — the caller sees the same interface.

### Streaming Encode

Not yet available in underlying codecs (except zenjpeg's `push_rows`). When it is:

```rust
let mut encoder = EncodeRequest::new(ImageFormat::Jpeg, &registry)
    .with_quality(85.0)
    .streaming_encoder(width, height)?;

for strip in strips {
    encoder.push_rows(strip)?;
}
let output = encoder.finish()?;
```

### Capability Query

```rust
// Can I stream-decode this format?
if registry.streaming_decode_available(ImageFormat::Jpeg) {
    // Use streaming path
} else {
    // Use one-shot
}
```

---

## 11. Animation

### Decode

```rust
let registry = CodecRegistry::defaults();
let mut decoder = DecodeRequest::new(&gif_data, &registry)
    .full_frame_decoder()?;

let info = decoder.info();
println!("{}x{}, {} frames", info.width, info.height,
    info.sequence.frame_count().unwrap_or(0));

while let Some(frame) = decoder.render_next_frame_owned(None)? {
    process_frame(&frame.pixels, frame.duration_ms);
}
```

### Encode

```rust
let registry = CodecRegistry::defaults();
let mut encoder = EncodeRequest::new(ImageFormat::Gif, &registry)
    .with_quality(80.0)
    .full_frame_encoder(width, height)?;

for frame in frames {
    encoder.push_frame(frame.pixels.as_pixel_slice(), frame.delay_ms, None)?;
}
let output = encoder.finish(None)?;
```

---

## 12. Backward Compatibility & Migration

### Phase 1: Registry Internals (Non-Breaking)

Replace internal dispatch with registry-based lookup. Keep existing public API.
- `CodecRegistry::defaults()` replaces `CodecRegistry::all()`
- Internally, `decode_format()` and `build_encoder()` use registry entries
- `CodecConfig` still works (mapped to extensions)

### Phase 2: Policy + Quality (Additive)

Add new API surface alongside existing:
- `DecodeRequest::with_policy()`
- `EncodeRequest::with_quality_profile()`
- `EncodeRequest::with_dpr()`
- Existing `with_quality(f32)` still works (bypasses profiles)

### Phase 3: Streaming + Animation (Additive)

New methods on existing types:
- `DecodeRequest::streaming_decoder()`
- `DecodeRequest::full_frame_decoder()`
- `DecodeRequest::push_decode()`
- `EncodeRequest::full_frame_encoder()`

### Phase 4: Format Auto-Selection (Enhanced)

Upgrade auto-selection from 12-line heuristic to full preference engine.
- `EncodeRequest::auto()` uses new algorithm
- `EncodeRequest::with_image_facts()` for caller-provided hints

### Phase 5: Deprecation

After zenimage migrates:
- Deprecate `CodecConfig` (use quality profiles + extensions)
- Deprecate `EncodeRequest::with_codec_config()`
- Deprecate typed decode methods like `decode_into_rgb8()` (use `with_preferred()` + `decode()`)

### Breaking Change: Registry Required

The biggest breaking change: `DecodeRequest::new()` and `EncodeRequest::new()` require
`&CodecRegistry`. Currently optional. This is correct — the registry is the source
of truth for what's available.

For convenience:
```rust
/// Module-level shorthand using a lazily-initialized default registry.
pub fn decode(data: &[u8]) -> Result<DecodeOutput> {
    DecodeRequest::new(data, &CodecRegistry::defaults()).decode()
}

pub fn encode_jpeg(img: ImgRef<Rgb<u8>>, quality: f32) -> Result<EncodeOutput> {
    EncodeRequest::new(ImageFormat::Jpeg, &CodecRegistry::defaults())
        .with_quality(quality)
        .encode_rgb8(img)
}
```

---

## 13. Implementation Order

1. **`CodecId`** + **`FormatSet`** — simple enums, no codec deps
2. **`SelectionTrace`** — standalone type
3. **`QualityIntent`** + calibration tables — standalone, unit-testable
4. **`CodecPolicy`** — standalone, unit-testable
5. **`EncoderEntry`** / **`DecoderEntry`** — factory types
6. **`CodecRegistry` v2** — internals, keeping old public API working
7. **Registration functions** — one per codec, feature-gated
8. **Wire decode dispatch** through registry — replace `decode_format()` match
9. **Wire encode dispatch** through registry — replace `build_encoder()` match
10. **`DecodeRequest::with_policy()`** — policy filtering
11. **`EncodeRequest::with_quality_profile()` + DPR** — quality mapping
12. **Format auto-selection v2** — preference engine
13. **`DecodeRequest::streaming_decoder()`** — streaming exposure
14. **`DecodeRequest::full_frame_decoder()`** — animation decode
15. **`EncodeRequest::full_frame_encoder()`** — animation encode
16. **`DecodeRequest::push_decode()`** — push-based decode
17. **Fallback chains** — multi-decoder retry on error
18. **Traced variants** — `*_traced()` methods

Steps 1-9 can be done without breaking the public API.
Steps 10-18 are additive.

---

## Appendix: What zenimage Drops

After zencodecs v2 is complete, zenimage replaces:

| zenimage module | Lines | Replaced by |
|-----------------|-------|-------------|
| `auto_codec.rs` | ~1,100 | `QualityIntent` + `select_format()` |
| `registry.rs` | ~500 | `CodecRegistry` + entries |
| `policy.rs` | ~300 | `CodecPolicy` |
| `codec_id.rs` | ~200 | `CodecId` |
| `zenjpeg.rs` | ~700 | `streaming_decoder()` / `push_decode()` |
| `image_png/` | ~400 | `streaming_decoder()` / `push_decode()` |
| `webp/` | ~350 | `full_frame_decoder()` |
| `gif/` | ~300 | `full_frame_decoder()` |
| `avif/` | ~400 | `full_frame_decoder()` |
| **Total** | **~4,250** | |

zenimage keeps: `types.rs`, `traits.rs` (strip abstraction), `strip.rs`,
`limits.rs` (megapixel conversion), `animated_sink.rs`, `codec_types_bridge.rs` (shrinks).
