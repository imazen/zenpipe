# zenfilters Roadmap

Consolidated from: `featreq.md`, zenpipe `demo/SPEC.md` §13/§28, imazen/zenfilters#6, and `SIMD_WARP_NOTES.md`.

## Status Key
- ✅ Shipped
- 🔧 Partially done
- ⬜ Not started

---

## Existing (69 filters + document module + warp)

### Photo Filters (✅ shipped, 69 filters)
AdaptiveSharpen, Alpha, AscCdl, AutoContrast, AutoExposure, AutoLevels, AutoTone,
AutoVibrance, AutoWhiteBalance, BasecurveToneMap, Bilateral, BlackPoint, Bloom,
Blur, Brilliance, BwMixer, CameraCalibration, ChannelCurves, ChromaticAberration,
Clarity, ColorGrading, ColorMatrix, Contrast, CubeLut, Dehaze, Devignette,
DtSigmoid, EdgeDetect, Exposure, FilmLook (32 presets), FusedAdjust, GamutExpand,
Grain, Grayscale, HighlightRecovery, HighlightsShadows, HslAdjust, HueCurves,
HueRotate, Invert, Levels, LocalToneMap, MedianBlur, NoiseReduction,
ParametricCurve, Saturation, Sepia, ShadowLift, Sharpen, Sigmoid, Temperature,
Texture, Tint, ToneCurve, ToneEqualizer, Vibrance, Vignette, Warp, WhitePoint,
WhitesBlacks

### Warp / Geometry (✅ shipped, behind `experimental` feature)
- `Rotate` — arbitrary angle, 4 border modes (Crop, Deskew, FillClamp, Fill(color))
- `Warp` — full 3×3 projective matrix, affine 2×3, SIMD-accelerated
- Cardinal rotation detection (90/180/270 → pixel-perfect, no interpolation)
- 4 interpolation modes: Bilinear, Bicubic, Robidoux (default), Lanczos3
- SIMD: AVX2/FMA, NEON, scalar fallback, WASM scalar

### Document Module (✅ shipped)
- `detect_skew_angle()` — Otsu binarize + projection profile, ~0.05° accuracy
- `compute_homography()` — DLT for 4-point correspondence, returns [f32; 9]
- `rectify_quad()` — corners → rectangle transform
- `find_document_quad()` — LSD + polygon fitting for document boundary detection
- `detect_line_segments()` — Line Segment Detector
- `otsu_threshold()` + `binarize()` — adaptive threshold for documents

### Auto Modes (✅ shipped — see AutoModesSpec.md)
- AutoExposure, AutoLevels, AutoContrast, AutoTone, AutoVibrance, AutoWhiteBalance

---

## Priority 1: HDR Tone Mapping (from featreq.md)

### FilmicTonemap ⬜
ACES-style S-curve. Linear RGB, pre-scatter.
- Params: exposure (pre-scale), whitepoint (HDR peak → 1.0)
- ~50 lines, point operation, trivially SIMD-able

### ReinhardTonemap ⬜
Extended Reinhard for photography.
- Params: exposure, l_max
- ~30 lines, point operation

### Bt2390Tonemap ⬜
ITU BT.2390 EETF for broadcast HDR10/HLG.
- Params: source_peak (nits), target_peak
- Cubic Hermite spline soft rolloff
- ~80 lines

### Pipeline integration ⬜
These operate in **linear RGB before Oklab scatter**. Options:
1. Pre-scatter hook on Pipeline (cleanest)
2. Separate pipeline phase
3. Dual-path Pipeline (HDR → tonemap → scatter → filters → gather)

---

## Priority 2: Generic Convolution API ✅

### Custom kernel filter ✅ (`worktree-feature-requests`)
`Convolve` filter with `ConvolutionKernel` enum:
- `Separable { h_coeffs, v_coeffs }` — two-pass O(w+h) per pixel
- `Matrix { coeffs, width, height }` — direct O(N*M) per pixel
- Factory kernels: `gaussian()`, `box_blur()`, `emboss()`, `emboss_angle()`, `ridge_detect()`, `sharpen_3x3()`
- Configurable normalize, bias, target channels

---

## Priority 3: Directional Blur ✅

### MotionBlur ✅ (`worktree-feature-requests`)
Uniform-weighted line kernel at arbitrary angle + length.

### ZoomBlur ✅ (`worktree-feature-requests`)
Radial zoom blur from configurable center point with distance-based falloff.

### GaussianMotionBlur ✅ (`srgb_compat` module)
Gaussian-weighted line kernel matching IM's `-motion-blur`.

### RotationalBlur ⬜
Radial blur from center point. Sample along circular arcs, weighted average.

---

## Priority 4: Polar Warp Extensions ✅

### PolarWarp ✅ (`worktree-feature-requests`, behind `experimental`)
`PolarWarp` enum with 4 variants, each computing custom (sx, sy) per pixel:

- **Swirl** ✅ — `θ += strength * (1 - r/radius)`
- **Implode** ✅ — `r' = r^factor`
- **Wave** ✅ — `y' = y + A*sin(x*f)` with direction control
- **Barrel** ✅ — `r' = r*(1 + k1*r² + k2*r⁴ + k3*r⁶)` lens distortion

All use existing bicubic interpolation infrastructure with clamped edges.

---

## Priority 5: Morphology ✅

### Basic morphological operations ✅ (`worktree-feature-requests`)
`Morphology` filter with `MorphOp` enum:
- Erode, Dilate, Open, Close, TopHat, BlackHat
- Square structuring element, configurable radius 1-5
- Optional chroma processing
- 99+ zensim agreement with ImageMagick

---

## Priority 6: Artistic Effects ✅ (partial)

### Implemented ✅ (`worktree-feature-requests`):
| Effect | Implementation |
|--------|---------------|
| Posterize | `Posterize` (Oklab L/chroma) + `ChannelPosterize` (sRGB all-channel) |
| Solarize | `Solarize` (Oklab) + `ChannelSolarize` (sRGB) |
| Emboss | `Convolve::emboss()` (3×3 kernel) + `DifferenceEmboss` (IM-compat blur→diff) |

### Not started ⬜:
| Effect | Recipe |
|--------|--------|
| Charcoal | `Grayscale → EdgeDetect(Sobel) → Invert → Blur(0.5)` |
| Sketch | `Grayscale → EdgeDetect(Canny) → Invert → Grain(0.1)` |
| OilPaint | Kuwahara filter (new algorithm, ~200 lines) |

## Priority 6b: ImageMagick Compatibility ✅

### Architecture ✅ (`worktree-feature-requests`)
- `WorkingSpace::Srgb` on `PipelineConfig` — sRGB passthrough scatter/gather
- `PlaneSemantics` enum on Filter trait — push-time validation
- Separate filter types in `srgb_compat.rs` — each does one thing, no dual-behavior

### sRGB compat filters ✅ (10 filters):
`LinearContrast`, `LinearBrightness`, `SigmoidalContrast`, `HslSaturate`,
`LumaGrayscale`, `ChannelPosterize`, `ChannelSolarize`, `ChannelSharpen`,
`DifferenceEmboss`, `GaussianMotionBlur`

### Zensim agreement vs IM 6.9.11 (5 images):
| Operation | Score | Notes |
|-----------|-------|-------|
| Morphology | 99 | Pixel-perfect |
| Solarize | 99 | Pixel-perfect |
| Brightness | 95-99 | Additive offset |
| Contrast | 95 | tan(π*(1+C/100)/4) slope |
| Saturation | 94-95 | HSL with unclamped S |
| Grayscale | 94 | Rec.709 luma |
| Blur | 72 | Kernel radius convention differs |
| Sharpen | 67-69 | USM on all channels |
| Posterize | 40 | Rounding edge cases |
| Emboss/Edge/MotionBlur | <0 | Fundamentally different algorithms |

---

## Priority 7: Gamut Operations (from featreq.md)

### Gamut Expansion ⬜
sRGB → Display P3 / BT.2020 with intelligent chroma expansion.
- Oklch chroma boost at gamut boundary (simple, works in existing Oklab pipeline)
- More sophisticated: 3D LUT or neural network (from dead zenimage code)

### Gamut Compression 🔧
`GamutMapping::SoftCompress` exists in pipeline gather step.
- Verify it handles P3 → sRGB correctly
- May need per-channel awareness

---

## Feature Flags

| Feature | What it enables | Default |
|---------|----------------|---------|
| `zennode` | Node definitions + `node_to_filter()` bridge | Off |
| `experimental` | `Rotate` and `Warp` zennode defs | Off |
| `document` | Document module (deskew, homography, quad, LSD, otsu) | On |
| `wasm128` | WASM SIMD tier in `incant!` calls | On (via archmage) |
| `parallel` | Rayon parallelism for windowed filters | Off |

---

## Node-to-Filter Bridge Gaps

### Currently missing from `node_to_filter()`:
| Node | Status | Issue |
|------|--------|-------|
| `zenfilters.dt_sigmoid` | ⬜ | `DtSigmoid` doesn't impl `Filter` — needs wrapper |

### All other nodes (43/44) are fully bridged.

---

## Testing

### Current: 443+ lib tests, 1 integration test (all passing)
### Added (`worktree-feature-requests`):
- 29 unit tests for new filters (convolve, morphology, motion blur, posterize, solarize, polar warp)
- 9 unit tests for sRGB compat filters
- `imageflow_comparison` integration test — 21 operations × 5 images vs ImageMagick, zensim scoring
### Needed:
- Warp round-trip tests (rotate → inverse rotate → compare)
- Document pipeline integration tests (full quad→rectify→deskew→crop→enhance chain)
- Film preset visual regression tests
- WASM target compilation test in CI
