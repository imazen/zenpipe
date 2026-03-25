# zencodecs API Design

> **Historical document.** This early API proposal was superseded before implementation. See README.md for the current API.

Unified codec API for zenjpeg, zenwebp, zengif, zenavif, png, ravif.

## Crate Architecture

```
zencodec  (traits + shared types, tiny)
    ^          ^          ^          ^
zenjpeg    zenwebp    zengif    zenavif    (implement traits)
    ^          ^          ^          ^
              zencodecs                    (dispatch + format-specific entry points)
```

- **`zencodec`** — Defines `Encoding`, `Decoding` traits and shared types (`EncodeOutput`,
  `DecodeOutput`, `PixelData`, `ImageInfo`, `ImageMetadata`). Tiny crate, stable, rarely changes.
- **`zen*` codecs** — Each implements `zencodec::Encoding` / `zencodec::Decoding` on their
  own config types. Format-specific methods live on the concrete types, not the trait.
- **`zencodecs`** — Multi-format dispatch. Provides `encoding_jpeg()`, `encoding_webp()` etc.
  that return the concrete codec types. Also provides `encoding(format)` for runtime dispatch
  and `decode()` / `probe()` convenience functions.

## Design Principles

- **Short default path, full control when needed** — one-liner for common cases, builder chain for advanced
- **Config is reusable, Job is per-operation** — Config has no lifetimes (storable), Job borrows (temporary)
- **`.job()` bridges the two** — type system enforces the lifetime boundary
- **Pixel type is the terminal method** — `encode_rgb8(img)` provides data AND executes
- **Trait for the common surface** — `Encoding` / `Decoding` traits let you abstract over codecs when useful
- **Concrete types for format-specific features** — `encoding_jpeg()` returns `zenjpeg::EncodeConfig` with JPEG-specific methods

## Entry Points (zencodecs)

### Free Functions

```rust
// ── One-shot convenience ──
pub fn decode(data: &[u8]) -> Result<DecodeOutput, CodecError>;
pub fn probe(data: &[u8]) -> Result<ImageInfo, CodecError>;

// ── Config creation: format-specific (returns concrete codec type) ──
pub fn encoding_jpeg() -> zenjpeg::EncodeConfig;
pub fn encoding_webp() -> zenwebp::EncodeConfig;
pub fn encoding_png()  -> png_adapter::EncodeConfig;
pub fn encoding_gif()  -> zengif::EncodeConfig;
pub fn encoding_avif() -> avif_adapter::EncodeConfig;

pub fn decoding_jpeg() -> zenjpeg::DecodeConfig;
pub fn decoding_webp() -> zenwebp::DecodeConfig;
// ... etc

// ── Config creation: runtime dispatch (returns zencodecs wrapper) ──
pub fn encoding(format: ImageFormat) -> EncodeConfig;  // zencodecs::EncodeConfig
pub fn decoding() -> DecodeConfig;                      // zencodecs::DecodeConfig
```

### Choosing Your Abstraction Level

```rust
// Level 1: Format-specific — full access to JPEG features
let bytes = encoding_jpeg()
    .with_quality(90.0)
    .with_progressive(true)      // JPEG-only, not on trait
    .with_sharp_yuv(true)        // JPEG-only
    .encode_rgb8(img.as_ref())?  // from Encoding trait
    .into_vec();

// Level 2: Generic over codecs — only trait methods
fn compress(config: &impl Encoding, img: ImgRef<Rgb<u8>>) -> Result<Vec<u8>> {
    Ok(config.encode_rgb8(img)?.into_vec())
}
compress(&encoding_jpeg().with_quality(85.0), img.as_ref())?;
compress(&encoding_webp().with_quality(80.0), img.as_ref())?;

// Level 3: Runtime dispatch — format chosen at runtime
let config = encoding(format).with_quality(80.0);
let bytes = config.encode_rgb8(img.as_ref())?.into_vec();
```

## Usage Chains (Shortest to Fullest)

```rust
// ═══ DECODE ═══

// One-liner
let img = decode(&data)?.into_rgb8();

// With limits
let img = decoding()
    .with_limit_pixels(100_000_000)
    .decode(&data)?
    .into_rgb8();

// Full control
let img = decoding()
    .with_limit_pixels(100_000_000)
    .with_limit_memory(512_000_000)
    .job()
    .with_stop(&stop)
    .decode(&data)?
    .into_rgb8();

// Format-specific decode config
let img = decoding_jpeg()
    .with_strictness(Strictness::Lenient)   // JPEG-only
    .job()
    .with_stop(&stop)
    .decode(&data)?
    .into_rgb8();

// Streaming (animation)
let mut dec = decoding()
    .job()
    .with_stop(&stop)
    .decoder(&data)?;
while let Some(frame) = dec.next_frame()? {
    process(frame.into_rgba8(), frame.delay_ms());
}

// ═══ ENCODE ═══

// One-liner
let bytes = encoding_jpeg().encode_rgb8(img.as_ref())?.into_vec();

// With quality
let bytes = encoding_jpeg()
    .with_quality(90.0)
    .encode_rgb8(img.as_ref())?
    .into_vec();

// Full control (format-specific)
let bytes = encoding_jpeg()
    .with_quality(90.0)
    .with_effort(8)
    .with_progressive(true)       // JPEG-only
    .with_sharp_yuv(true)         // JPEG-only
    .job()
    .with_metadata(&decoded.metadata())
    .with_stop(&stop)
    .encode_rgb8(img.as_ref())?
    .into_vec();

// Runtime format dispatch
let output = encoding(format)
    .with_quality(80.0)
    .encode_rgb8(img.as_ref())?;
println!("format: {:?}, {} bytes", output.format(), output.len());

// Auto-select format
let output = encoding(Auto)
    .with_quality(80.0)
    .encode_rgb8(img.as_ref())?;

// Streaming (animation)
let mut enc = encoding_gif()
    .with_quality(80)
    .with_dithering(0.5)          // GIF-only
    .job()
    .with_stop(&stop)
    .encoder(width, height)?;
for frame in frames {
    enc.add_frame_rgba8(frame.as_ref(), 100)?;
}
let bytes = enc.finish()?.into_vec();

// ═══ PROBE ═══

let info = probe(&data)?;
println!("{}x{} {:?}", info.width(), info.height(), info.format());

// ═══ ROUNDTRIP ═══

let decoded = decode(&data)?;
let meta = decoded.metadata();
let bytes = encoding_webp()
    .with_quality(80.0)
    .job()
    .with_metadata(&meta)
    .encode_rgb8(decoded.as_rgb8().unwrap())?
    .into_vec();

// ═══ REUSABLE CONFIG (server pattern) ═══

struct ImageProxy {
    jpeg_high: zenjpeg::EncodeConfig,    // concrete type, has JPEG methods
    jpeg_low: zenjpeg::EncodeConfig,
    webp: zenwebp::EncodeConfig,         // concrete type, has WebP methods
    decode: zencodecs::DecodeConfig,     // dispatch type
}

impl ImageProxy {
    fn new() -> Self {
        Self {
            jpeg_high: encoding_jpeg().with_quality(90.0).with_effort(8).with_progressive(true),
            jpeg_low: encoding_jpeg().with_quality(60.0).with_effort(4),
            webp: encoding_webp().with_quality(80.0),
            decode: decoding()
                .with_limit_pixels(100_000_000)
                .with_limit_memory(512_000_000),
        }
    }

    fn transcode(&self, data: &[u8], stop: &dyn Stop) -> Result<Vec<u8>> {
        let decoded = self.decode.job().with_stop(stop).decode(data)?;
        let meta = decoded.metadata();
        Ok(self.jpeg_high.job()
            .with_metadata(&meta)
            .with_stop(stop)
            .encode_rgb8(decoded.as_rgb8().unwrap())?
            .into_vec())
    }

    // Generic helper — works with any stored config
    fn encode_with(&self, config: &impl Encoding, img: ImgRef<Rgb<u8>>, stop: &dyn Stop)
        -> Result<Vec<u8>>
    {
        Ok(config.job().with_stop(stop).encode_rgb8(img)?.into_vec())
    }
}
```

## Why `.job()` Exists

Config and Job are separate types for a fundamental reason: **lifetimes**.

```
EncodeConfig                    EncodeJob<'a>
(no lifetimes, Clone, storable) (borrows things with lifetime 'a)
┌───────────────────────┐      ┌──────────────────────────┐
│ quality: Option<f32>  │      │ config: &'a EncodeConfig  │
│ effort: Option<u32>   │─────>│ stop: Option<&'a dyn Stop>│
│ lossless: bool        │.job()│ metadata: Option<&'a ...> │
│ ...                   │      │ limit overrides           │
└───────────────────────┘      └──────────┬───────────────┘
  Storable in structs              .encode_rgb8(img)?
  Shareable across threads           (terminal, consumes Job)
  Clone for reuse
```

Without `.job()`, you'd need lifetimes on the config, which prevents storing it.
The short path skips `.job()` entirely — convenience terminals on `&EncodeConfig`
create a default Job internally.

## `zencodec` Trait Crate

### What Lives Here

Small, stable crate. Dependencies: `imgref`, `rgb`, `enough`, `alloc`.

**Traits:**
- `Encoding` — common encode config interface
- `Decoding` — common decode config interface
- `EncodingJob` — common per-operation encode interface
- `DecodingJob` — common per-operation decode interface

**Shared types:**
- `EncodeOutput`, `DecodeOutput`, `DecodeFrame`
- `PixelData` enum
- `ImageInfo`, `ImageMetadata`
- `ImageFormat` enum
- Re-exports: `imgref::{ImgRef, ImgVec}`, `rgb::{Rgb, Rgba, Gray}`, `enough::{Stop, Unstoppable}`

**NOT here:**
- `CodecError` — each codec has its own error type (associated type on trait)
- `CodecRegistry` — zencodecs-only concept
- Format-specific enums (`Preset`, `Quantizer`, etc.) — live in their codecs

### Trait Definitions

```rust
// ── zencodec::Encoding ──

pub trait Encoding: Sized + Clone {
    type Error: core::error::Error + Send + Sync + 'static;
    type Job<'a>: EncodingJob<'a, Error = Self::Error> where Self: 'a;

    // Config builder methods
    fn with_quality(self, q: f32) -> Self;
    fn with_effort(self, e: u32) -> Self;
    fn with_lossless(self, lossless: bool) -> Self;
    fn with_alpha_quality(self, q: f32) -> Self;
    fn with_limit_pixels(self, max: u64) -> Self;
    fn with_limit_memory(self, bytes: u64) -> Self;
    fn with_limit_output(self, bytes: u64) -> Self;

    // Job creation
    fn job(&self) -> Self::Job<'_>;

    // Convenience terminals (create default Job internally)
    fn encode_rgb8(&self, img: ImgRef<Rgb<u8>>) -> Result<EncodeOutput, Self::Error>;
    fn encode_rgba8(&self, img: ImgRef<Rgba<u8>>) -> Result<EncodeOutput, Self::Error>;
    fn encode_gray8(&self, img: ImgRef<Gray<u8>>) -> Result<EncodeOutput, Self::Error>;
}


// ── zencodec::EncodingJob ──

pub trait EncodingJob<'a>: Sized {
    type Error: core::error::Error + Send + Sync + 'static;

    fn with_stop(self, s: &'a dyn Stop) -> Self;
    fn with_metadata(self, m: &'a ImageMetadata<'a>) -> Self;
    fn with_icc(self, icc: &'a [u8]) -> Self;
    fn with_exif(self, exif: &'a [u8]) -> Self;
    fn with_xmp(self, xmp: &'a [u8]) -> Self;
    fn with_limit_pixels(self, max: u64) -> Self;    // override config
    fn with_limit_memory(self, bytes: u64) -> Self;   // override config

    fn encode_rgb8(self, img: ImgRef<'_, Rgb<u8>>) -> Result<EncodeOutput, Self::Error>;
    fn encode_rgba8(self, img: ImgRef<'_, Rgba<u8>>) -> Result<EncodeOutput, Self::Error>;
    fn encode_gray8(self, img: ImgRef<'_, Gray<u8>>) -> Result<EncodeOutput, Self::Error>;
}


// ── zencodec::Decoding ──

pub trait Decoding: Sized + Clone {
    type Error: core::error::Error + Send + Sync + 'static;
    type Job<'a>: DecodingJob<'a, Error = Self::Error> where Self: 'a;

    // Config builder methods
    fn with_limit_pixels(self, max: u64) -> Self;
    fn with_limit_memory(self, bytes: u64) -> Self;
    fn with_limit_dimensions(self, w: u32, h: u32) -> Self;
    fn with_limit_file_size(self, bytes: u64) -> Self;

    // Job creation
    fn job(&self) -> Self::Job<'_>;

    // Convenience terminals
    fn decode(&self, data: &[u8]) -> Result<DecodeOutput, Self::Error>;
    fn probe(&self, data: &[u8]) -> Result<ImageInfo, Self::Error>;
}


// ── zencodec::DecodingJob ──

pub trait DecodingJob<'a>: Sized {
    type Error: core::error::Error + Send + Sync + 'static;

    fn with_stop(self, s: &'a dyn Stop) -> Self;
    fn with_limit_pixels(self, max: u64) -> Self;    // override config
    fn with_limit_memory(self, bytes: u64) -> Self;   // override config

    fn decode(self, data: &[u8]) -> Result<DecodeOutput, Self::Error>;
}
```

### How Codecs Implement It

```rust
// In zenjpeg:
impl zencodec::Encoding for zenjpeg::EncodeConfig {
    type Error = zenjpeg::Error;
    type Job<'a> = zenjpeg::EncodeJob<'a>;

    fn with_quality(self, q: f32) -> Self { self.with_quality(q) }
    fn encode_rgb8(&self, img: ImgRef<Rgb<u8>>) -> Result<EncodeOutput, Self::Error> {
        // delegate to native API, wrap output
    }
    // ...
}

// JPEG-specific methods are NOT on the trait — they're on the concrete type:
impl zenjpeg::EncodeConfig {
    pub fn with_progressive(self, b: bool) -> Self { ... }
    pub fn with_sharp_yuv(self, b: bool) -> Self { ... }
    pub fn with_chroma(self, c: ChromaSubsampling) -> Self { ... }
    pub fn with_trellis(self, t: TrellisConfig) -> Self { ... }
}
```

### Using the Trait Generically

```rust
use zencodec::Encoding;

// Works with any codec
fn compress_with(config: &impl Encoding, img: ImgRef<Rgb<u8>>) -> Vec<u8> {
    config.encode_rgb8(img).expect("encode failed").into_vec()
}

// Works with any codec + cancellation
fn compress_with_stop<'a>(
    config: &'a impl Encoding,
    img: ImgRef<'_, Rgb<u8>>,
    stop: &'a dyn Stop,
) -> Result<Vec<u8>, Box<dyn core::error::Error>> {
    Ok(config.job().with_stop(stop).encode_rgb8(img)?.into_vec())
}

// Call with any codec:
compress_with(&encoding_jpeg().with_quality(85.0), img);
compress_with(&encoding_webp().with_quality(80.0), img);
compress_with(&encoding(Jpeg).with_quality(85.0), img);  // dispatch type also implements trait
```

## Full Method Reference

### Free Functions (zencodecs)

| Function | Signature | Returns | Notes |
|---|---|---|---|
| `decode` | `fn decode(data: &[u8]) -> Result<DecodeOutput>` | `DecodeOutput` | Auto-detect, default config |
| `probe` | `fn probe(data: &[u8]) -> Result<ImageInfo>` | `ImageInfo` | Header-only |
| `encoding` | `fn encoding(format: ImageFormat) -> EncodeConfig` | dispatch `EncodeConfig` | Runtime format selection |
| `decoding` | `fn decoding() -> DecodeConfig` | dispatch `DecodeConfig` | Runtime format detection |
| `encoding_jpeg` | `fn encoding_jpeg() -> zenjpeg::EncodeConfig` | concrete type | JPEG-specific methods available |
| `encoding_webp` | `fn encoding_webp() -> zenwebp::EncodeConfig` | concrete type | WebP-specific methods available |
| `encoding_png` | `fn encoding_png() -> png_adapter::EncodeConfig` | concrete type | |
| `encoding_gif` | `fn encoding_gif() -> zengif::EncodeConfig` | concrete type | GIF-specific methods available |
| `encoding_avif` | `fn encoding_avif() -> avif_adapter::EncodeConfig` | concrete type | AVIF-specific methods available |
| `decoding_jpeg` | `fn decoding_jpeg() -> zenjpeg::DecodeConfig` | concrete type | JPEG-specific methods available |
| `decoding_webp` | `fn decoding_webp() -> zenwebp::DecodeConfig` | concrete type | |
| etc. | | | |

### ImageFormat

```rust
#[non_exhaustive]
pub enum ImageFormat { Jpeg, WebP, Gif, Png, Avif, Jxl }
```

`encoding(Auto)` is not a format variant — it's a constructor on zencodecs' dispatch
`EncodeConfig` that enables auto-selection.

### Trait Methods (from `zencodec`)

See [Trait Definitions](#trait-definitions) above. These are the methods available on
ALL codec configs, including the zencodecs dispatch type.

### Dispatch DecodeConfig (zencodecs)

Methods beyond the `Decoding` trait:

| Method | Signature | Notes |
|---|---|---|
| `with_registry` | `fn with_registry(self, r: &CodecRegistry) -> Self` | Restrict which formats are tried |

### Dispatch EncodeConfig (zencodecs)

Methods beyond the `Encoding` trait:

| Method | Signature | Notes |
|---|---|---|
| `auto` | `fn auto() -> Self` | Auto-select best format |
| `with_registry` | `fn with_registry(self, r: &CodecRegistry) -> Self` | Restrict candidate formats |

### Format-Specific Config Methods (NOT on trait)

#### zenjpeg::EncodeConfig

| Method | Signature | Notes |
|---|---|---|
| `with_progressive` | `fn with_progressive(self, b: bool) -> Self` | Progressive JPEG |
| `with_sharp_yuv` | `fn with_sharp_yuv(self, b: bool) -> Self` | Better chroma edges |
| `with_chroma` | `fn with_chroma(self, c: ChromaSubsampling) -> Self` | 4:4:4 / 4:2:2 / 4:2:0 |
| `with_content_hint` | `fn with_content_hint(self, h: ContentHint) -> Self` | Photo/Drawing/Text |

#### zenwebp::EncodeConfig

| Method | Signature | Notes |
|---|---|---|
| `with_sharp_yuv` | `fn with_sharp_yuv(self, b: bool) -> Self` | Better chroma edges |
| `with_preset` | `fn with_preset(self, p: Preset) -> Self` | Photo/Drawing/Icon/Text |
| `with_near_lossless` | `fn with_near_lossless(self, v: u8) -> Self` | 0-100 (100=lossless) |
| `with_sns_strength` | `fn with_sns_strength(self, s: u8) -> Self` | Spatial noise shaping |

#### zengif::EncodeConfig

| Method | Signature | Notes |
|---|---|---|
| `with_dithering` | `fn with_dithering(self, d: f32) -> Self` | 0.0-1.0 |
| `with_quantizer` | `fn with_quantizer(self, q: Quantizer) -> Self` | imagequant/quantizr/etc |
| `with_lossy_tolerance` | `fn with_lossy_tolerance(self, t: u8) -> Self` | Frame differencing fuzz |
| `with_repeat` | `fn with_repeat(self, r: Repeat) -> Self` | Loop count |

#### avif_adapter::EncodeConfig

| Method | Signature | Notes |
|---|---|---|
| `with_bit_depth` | `fn with_bit_depth(self, d: BitDepth) -> Self` | 8/10/12/Auto |
| `with_chroma` | `fn with_chroma(self, c: ChromaSubsampling) -> Self` | 4:4:4 / 4:2:0 |
| `with_color_model` | `fn with_color_model(self, m: ColorModel) -> Self` | YCbCr / RGB |

#### zenjpeg::DecodeConfig

| Method | Signature | Notes |
|---|---|---|
| `with_strictness` | `fn with_strictness(self, s: Strictness) -> Self` | Strict/Balanced/Lenient |
| `with_output_target` | `fn with_output_target(self, t: OutputTarget) -> Self` | Srgb8/SrgbF32/LinearF32 |
| `with_auto_orient` | `fn with_auto_orient(self, b: bool) -> Self` | Apply EXIF rotation |

#### zenwebp::DecodeConfig

| Method | Signature | Notes |
|---|---|---|
| `with_upsampling` | `fn with_upsampling(self, m: UpsamplingMethod) -> Self` | Bilinear/Simple |

### EncodeJob (trait: `EncodingJob`)

Per-operation, borrows config. Has lifetime `'a`.

| Method | Signature | Notes |
|---|---|---|
| `with_stop` | `fn with_stop(self, s: &'a dyn Stop) -> Self` | Cancellation |
| `with_metadata` | `fn with_metadata(self, m: &'a ImageMetadata) -> Self` | ICC + EXIF + XMP |
| `with_icc` | `fn with_icc(self, icc: &'a [u8]) -> Self` | Just ICC profile |
| `with_exif` | `fn with_exif(self, exif: &'a [u8]) -> Self` | Just EXIF |
| `with_xmp` | `fn with_xmp(self, xmp: &'a [u8]) -> Self` | Just XMP |
| `with_limit_pixels` | `fn with_limit_pixels(self, max: u64) -> Self` | Override config |
| `with_limit_memory` | `fn with_limit_memory(self, bytes: u64) -> Self` | Override config |
| | | |
| **One-shot terminals** | | |
| `encode_rgb8` | `fn encode_rgb8(self, img: ImgRef<Rgb<u8>>) -> Result<EncodeOutput>` | |
| `encode_rgba8` | `fn encode_rgba8(self, img: ImgRef<Rgba<u8>>) -> Result<EncodeOutput>` | |
| `encode_gray8` | `fn encode_gray8(self, img: ImgRef<Gray<u8>>) -> Result<EncodeOutput>` | |
| `encode_bytes` | `fn encode_bytes(self, b: &[u8], w: u32, h: u32, layout: PixelLayout) -> Result<EncodeOutput>` | Escape hatch (not on trait) |
| | | |
| **Streaming terminals** (not on trait) | | |
| `encoder` | `fn encoder(self, w: u32, h: u32) -> Result<StreamingEncoder>` | Frame-push |

### DecodeJob (trait: `DecodingJob`)

| Method | Signature | Notes |
|---|---|---|
| `with_stop` | `fn with_stop(self, s: &'a dyn Stop) -> Self` | Cancellation |
| `with_format` | `fn with_format(self, f: ImageFormat) -> Self` | Skip auto-detect (not on trait) |
| `with_limit_pixels` | `fn with_limit_pixels(self, max: u64) -> Self` | Override config |
| `with_limit_memory` | `fn with_limit_memory(self, bytes: u64) -> Self` | Override config |
| | | |
| **One-shot terminals** | | |
| `decode` | `fn decode(self, data: &[u8]) -> Result<DecodeOutput>` | |
| | | |
| **Streaming terminals** (not on trait) | | |
| `decoder` | `fn decoder(self, data: &[u8]) -> Result<StreamingDecoder>` | Frame iteration |

### DecodeOutput

| Method | Signature | Notes |
|---|---|---|
| **Pixel access (borrowing)** | | |
| `pixels` | `fn pixels(&self) -> &PixelData` | Native format enum |
| `as_rgb8` | `fn as_rgb8(&self) -> Option<ImgRef<Rgb<u8>>>` | None if not natively Rgb8 |
| `as_rgba8` | `fn as_rgba8(&self) -> Option<ImgRef<Rgba<u8>>>` | None if not natively Rgba8 |
| `as_gray8` | `fn as_gray8(&self) -> Option<ImgRef<Gray<u8>>>` | None if not natively Gray8 |
| | | |
| **Pixel conversion (consuming)** | | |
| `into_rgb8` | `fn into_rgb8(self) -> ImgVec<Rgb<u8>>` | Converts any format to Rgb8 |
| `into_rgba8` | `fn into_rgba8(self) -> ImgVec<Rgba<u8>>` | Converts any format to Rgba8 |
| `into_pixels` | `fn into_pixels(self) -> PixelData` | Take native format |
| | | |
| **Info** | | |
| `info` | `fn info(&self) -> &ImageInfo` | Full image info |
| `width` | `fn width(&self) -> u32` | Shortcut |
| `height` | `fn height(&self) -> u32` | Shortcut |
| `has_alpha` | `fn has_alpha(&self) -> bool` | Shortcut |
| `format` | `fn format(&self) -> ImageFormat` | Shortcut |
| `metadata` | `fn metadata(&self) -> ImageMetadata<'_>` | Borrow ICC/EXIF/XMP for roundtrip |

### EncodeOutput

| Method | Signature | Notes |
|---|---|---|
| `into_vec` | `fn into_vec(self) -> Vec<u8>` | Consuming, extracts bytes |
| `bytes` | `fn bytes(&self) -> &[u8]` | Borrowing view |
| `len` | `fn len(&self) -> usize` | Output byte count |
| `format` | `fn format(&self) -> ImageFormat` | Useful with auto-select |

### StreamingDecoder

| Method | Signature | Notes |
|---|---|---|
| `info` | `fn info(&self) -> &ImageInfo` | Available after construction |
| `next_frame` | `fn next_frame(&mut self) -> Result<Option<DecodeFrame>>` | None = done |
| `frame_count` | `fn frame_count(&self) -> Option<u32>` | None if unknown |
| `reset` | `fn reset(&mut self) -> Result<()>` | Restart from first frame |

### StreamingEncoder

| Method | Signature | Notes |
|---|---|---|
| `add_frame_rgb8` | `fn add_frame_rgb8(&mut self, img: ImgRef<Rgb<u8>>, delay_ms: u32) -> Result<()>` | Push RGB frame |
| `add_frame_rgba8` | `fn add_frame_rgba8(&mut self, img: ImgRef<Rgba<u8>>, delay_ms: u32) -> Result<()>` | Push RGBA frame |
| `finish` | `fn finish(self) -> Result<EncodeOutput>` | Flush + return bytes |
| `frame_count` | `fn frame_count(&self) -> u32` | Frames pushed so far |

### DecodeFrame

| Method | Signature | Notes |
|---|---|---|
| `into_rgb8` | `fn into_rgb8(self) -> ImgVec<Rgb<u8>>` | Convert |
| `into_rgba8` | `fn into_rgba8(self) -> ImgVec<Rgba<u8>>` | Convert |
| `pixels` | `fn pixels(&self) -> &PixelData` | Native format |
| `delay_ms` | `fn delay_ms(&self) -> u32` | Frame timing |
| `index` | `fn index(&self) -> u32` | Frame number |
| `width` | `fn width(&self) -> u32` | Frame width |
| `height` | `fn height(&self) -> u32` | Frame height |

### ImageInfo

| Field | Type | Notes |
|---|---|---|
| `width` | `u32` | |
| `height` | `u32` | |
| `format` | `ImageFormat` | |
| `has_alpha` | `bool` | |
| `has_animation` | `bool` | |
| `frame_count` | `Option<u32>` | None if unknown without full parse |
| `icc_profile` | `Option<Vec<u8>>` | |
| `exif` | `Option<Vec<u8>>` | |
| `xmp` | `Option<Vec<u8>>` | |
| `metadata()` | `fn -> ImageMetadata<'_>` | Borrows ICC/EXIF/XMP for roundtrip |

### ImageMetadata

Borrowed view for passing between decode and encode.

```rust
pub struct ImageMetadata<'a> {
    pub icc_profile: Option<&'a [u8]>,
    pub exif: Option<&'a [u8]>,
    pub xmp: Option<&'a [u8]>,
}
```

### PixelData

```rust
pub enum PixelData {
    Rgb8(ImgVec<Rgb<u8>>),
    Rgba8(ImgVec<Rgba<u8>>),
    Rgb16(ImgVec<Rgb<u16>>),
    Rgba16(ImgVec<Rgba<u16>>),
    RgbF32(ImgVec<Rgb<f32>>),
    RgbaF32(ImgVec<Rgba<f32>>),
    Gray8(ImgVec<Gray<u8>>),
}
```

### Re-exports from `zencodec`

```rust
pub use imgref::{ImgRef, ImgVec};
pub use rgb::{Rgb, Rgba, Gray};
pub use enough::{Stop, Unstoppable};
```

## Effort Mapping

| zencodec effort | JPEG | WebP method | AVIF speed | PNG | GIF |
|---|---|---|---|---|---|
| 0 | — | 0 (fastest) | 10 (fastest) | BestSpeed | — |
| 5 (default) | — | 4 | 5 | Default | — |
| 10 | — | 6 (slowest) | 1 (slowest) | BestCompression | — |

## Quality Mapping

| zencodec quality | JPEG | WebP | AVIF | PNG | GIF |
|---|---|---|---|---|---|
| 0-100 | 0-100 (native) | 0-100 (native) | 0-100 (native) | N/A | palette quality |
| lossless=true | Error | VP8L lossless | Error | Default | Default |

## Format Capabilities

| Format | Lossy | Lossless | Alpha | Animation | Effort | Sharp YUV | Progressive |
|---|---|---|---|---|---|---|---|
| JPEG | yes | — | — | — | — | yes | yes |
| WebP | yes | yes | yes | yes | 0-6 | yes | — |
| PNG | — | yes | yes | APNG | compression | — | — |
| GIF | palette | palette | 1-bit | yes | — | — | — |
| AVIF | yes | — | yes | — | 1-10 | — | — |
| JXL | yes | yes | yes | yes | 1-10 | — | — |

## Auto-Select Logic

When `encoding(Auto)` or `EncodeConfig::auto()` is used:

```
if lossless:
    if alpha: prefer WebP lossless > PNG
    else: prefer PNG > WebP lossless
else:
    if alpha: prefer WebP > AVIF > PNG
    else: prefer JPEG > WebP > AVIF
```

Filtered by registry (only considers enabled formats).

## Open Questions

- Do we need `encode_rgb16` / `encode_rgba16` on the trait for HDR workflows?
- Should `EncodeOutput` implement `AsRef<[u8]>` and `Deref<Target=[u8]>`?
- Should streaming be on the trait? (GAT complexity vs. usefulness of generic streaming)
- Should `zencodec` re-export `ContentHint` or is it format-specific?
- Name: `zencodec` vs `zencodec-api` vs `codec-api`?
