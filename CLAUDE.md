# zencodecs

Unified image codec abstraction over zenjpeg, zenwebp, zengif, zenavif, png, and ravif.
Optional color management via moxcms.

See `/home/lilith/work/codec-design/README.md` for API design guidelines.
See `/home/lilith/work/zendiff/API_COMPARISON.md` for per-codec convergence status.

## Purpose

A codec dispatch layer for image proxies, CLI tools, and batch processors that handle
multiple formats. Provides:

- **Format detection** and codec dispatch
- **Typed pixel buffers** via `ImgVec<Rgb<u8>>`, `ImgVec<Rgba<u8>>`, etc. (rgb + imgref crates)
- **Runtime codec registry** — callers control which codecs are available at runtime
- **Color management** via moxcms (feature-gated)
- **Streaming decode/encode** — delegates to codec when supported, buffers when not
- **Animation** — frame iteration for animated formats (GIF, animated WebP)
- **Auto-selection** — pick optimal encoder based on image stats and allowed formats
- **Re-export of codec configs** — callers can use format-specific config types for
  fine-grained control, feature-gated behind each codec's feature

Does NOT do resizing, compositing, or image processing — that's `zenimage`.

## Design Rules

**No backwards compatibility required** — we have no external users. Just bump the 0.x major version for breaking changes. No deprecation shims or legacy aliases — delete old APIs. Prefer one obvious way to do things — no duplicate entry points. Minimize API surface for forwards compatibility. Avoid free functions — use methods on types instead.

**Builder convention**: `with_` prefix for consuming builder setters, bare-name for getters.

**Project standards**: `#![forbid(unsafe_code)]` with default features. no_std+alloc (minimum: wasm32). CI with codecov. README with badges and usage examples. As of Rust 1.92, almost everything is in `core::` (including `Error`) — don't assume `std` is needed. Use `wasmtimer` crate for timing on wasm. Fuzz targets required (decode, roundtrip, limits, streaming). Codecs must be safe for malicious input on real-time image proxies — no amplification, bound memory/CPU, periodic DoS/security audits.

## Architecture

```
zencodecs/
├── src/
│   ├── lib.rs            # Public API, re-exports
│   ├── codec_id.rs       # CodecId — identifies specific codec implementations
│   ├── format_set.rs     # FormatSet — bitflag set of ImageFormat values
│   ├── quality.rs        # QualityIntent, QualityProfile, calibration tables
│   ├── policy.rs         # CodecPolicy — per-request killbits/allowlist/preferences
│   ├── select.rs         # Format auto-selection engine (ImageFacts + preferences)
│   ├── trace.rs          # SelectionTrace — audit trail for decisions
│   ├── dyn_dispatch.rs   # Dynamic dispatch via zencodec dyn traits
│   ├── dispatch.rs       # Encoder closure dispatch (BuiltEncoder pattern)
│   ├── error.rs          # CodecError (unified error type)
│   ├── pixel.rs          # Pixel type re-exports (rgb + imgref)
│   ├── config.rs         # CodecConfig struct, format-specific config re-exports
│   ├── limits.rs         # Limits, Stop
│   ├── info.rs           # ImageInfo, probe functions
│   ├── registry.rs       # CodecRegistry — runtime enable/disable + capability queries
│   ├── decode.rs         # DecodeRequest (one-shot, push, animation, gain map, depth map)
│   ├── encode.rs         # EncodeRequest (one-shot, animation, quality profiles, gain map)
│   ├── gainmap.rs        # Format-agnostic gain map types (DecodedGainMap, GainMapSource)
│   ├── depthmap.rs       # Format-agnostic depth map types (DecodedDepthMap, DepthImage, conversions)
│   └── codecs/
│       ├── mod.rs        # Codec adapter modules
│       ├── jpeg.rs       # zenjpeg adapter
│       ├── webp.rs       # zenwebp adapter
│       ├── gif.rs        # zengif adapter
│       ├── png.rs        # zenpng adapter
│       ├── avif_dec.rs   # zenavif adapter
│       ├── avif_enc.rs   # ravif adapter
│       ├── jxl_dec.rs    # zenjxl decoder adapter
│       ├── jxl_enc.rs    # jxl-encoder adapter
│       ├── heic.rs       # heic-decoder adapter
│       ├── raw.rs        # zenraw RAW/DNG adapter (feature: raw-decode)
│       ├── pnm.rs        # zenbitmaps PNM adapter
│       ├── bmp.rs        # zenbitmaps BMP adapter
│       └── farbfeld.rs   # zenbitmaps Farbfeld adapter
├── DESIGN-v2.md          # Full redesign document
├── Cargo.toml
├── CLAUDE.md
├── justfile
└── README.md
```

## Current Implementation (as of 2026-02-07)

### Pixel Handling
- `rgb` and `imgref` are mandatory dependencies (not feature-gated)
- `PixelData` enum replaces old `PixelLayout`: `Rgb8(ImgVec<Rgb<u8>>)`, `Rgba8`, `Rgb16`, `Rgba16`, `RgbF32`, `RgbaF32`, `Gray8`
- `DecodeOutput` contains `pixels: PixelData` + `info: ImageInfo` (no separate width/height/layout)
- Encode uses typed methods: `encode_rgb8(ImgRef<Rgb<u8>>)`, `encode_rgba8(ImgRef<Rgba<u8>>)`

### Codec Configs
- `config.rs` re-exports format-specific config types behind feature gates
- `CodecConfig` struct with `Option<Box<T>>` for each codec's config
- Passed to `EncodeRequest::with_codec_config(&config)`

### Metadata (ICC/EXIF/XMP)
- `ImageInfo` has `icc_profile`, `exif`, `xmp` fields (all `Option<Vec<u8>>`)
- `ImageInfo::metadata()` returns `ImageMetadata` borrowing from the info (for roundtrip convenience)
- `ImageMetadata<'a>` with `icc_profile`, `exif`, `xmp` fields (all `Option<&'a [u8]>`)
- Passed to `EncodeRequest::with_metadata(&meta)`
- **Decode extraction**: JPEG (ICC/EXIF/XMP via zenjpeg extras), WebP (ICC/EXIF/XMP via demuxer), PNG (ICC/EXIF/XMP via iTXt), JXL (ICC from codestream, EXIF/XMP from container Exif/xml boxes), AVIF/GIF (none)
- **Encode embedding**: JPEG (ICC/EXIF/XMP), WebP (ICC/EXIF/XMP via mux), PNG (ICC/EXIF/XMP via iTXt), JXL (ICC/EXIF/XMP via container boxes), AVIF (EXIF only), GIF (none)

### Pixel Conversions
- `PixelData::to_rgb8()` and `to_rgba8()` convert any variant to 8-bit
- `PixelData::as_bytes()` returns raw bytes
- `PixelData::has_alpha()`, `width()`, `height()` accessors

### v2 Additions (2026-03-16)
- **CodecId**: identifies specific codec implementations for policy targeting
- **FormatSet**: public bitflag set of formats (web_safe, modern_web, custom)
- **QualityIntent + QualityProfile**: named quality presets with per-codec calibration tables from imageflow, DPR adjustment
- **CodecPolicy**: per-request killbits, allowlist, preference ordering, format restrictions, composable via merge()
- **SelectionTrace**: audit trail for format/encoder/decoder selection decisions
- **Format auto-selection engine**: imageflow-derived preference hierarchy (JXL > AVIF > JPEG > WebP > PNG with scenario-specific reordering)
- **DecodeRequest::push_decode(sink)**: zero-copy streaming decode via DecodeRowSink
- **DecodeRequest::full_frame_decoder()**: animation decode via DynFullFrameDecoder ('static, data copied)
- **DecodeRequest::probe()**: header-only metadata extraction
- **EncodeRequest::full_frame_encoder(w, h)**: animation encode via DynFullFrameEncoder (GIF, WebP, PNG)
- **EncodeRequest::with_quality_profile()**: named quality presets instead of raw float
- **EncodeRequest::with_dpr()**: device pixel ratio quality adjustment
- **EncodeRequest::with_policy()**: per-request codec/format filtering
- **EncodeRequest::with_image_facts()**: source image properties for auto-selection
- **EncodeRequest::quality_intent()**: accessor for resolved per-codec quality values
- **CodecRegistry capability queries**: streaming_decode_available, animation_decode/encode_available
- Re-exports: DynFullFrameDecoder, DynFullFrameEncoder, DynStreamingDecoder, DecodeRowSink, OutputInfo, OwnedFullFrame, FullFrame

### Gain Map Support (2026-03-18, simplified 2026-03-22)
- **gainmap.rs**: Format-agnostic gain map types (ISO 21496-1)
- **DecodedGainMap**: Thin result struct — `gain_map: GainMap` (ultrahdr-core type) + `metadata: GainMapMetadata` + direction flag (base_is_hdr) + source format
  - `params()`: Convert linear-domain metadata to log2-domain `GainMapParams`
  - `to_gain_map_info()`: Build `GainMapInfo` for zencodec trait layer
  - No reconstruction methods — callers use `ultrahdr_core::apply_gainmap()` directly (LUT-optimized, streaming)
- **GainMapSource**: Pre-computed gain map for encode passthrough
- **params_to_metadata() / metadata_to_params()**: Convert between log2 (GainMapParams) and linear (GainMapMetadata) domains
- **DecodeRequest::decode_gain_map()**: Decode + extract gain map in one call
  - JPEG: UltraHDR XMP + MPF, with Apple MPF fallback for AMPF (iPhone 17 Pro)
  - AVIF: tmap gain map from AV1 auxiliary image
  - JXL: jhgm gain map from JXL codestream
  - DNG/RAW: ISO 21496-1 gain map from embedded preview JPEG's MPF (Apple ProRAW)
  - AMPF: Detected as JPEG, gain map extracted via Apple MPF fallback path
- **EncodeRequest::with_gain_map()**: Builder method to attach gain map source
  - Actual embedding during encode not yet wired (builder only)
- Feature-gated: `jpeg-ultrahdr` (JPEG/AVIF/JXL), `raw-decode-gainmap` (DNG/AMPF)
- 14 unit tests + 7 integration tests + 2 doc-tests + 3 RAW gain map tests

### Depth Map Support (2026-03-17)
- **depthmap.rs**: Format-agnostic depth map types
- **DepthImage**: Raw depth pixel data (Gray8/Gray16/Float32/Float16) with validation
- **DecodedDepthMap**: Depth pixels + metadata + optional confidence map + source info
  - `to_normalized_f32()`: Convert any representation to [0.0, 1.0] range
  - `to_meters()`: Convert to metric depth (returns None for Normalized units)
  - `resize()`: Bilinear interpolation for resolution matching
- **DepthFormat**: RangeLinear, RangeInverse, Disparity, AbsoluteDepth
- **DepthUnits**: Meters, Millimeters, Diopters, Normalized
- **DepthSource**: AndroidGDepth, AndroidDdf, AppleMpf, AppleHeic, Unknown
- **Integer vs float semantics**: Gray8/Gray16 store normalized position for Range formats;
  Float32/Float16 store actual depth/disparity values
- **DecodeRequest::decode_depth_map()**: Decode + extract depth map in one call
  - JPEG: Extracts MPF Disparity secondary image via zenjpeg extras
  - HEIC: Stub for future auxiliary depth image extraction
  - Other formats: Returns None
- Internal f16<->f32 conversion (pure math, no unsafe)
- 30+ unit tests covering all formats, pixel types, f16 roundtrip, resize, edge cases

### RAW/DNG Support (2026-03-18)
- **Feature-gated**: `raw-decode`, `raw-decode-exif`, `raw-decode-xmp` (not in default features — rawloader is LGPL)
- **codecs/raw.rs**: zenraw adapter (decode, probe, preview extraction, metadata)
- **Format detection**: `detect_format()` augmented to check `zenraw::is_raw_file()` after common registry
- **ImageFormat::Custom**: RAW/DNG use `ImageFormat::Custom(&DNG_FORMAT)` / `Custom(&RAW_FORMAT)` — matched via `def.name == "dng" || "raw"` in dispatch
- **CodecId::ZenrawDecode**: New decoder ID for RAW/DNG
- **CodecConfig::raw_decoder**: Optional `Box<RawDecodeConfig>` for demosaic method, gamma, crop, orientation
- **config::raw module**: Re-exports `RawDecodeConfig`, `DemosaicMethod`
- **DecodeRequest::extract_raw_preview()**: Extract embedded JPEG preview from DNG/RAW (feature: raw-decode-exif)
- **DecodeRequest::read_raw_metadata()**: Read structured EXIF+DNG metadata via zenraw's kamadak-exif (feature: raw-decode-exif)
- **exif::from_raw_metadata()**: Convert `zenraw::exif::ExifMetadata` → `ExifData` (feature: raw-decode-exif)
- **ExifData DNG fields**: `dng_version`, `unique_camera_model`, `color_matrix_1`/`_2`, `forward_matrix_1`/`_2`, `analog_balance`, `as_shot_neutral`, `as_shot_white_xy`, `baseline_exposure`, `calibration_illuminant_1`/`_2`
- **EXIF parser**: IFD0 now parses all DNG tags (0xC612-0xC715) into ExifData
- Wired into: decode dispatch, info probe, dyn_dispatch (push_decode, full_frame_decoder), codec_id mapping
- Re-exports: `RawDecodeConfig`, `RawDecoderConfig` from lib.rs
- 4 new DNG EXIF tests (dng_version, unique_camera_model, color_matrix+white_balance, as_shot_white_xy)

### What's NOT implemented yet
- Pull-based streaming decode (DynStreamingDecoder + 'a lifetime blocked by GAT config borrow — use push_decode instead)
- Color management (moxcms)
- Fallback chains (structural pieces ready, needs multi-decoder-per-format registry)
- Registry v2 entries with factories (deferred — current match-based dispatch works)
- Gain map encode embedding (with_gain_map builder exists but doesn't wire to actual encode yet)
- JPEG GDepth XMP depth extraction (requires XMP parsing in zenjpeg)
- JPEG Android DDF depth extraction (requires container directory parsing)
- HEIC auxiliary depth image extraction (requires heic-decoder auxid support)

## Public API Spec (Design Intent)

### Runtime Codec Registry

Compile-time features control which codecs are *available*. The registry controls
which are *enabled* for a given operation. This lets image proxies restrict codecs
per-request (e.g., disable AVIF for clients that don't support it).

```rust
#[derive(Clone, Debug)]
pub struct CodecRegistry {
    // Bitflags or small vec of enabled formats
    decode_enabled: FormatSet,
    encode_enabled: FormatSet,
}

impl CodecRegistry {
    /// All compiled-in codecs enabled.
    pub fn all() -> Self;

    /// Nothing enabled — caller must opt in.
    pub fn none() -> Self;

    pub fn with_decode(self, format: ImageFormat, enabled: bool) -> Self;
    pub fn with_encode(self, format: ImageFormat, enabled: bool) -> Self;

    /// Is this format available (compiled in) AND enabled?
    pub fn can_decode(&self, format: ImageFormat) -> bool;
    pub fn can_encode(&self, format: ImageFormat) -> bool;

    /// Formats that are both compiled in and enabled.
    pub fn decodable_formats(&self) -> impl Iterator<Item = ImageFormat>;
    pub fn encodable_formats(&self) -> impl Iterator<Item = ImageFormat>;
}

impl Default for CodecRegistry {
    fn default() -> Self { Self::all() }
}
```

Requests accept an optional registry. When omitted, all compiled-in codecs are used.

### Format Detection

```rust
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Jpeg,
    WebP,
    Gif,
    Png,
    Avif,
}

impl ImageFormat {
    /// Detect format from magic bytes. Returns None if unrecognized.
    pub fn detect(data: &[u8]) -> Option<Self>;

    /// Detect format from file extension (case-insensitive).
    pub fn from_extension(ext: &str) -> Option<Self>;

    /// MIME type string.
    pub fn mime_type(&self) -> &'static str;

    /// Common file extensions.
    pub fn extensions(&self) -> &'static [&'static str];

    /// Whether this format supports lossy encoding.
    pub fn supports_lossy(&self) -> bool;

    /// Whether this format supports lossless encoding.
    pub fn supports_lossless(&self) -> bool;

    /// Whether this format supports animation.
    pub fn supports_animation(&self) -> bool;

    /// Whether this format supports alpha channel.
    pub fn supports_alpha(&self) -> bool;
}
```

### Probing

```rust
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    pub format: ImageFormat,
    pub has_alpha: bool,
    pub has_animation: bool,
    pub frame_count: Option<u32>,  // None if unknown without full parse
    pub icc_profile: Option<alloc::vec::Vec<u8>>,
}

impl ImageInfo {
    /// Probe image metadata without decoding pixels.
    /// Uses the registry to restrict which formats are attempted.
    pub fn from_bytes(data: &[u8]) -> Result<Self, CodecError>;

    pub fn from_bytes_with_registry(
        data: &[u8],
        registry: &CodecRegistry,
    ) -> Result<Self, CodecError>;
}
```

### Decoding (One-Shot)

**Note: Implemented API differs — no output_layout (decoder returns native format), no color_management yet.**

```rust
pub struct DecodeRequest<'a> {
    data: &'a [u8],
    format: Option<ImageFormat>,   // None = auto-detect
    limits: Option<&'a Limits>,
    stop: Option<&'a dyn Stop>,
    registry: Option<&'a CodecRegistry>,
    // color_management: ColorIntent, // planned
    // Format-specific config overrides (feature-gated)
    #[cfg(feature = "jpeg")]
    jpeg_config: Option<&'a zenjpeg::DecoderConfig>,
    #[cfg(feature = "webp")]
    webp_config: Option<&'a zenwebp::DecodeConfig>,
    // ... etc
}

impl<'a> DecodeRequest<'a> {
    pub fn new(data: &'a [u8]) -> Self;

    pub fn with_format(self, format: ImageFormat) -> Self;
    pub fn with_output_layout(self, layout: PixelLayout) -> Self;
    pub fn with_limits(self, limits: &'a Limits) -> Self;
    pub fn with_stop(self, stop: &'a dyn Stop) -> Self;
    pub fn with_registry(self, registry: &'a CodecRegistry) -> Self;
    pub fn with_color_intent(self, intent: ColorIntent) -> Self;

    /// Use a format-specific decoder config for fine-grained control.
    #[cfg(feature = "jpeg")]
    pub fn with_jpeg_config(self, config: &'a zenjpeg::DecoderConfig) -> Self;
    #[cfg(feature = "webp")]
    pub fn with_webp_config(self, config: &'a zenwebp::DecodeConfig) -> Self;
    // ... etc for each codec

    /// Decode first frame to pixels.
    pub fn decode(self) -> Result<DecodeOutput, CodecError>;

    /// Decode into a pre-allocated buffer.
    pub fn decode_into(self, output: &mut [u8]) -> Result<ImageInfo, CodecError>;

    /// Start streaming decode (for animated or progressive).
    pub fn build(self) -> Result<StreamingDecoder<'a>, CodecError>;
}

pub struct DecodeOutput {
    pub pixels: alloc::vec::Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub layout: PixelLayout,
    pub info: ImageInfo,
}
```

### Streaming Decode

```rust
pub struct StreamingDecoder<'a> {
    // Internal: wraps format-specific decoder or buffers for codecs
    // that don't support streaming
}

impl<'a> StreamingDecoder<'a> {
    /// Image metadata (available after build).
    pub fn info(&self) -> &ImageInfo;

    /// Decode next frame. Returns None when all frames consumed.
    /// For single-frame formats, returns one frame then None.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, CodecError>;
}

pub struct Frame {
    pub pixels: alloc::vec::Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub layout: PixelLayout,
    pub delay_ms: u32,       // 0 for still images
    pub frame_index: u32,
}
```

### Encoding

```rust
pub struct EncodeRequest<'a> {
    format: Option<ImageFormat>,   // None = auto-select
    quality: Option<f32>,          // 0-100, format-mapped
    effort: Option<u32>,           // speed/quality tradeoff
    lossless: bool,
    limits: Option<&'a Limits>,
    stop: Option<&'a dyn Stop>,
    metadata: Option<&'a ImageMetadata<'a>>,
    registry: Option<&'a CodecRegistry>,
    color_management: ColorIntent,
    // Format-specific config overrides (feature-gated)
    #[cfg(feature = "jpeg")]
    jpeg_config: Option<&'a zenjpeg::EncoderConfig>,
    #[cfg(feature = "webp")]
    webp_lossy_config: Option<&'a zenwebp::LossyConfig>,
    #[cfg(feature = "webp")]
    webp_lossless_config: Option<&'a zenwebp::LosslessConfig>,
    // ... etc
}

impl<'a> EncodeRequest<'a> {
    /// Encode to a specific format.
    pub fn new(format: ImageFormat) -> Self;

    /// Auto-select best format based on image stats and allowed encoders.
    /// Uses the registry to determine which formats are candidates.
    pub fn auto() -> Self;

    pub fn with_quality(self, quality: f32) -> Self;
    pub fn with_effort(self, effort: u32) -> Self;
    pub fn with_lossless(self, lossless: bool) -> Self;
    pub fn with_limits(self, limits: &'a Limits) -> Self;
    pub fn with_stop(self, stop: &'a dyn Stop) -> Self;
    pub fn with_metadata(self, meta: &'a ImageMetadata<'a>) -> Self;
    pub fn with_registry(self, registry: &'a CodecRegistry) -> Self;
    pub fn with_color_intent(self, intent: ColorIntent) -> Self;

    /// Format-specific config for fine-grained control.
    #[cfg(feature = "jpeg")]
    pub fn with_jpeg_config(self, config: &'a zenjpeg::EncoderConfig) -> Self;
    #[cfg(feature = "webp")]
    pub fn with_webp_lossy_config(self, config: &'a zenwebp::LossyConfig) -> Self;
    // ... etc

    /// Encode pixels.
    pub fn encode(
        self,
        pixels: &[u8],
        width: u32,
        height: u32,
        layout: PixelLayout,
    ) -> Result<EncodeOutput, CodecError>;

    /// Encode into a pre-allocated buffer.
    pub fn encode_into(
        self,
        pixels: &[u8],
        width: u32,
        height: u32,
        layout: PixelLayout,
        output: &mut alloc::vec::Vec<u8>,
    ) -> Result<EncodeOutput, CodecError>;

    /// Start streaming encode (for animation or row-push).
    pub fn build(self, width: u32, height: u32, layout: PixelLayout)
        -> Result<StreamingEncoder<'a>, CodecError>;
}

pub struct EncodeOutput {
    pub data: alloc::vec::Vec<u8>,
    pub format: ImageFormat,  // Useful when auto-selected
}

pub struct StreamingEncoder<'a> {
    // Wraps format-specific streaming encoder, or buffers internally
    // for codecs that require full image (like WebP)
}

impl<'a> StreamingEncoder<'a> {
    /// Push a frame (for animation encoding).
    pub fn add_frame(&mut self, frame: &Frame) -> Result<(), CodecError>;

    /// Finish and return encoded bytes.
    pub fn finish(self) -> Result<EncodeOutput, CodecError>;

    /// Finish into pre-allocated buffer.
    pub fn finish_into(self, output: &mut alloc::vec::Vec<u8>)
        -> Result<EncodeOutput, CodecError>;
}
```

### Auto-Selection

When `EncodeRequest::auto()` is used, the encoder picks the best format based on:

1. **Registry**: Only formats enabled in the registry are candidates.
2. **Image stats**: Has alpha? → exclude JPEG. Is photographic? → prefer JPEG/WebP/AVIF.
   Is flat color / screenshots? → prefer WebP lossless or PNG.
3. **Lossless flag**: If lossless requested, exclude lossy-only formats.
4. **Quality target**: Some formats are better at low quality (AVIF) vs high (JPEG).

The selection logic should be simple, deterministic, and documented. It is NOT a
quality optimizer — it picks a reasonable default, not the optimal format. Callers
who need optimal selection should use `codec-eval` or their own heuristics.

```rust
// Auto-select: "give me a good lossy encoding"
let output = EncodeRequest::auto()
    .with_quality(80.0)
    .with_registry(&registry)  // only JPEG and WebP allowed
    .encode(&pixels, w, h, PixelLayout::Rgba8)?;
println!("Selected: {:?}", output.format);

// Auto-select with lossless preference
let output = EncodeRequest::auto()
    .with_lossless(true)
    .encode(&pixels, w, h, PixelLayout::Rgba8)?;
```

### Color Management

```rust
/// How to handle ICC profiles during decode/encode.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default)]
pub enum ColorIntent {
    /// Don't touch pixel data. Pass ICC profile through as metadata.
    #[default]
    PreserveBytes,

    /// Convert to sRGB during decode, embed sRGB profile on encode.
    /// Requires the `moxcms` feature.
    #[cfg(feature = "moxcms")]
    ConvertToSrgb,

    /// Convert to a specific ICC profile during decode.
    /// Requires the `moxcms` feature.
    #[cfg(feature = "moxcms")]
    ConvertTo { profile: &'static [u8] }, // TODO: lifetime design
}
```

Color management is opt-in. Default is `PreserveBytes` — pixel bytes are untouched,
ICC profiles are passed through as metadata. When `moxcms` feature is enabled, callers
can request conversion to sRGB or a specific profile.

### Shared Types

```rust
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelLayout {
    Rgb8,
    Rgba8,
    Bgr8,
    Bgra8,
}

#[derive(Clone, Debug, Default)]
pub struct Limits {
    pub max_width: Option<u64>,
    pub max_height: Option<u64>,
    pub max_pixels: Option<u64>,
    pub max_memory_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct ImageMetadata<'a> {
    pub icc_profile: Option<&'a [u8]>,
    pub exif: Option<&'a [u8]>,
    pub xmp: Option<&'a [u8]>,
}

/// Unified error type.
#[derive(Debug)]
#[non_exhaustive]
pub enum CodecError {
    /// Format not recognized from magic bytes.
    UnrecognizedFormat,
    /// Format recognized but codec not compiled in or not enabled in registry.
    UnsupportedFormat(ImageFormat),
    /// Format doesn't support requested operation.
    UnsupportedOperation { format: ImageFormat, detail: &'static str },
    /// Codec not enabled in the provided registry.
    DisabledFormat(ImageFormat),
    /// Input validation failed.
    InvalidInput(alloc::string::String),
    /// Resource limit exceeded.
    LimitExceeded(alloc::string::String),
    /// Operation cancelled via Stop token.
    Cancelled,
    /// Allocation failure.
    Oom,
    /// No suitable encoder found for auto-selection.
    NoSuitableEncoder,
    /// Color management error.
    #[cfg(feature = "moxcms")]
    ColorManagement(alloc::string::String),
    /// Underlying codec error.
    Codec { format: ImageFormat, source: alloc::boxed::Box<dyn core::error::Error + Send + Sync> },
}
```

### Re-exports (Feature-Gated)

Each codec's public config types are re-exported so callers can use them for
fine-grained control without adding the codec crate as a direct dependency:

```rust
/// Format-specific config types for advanced usage.
/// These are re-exports from the underlying codec crates.
#[cfg(feature = "jpeg")]
pub mod jpeg {
    pub use zenjpeg::{EncoderConfig, DecoderConfig, EncodeRequest, Limits as JpegLimits};
}

#[cfg(feature = "webp")]
pub mod webp {
    pub use zenwebp::{LossyConfig, LosslessConfig, DecodeConfig, PixelLayout as WebpPixelLayout};
}

#[cfg(feature = "gif")]
pub mod gif {
    pub use zengif::{EncoderConfig, EncodeRequest as GifEncodeRequest};
}

#[cfg(feature = "avif-decode")]
pub mod avif {
    pub use zenavif::{DecoderConfig as AvifDecoderConfig};
}

#[cfg(feature = "avif-encode")]
pub mod avif_enc {
    pub use ravif::Encoder as RavifEncoder;
}

#[cfg(feature = "png")]
pub mod png_codec {
    // Relevant png crate types
}
```

## Internal Codec Adapter Pattern

Each `codecs/*.rs` file implements a thin adapter. The adapter is responsible for:

1. **Pixel format conversion**: RGBA8/BGRA8 always works. A=255 on decode for
   alpha-less formats, ignore A on encode.

2. **Quality mapping**: Map 0-100 to codec's native scale. Document the mapping.

3. **Error wrapping**: `CodecError::Codec { format, source }`.

4. **Limits/Stop forwarding**: Pass through to underlying codec.

5. **Streaming**: If codec supports streaming natively, delegate. If not (e.g., WebP
   needs full image), buffer internally in the StreamingEncoder/StreamingDecoder and
   flush on finish. Document which codecs buffer.

6. **Animation**: GIF and animated WebP support frame iteration natively.
   JPEG/PNG/AVIF return a single frame then None.

### Streaming Buffering Policy

| Codec | Decode Streaming | Encode Streaming |
|-------|-----------------|-----------------|
| JPEG (zenjpeg) | Native row-push | Native row-push |
| WebP (zenwebp) | Buffered (needs full data) | Buffered (needs full image) |
| GIF (zengif) | Native frame iteration | Native frame-push |
| PNG (png) | Native row-push | Native row-push |
| AVIF (zenavif) | Buffered | Buffered |

"Buffered" means zencodecs accumulates data/frames internally and dispatches to the
codec's one-shot API when `finish()` is called. This is transparent to the caller.

## Quality Mapping Table

| zencodecs quality | JPEG (zenjpeg) | WebP lossy (zenwebp) | AVIF (ravif) | PNG | GIF |
|-------------------|----------------|----------------------|--------------|-----|-----|
| 0-100 | 0-100 (native) | 0-100 (native) | 0-100 (ravif native) | N/A | N/A |
| lossless=true | Error | WebP lossless | Error (no lossless) | Default | Default |

## Auto-Selection Logic

```
fn select_format(pixels, w, h, layout, quality, lossless, registry) -> ImageFormat:
    if lossless:
        candidates = [WebP, Png].filter(|f| registry.can_encode(f))
        if has_alpha: prefer WebP lossless (better compression)
        else: prefer PNG for screenshots, WebP lossless for photos
        return first available candidate

    candidates = [Jpeg, WebP, Avif, Png].filter(|f| registry.can_encode(f))

    if has_alpha:
        remove Jpeg from candidates
        prefer WebP > Avif > Png

    if is_photographic (high entropy, many colors):
        prefer Avif > WebP > Jpeg (by quality/size)
    else (flat, few colors):
        prefer WebP > Png

    return first match from preference order
```

The `is_photographic` heuristic should be simple — unique color count, edge density,
or similar. Not a full classifier. Document the heuristic precisely.

## Implementation Order

1. **format.rs**: Magic byte detection, format enum — no codec deps
2. **error.rs**: CodecError enum
3. **pixel.rs**: PixelLayout
4. **limits.rs**: Limits, ImageMetadata
5. **registry.rs**: CodecRegistry with compile-time + runtime gating
6. **info.rs**: ImageInfo + probe
7. **decode.rs**: DecodeRequest + DecodeOutput (one-shot first)
8. **encode.rs**: EncodeRequest + EncodeOutput (one-shot first)
9. **codecs/jpeg.rs**: First adapter (zenjpeg closest to target API)
10. **codecs/webp.rs**: Second adapter
11. **codecs/png.rs**: Third adapter
12. **codecs/gif.rs**: Fourth adapter
13. **codecs/avif_dec.rs**: Fifth
14. **codecs/avif_enc.rs**: Sixth
15. **animation.rs**: StreamingDecoder with frame iteration
16. **Streaming encode**: StreamingEncoder with buffering for non-streaming codecs
17. **Auto-selection**: `EncodeRequest::auto()` with simple heuristics
18. **color.rs**: moxcms integration (feature-gated)

## Underlying Codec Status

| Codec | Crate | API Maturity | Streaming | Animation | Notes |
|-------|-------|-------------|-----------|-----------|-------|
| JPEG | zenjpeg | High | Encode: row-push | No | Three-layer, Limits, EncodeStats |
| WebP | zenwebp | High | No (full image) | Encode+Decode | All convergence done 2026-02-06 |
| GIF | zengif | Medium | Encode: frame-push | Yes | EncodeRequest done, modularized |
| PNG | png 0.18 | External | Row-push both | No (APNG?) | Well-maintained |
| AVIF decode | zenavif | Low | No | No | Needs API convergence |
| AVIF encode | ravif 0.13 | External | No | No | Own API, quality mapping needed |

## Known Issues

(none yet — crate not implemented)

## User Feedback Log

See [FEEDBACK.md](FEEDBACK.md) if it exists.
