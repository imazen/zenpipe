# HDR / Gain Map / Wide Gamut Capability Tree

Draft. This document enumerates every atomic capability needed for a
correct HDR + gain map + wide gamut pipeline, and serves as the
source-of-truth for the test matrix being built out per codec.

Each leaf is a **testable proposition** — something a unit or
integration test can assert to be true or false.

## Scope terms

- **Base image** — the SDR rendition that legacy viewers see
- **Gain map** — the per-pixel log-domain multiplier that reconstructs HDR
- **Gain map metadata** — `min`, `max`, `gamma`, `offset_sdr`, `offset_hdr`, `hdr_capacity_min/max`, `base_rendition_is_hdr` (ISO 21496-1)
- **Merged HDR** — the reconstructed high-dynamic-range image (Rec2020 linear f32, PQ, or HLG)
- **Resplit** — decomposing a merged HDR back into (base + gain map + metadata)
- **CICP** — Coding-Independent Code Points: primaries, transfer, matrix, range quad
- **ColorContext** — our internal type carrying CICP *or* ICC profile

## 1. Core color-metadata plumbing (codec-agnostic)

### 1.1 Type surface
- [ ] `ColorContext` can carry: sRGB / Display-P3 / Rec2020 primaries
- [ ] `ColorContext` can carry: gamma / linear / sRGB-TRC / PQ / HLG transfers
- [ ] `ColorContext` distinguishes full-range from studio-swing
- [ ] CICP ↔ ICC conversion is lossless for the 22 standard sRGB profiles
- [ ] CICP ↔ ICC conversion is lossless for Display-P3 and Rec2020 canonicals
- [ ] Unknown ICC profiles survive round-trip as opaque bytes

### 1.2 Pixel buffer capability
- [ ] `PixelBuffer` can hold ≥10-bit integer
- [ ] `PixelBuffer` can hold f16 (for HDR scene-referred)
- [ ] `PixelBuffer` can hold f32 linear (for HDR working space)
- [ ] Alpha semantics: premultiplied vs straight, linear vs encoded — unambiguous

### 1.3 Gain map data structure
- [ ] `DecodedGainMap` carries per-channel `min/max/gamma/offset`
- [ ] `DecodedGainMap` carries `hdr_capacity_min/max` (log2)
- [ ] `DecodedGainMap` carries `base_rendition_is_hdr` flag
- [ ] `GainMapSource` is a minimal encoder-facing handle (pre-computed)
- [ ] Linear-domain (`GainMapMetadata`) ↔ log-domain (`GainMapParams`) round-trip exact

## 2. Per-codec decode capabilities

For each codec the tree under "decode" must be exhaustive:

```
decode/
├── base pixels
│   ├── 8-bit
│   ├── 10/12-bit (where format supports)
│   ├── f16/f32 (where format supports)
│   └── alpha correct
├── metadata extraction
│   ├── ICC
│   ├── CICP
│   ├── EXIF
│   └── XMP
├── gain map extraction
│   ├── locate payload (MPF, auxiliary image, codestream box)
│   ├── decode payload to pixels
│   ├── parse metadata
│   └── classify direction (base_is_hdr flag)
├── depth map extraction (where applicable)
└── color-context attachment
    ├── CICP → ColorContext when present
    ├── ICC → ColorContext when present
    └── fallback (sRGB assumption, documented)
```

## 3. Per-codec encode capabilities

```
encode/
├── base pixels at native bit depth
├── metadata embed
│   ├── ICC (preserve input profile)
│   ├── CICP (preserve when lossless-equivalent)
│   ├── EXIF
│   └── XMP
├── gain map embed
│   ├── accept pre-computed GainMapSource
│   ├── write ISO 21496-1 metadata
│   ├── write secondary image (MPF / aux / codestream box)
│   └── preserve base rendition untouched (validator check)
└── color-context honoring
    ├── encode at caller's requested primaries
    ├── encode at caller's requested transfer
    └── refuse / warn when requested combination is unsupported
```

## 4. Gain-map math operations (codec-independent)

These live under `ultrahdr-core` or a codec-independent crate; they
compose with any codec that can carry the base + gain map parts.

- [ ] **apply**: `(base_sdr, gain_map, metadata, target_hdr_capacity) → hdr_pixels`
  - LUT-optimized path (precompute once per frame)
  - Streaming path (row-at-a-time)
  - Per-channel and single-channel gain maps both work
- [ ] **resplit**: `(hdr_pixels, base_primaries, target_hdr_capacity) → (base_sdr, gain_map, metadata)`
  - Well-defined inverse of apply
  - Dimensions: gain map can be at reduced resolution vs base
  - Alpha pass-through (gain map doesn't modify alpha)
- [ ] **verify round-trip**: apply(resplit(x)) ≈ x within published tolerance
- [ ] **capacity projection**: reconstruct to target display capacity between `hdr_capacity_min` and `hdr_capacity_max`

## 5. Wide gamut conversions (codec-independent)

- [ ] sRGB → Display-P3 (linear + gamma-encoded paths)
- [ ] sRGB → Rec2020
- [ ] Display-P3 → sRGB (with optional gamut compression)
- [ ] Rec2020 → Display-P3 (with optional gamut compression)
- [ ] PQ ↔ linear light (10000 cd/m² reference)
- [ ] HLG ↔ linear light (variable peak)
- [ ] Scene-linear f32 → any-target via moxcms

## 6. Pipeline-level HDR behavior

- [ ] Resize preserves bit depth (10/12/f16/f32 in → same out)
- [ ] Resize preserves transfer (caller indicates linear vs encoded)
- [ ] Alpha compositing correct for PQ / HLG sources
- [ ] Gain map bundled through pipeline as a `sidecar` plane
- [ ] Gain map resized in lockstep with base
- [ ] `HdrMode` enum selects: `SdrOnly` / `Preserve` / `Reconstruct`
- [ ] When `Reconstruct`, upstream gain map is applied and forwarded as HDR
- [ ] When `Preserve`, gain map rides through untouched for re-embedding

## 7. End-to-end capabilities (matrix cells)

A matrix of `(input_codec, operation, output_codec)` where the full
pipeline is expected to work:

- [ ] JPEG UltraHDR → decode → resize → encode AVIF HDR
- [ ] AVIF HDR → decode → resize → encode JPEG UltraHDR
- [ ] HEIC (iPhone) → decode → resize → encode AVIF HDR
- [ ] Apple ProRAW (DNG) → demosaic → tone-map → encode JPEG UltraHDR
- [ ] JXL HDR → decode → resize → encode JXL HDR (round-trip)
- [ ] JPEG UltraHDR → decode → resize → re-encode JPEG UltraHDR (preserve)
- [ ] Any above pipeline preserves EXIF / XMP / orientation

## 8. Per-codec status cell legend

When filling per-codec rows in the test matrix:

- **✓** — implemented, tested, green
- **○** — implemented, no test / red
- **-** — stub, panics or returns Unsupported
- **×** — format does not support this capability (e.g. PNG has no gain map)
- **?** — unknown; audit needed

The remainder of this document is the per-codec status table. See
`docs/hdr-per-codec.md` for the filled-in matrix (populated once the
audit lands).
