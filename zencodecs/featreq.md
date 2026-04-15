# Feature Requests from zenimage

zenimage currently maintains ~2,500 lines of thin codec wrappers (zenjpeg.rs, image_png/,
webp/, gif/, avif/) plus ~1,600 lines of registry/policy/auto-selection infrastructure.
Most of this could be eliminated if zencodecs exposed the streaming capabilities that
already exist in the underlying zencodec trait crate.

## 1. Streaming Decode API [DONE]

**Priority: High**

zenimage requires strip-by-strip decode for O(strip x depth) memory. The `zencodec`
trait crate already defines `StreamingDecode` with `next_batch()`, and zenjpeg/zenpng
implement it. zencodecs just doesn't expose it.

```rust
// What zenimage needs:
let decoder = DecodeRequest::new(data)
    .streaming_decoder()?;  // Returns Box<dyn StreamingDecode>

// Pull strips on demand:
while let Some((y, pixels)) = decoder.next_batch()? {
    pipeline.feed(y, pixels);
}
```

Also useful: `registry.streaming_available(format)` to query whether a given format
supports streaming decode (vs. full-frame fallback).

## 2. Codec Policy / Killbits [DONE]

**Priority: Medium**

zenimage has a per-job `CodecPolicy` that disables specific codecs, allows only certain
codecs, and applies format preferences with priority bonuses. zencodecs' registry is
simpler (enable/disable at startup).

Needed:
- Per-request codec restrictions (not just global registry state)
- Killbit set: "don't use zenjpeg for this request, fall back to zune-jpeg"
- Allowlist: "only use pure-Rust codecs for this request"
- Priority bonuses: "prefer WebP over JPEG when both are viable"

## 3. Fallback Chains [PARTIAL]

**Priority: Medium**

When a decoder fails (corrupt data, unsupported subformat), zenimage tries the next
lower-priority decoder for the same format. zencodecs would need:

- Ordered decoder list per format (by priority)
- Automatic fallback on decode error (configurable: on/off per request)
- Error reporting: which decoders were tried, which failed, why

## 4. Format Auto-Selection for Encode [DONE]

**Priority: Medium**

zenimage has ~1,100 lines in `auto_codec.rs` mirroring imageflow's format selection
logic. This chooses the best output format based on:

- Input properties (has_alpha, has_animation, is_lossless, is_hdr)
- Available encoders (feature-gated at compile time)
- Quality profile (Lowest → Lossless, with per-format interpolation)
- Allowed formats (web_safe, modern_web_safe, all)
- Device pixel ratio adjustment

If zencodecs provided a `format_auto_select()` with these inputs, zenimage could
drop its copy.

## 5. Streaming Encode API [DONE]

**Priority: Low (future)**

Currently no zen codec supports row-level streaming encode. When they do, zencodecs
should expose it:

```rust
let mut encoder = EncodeRequest::new(ImageFormat::Jpeg)
    .quality(85)
    .streaming_encoder(width, height, pixel_format)?;

for strip in strips {
    encoder.encode_strip(strip)?;
}
let output = encoder.finish()?;
```

## 6. Metadata Extraction at Probe Time [DONE]

**Priority: Low**

zenimage extracts ICC profiles and EXIF during the probe/decode phase for color
management decisions. zencodecs probes format and dimensions but doesn't yet expose
metadata without a full decode. Useful:

- `probe_with_metadata(data)` → ImageInfo + ICC + EXIF
- Or: metadata available from streaming decoder before first `next_batch()`

---

## What This Would Replace in zenimage

| zenimage file | Lines | Replaced by |
|---------------|-------|-------------|
| io/streaming/zenjpeg.rs | ~700 | zencodecs streaming decode/encode |
| io/streaming/zune_jpeg.rs | ~200 | zencodecs fallback chain |
| io/streaming/image_png/ (3 files) | ~400 | zencodecs streaming decode/encode |
| io/streaming/webp/ (3 files) | ~350 | zencodecs streaming decode/encode |
| io/streaming/avif/ (3 files) | ~400 | zencodecs streaming decode/encode |
| io/streaming/gif/ (3 files) | ~300 | zencodecs streaming decode/encode |
| io/streaming/auto_codec.rs | ~1,100 | zencodecs format auto-selection |
| io/streaming/registry.rs | ~500 | zencodecs CodecRegistry |
| **Total** | **~3,950** | |

zenimage would keep: types.rs, traits.rs (may shrink), strip.rs, limits.rs,
policy.rs (may shrink), codec_types_bridge.rs (may shrink), animated_sink.rs.
