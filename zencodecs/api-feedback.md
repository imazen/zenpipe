# Upstream Codec API Feedback

Issues discovered while implementing zencodecs adapters (src/codecs/*.rs).
Updated 2026-02-07 after full Limits/Stop/Config wiring pass.

---

## zenjpeg

### 1. Probe requires full decode

`read_info()` returns `JpegInfo` but doesn't extract ICC/EXIF/XMP. The adapter does a full `decode()` just for `probe()` (metadata-only inspection).

**Fix:** `read_info()` should parse APP markers and expose ICC/EXIF/XMP.

### 2. Limits are public fields, not builder methods

`DecodeConfig` uses builder methods for everything (`output_format()`, `chroma_upsampling()`, etc.) but `max_pixels` and `max_memory` are bare `pub` fields. Inconsistent.

**Fix:** Add `with_max_pixels()` / `with_max_memory()` builder methods.

### 3. `max_memory` is `usize`, not `u64`

zenwebp, zengif, and zencodecs all use `u64` for memory limits. zenjpeg uses `usize`. Requires a cast (`max_mem as usize`). On 32-bit targets this silently truncates.

**Fix:** Change to `u64`.

### 4. Encoder metadata methods require owned `Vec<u8>`

`EncoderConfig::icc_profile()`, `xmp()` take owned `Vec<u8>`. The adapter has borrowed `&[u8]` from `ImageMetadata` and must `.to_vec()` every time. `EncodeRequest` has both borrowed and `_owned` variants, but `EncoderConfig` doesn't.

**Fix:** Accept `impl Into<Cow<'_, [u8]>>` or add borrowed variants.

### 5. `Decoder` vs `DecodeConfig` naming

The main type is `DecodeConfig` but it has `decode()`, `scanline_reader()`, `ultrahdr_reader()` — it *is* the decoder. The old `Decoder` name exists as an alias. Both are in scope.

**Fix:** Pick one name. `Decoder` is more natural since users call `.decode()` on it.

### 6. `decode_f32()` is redundant

Identical to `decode()` with `.output_target(OutputTarget::SrgbF32)`. Extra API surface with no additional capability.

**Suggestion:** Deprecate in favor of `OutputTarget`.

### 7. PixelFormat vs PixelLayout

Decoder result has `PixelFormat`, encoder expects `PixelLayout`. Different types for the same concept. Requires manual mapping in the adapter.

**Fix:** Unify or add explicit conversions between them.

### What works well

- `DecodedExtras` API for segment preservation — `segments()`, `mpf()`, `secondary_images()`, `gainmap()`, `to_encoder_segments()` is a clean roundtrip story
- `GainMapHandling` enum gives fine-grained control over gain map processing cost
- `PreserveConfig` letting callers choose what to preserve
- `EncodeRequest` → `RgbEncoder` → `push_packed()` → `finish()` streaming pipeline
- `EncoderSegments::add_gainmap()` makes gain map roundtrip trivial
- Quality 0-100 native scale is intuitive
- `ChromaSubsampling` enum is clear and self-documenting

---

## zenwebp

### 8. Probe requires two parsers

`WebPDecoder::new()` for dimensions/alpha, then `WebPDemuxer::new()` for metadata and animation info. Both parse the same data.

**Fix:** Single `probe()` or `ImageInfo` that returns everything in one pass.

### 9. `has_alpha` not available from `DecodeRequest`

The adapter creates a `WebPDecoder` just to check `has_alpha()` before calling `DecodeRequest::decode_rgba()` or `decode_rgb()`. `DecodeRequest` doesn't expose image info after construction.

**Fix:** Add `DecodeRequest::has_alpha()` / `info()`, or provide a single `decode()` that returns pixels in native format without the caller needing to choose.

### 10. Triple re-parse for metadata embedding

`embed_icc()`, `embed_exif()`, `embed_xmp()` each re-parse and re-serialize the entire RIFF container. Three metadata chunks = three full RIFF re-parses.

**Fix:** Single `embed_metadata(data, icc, exif, xmp)` or a mux builder that batches mutations.

### 11. `EncodeRequest` doesn't accept metadata

Metadata has to be embedded post-encode via separate mux functions. Every other codec (zenjpeg, png, ravif) accepts metadata as part of the encode config.

**Fix:** Add `.with_icc()` / `.with_exif()` / `.with_xmp()` on `EncodeRequest`.

### 12. `DecodeRequest::decode_rgba()` buffer size bug (FIXED)

Allocated `w*h*4` for all images but `read_image()` expects `output_buffer_size()` which is `w*h*3` for non-alpha images. Caused `ImageTooLarge` on valid non-alpha extended-format WebP.

**Status:** Fixed in zenwebp commit 82994d9.

### What works well

- `WebPDemuxer` metadata extraction (ICC/EXIF/XMP) is clean
- `LossyConfig` / `LosslessConfig` separation is clear
- `EncodeRequest::lossy()` / `lossless()` factory methods are intuitive
- Limits struct with named fields is more readable than positional

---

## zengif

### 13. `Stop` requires `Clone`

`Decoder::new(reader, limits, stop)` requires `S: Stop + Clone`. `&dyn Stop` doesn't implement `Clone`, so the adapter can't forward the stop token from zencodecs. Has to pass `enough::Unstoppable` instead.

**Fix:** Remove the `Clone` bound on `Stop`. Accept `&dyn Stop` or `impl Stop` without `Clone`.

### 14. No metadata support

GIF supports XMP via application extension block (`XMP DataXMP`) and ICC via application extension (`ICCRGBG1012`). Neither is extracted on decode or embeddable on encode. Not a blocker for v1 but worth noting.

### What works well

- `Decoder::new()` gives metadata without decoding frames — good for probe
- `Limits` struct is clean and consistent with zenwebp
- `FrameInput::from_bytes()` is convenient
- `EncoderConfig` builder pattern is straightforward

---

## zenavif

### 15. No metadata extraction on decode

`zenavif::decode_with()` returns `DecodedImage` with no ICC/EXIF/XMP. The AVIF container (ISOBMFF) stores these in `colr` (ICC), `Exif`, and `mime` (XMP) boxes. This is the biggest metadata gap across all codecs.

**Fix:** Extract and expose metadata from the ISOBMFF container during decode.

### 16. No ICC or XMP embedding on encode

ravif only supports EXIF (via `with_exif()`). ICC profiles belong in a `colr` box, XMP in a `mime` box. Both are standard AVIF/ISOBMFF features.

**Fix:** Add `with_icc_profile()` and `with_xmp()` to ravif's `Encoder`.

### 17. `AvifDecoder` only available behind `asm` feature

The struct-based `AvifDecoder` API only compiles with the `asm` feature gate. Without it, only `decode()` / `decode_with()` free functions are available. No streaming or incremental parsing in non-asm builds.

**Fix:** Make `AvifDecoder` available regardless of feature flags, or document this clearly.

### 18. No probe-only function

To get image dimensions and alpha, the adapter must fully decode the image. No lightweight metadata inspection.

**Fix:** Add a `probe()` or `read_info()` that parses the ISOBMFF container without decoding pixels.

### What works well

- ravif's builder pattern (`Encoder::new().with_quality().with_speed()`) is clean
- `DecodedImage` enum variants map naturally to `PixelData`
- `DecoderConfig::frame_size_limit()` builder method is a good pattern

---

## png (external crate)

No issues. Exemplary API:

- `Encoder::with_info()` accepting a pre-built `Info` struct is flexible
- `set_compression()` / `set_filter()` give full control
- ICC via `info.icc_profile`, EXIF via `info.exif_metadata`, XMP via `info.utf8_text` iTXt chunks
- `Decoder::new_with_limits()` for memory limits

---

## Cross-crate: Inconsistent Limits types

Every crate has its own Limits struct with different field names and types:

| Crate | Width type | Pixels field | Memory type |
|-------|-----------|-------------|-------------|
| zenjpeg | N/A | `max_pixels: u64` (pub field) | `max_memory: usize` |
| zenwebp | `max_width: Option<u32>` | `max_total_pixels: Option<u64>` | `max_memory: Option<u64>` |
| zengif | `max_width: Option<u16>` | `max_total_pixels: Option<u64>` | `max_memory: Option<u64>` |
| zenavif | N/A | `frame_size_limit(u32)` builder | N/A |
| png | N/A | N/A | `bytes: usize` |

Every adapter has a `to_X_limits()` conversion function.

**Fix:** Consider a shared limits type in `enough` (already a common dependency), or at minimum use consistent field names and types (`Option<u64>` everywhere).

---

## Priority

| Priority | Issue | Crate | Effort |
|----------|-------|-------|--------|
| High | #15 No metadata extraction | zenavif | High |
| High | #16 No ICC/XMP on encode | ravif | Medium |
| High | #10+#11 Metadata embedding | zenwebp | Medium |
| Medium | #8+#9 Probe/has_alpha duplication | zenwebp | Low |
| Medium | #1 Probe full decode | zenjpeg | Low |
| Medium | #13 Stop requires Clone | zengif | Low |
| Medium | #19 Inconsistent Limits | all | Medium |
| Low | #2 Limits builder methods | zenjpeg | Low |
| Low | #3 max_memory u64 | zenjpeg | Low |
| Low | #4 Encoder owned vecs | zenjpeg | Low |
| Low | #5 Decoder naming | zenjpeg | Low |
| Low | #6 decode_f32 redundant | zenjpeg | Low |
| Low | #7 PixelFormat vs PixelLayout | zenjpeg | Low |
| Low | #14 GIF metadata | zengif | Medium |
| Low | #17 AvifDecoder feature gate | zenavif | Low |
| Low | #18 AVIF probe | zenavif | Medium |
