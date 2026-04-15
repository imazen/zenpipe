# Slim Plan: Replace zencodecs Adapters with zencodec Traits

## Problem

zencodecs has ~4,100 lines of codec adapter code, ~70% of which is mechanical
boilerplate reimplementing pixel-format dispatch that belongs in the codecs themselves.

**Update (2026-03):** Phase 0 is complete — all codecs now implement the type-erased
`Encoder` trait. The remaining phases (1-4) can proceed to remove the adapter boilerplate.

The original root cause was that no codec implemented `Encoder`. That has been resolved.
The remaining adapter code in zencodecs still reimplements dispatch via its own patterns,
which Phases 1-4 aim to eliminate.

## Current Trait Implementation Status

| Codec | `EncoderConfig` | `EncodeJob` | Per-format encode | `Encoder` (type-erased) | `DecoderConfig` | `DecodeJob` | `Decode` |
|-------|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| zenjpeg | Y | Y | Y (9 traits) | **Y** | Y | Y | Y |
| zenwebp | Y | Y | Y (6 traits) | **Y** | Y | Y | Y |
| zengif | Y | Y | Y (6 traits) | **Y** | Y | Y | Y |
| zenavif | Y | Y | Y (6 traits) | **Y** | Y | Y | Y |
| zenpng | Y | Y | Y (6 traits) | **Y** | Y | Y | Y |
| zenjxl | Y | Y | Y (7 traits) | **Y** | Y | Y | Y |
| zenbitmaps | Y | Y | Y | **Y** | Y | Y | Y |
| zentiff | Y | Y | Y | **Y** | Y | Y | Y |

All codecs now implement both per-format traits and the type-erased `Encoder`.
Phase 0 is complete.

## What the `Encoder` Trait Is

From `zencodec/src/traits.rs`:

```rust
trait Encoder: Sized {
    type Error: core::error::Error + Send + Sync + 'static + From<UnsupportedOperation>;

    fn preferred_strip_height(&self) -> u32 { 0 }
    fn encode(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, Self::Error>;
    fn push_rows(&mut self, rows: PixelSlice<'_>) -> Result<(), Self::Error> { /* default: unsupported */ }
    fn finish(self) -> Result<EncodeOutput, Self::Error> { /* default: unsupported */ }
    fn encode_from(self, source: &mut dyn FnMut(u32, PixelSliceMut<'_>) -> usize) -> Result<EncodeOutput, Self::Error> { /* default: unsupported */ }
}
```

The key method is `encode(self, pixels: PixelSlice<'_>)`. It accepts any pixel format at
runtime via the type-erased `PixelSlice<'_>` (which carries a `PixelDescriptor`). The
implementation inspects the descriptor and dispatches to the appropriate per-format trait.

Once a codec implements `Encoder`, `EncodeJob::dyn_encoder()` becomes available:

```rust
// Returns Box<dyn FnOnce(PixelSlice<'_>) -> Result<EncodeOutput, BoxedError> + 'a>
let encode = config.job().with_metadata(&meta).dyn_encoder()?;
let output = encode(pixels)?;
```

This is exactly what zencodecs' `DynEncoder` trait does, but provided by zencodec
with zero adapter code needed.

## What the `Encoder` Implementation Looks Like

For each codec, it's ~30-50 lines. Example for a JPEG-like codec:

```rust
impl Encoder for JpegEncoder<'_> {
    type Error = At<JpegError>;

    fn encode(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, Self::Error> {
        match pixels.descriptor().pixel_format() {
            Some(PixelFormat::Rgb8) => self.encode_rgb8(pixels.cast_typed()?),
            Some(PixelFormat::Rgba8) => self.encode_rgba8(pixels.cast_typed()?),
            Some(PixelFormat::Gray8) => self.encode_gray8(pixels.cast_typed()?),
            Some(PixelFormat::RgbF32) => self.encode_rgb_f32(pixels),
            Some(PixelFormat::RgbaF32) => self.encode_rgba_f32(pixels),
            Some(PixelFormat::GrayF32) => self.encode_gray_f32(pixels),
            _ => Err(UnsupportedOperation::from(/* ... */).into()),
        }
    }
}
```

This match is the same match that currently lives in zencodecs' `JpegDynEncoder::encode_pixels`.
It moves into the codec where it belongs — the codec knows its own capabilities.

---

## Phase 0: Implement `Encoder` in Each Codec

### Scope

Add `impl Encoder for FooEncoder<'_>` to each codec's `zencodec.rs`. Each implementation
dispatches on `PixelSlice::descriptor().pixel_format()` to the per-format traits the codec
already implements.

All 6 codecs can be done independently and in parallel.

### 0a. zenjpeg: `impl Encoder for JpegEncoder<'_>`

**File:** `zenjpeg/zenjpeg/src/zencodec.rs`

**Already implements:** `EncodeRgb8`, `EncodeRgba8`, `EncodeGray8`, `EncodeRgb16`,
`EncodeRgba16`, `EncodeGray16`, `EncodeRgbF32`, `EncodeRgbaF32`, `EncodeGrayF32`

**Match arms:** 9 (Rgb8, Rgba8, Gray8, Rgb16, Rgba16, Gray16, RgbF32, RgbaF32, GrayF32)

**BGRA/BGRX note:** The native BGRA/BGRX encode paths in zencodecs bypass the trait and
call `zenjpeg::encoder::EncodeRequest::encode_bytes()` directly with a `PixelLayout::Bgra8Srgb`
/ `Bgrx8Srgb`. For `Encoder` to handle these:
- Option A: Add `EncodeRgba8`-style BGRA handling inside `Encoder::encode()` that converts
  via the native `encode_bytes` path. This means `JpegEncoder` needs access to the raw
  `EncoderConfig` (it already does — it's stored in the job).
- Option B: Leave BGRA/BGRX as a zencodecs-side fallback that converts to RGB8 first.
  This loses the zero-copy benefit but is simpler.
- **Recommended:** Option A. The codec should handle its own native pixel formats. Add
  `PixelFormat::Bgra8` and `PixelFormat::Bgrx8` arms that use the native API.

**Error type:** `JpegEncoder::Error` is `At<zenjpeg::Error>`. Need to add
`impl From<UnsupportedOperation> for At<zenjpeg::Error>` — already exists
(`zenjpeg::Error::UnsupportedOperation` variant).

**Estimated:** ~45 lines.

### 0b. zenwebp: `impl Encoder for WebpEncoder<'_>`

**File:** `zenwebp/src/zencodec.rs`

**Already implements:** `EncodeRgb8`, `EncodeRgba8`, `EncodeGray8`, `EncodeRgbF32`,
`EncodeRgbaF32`, `EncodeGrayF32`

**Match arms:** 6 (Rgb8, Rgba8, Gray8, RgbF32, RgbaF32, GrayF32).
Plus BGRA8 if we want the native path (zenwebp VP8 encoder supports BGRA natively).

**BGRA note:** zencodecs currently has a native BGRA encode path for WebP. This should
move into the `Encoder` impl if zenwebp's native API supports it.

**Estimated:** ~35 lines.

### 0c. zengif: `impl Encoder for GifEncoder<'_>`

**File:** `zengif/src/zencodec.rs`

**Already implements:** `EncodeRgba8`, `EncodeRgb8`, `EncodeGray8`, `EncodeRgbF32`,
`EncodeRgbaF32`, `EncodeGrayF32`

**Match arms:** 6.

**Estimated:** ~30 lines.

### 0d. zenavif: `impl Encoder for AvifEncoder<'_>`

**File:** `zenavif/src/zencodec.rs`

**Already implements:** `EncodeRgb8`, `EncodeRgba8`, `EncodeGray8`, `EncodeRgbF32`,
`EncodeRgbaF32`, `EncodeGrayF32`

**Match arms:** 6.

**Estimated:** ~30 lines.

### 0e. zenpng: `impl Encoder for PngEncoder<'_>`

**File:** `zenpng/src/zencodec.rs`

**Already implements:** `EncodeRgb8`, `EncodeRgba8`, `EncodeGray8` (and likely 16-bit
variants — needs verification).

**Match arms:** 6-9 depending on 16-bit support.

**Estimated:** ~35 lines.

### 0f. zenjxl: `impl Encoder for JxlEncoder<'_>`

**File:** `zenjxl/src/zencodec.rs`

**Blocked on:** Build fix (TRAIT-WIRING-PLAN Phase 7a — 4 compile errors).

**Already implements:** `EncodeRgb8`, `EncodeRgba8`, `EncodeGray8`, `EncodeRgb16`,
`EncodeRgba16`, `EncodeGray16`, `EncodeRgbF32`, `EncodeRgbaF32`, `EncodeGrayF32`,
`EncodeRgbF16`, `EncodeRgbaF16`

**Match arms:** 11 (most of any codec — JXL supports everything).

**Estimated:** ~50 lines.

### Phase 0 Verification

For each codec, after implementing `Encoder`:

1. `cargo test` passes
2. `EncodeJob::dyn_encoder()` compiles and returns a working closure
3. Add a test that roundtrips through `dyn_encoder()`:
   ```rust
   let enc = FooEncoderConfig::new().with_generic_quality(85.0).job().dyn_encoder()?;
   let output = enc(PixelSlice::from(test_img))?;
   assert!(!output.data().is_empty());
   ```

---

## Phase 1: Replace zencodecs `DynEncoder` with Trait `dyn_encoder()`

### Prerequisite

All codecs from Phase 0 have `Encoder` implemented and published (or path-dep'd).

### 1a. New generic encode builder

Replace `dispatch.rs` with a single function that builds a `DynEncoder<'a>` (the
zencodec type alias: `Box<dyn FnOnce(PixelSlice<'_>) -> Result<EncodeOutput, BoxedError>>`)
using the trait interface:

```rust
// src/dispatch.rs — complete replacement

use crate::{CodecError, ImageFormat, Limits, MetadataView, Stop};
use crate::config::CodecConfig;
use crate::limits::to_resource_limits;
use zencodec::{BoxedError, DynEncoder, EncodeOutput, EncoderConfig, EncodeJob};
use zenpixels::PixelDescriptor;

pub(crate) struct EncodeParams<'a> {
    pub quality: Option<f32>,
    pub effort: Option<u32>,
    pub lossless: bool,
    pub metadata: Option<&'a MetadataView<'a>>,
    pub codec_config: Option<&'a CodecConfig>,
    pub limits: Option<&'a Limits>,
    pub stop: Option<&'a dyn Stop>,
}

/// Returns (dyn_encoder_closure, supported_descriptors) for the given format.
pub(crate) fn build_encoder(
    format: ImageFormat,
    params: EncodeParams<'_>,
) -> Result<(DynEncoder<'_>, &'static [PixelDescriptor]), CodecError> {
    match format {
        #[cfg(feature = "jpeg")]
        ImageFormat::Jpeg => build_typed::<zenjpeg::JpegEncoderConfig>(params),
        #[cfg(feature = "webp")]
        ImageFormat::WebP => build_typed::<zenwebp::WebpEncoderConfig>(params),
        #[cfg(feature = "gif")]
        ImageFormat::Gif => build_typed::<zengif::GifEncoderConfig>(params),
        #[cfg(feature = "png")]
        ImageFormat::Png => build_typed::<zenpng::PngEncoderConfig>(params),
        #[cfg(feature = "avif-encode")]
        ImageFormat::Avif => build_typed::<zenavif::AvifEncoderConfig>(params),
        #[cfg(feature = "jxl-encode")]
        ImageFormat::Jxl => build_typed::<zenjxl::JxlEncoderConfig>(params),
        _ => Err(CodecError::UnsupportedFormat(format)),
    }
}

fn build_typed<C>(params: EncodeParams<'_>) -> Result<(DynEncoder<'_>, &'static [PixelDescriptor]), CodecError>
where
    C: EncoderConfig + Default,
    C::Job<'_>: EncodeJob<'_>,
    // Enc must implement Encoder for dyn_encoder() to be available
{
    let descriptors = C::supported_descriptors();

    let mut config = C::default();
    // TODO: apply codec_config override if present (format-specific)
    if let Some(q) = params.quality {
        config = config.with_generic_quality(q);
    }
    if let Some(e) = params.effort {
        config = config.with_generic_effort(e as i32);
    }
    if params.lossless {
        config = config.with_lossless(true);
    }

    let mut job = config.job();
    if let Some(lim) = params.limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(meta) = params.metadata {
        job = job.with_metadata(meta);
    }
    if let Some(s) = params.stop {
        job = job.with_stop(s);
    }

    let dyn_enc = job.dyn_encoder()
        .map_err(|e| CodecError::Codec {
            format: C::format(),
            source: e,
        })?;

    Ok((dyn_enc, descriptors))
}
```

**Note:** The `codec_config` override (format-specific config like chroma subsampling)
needs a per-format match to apply. Two options:

1. **Simple:** Keep a small match in `build_encoder` that applies codec-specific config
   before calling `build_typed`. ~10 lines per codec.
2. **Trait-based:** Add a local trait `ApplyCodecConfig` with a blanket method. Cleaner
   but possibly over-engineered for 6 codecs.

Option 1 is sufficient. The codec_config application is the one place where format-specific
knowledge is genuinely needed.

### 1b. Update `encode_dispatch` in `encode.rs`

The current flow:

```
encode_dispatch → build_encoder (returns Box<dyn DynEncoder>)
              → adapt_for_encode (pixel negotiation)
              → encoder.encode_pixels(adapted_data, ...)
```

New flow:

```
encode_dispatch → build_encoder (returns (DynEncoder closure, &[PixelDescriptor]))
              → adapt_for_encode (pixel negotiation, using descriptors)
              → dyn_enc(PixelSlice::from_raw(...))
```

The change is mechanical. `encode_dispatch` currently calls:
- `encoder.supported_descriptors()` — replaced by the returned descriptors
- `encoder.encode_pixels(data, descriptor, w, h, stride)` — replaced by calling the
  closure with a `PixelSlice`

### 1c. Delete old adapter code

For each codec adapter file, delete:
- The `FooDynEncoder` struct
- The `build_dyn_encoder` function
- The `impl DynEncoder for FooDynEncoder` block
- The `FOO_SUPPORTED` static descriptor list (now lives in the codec)

**Files modified:**
- `src/codecs/jpeg.rs`: Delete lines 490-660 (~170 lines)
- `src/codecs/webp.rs`: Delete DynEncoder section (~140 lines)
- `src/codecs/png.rs`: Delete DynEncoder section (~130 lines)
- `src/codecs/gif.rs`: Delete DynEncoder section (~85 lines)
- `src/codecs/avif_enc.rs`: Delete DynEncoder section (~120 lines)
- `src/codecs/jxl_enc.rs`: Delete DynEncoder section (~120 lines)

**Total deleted:** ~765 lines.

### 1d. Handle BGRA/BGRX special cases

Currently JPEG and WebP have native BGRA/BGRX encode paths that bypass the trait.
After Phase 0, these paths move into the codec's `Encoder` impl. The `adapt_for_encode`
negotiation already handles this — if the codec advertises `BGRA8_SRGB` in its
supported_descriptors, the adapter will pass BGRA through without conversion.

If a codec does NOT advertise BGRA in its supported_descriptors, `adapt_for_encode`
will convert BGRA→RGBA or BGRA→RGB automatically. No special handling needed in
zencodecs.

---

## Phase 2: Delete Typed Encode Functions

### What gets deleted

Each codec adapter currently has 6-8 typed encode functions like:

```rust
pub(crate) fn encode_rgb8(
    img: ImgRef<Rgb<u8>>,
    quality: Option<f32>,
    metadata: Option<&MetadataView<'_>>,
    codec_config: Option<&CodecConfig>,
    limits: Option<&Limits>,
    stop: Option<&dyn Stop>,
) -> Result<EncodeOutput, CodecError> {
    let enc = build_encoding(quality, codec_config);
    let mut job = enc.job();
    if let Some(lim) = limits { job = job.with_limits(to_resource_limits(lim)); }
    if let Some(meta) = metadata { job = job.with_metadata(meta); }
    if let Some(s) = stop { job = job.with_stop(s); }
    job.encoder()?.encode_rgb8(PixelSlice::from(img))
        .map_err(|e| CodecError::from_codec(ImageFormat::Jpeg, e))
}
```

These are only called from the old `DynEncoder::encode_pixels` match arms — which were
deleted in Phase 1. With the trait-based `dyn_encoder()` path, the codec handles pixel
format dispatch internally. These functions are dead code.

### Per-codec deletions

**`jpeg.rs`:** Delete `encode_rgb8`, `encode_rgba8`, `encode_bgra8`, `encode_bgrx8`,
`encode_gray8`, `encode_rgb_f32`, `encode_rgba_f32`, `encode_gray_f32`, and `build_encoding`.
~238 lines.

**`webp.rs`:** Delete `encode_rgb8`, `encode_rgba8`, `encode_bgra8`, `encode_gray8`,
`encode_rgb_f32`, `encode_rgba_f32`, `encode_gray_f32`, and `build_encoding`.
~286 lines.

**`png.rs`:** Delete all `encode_*` functions and `build_encoding`. ~154 lines.

**`gif.rs`:** Delete all `encode_*` functions and `build_encoding`. ~80 lines.

**`avif_enc.rs`:** Delete all `encode_*` functions and `build_encoding`. ~181 lines.

**`jxl_enc.rs`:** Delete all `encode_*` functions and `build_encoding`. ~200 lines.

**Total deleted:** ~1,139 lines.

### What remains in each encode adapter

After Phases 1 and 2, each encode adapter file contains only genuinely format-specific
code:

- **`jpeg.rs`:** UltraHDR encode/decode (~230 lines), `apply_metadata` helper (~17 lines),
  probe with permissive strictness (~30 lines), decode with extras preservation (~65 lines),
  decode-into methods (~65 lines). Total: ~407 lines.
- **`webp.rs`:** Decode (~40 lines), decode-into (~30 lines), probe (~10 lines),
  metadata bridge (~20 lines). Total: ~100 lines.
- **`png.rs`:** Decode (~40 lines), probe (~10 lines). Total: ~50 lines.
- **`gif.rs`:** Decode with frame count (~50 lines), probe (~15 lines). Total: ~65 lines.
- **`avif_enc.rs`:** Probe (~10 lines) if needed, otherwise empty/deleted. Total: ~10 lines.
- **`avif_dec.rs`:** Already slim: 41 lines. No change.
- **`jxl_enc.rs`:** Probe (~10 lines) if needed, otherwise empty/deleted. Total: ~10 lines.
- **`jxl_dec.rs`:** Already slim: 35 lines. No change.
- **`heic.rs`:** Already slim: 34 lines. No change.

---

## Phase 3: Migrate Decode Adapters to Trait Path

### The pattern that works

AVIF, JXL, and HEIC decode adapters are already slim (~35 lines each) because they use
the trait interface:

```rust
pub(crate) fn decode(data, limits, stop) -> Result<DecodeOutput, CodecError> {
    let dec = FooDecoderConfig::new();
    let mut job = dec.job();
    if let Some(lim) = limits { job = job.with_limits(to_resource_limits(lim)); }
    if let Some(s) = stop { job = job.with_stop(s); }
    job.decoder(data, &[])
        .map_err(|e| CodecError::from_codec(ImageFormat::Foo, e))?
        .decode()
        .map_err(|e| CodecError::from_codec(ImageFormat::Foo, e))
}
```

### 3a. WebP decode: migrate to trait path

**Current:** ~120 lines. Calls native zenwebp API, manual ImageInfo construction, config
passthrough.

**After:** ~40 lines. Same pattern as AVIF decode, with codec_config application:

```rust
pub(crate) fn decode(data, codec_config, limits, stop) -> Result<DecodeOutput, CodecError> {
    let mut dec = zenwebp::WebpDecoderConfig::new();
    if let Some(cfg) = codec_config.and_then(|c| c.webp_decoder.as_ref()) {
        // Apply format-specific config
    }
    let mut job = dec.job();
    if let Some(lim) = limits { job = job.with_limits(to_resource_limits(lim)); }
    if let Some(s) = stop { job = job.with_stop(s); }
    job.decoder(data, &[])
        .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))?
        .decode()
        .map_err(|e| CodecError::from_codec(ImageFormat::WebP, e))
}
```

**Prerequisite:** zenwebp's `Decode` impl must return a complete `DecodeOutput` with
correct `ImageInfo` (ICC, EXIF, XMP populated). Verify this is the case.

**Reduction:** ~80 lines.

### 3b. GIF decode: migrate to trait path

**Current:** ~120 lines. Manual limits conversion (u64 -> u16), frame counting, memory
pre-check.

**After:** ~40 lines. The u64 -> u16 limits conversion should already be handled in
zengif's `DecodeJob::with_limits()` implementation. Verify this.

**Prerequisite:** zengif's `Decode` impl must handle limits clamping internally.

**Reduction:** ~80 lines.

### 3c. PNG decode: migrate to trait path

**Current:** ~100 lines. Uses the `png` crate directly, manual pixel buffer construction.

**After:** ~40 lines. Same pattern as AVIF. zenpng already has `impl Decode for PngDecoder`.

**Prerequisite:** zenpng's `Decode` impl must be complete and return proper `DecodeOutput`.

**Reduction:** ~60 lines.

### 3d. JPEG decode: partial migration

**Current:** ~150 lines for standard decode. Uses native API to preserve `DecodedExtras`.

**Cannot fully migrate because:** The `DecodedExtras` (DCT coefficients, quantization
tables) are JPEG-specific data attached via `DecodeOutput::with_extras()`. The trait's
`Decode::decode()` must return these extras for the pipeline to support lossless JPEG
operations (crop, rotate in DCT domain).

**Check:** Does zenjpeg's `Decode` impl already return extras via `DecodeOutput::with_extras()`?
- If yes: migrate to trait path, ~40 lines. The extras come through automatically.
- If no: keep native decode path for extras. The trait decode path would lose extras.

**Likely outcome:** zenjpeg's `Decode` impl probably does return extras (it was designed
with this in mind). Verify by reading `zenjpeg/zenjpeg/src/zencodec.rs` line 969+.

If zenjpeg's trait impl returns extras:
- Standard decode: migrate to trait path (~40 lines)
- UltraHDR decode: stays native (needs raw pixel access for gain map reconstruction)
- decode-into methods: already use trait path (clean)

**Reduction:** ~110 lines (if extras work), ~0 lines (if they don't).

### 3e. Simplify `decode.rs` dispatch

After migrating all decode adapters to the trait path, the `decode_format` match in
`decode.rs` can be simplified. Currently each arm calls a different method
(`self.decode_jpeg()`, `self.decode_webp()`, etc.) which each call
`crate::codecs::foo::decode(...)`. These intermediate methods can be inlined or
collapsed.

The `decode_into_*` methods in `decode.rs` are a separate concern — they provide
typed buffer output and use codec-specific fast paths where available. These stay
but could be simplified with a generic fallback pattern.

---

## Phase 4: Clean Up `encode_dispatch` Flow

### Current flow

```
EncodeRequest::encode_rgb8(img)
  → bytemuck::cast_slice to get &[u8]
  → encode_dispatch(data, RGB8_SRGB, w, h, stride, has_alpha)
    → build_encoder(format, params) → Box<dyn DynEncoder>
    → adapt_for_encode(data, descriptor, w, h, stride, encoder.supported_descriptors())
    → encoder.encode_pixels(adapted_data, adapted_descriptor, ...)
```

### New flow

```
EncodeRequest::encode_rgb8(img)
  → bytemuck::cast_slice to get &[u8]
  → encode_dispatch(data, RGB8_SRGB, w, h, stride, has_alpha)
    → build_encoder(format, params) → (DynEncoder closure, &[PixelDescriptor])
    → adapt_for_encode(data, descriptor, w, h, stride, descriptors)
    → let pixels = PixelSlice::from_raw(adapted_data, adapted_descriptor, w, h)
    → dyn_enc(pixels)?
```

### Simplify typed encode methods on `EncodeRequest`

The 9 typed methods (`encode_rgb8`, `encode_rgba8`, `encode_bgra8`, `encode_bgrx8`,
`encode_gray8`, `encode_rgb_f32`, `encode_rgba_f32`, `encode_gray_f32`) all follow the
same pattern:

```rust
pub fn encode_FOO(self, img: ImgRef<Foo>) -> Result<EncodeOutput, CodecError> {
    let data: &[u8] = bytemuck::cast_slice(img.buf());
    let stride = img.stride() * core::mem::size_of::<Foo>();
    self.encode_dispatch(data, DESCRIPTOR, img.width() as u32, img.height() as u32, stride, has_alpha)
}
```

These are thin enough to keep as-is — they provide type safety at the public API boundary.
But the `encode_dispatch` method they all call gets simpler.

### Supported descriptors query

`adapt_for_encode` needs the encoder's supported descriptors to negotiate pixel format.
With the old `DynEncoder`, this was `encoder.supported_descriptors()`. With the new path,
we have two options:

1. **Return descriptors from `build_encoder`:** The `build_encoder` function returns both
   the closure and the descriptors. This is what the Phase 1 sketch shows.
2. **Query statically:** Call `FooEncoderConfig::supported_descriptors()` directly in the
   format match, before building the encoder. Avoids coupling.

Option 1 is cleaner — the descriptors and closure come from the same code path.

---

## Summary: Lines Changed

### Deletions from zencodecs

| File | Lines before | Lines after | Deleted |
|------|:-----------:|:-----------:|:-------:|
| `dispatch.rs` | 98 | ~60 | 38 |
| `codecs/jpeg.rs` | 914 | ~350 | 564 |
| `codecs/webp.rs` | 551 | ~100 | 451 |
| `codecs/png.rs` | 356 | ~50 | 306 |
| `codecs/gif.rs` | 364 | ~65 | 299 |
| `codecs/avif_enc.rs` | 360 | ~10 | 350 |
| `codecs/jxl_enc.rs` | 373 | ~10 | 363 |
| `codecs/avif_dec.rs` | 41 | 41 | 0 |
| `codecs/jxl_dec.rs` | 35 | 35 | 0 |
| `codecs/heic.rs` | 34 | 34 | 0 |
| `encode.rs` | 495 | ~350 | 145 |
| `decode.rs` | 588 | ~350 | 238 |
| **Total** | **4,209** | **~1,455** | **~2,754** |

### Additions to codecs (Phase 0)

| Codec | Lines added |
|-------|:-----------:|
| zenjpeg | ~45 |
| zenwebp | ~35 |
| zengif | ~30 |
| zenavif | ~30 |
| zenpng | ~35 |
| zenjxl | ~50 |
| **Total** | **~225** |

### Net reduction

- zencodecs: **-2,754 lines** (65% reduction in adapter code)
- codecs: **+225 lines** (pixel dispatch moves to where it belongs)
- **Net: -2,529 lines** across the ecosystem

---

## Ordering and Dependencies

```
Phase 0: Implement Encoder trait in each codec
  |
  ├── 0a. zenjpeg  ─┐
  ├── 0b. zenwebp  ─┤
  ├── 0c. zengif   ─┤  All independent, parallelizable.
  ├── 0d. zenavif  ─┤  Each is a ~30-50 line addition.
  ├── 0e. zenpng   ─┤
  └── 0f. zenjxl   ─┘  (blocked on build fix)
         │
         ▼
Phase 1: Replace DynEncoder in zencodecs
  │      Requires: all Phase 0 codecs published or path-dep'd
  │      Deletes: ~765 lines (DynEncoder impls)
  │
  ▼
Phase 2: Delete typed encode functions
  │      Requires: Phase 1 (these become dead code)
  │      Deletes: ~1,139 lines
  │
  ▼
Phase 3: Migrate decode adapters to trait path
  │      Independent of Phases 1-2 (can be done before or after)
  │      Requires: verify each codec's Decode impl is complete
  │      Deletes: ~238 lines
  │
  ▼
Phase 4: Clean up encode_dispatch flow
         Polish. Small changes to encode.rs.
         Deletes: ~145 lines
```

Phase 3 is fully independent of Phases 1-2 and can happen at any time.

Within Phase 0, all codecs are independent.

---

## Risks and Mitigations

### Risk: Codec's `Encoder` impl doesn't handle BGRA/BGRX

**Impact:** Loses zero-copy BGRA encode for JPEG and WebP.
**Mitigation:** `adapt_for_encode` already converts BGRA→RGBA as fallback. The performance
cost is one channel swizzle, which is cheap relative to encoding. Can add native BGRA
support to the codec's `Encoder` impl later.

### Risk: Codec's `Decode` impl doesn't preserve extras

**Impact:** Can't migrate JPEG decode to trait path; extras (DCT coefficients) lost.
**Mitigation:** Keep native JPEG decode path alongside trait decode. Only ~110 lines
of savings at stake.

### Risk: `codec_config` override is harder through the generic path

**Impact:** Format-specific config (e.g., JPEG chroma subsampling) needs to be applied
to the concrete `FooEncoderConfig` before erasure to `DynEncoder`.
**Mitigation:** The `build_encoder` match on format is the right place for this. Each
arm can apply codec-specific config before calling `build_typed`. This is a small amount
of per-format code (~10 lines) that genuinely needs to exist.

### Risk: zenjxl blocked on build fix

**Impact:** Can't implement Phase 0f until zenjxl compiles.
**Mitigation:** All other codecs proceed independently. zenjxl is lowest priority codec.
zencodecs can keep the old jxl_enc adapter until zenjxl is fixed, gated behind `cfg`.

### Risk: Breaking change in zencodec `Encoder` trait

**Impact:** If trait signature changes, all 6 impls need updating.
**Mitigation:** zencodec is internal. We control the trait. The `Encoder` trait
has been stable since zenbitmaps implemented it. Lock zencodec version in
Cargo.toml.

---

## What This Plan Does NOT Cover

- **Streaming encode/decode:** New capability, not wiring. Separate plan.
- **Animation encode:** `FrameEncoder` trait implementations. Separate concern.
- **Color management / moxcms:** Separate feature.
- **Auto-format selection:** The `auto_select_format` logic stays unchanged.
- **`EncodeRequest`/`DecodeRequest` public API:** Unchanged. Callers see no difference.
- **TRAIT-WIRING-PLAN.md items:** That plan covers wiring codec-specific features
  (crop hints, orientation, metadata, progressive mode, etc.). This plan covers
  eliminating adapter boilerplate. They are complementary.
- **`decode_into_*` methods:** These provide typed buffer output with codec-specific
  fast paths. They stay. Some use the trait already (JPEG decode-into).
- **UltraHDR:** Stays as JPEG-specific code. Cannot be generalized.
- **Pipeline feature:** The `pipeline` module uses the encode/decode public API.
  No changes needed there — it benefits automatically.
