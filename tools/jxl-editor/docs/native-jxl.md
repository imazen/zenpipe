# Native JPEG XL access and authoring

Open a JPEG XL file and select **Native JXL**, **Coding**, **Splines**, or the
**Timeline** dock to inspect and author native JPEG XL structures directly in
the browser.

## Remaining spec gaps (not yet implemented)

The following ISO/IEC 18181-1 / 18181-2 features are **not** yet authored in the
editor (original source files keep any existing bytes untouched in `.project.jxl`):

- **Chroma subsampling encoding (`4:2:2`, `4:2:0`, `4:4:0` YCbCr)**: Unavailable
  in this build because `jxl-encoder`'s subsampled YCbCr coefficient path
  requires both `chroma-subsampling` and `jpeg-reencoding` features (which depend
  on unpublished `zenjpeg 0.9.0` and `ultrahdr-core`). Existing YCbCr JPEG XL
  files **are** inspected (`doYcbcr`, `jpegUpsampling`, `chromaSubsampling`) and
  reconstructed via `jbrd` when present.
- **Per-extra-channel blend mode overrides on animation frames**: `jxl-encoder`'s
  public `AnimationFrame` API exposes per-frame color `BlendMode` (`Replace`,
  `Add`, `Blend`, `AlphaWeightedAdd`, `Mul`), while the alpha extra channel uses
  `Replace` except on encoder-generated auto-delta frames.
- **Lossless CMYK animation**: Rejected by `jxl-encoder` (`UnsupportedPixelLayout`).
  Still lossless `Cmyk8`/`Cmyk16` and still lossy `Cmyk8`/`Cmyk16` are supported.
- **Manual splines in animations or CMYK**: Rejected by the encoder (manual
  splines apply to still VarDCT XYB frames only).
- **Full-precision HDR / float16 / float32 edited export, gain-map (`jhgm`)
  authoring, arbitrary custom extra-channel authoring (depth, thermal, spot
  color, CFA), custom EXIF/XMP/JUMBF box editing, embedded preview-frame
  authoring, direct VarDCT coefficient / quant-field painting, and manual patch
  dictionary authoring**: Not yet wired; original headers, extra channels, gain
  map presence, EXIF/XMP sizes, and container boxes are inspected and preserved
  byte for byte on the source asset.
- **Progressive partial flush and random-access seeking UI**: The decoder exposes
  keyframe / seek offsets in the inspector JSON, while interactive progressive
  pass scrubbing is not yet wired.

## Verified capabilities

| JPEG XL area | Editor capability | Independent verification |
| --- | --- | --- |
| **Native animation timeline** | Create, duplicate, reorder, delete, and import frames; per-frame layer transforms/visibility/ZenFilters adjustments; live playback; custom TPS numerator/denominator, loop count (`0` = infinite), frame names, durations, SMPTE timecodes, blend modes, reference slots, `ReferenceOnly` frames, and auto-delta trials. Full `.project.jxl` save/reopen and standalone `.jxl` animation export/reopen. | Verified via Chromium Playwright, `native_jxl_info`/`native_jxl_timeline`, pinned `djxl v0.12.0 a7a9c78` (`.apng`), and upstream `jxl_cli` (`.apng`) checking all frames byte for byte, exact durations, and loop count. |
| **Manual native splines** | Author Catmull-Rom Gaussian splines in VarDCT still mode with live canvas spline rendering, interactive click-to-add control points, stroke color-to-XYB DCT conversion, Gaussian sigma width, direct XYB/sigma DCT[0] editing, optional baking onto the VarDCT target before subtraction, clear compatibility gating for Modular/Animation/CMYK, and full `.project.jxl` save/reopen. | Verified via `native_jxl_info` (`coding.flags.splines === true`), `vardctQuantizer`, and visible decoded spline curves in both `djxl v0.12.0` and `jxl_cli`. |
| **Modular coding controls** | Effort (1–10), lossy delta palette, palette color cap, responsive Haar squeeze, RCT (`none`, `subtract_green`, `ycocg`, `gbr_subgr`), modular group size (`128`/`256`/`512`/`1024`), predictor (`0`–`15`), faster decoding (`0`–`4`), modular patches, keep invisible RGB, container box wrapper (`auto`/`always`/`never`). | Every knob tested in Rust unit tests for codestream differentiation and lossless/valid decode; squeeze + 128 px group + YCoCg verified in browser + `djxl` + `jxl_cli`. |
| **VarDCT coding controls** | Butteraugli distance (`0.01`–`15.0`), effort (`1`–`10`), progressive AC (`Single`, `QuantizedAcFullAc`, `DcVlfLfAc`), progressive DC LF frame (`0`/`1`), noise synthesis (`off`, `content`, `photon_iso`, `manual_lut`), Wiener denoise, patches, auto splines, gaborish, EPF (`-1`..`3`), spatial resampling (`1×`/`2×`/`4×`/`8×`), center-first AC groups, lossy alpha distance & squeeze, faster decoding (`0`–`4`), encoder strategy (`zenjxl`, `libjxl`, `lean_faster`), container wrapper. | Progressive 3-pass + LF frame (`useLfFrame`), photon noise (`flags.noise`), gaborish/EPF, and `2×` resampling verified via bitstream header inspection (`native_jxl_info`) and decoded by both `djxl` and `jxl_cli`. |
| **CMYK (`Cmyk8` & `Cmyk16`, Level 10)** | Auto-detects opened CMYK JXL files and extracts exact inverted-ink samples (`0 = full ink`, `max = no ink`) and embedded CMYK ICC. Supports lossless Modular `Cmyk8`/`Cmyk16` (exact C, M, Y, K codes + Black extra channel 0 + exact ICC bytes), lossy VarDCT CMYK (explicitly labeled as ICC conversion to linear sRGB XYB while retaining Black extra channel 0 + CMYK ICC), custom `.icc` upload, built-in minimal subtractive CMYK ICC, live `MoxCms` preview, per-channel ink offsets, and `.project.jxl` raw CMYK (`zncm`/`znic`) save/reopen. | Verified via `native_extract_cmyk`, `djxl --orig_icc_out`, and `jxl_cli --allow-level10 out.npy --original-icc-out` checking all CMYK8 codes, Level 10 Black channel, and ICC bytes. |
| **Native JXL inspector** | Inspects regular frame geometry, signed origin, per-channel blends, reference slots, durations, timecodes, exact FrameHeader coding flags (`encoding`, `flags`, `numPasses`, `upsampling`, `doYcbcr`, `chromaSubsampling`, `modularGroupSize`, `gaborish`, `epfIters`), `codestreamLevel`, `vardctQuantizer`, `hasGainMap`, `exifBytes`, `xmpBytes`, `hasFrameIndex`, keyframes, raw F32 plane download, embedded ICC download, container boxes, and lossless JPEG (`jbrd`) reconstruction. | Verified across relaxed SIMD and compatibility Wasm workers in Chromium plus `just browser-jpeg` and `just oracle`. |

## Codec architecture

- Main codec module (`tools/jxl-editor/Cargo.toml`): `jxl-encoder` `459e86b7` with
  `features = ["std"]` (enabling `moxcms`-backed lossy CMYK conversion on both
  native and `wasm32-unknown-unknown`) and `zenjxl-decoder` `025852d2`.
- Native inspection/timeline/CMYK module (`native-wasm/`): `zenjxl-decoder`
  `907b91b9` (PR 75 qualified local F32 frames, `MoxCms`, `scanned_frames`,
  `vardct_quantizer`, `gain_map`, `frame_index`, `reconstruct_jpeg_with`) paired
  with a `#![forbid(unsafe_code)]` ISO/IEC 18181-1 `FrameHeader` parser.
- ZenFilters module (`filters-wasm/`): Oklab exposure, contrast, and saturation.

## Verification commands

```bash
just test
just build
just wasm-audit
just serve &
just browser-jpeg <jpeg-carrier.jxl> <expected.icc>
just oracle test-results
```
