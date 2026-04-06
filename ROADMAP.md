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

## Priority 2: Generic Convolution API

### Custom kernel filter ⬜
```rust
pub struct Convolve {
    kernel: Vec<f32>,
    width: u32,
    height: u32,
    normalize: bool,
}
```
Extract from existing `GaussianKernel` and edge detection SIMD code.

### Unlocks:
| Effect | Kernel |
|--------|--------|
| Emboss | `[[-2,-1,0],[-1,1,1],[0,1,2]]` |
| Ridge detect | `[[-1,-1,-1],[-1,8,-1],[-1,-1,-1]]` |
| Custom sharpen | User-defined NxN |

---

## Priority 3: Directional Blur

### MotionBlur ⬜
Blur along a direction (angle + length).
- Implementation: line kernel at angle, convolve
- SIMD: extend existing blur infrastructure

### RotationalBlur ⬜
Radial blur from center point.
- Sample along circular arcs, weighted average
- Uses warp infrastructure for sampling

### ZoomBlur ⬜
Radial blur toward/from center.
- Sample along radial lines from center

---

## Priority 4: Polar Warp Extensions

Extend existing `Warp` struct with new coordinate mapping functions:

```rust
pub enum WarpFunction {
    Matrix([f32; 9]),                            // existing affine/projective
    Swirl { strength: f32, radius: f32 },        // θ += strength * (1 - r/radius)
    Implode { factor: f32 },                     // r' = r^factor
    Wave { amplitude: f32, frequency: f32 },     // y' = y + A*sin(x*f)
    Barrel { k1: f32, k2: f32, k3: f32 },       // lens distortion polynomial
}
```

All reuse the existing SIMD warp infrastructure — only the coordinate mapping function changes.

### Barrel/Pincushion ⬜
Lens distortion correction. Critical for:
- Camera lens correction profiles
- Document scanner lens correction

### Swirl ⬜
ImageMagick `-swirl`. Rotation angle varies with distance from center.

### Wave ⬜
ImageMagick `-wave`. Sinusoidal displacement.

### Implode/Explode ⬜
ImageMagick `-implode`. Radial displacement.

---

## Priority 5: Morphology

### Basic morphological operations ⬜
Windowed min/max filter with structuring element (similar to MedianBlur):

```rust
pub enum MorphOp {
    Erode,     // min within kernel — shrink bright regions
    Dilate,    // max within kernel — expand bright regions
    Open,      // erode → dilate — remove small bright noise
    Close,     // dilate → erode — fill small dark holes
    Gradient,  // dilate - erode — edge detection
    TopHat,    // original - open — extract small bright details
    BlackHat,  // close - original — extract small dark details
}

pub struct Morphology {
    op: MorphOp,
    kernel: MorphKernel, // Diamond, Disk(radius), Square(size), Cross, Custom
    iterations: u32,
}
```

Use case: document cleanup, text extraction preprocessing.

---

## Priority 6: Artistic Filter Chains

Composite presets built from existing filters (no new algorithms):

| Effect | Recipe |
|--------|--------|
| Charcoal | `Grayscale → EdgeDetect(Sobel) → Invert → Blur(0.5)` |
| Sketch | `Grayscale → EdgeDetect(Canny) → Invert → Grain(0.1)` |
| Posterize | `Levels` with quantized steps per channel |
| Solarize | Invert pixels above threshold (simple PixelOp) |
| OilPaint | Kuwahara filter (new algorithm, ~200 lines) |

Implementation: `ArtisticPreset` enum that expands to a `Pipeline` of existing filters.

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

### Current: 443 tests (all passing)
### Needed:
- Warp round-trip tests (rotate → inverse rotate → compare)
- Document pipeline integration tests (full quad→rectify→deskew→crop→enhance chain)
- Film preset visual regression tests
- WASM target compilation test in CI
