# zencodecs fuzz testing

Fuzz targets for the zencodecs unified codec dispatch layer, covering format
detection, decode, encode, transcode, EXIF parsing, gain map / depth map
extraction, and format auto-selection.

## Requirements

- Nightly Rust: `rustup toolchain install nightly`
- cargo-fuzz: `cargo install cargo-fuzz`

## Targets

| Target | Priority | Description |
|--------|----------|-------------|
| `fuzz_probe` | HIGH | Format detection + header-only metadata parse (all 15+ formats) |
| `fuzz_decode` | HIGH | Full-frame decode through dispatch with tight limits |
| `fuzz_exif` | HIGH | EXIF/TIFF IFD parser on untrusted binary data |
| `fuzz_decode_limits` | HIGH | Structured: arbitrary limits + data, assert limits obeyed |
| `fuzz_gainmap` | MEDIUM | UltraHDR MPF/XMP parse + gain map decode |
| `fuzz_depthmap` | MEDIUM | Depth map extraction + resize (bilinear interpolation) |
| `fuzz_push_decode` | MEDIUM | Streaming push decode with counting sink |
| `fuzz_animation` | MEDIUM | Animation frame iteration (GIF, WebP, APNG) |
| `fuzz_transcode` | MEDIUM | Structured: decode→encode with arbitrary target format |
| `fuzz_roundtrip` | LOW | Structured: encode small image → decode → assert match |
| `fuzz_select` | LOW | Format auto-selection logic with arbitrary ImageFacts |

## Running

```bash
# From the zencodecs root:

# Seed corpus (local sibling crates + external GitHub repos)
just fuzz-seed

# Seed corpus (local only, no network)
just fuzz-seed-local

# List targets
just fuzz-list

# Run a specific target (auto-seeds local corpus first)
just fuzz fuzz_probe -- -max_total_time=60

# Quick smoke test: all targets, 60s each
just fuzz-smoke

# Deep fuzzing: all targets, 30min each (seeds external corpora first)
just fuzz-deep

# CI mode: high-priority targets only
just fuzz-ci 60

# Coverage report
just fuzz-cov fuzz_decode
```

Or use cargo-fuzz directly:

```bash
cargo +nightly fuzz run fuzz_decode fuzz/corpus/seed/mixed -- \
    -dict=fuzz/multiformat.dict \
    -max_total_time=300
```

## Corpus

Seeds are gathered from sibling codec crate fuzz corpora:

| Source | Format | Files |
|--------|--------|-------|
| zenjpeg seed corpus | JPEG | ~9 |
| zenpng fuzz corpus | PNG | ~687 |
| zengif test corpus | GIF | ~3 |
| zenavif fuzz corpus | AVIF | ~86 |
| zenjxl-decoder resources | JXL | ~146 |
| zenbitmaps fuzz+test | BMP/PNM/PAM | ~950 |
| zencodecs test images | JPEG (UltraHDR) | 1 |
| Crafted EXIF blobs | EXIF | 3 |

External sources (fetched by `seed_corpus.sh` without `--local-only`):

- [dvyukov/go-fuzz-corpus](https://github.com/dvyukov/go-fuzz-corpus) — GIF, PNG, JPEG
- [libjpeg-turbo/fuzz](https://github.com/libjpeg-turbo/fuzz) — JPEG
- [image-rs/image](https://github.com/image-rs/image) — multi-format test images

Optional (requires gsutil):
- OSS-Fuzz corpora for libjxl, libpng, libwebp, ImageMagick

## Dictionary

`multiformat.dict` contains magic bytes and structural tokens for all 15+
supported formats: JPEG, PNG, GIF, WebP, AVIF, HEIC, JXL, TIFF, BMP, PNM,
PAM, QOI, Farbfeld, TGA, Radiance HDR, and EXIF.

## Security goals

- **No panics** on any input (including truncated, malformed, or adversarial)
- **No OOM** when limits are set (limits must be enforced, not advisory)
- **No excessive CPU** — decompression bombs must be bounded
- **Graceful rejection** of unrecognized or corrupt data
