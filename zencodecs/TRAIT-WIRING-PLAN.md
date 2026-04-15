# Trait Wiring Plan: Surface Existing Capabilities

Every codec has features that work through native APIs but aren't reachable through the
zencodec trait interface. This plan wires them through — no new implementations,
just connecting what's already there.

## Phase 1: Decode Hints (Crop + Orientation)

The highest-value work. These are the trait methods that exist (`with_crop_hint`,
`with_orientation_hint`) but no codec implements.

**Design principle:** Orientation and spatial transforms are handled by zenlayout and
zenresize, which support all EXIF orientations, transpositions, rotations, and flipping.
Codecs should only implement `with_orientation_hint` / `with_crop_hint` when they have
a **particularly efficient** path — e.g., JPEG lossless DCT rotation avoids full pixel
decode. Codecs without efficient in-codec transforms should leave the default no-op and
let the pipeline handle it.

### 1a. zenjpeg: Wire `with_orientation_hint` → lossless DCT transform

**What exists:**
- `DecodeConfig::auto_orient()` — reads EXIF, applies lossless DCT rotation
- `DecodeConfig::transform()` — arbitrary `LosslessTransform` on coefficients
- `lossless::apply_exif_orientation()` — full pipeline
- `lossless::transform()` — rotate/flip/transpose in DCT domain

**Why this matters:** Rotating MCU blocks in the DCT domain is O(coefficient shuffling)
vs O(full pixel buffer transpose + allocation) after decode. For a 20MP JPEG this is a
significant win.

**What to do:**
- In `JpegDecodeJob`, store an `Option<Orientation>` field
- Implement `with_orientation_hint()` to store the requested orientation
- In the decode path, when orientation hint is set:
  - If it matches EXIF orientation → use `auto_orient()` (lossless DCT path)
  - If it's an explicit rotation → map `Orientation` → `LosslessTransform`, apply via
    `DecodeConfig::transform()`
- Set `OutputInfo::orientation_applied` to reflect what was done
- Update `DECODE_CAPS` to advertise orientation support

**Files:** `zenjpeg/zenjpeg/src/zencodec.rs`

### 1b. zenjpeg: Wire `with_crop_hint` → MCU-aligned crop

**What exists:**
- `DecodeConfig::crop(CropRegion)` — MCU-row-aware cropping, skips IDCT outside region
- `CropRegion` enum with pixel and percentage modes

**Why this matters:** Skipping IDCT for MCU blocks outside the crop region saves real
compute. For a narrow crop of a large JPEG, most of the image is never decoded.

**What to do:**
- In `JpegDecodeJob`, store an `Option<(u32, u32, u32, u32)>` for crop rect
- Implement `with_crop_hint()` to store coordinates
- In the decode path, map to `CropRegion::pixels(x, y, w, h)` on `DecodeConfig`
- Set `OutputInfo::crop_applied` with the actual (possibly MCU-snapped) crop rect
- Update `DECODE_CAPS`

**Files:** `zenjpeg/zenjpeg/src/zencodec.rs`

### 1c. Other codecs: No orientation/crop wiring needed

zenavif, zenpng, zengif, zenwebp, zenjxl — none of these have an efficient in-codec
path for orientation or cropping. The default no-op on `with_orientation_hint` /
`with_crop_hint` is correct. zenlayout and zenresize handle orientation downstream.

Codecs should still accurately report orientation metadata in `ImageInfo` (most already
do), so the pipeline knows what transform to apply.

---

## Phase 2: Fix Bugs & Inaccurate Capabilities

Quick wins. Things that are broken or misleading.

### 2a. zenwebp: Fix upsampling method bug

**Bug:** `WebpDecoderConfig.upsampling` is stored but never applied to the
`DecodeRequest`. Setting upsampling through trait has no effect.
**Fix:** Pass `upsampling` to the `DecodeRequest` in the decode path.
**File:** `zenwebp/src/zencodec.rs:905-908`

### 2b. All codecs: Audit and fix CodecCapabilities flags

Every codec has capability flags that don't match reality:

| Codec | Missing Flags |
|-------|---------------|
| zenjpeg | `decode_crop`, `decode_orientation`, `decode_f32`, `decode_gain_map`, `encode_progressive` |
| zenwebp | (accurate after upsampling fix) |
| zengif | `decode_animation` (missing!), `encode_animation` (already set) |
| zenavif | `encode_xmp`, `encode_icc`, `decode_orientation`, `decode_mastering_display`, `decode_content_light_level` |
| zenpng | `encode_animation` (APNG works), `decode_animation`, fix `native_gray` (claims true, encode rejects it) |
| zenjxl | Multiple — see Phase 5 |

**Files:** Each codec's `zencodec.rs`, constants at top

### 2c. All codecs: Audit and fix pixel format descriptors

Several codecs implement pixel format conversions but don't advertise them in
`ENCODE_DESCRIPTORS`/`DECODE_DESCRIPTORS`:

| Codec | Missing Descriptors |
|-------|--------------------|
| zenavif | BGRA8, GRAY8, GRAYF32, RGBF32, RGBAF32, RGB16, RGBA16 (encode) |
| zenavif | All non-RGB8/RGBA8 formats (decode) |
| zenjxl | Check all descriptors match actual support |

---

## Phase 3: Metadata Wiring

### 3a. zenavif: Wire ICC and XMP on encode

**Bug:** `AvifEncodeJob::with_metadata()` only processes EXIF, ignores ICC and XMP.
Both `encoder.rs::icc_profile()` and `encoder.rs::xmp()` exist.
**Fix:** In `with_metadata()`, extract and pass through ICC and XMP from MetadataView.
**File:** `zenavif/src/zencodec.rs:218-223`

### 3b. zenavif: Advertise HDR metadata encode support

**What exists:** `encoder.rs::content_light_level()`, `encoder.rs::mastering_display()`
**What to do:** If MetadataView carries ContentLightLevel/MasteringDisplay, pass them
to the encoder. Update ENCODE_CAPS.

### 3c. zenjxl: Extract EXIF and XMP on decode

**Gap:** Only ICC is extracted during decode. EXIF and XMP are parsed for orientation
but not returned in ImageInfo.
**Fix:** Store raw EXIF and XMP bytes in ImageInfo fields.
**File:** `zenjxl/src/decode.rs`

### 3d. zengif: Expose loop count and metadata on decode

**Gap:** GIF metadata (comments, repeat/loop count, background color) is parsed but
not returned through the trait.
**Fix:** Populate `ImageInfo.frame_count`, and add repeat info. Comments are low
priority.

---

## Phase 4: Encode Config Wiring

Wire the most impactful encoder options that exist but aren't reachable through traits.
Focus on options that affect output quality/size significantly.

### 4a. zenjpeg: Wire progressive scan strategy

**What exists:** `ProgressiveScanMode` (Baseline, Progressive, ProgressiveMozjpeg,
ProgressiveSearch). ProgressiveSearch finds optimal scan order, ~2% smaller.
**What to do:** Expose via `JpegEncoderConfig`. The trait's `with_effort()` should
influence this (higher effort → ProgressiveSearch), but allow explicit override.
**File:** `zenjpeg/zenjpeg/src/zencodec.rs`

### 4b. zenjpeg: Wire chroma downsampling method

**What exists:** `DownsamplingMethod` (Box, Linear, GammaAwareIterative/SharpYUV).
SharpYUV is 3x slower but better color preservation.
**What to do:** Add `with_sharp_yuv(bool)` or `with_downsampling_method()` to
`JpegEncoderConfig`.

### 4c. zenjpeg: Wire Gaussian blur (pre-encode)

**What exists:** `EncoderConfig::pre_blur()` — σ≈0.4 reduces size ~5% with negligible
quality loss.
**What to do:** Add `with_pre_blur(f32)` to `JpegEncoderConfig`.

### 4d. zenwebp: Wire lossy target_size and preset

**What exists:** `LossyConfig::target_size`, `LossyConfig::target_psnr`,
`LossyConfig::preset` (Picture, Photo, Drawing, Icon, Text).
**What to do:** Add methods to `WebpEncoderConfig`.

### 4e. zenwebp: Wire lossless exact mode

**What exists:** `LosslessConfig::exact` — preserves RGB under transparent pixels.
**What to do:** Add `with_exact(bool)` to `WebpEncoderConfig`.

---

## Phase 5: Decode Config Wiring

### 5a. zenjpeg: Wire strictness / error recovery

**What exists:** `Strictness` enum (Strict, Balanced, Lenient, Permissive) with a
12-level error handling matrix. Critical for image proxies handling malformed JPEGs.
**What to do:** Add `with_strictness()` to `JpegDecoderConfig`.

### 5b. zenjpeg: Wire output target (f32 / precise)

**What exists:** `OutputTarget` (Srgb8, SrgbF32, LinearF32, SrgbF32Precise,
LinearF32Precise). Precise variants use Laplacian dequant bias for better reconstruction.
**What to do:** Wire to `JpegDecoderConfig` so callers can request f32 or precise output.

### 5c. zenjpeg: Wire chroma upsampling method

**What exists:** `ChromaUpsampling` (Triangle, LibjpegCompat, NearestNeighbor,
HorizontalFancy).
**What to do:** Add to `JpegDecoderConfig`.

### 5d. zenavif: Wire threading and film grain synthesis

**What exists:** `DecoderConfig::threads(u32)`, `DecoderConfig::apply_grain(bool)`.
**What to do:** Add to `AvifDecoderConfig`.

---

## Phase 6: Animation Wiring

### 6a. zenwebp: Wire animation encode config

**Gap:** `AnimationConfig` (background_color, loop_count, minimize_size) is not
accessible. Frame disposal/blend methods are hard-coded.
**What to do:**
- Add animation config methods to `WebpEncoderConfig` or `WebpFrameEncoder`
- Pass through `EncodeFrame.blend` and `EncodeFrame.disposal` instead of hard-coding

### 6b. zengif: Wire frame disposal in DecodeFrame

**Gap:** `DisposalMethod` is parsed but not returned in `DecodeFrame`.
**What to do:** Map `DisposalMethod` → `FrameDisposal` in `next_frame()`.

### 6c. zengif: Wire dithering and quantizer selection

**Gap:** Dithering level (0.0-1.0) and quantizer backend (zenquant/quantizr/imagequant)
are not accessible through the trait.
**What to do:** Add `with_dithering(f32)` and `with_quantizer()` to `GifEncoderConfig`.

### 6d. zengif: Wire shared palette controls

**Gap:** `shared_palette`, `max_buffer_frames`, `palette_error_threshold` exist but
aren't exposed.
**What to do:** Add methods to `GifEncoderConfig`.

---

## Phase 7: zenjxl Build Fix + Basic Wiring

zenjxl doesn't compile. Fix that first, then wire basics.

### 7a. Fix 4 build errors

1. `decode.rs:10` — `jxl::headers::extra_channels::ExtraChannel` → use public re-export
2. `decode.rs:79` — `jxl::error::Error` → use public `jxl::api::Error`
3. `error.rs:10` — same pub(crate) error module
4. `decode.rs:183` — `options.pixel_limit` → `options.limits.max_pixels`

### 7b. Wire decode cancellation (currently no-op)

**Gap:** Line 640: `self // JXL decoding is not cancellable`. But jxl-rs accepts
`Arc<dyn Stop>`.
**Fix:** Pass stop token through to decoder options.

### 7c. Wire EXIF/XMP extraction on decode

### 7d. Wire advanced encode options

At minimum: `gaborish`, `progressive` mode, `patches`, `threads`. These have
meaningful impact on output quality/size.

---

## Phase 8: zenpng Trait Completeness

### 8a. Fix grayscale encode (claimed but rejected)

**Bug:** `ENCODE_CAPS` claims `native_gray(true)` but encode() rejects Gray8 input.
**Fix:** Route Gray8/Gray16 through `encode_gray8()`/`encode_gray16()`.

### 8b. Wire APNG animation capabilities

**Gap:** APNG encode/decode works but capabilities don't advertise it.
**Fix:** Add `encode_animation(true)`, `decode_animation(true)` to caps.

### 8c. Wire gAMA/sRGB/cHRM chunks on encode

**Gap:** These PNG-specific chunks are decoded but not settable through the trait.
**Fix:** Add methods to `PngEncoderConfig` for gamma, sRGB intent, chromaticities.

### 8d. Wire indexed/palette encoding

**Gap:** Sophisticated indexed PNG encoding exists (`encode_indexed_rgba8`) but isn't
accessible through the trait.
**What to do:** Add `with_indexed(bool)` or `with_auto_optimize(bool)` to
`PngEncoderConfig` to enable the RGBA→RGB→Gray→Indexed optimization path.

---

## Ordering & Dependencies

```
Phase 1 (Decode Hints)     — highest value, user-requested priority
  ↓
Phase 2 (Bug Fixes)        — quick wins, correctness
  ↓
Phase 3 (Metadata)         — correctness, affects roundtrip fidelity
  ↓
Phase 4 (Encode Config)    — quality/size improvements
  ↓
Phase 5 (Decode Config)    — proxy/server use cases
  ↓
Phase 6 (Animation)        — animation roundtrip fidelity
  ↓
Phase 7 (zenjxl)           — blocked on build fix, lower priority codec
  ↓
Phase 8 (zenpng)           — blocked on DecodeRowSink integration
```

Phases 2-6 are largely independent and can be interleaved. Phase 1 is the priority.

## What This Plan Does NOT Cover

- New trait methods on `DecodeJob`/`EncodeJob` (beyond the existing hint methods)
- New types in zencodec (beyond what already exists)
- Streaming encode/decode (not wiring, would be new implementation)
- Color management / moxcms integration
- Auto-format selection in zencodecs
- Scale hints (low value, only JPEG benefits)
