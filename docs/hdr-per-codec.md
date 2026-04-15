# HDR / Gain Map / Color Metadata — Per-Codec Status & Test Plan

Generated from a full audit of `zencodecs/src/codecs/*.rs` and the
end-to-end color-context flow through zenpipe. See
`hdr-capability-tree.md` for the abstract capability tree.

Legend: **✓** working + tested / **○** working but no test / **-** stub / **×** N/A / **?** unknown.

## Per-codec capability matrix

### Decode

| Codec | Base | Bit depth | ICC | CICP | EXIF | XMP | Gainmap | Depth | HDR transfer | Wide gamut |
|---|---|---|---|---|---|---|---|---|---|---|
| JPEG | ✓ | 8 | ✓ | × | ✓ | ✓ | ✓ (UltraHDR MPF) | ✓ (MPF disparity) | × | × |
| WebP | ✓ | 8 | ✓ | × | ✓ | ✓ | × | × | × | × |
| PNG | ✓ | 8 (16→8) | ✓ | - (cICP chunk not surfaced) | ✓ | ✓ | × | × | × | × |
| GIF | ✓ | 8 | × | × | × | × | × | × | × | × |
| AVIF | ✓ | 8 / 10 / 12 (U16 buffer) | ✓ | ✓ | ✓ | ✓ | ✓ (tmap AV1 OBU) | - (auxl stub) | ✓ (CICP carries) | ✓ (Rec.2020 via CICP) |
| JXL | ✓ | 8 / 10 / 12 / f32 | ✓ | ✓ | ✓ | ✓ | ✓ (jhgm — **inverse** direction) | × | ✓ | ✓ (Rec.2020 / P3) |
| HEIC | - | - | - | - | - | - | - | - (stub) | - | - |
| RAW/DNG | ✓ | 16 / f32 | ✓ | × (DNG color matrices) | ✓ (DNG tags) | ✓ | ✓ (Apple ProRAW MPF) | × | × | ✓ (camera primaries) |
| TIFF | ○ | 16 | ○ | × | ○ | ○ | × | × | × | × |
| BMP | ✓ | 8 / 16 | × | × | × | × | × | × | × | × |
| PNM/PFM | ✓ | 8 / f32 (PFM) | × | × | × | × | × | × | × | × |
| Farbfeld | ✓ | 16 (U16 buffer) | × | × | × | × | × | × | × | × |

### Encode

| Codec | Base | Bit depth | ICC | CICP | EXIF | XMP | Gainmap (resplit) | HDR | Wide gamut |
|---|---|---|---|---|---|---|---|---|---|
| JPEG | ✓ | 8 | ✓ | × | ✓ | ✓ | **✓** `encode_with_precomputed_gainmap` + `encode_ultrahdr_*_f32` | ✓ (UltraHDR f32 path) | × |
| WebP | ✓ | 8 | ○ | × | ○ | ○ | × | × | × |
| PNG | ✓ | 8 / 16 | ○ | × (cICP chunk available) | ○ | ○ | × | × | × |
| GIF | ✓ | 8 | × | × | × | × | × | × | × |
| AVIF | ✓ | 8 (10/12 not surfaced) | ○ | ○ (auto colr box) | ○ | ○ | **✓** `encode_with_precomputed_gainmap` (tmap) | - (8-bit only via trait) | ○ (via CICP) |
| JXL | ✓ | 8 / HDR | ○ | ○ | ○ | ○ | **✓** `encode_with_precomputed_gainmap` (jhgm, inverse) | ✓ (HDR primary mode) | ○ |
| HEIC | × | - | - | - | - | - | - | - | - |
| RAW/DNG | × | × | × | × | × | × | × | × | × |
| TIFF | ○ | 8 / 16 | ○ | × | ○ | ○ | × | × | × |
| BMP | ✓ | 8 / 32 | × | × | × | × | × | × | × |
| PNM/PFM | ✓ | 8 / f32 | × | × | × | × | × | × | × |
| Farbfeld | ✓ | 16 | × | × | × | × | × | × | × |

### Critical pipeline-level findings

1. **`graph.rs:1608` and `:1617`** — `ensure_fmt!(source, format::RGBA8_SRGB, ...)`
   force-narrows every decoded image to 8-bit sRGB before the layout/composite
   nodes run. Bit depth, transfer, and primaries are all lost.
2. **`sources/resize.rs:46-51`** — refuses any input that isn't `RGBA8_SRGB`.
3. **`sources/effects.rs:51-53`** — same enforcement on filter / rotation.
4. **`job.rs:831-840`** — gain map is decoded and stored with `RGB8_SRGB`
   descriptor regardless of the codec's actual gain-map bit depth/transfer.
5. **`job.rs:1004-1006`** — encode receives sRGB pixels but the original
   ICC/CICP from the source. Pixels and metadata silently disagree.
6. **No tone mapping** — `TransferFunction::Pq` and `Hlg` exist on
   `PixelDescriptor` but no resize/composite path linearizes them.

These are not codec bugs; they are pipeline gaps. The codec-level
infrastructure (per the audit above) is in good shape.

## Test plan: ground-up, codec-by-codec

Each codec gets a dedicated test file that proves each leaf capability
in isolation. We use `assert!` for binary truths and zensim/PSNR for
numerical fidelity. Tests are added in priority order:

### Tier 1 — finish what's already half-tested

These codecs already have partial coverage; we fill the matrix.

#### `tests/jpeg_capability.rs`
Already substantial coverage in `tests/ultrahdr.rs` (12 tests) and
`tests/gainmap_e2e.rs` (12 tests). Gaps to close:

- ICC profile round-trip (encode arbitrary ICC bytes, decode, byte-equal)
- EXIF round-trip (orientation tag specifically — preserved through encode)
- XMP round-trip (UltraHDR XMP packet specifically — preserved)
- Gain map metadata field-by-field assertion (all 8 ISO 21496-1 fields)
- AMPF (iPhone 17 Pro) detection — currently goes through JPEG path; verify
- Depth map (MPF disparity) — minimal extraction test
- Negative case: decode regular JPEG → `decode_gain_map()` returns None

#### `tests/avif_capability.rs`
- Decode AVIF SDR 8-bit base case
- Decode AVIF with embedded ICC profile, byte-equal round-trip
- Decode AVIF with CICP `(9, 16, 9, true)` (BT.2100 PQ) — assert SourceColor.cicp
- Decode AVIF with embedded gain map (tmap aux image) → assert metadata fields
- Encode AVIF + precomputed gain map (`encode_with_precomputed_gainmap`)
- Encode → decode → re-extract gain map, compare metadata exact
- Negative: decode AVIF without gain map → `decode_gain_map()` returns None
- 10/12-bit decode: today we get RGBA8 (tone-mapped). Test asserts current behavior + xfail for true 10-bit pass-through.

#### `tests/jxl_capability.rs`
- Decode JXL HDR (synthetic Rec.2020 + PQ image → encode → decode round-trip)
- Decode JXL with gain map (jhgm) — note **inverse direction** (base = HDR)
- Encode + precomputed gain map round-trip
- ICC / CICP / EXIF / XMP each round-trip
- Negative: regular JXL → no gain map

#### `tests/raw_capability.rs`
- Decode rawloader-supported file (NEF/CR2/ARW)
- Decode rawler-supported file (CR3/X-Trans)
- Decode darktable backend file (any RAW)
- Decode Apple ProRAW DNG → extract gain map (Apple MPF path)
- Decode Apple ProRAW DNG → assert DNG EXIF tags (color_matrix, as_shot_neutral, etc.)
- Decode iPhone AMPF DNG → detected as JPEG path, gain map extracted

### Tier 2 — fill in untested but functional codecs

#### `tests/webp_capability.rs`
- Base encode/decode round-trip
- Lossy quality → quality
- Lossless mode round-trip pixel-identical
- ICC profile round-trip
- EXIF/XMP round-trip
- Animation: encode 3 frames, decode all 3, verify per-frame delays
- Negative: `decode_gain_map()` returns None for any WebP
- Negative: `decode_depth_map()` returns None

#### `tests/png_capability.rs`
- 8-bit RGBA round-trip
- 16-bit RGBA: encode 16-bit input, decode, verify bit depth (today we narrow → assert current)
- 1/2/4-bit grayscale decode
- iCCP chunk round-trip
- iTXt EXIF round-trip
- iTXt XMP round-trip
- APNG animation round-trip
- Negative gain map / depth map

#### `tests/gif_capability.rs`
- Static GIF round-trip
- Animated GIF: 3 frames + delays
- Quality mapping (zenquant integration)
- Palette dithering options each produce different output
- Negative metadata (no ICC/EXIF/XMP/gain map/depth)

#### `tests/tiff_capability.rs`
- Basic 8-bit RGBA
- 16-bit RGBA preserved (or assert current narrowing)
- f32 TIFF (where supported)
- ICC / EXIF / XMP round-trip

#### `tests/bitmap_capability.rs` (BMP / PNM / Farbfeld in one file)
- BMP 8/24/32-bit round-trips
- PNM (PBM/PGM/PPM/PAM) round-trips
- PFM (32-bit float) round-trip — verify bit depth preserved
- Farbfeld 16-bit round-trip — verify bit depth preserved
- All have no metadata; assert that any provided metadata is dropped without error

### Tier 3 — HEIC (blocked, just stubs)

#### `tests/heic_capability.rs`
- All tests `#[ignore]` with a note explaining heic-decoder isn't cloned locally
- Stub each capability so they activate when the feature lands

### Tier 4 — Cross-codec / pipeline-level (will fail today, document gaps)

#### `tests/cross_codec_gainmap.rs`
- JPEG UltraHDR → decode → encode AVIF + precomputed gainmap → decode AVIF + gainmap → metadata equal within tolerance
- AVIF (with gainmap) → JPEG UltraHDR round-trip
- JXL (inverse direction) → JPEG UltraHDR (forward direction) — verify direction inversion is handled
- DNG (Apple ProRAW) → JPEG UltraHDR (gainmap from RAW preview MPF re-embedded in JPEG)

#### `tests/wide_gamut_pipeline_gap.rs`
**Document the current narrowing — these are EXPECTED FAILURES today**, marked
`#[should_panic]` or `#[ignore = "TODO: pipeline forces RGBA8_SRGB"]` so the
gap is loudly visible in test output:

- Decode AVIF Rec.2020 PQ → run through `ImageJob` resize → encode AVIF →
  decode → assert primaries still Rec.2020. **WILL FAIL** today.
- Same with Display-P3.
- Decode 10-bit AVIF, run no ops, encode 10-bit AVIF, verify no bit-depth loss.
- Decode JXL HDR f32 → resize → encode JXL HDR → verify f32 preserved.
- Decode JPEG with Display-P3 ICC → encode JPEG → byte-compare ICC profile preserved.

#### `tests/hdr_reconstruct_pipeline.rs`
- Use `ImageJob` with `hdr_mode = "hdr_reconstruct"` on UltraHDR JPEG input.
- Verify pipeline applies the gain map to produce HDR pixels.
- **Today**: gain map flows as sidecar but no apply step is in the pipeline.
  Test should xfail / ignore until that's wired.

## Execution order

1. **`tests/jpeg_capability.rs`** — fills the most-mature codec; establishes
   the test pattern (helpers, fixtures, naming).
2. **`tests/avif_capability.rs`** — second most mature; mirrors JPEG layout.
3. **`tests/jxl_capability.rs`** — covers the inverse-direction case.
4. **`tests/raw_capability.rs`** — Apple ProRAW MPF gain map path.
5. **`tests/webp_capability.rs`** + **`tests/png_capability.rs`** — clear cases
   without gain maps; cement metadata round-trip patterns.
6. **`tests/gif_capability.rs`** + **`tests/tiff_capability.rs`** + **`tests/bitmap_capability.rs`** — fill remaining matrix cells.
7. **`tests/cross_codec_gainmap.rs`** — exercises gainmap re-split across codecs.
8. **`tests/wide_gamut_pipeline_gap.rs`** + **`tests/hdr_reconstruct_pipeline.rs`** — capture the pipeline-level gaps as live xfail tests so progress is measurable.

After Tier 4 lands, the pipeline-narrowing gaps in `graph.rs` /
`sources/resize.rs` / `sources/effects.rs` become a concrete refactor plan:
introduce a `WorkingFormat` enum that selects between `RGBA8_SRGB` (today) and
`RGBAF32_LINEAR_PREMUL` / `RGBA16_LINEAR_PREMUL` — and route HDR sources
through the linear-light path.

## Test scaffolding

Helpers go in `tests/common/mod.rs` (already exists). Add:

- `make_synthetic_hdr_rgb_f32(w, h, peak) -> ImgVec<Rgb<f32>>` — already exists
  in gainmap_e2e.rs; lift to common.
- `make_solid_color_with_icc(profile_bytes, color) -> Vec<u8>` per codec.
- `assert_gain_map_metadata_eq(a, b, tolerance)` — compare ISO 21496-1 fields.
- `assert_cicp_eq(actual, expected)` — compare 4-tuple.
- `assert_icc_byte_equal(a, b)` — assert ICC profile bytes preserved verbatim.
- `decode_and_extract_all(bytes, format) -> (pixels, info, gainmap?, depth?)`
  — single helper that exercises every metadata extraction at once.

Fixtures go in `tests/images/` (already exists for some codecs). New:

- `tests/images/p3_gradient.jpg` — JPEG with Display-P3 ICC
- `tests/images/rec2020_pq.avif` — synthetic AVIF Rec.2020 PQ
- `tests/images/jxl_hdr_inverse.jxl` — JXL with jhgm gain map
- `tests/images/iphone17_ampf.dng` — AMPF sample (already in /mnt/v/heic/)
