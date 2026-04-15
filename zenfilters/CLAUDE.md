# zenfilters

Oklab perceptual color space image filter library with SIMD dispatch via archmage.

## Goal Set (2026-03-10)

### 1. Feature Parity with Lightroom

Before training a neural model, zenfilters needs all the adjustment capabilities Lightroom offers. Current coverage: 51 stable filters across exposure, tone, color, detail, and effects (plus Warp behind the experimental feature flag).

**DONE (high priority, completed 2026-03-10):**
- ~~Whites/Blacks sliders~~ → `WhitesBlacks` (smoothstep-weighted extreme luminance control)
- ~~Parametric Tone Curve~~ → `ParametricCurve` (4 zones, 3 movable dividers, LUT-based)
- ~~Sharpening Detail + Masking~~ → `AdaptiveSharpen` now has `detail` + `masking` fields (4 controls)
- ~~Noise Reduction Detail + Contrast~~ → `NoiseReduction` now has `luminance_contrast` + `chroma_detail` (5 controls)
- ~~B&W Channel Mixer~~ → `BwMixer` (8 per-color luminance weights, chroma-aware)
- ~~Camera Calibration~~ → `CameraCalibration` (R/G/B primary hue+sat shifts, shadow tint)

**DONE (2026-03-18, GEGL gap analysis):**
- ~~Median Blur~~ → `MedianBlur` (neighborhood median, L-only or all channels, radius 1-5)
- ~~Edge Detection~~ → `EdgeDetect` (Sobel + Laplacian, gradient magnitude on L, configurable strength)
- ~~Geometric Transform~~ → `Warp` (experimental, 3×3 projective matrix, bilinear interp, rotation/deskew/affine/perspective)
- ~~Masked Filter~~ → `masked::MaskedFilter` (linear gradient, radial gradient, luminance range masks)

**Still missing (lower priority or needs external data):**
- **Tone Curve Saturation refinement** — per-region saturation on the curve
- **Lens Blur** — AI depth-based bokeh with bokeh shape styles
- **Transform/Upright** — perspective correction (auto, guided, level, vertical, full). Warp provides raw matrix support; needs auto-detection via edge analysis.
- **Lens Distortion** — barrel/pincushion correction with profiles
- **Blend Layers** — Oklab-space compositing of two planes with blend modes (design notes in `masked.rs`)

### 2. zentract Integration (Neural Model)

Replace or supplement the 64-cluster K-means model with a proper neural network via zentract (ONNX inference).

- **zentract location**: `/home/lilith/work/zen/zentract/`
- **Architecture**: 3-crate workspace (zentract-types, zentract-abi, zentract-api). Uses dlopen to keep tract's 267-crate dep out of host binary.
- **Plan**: Train an MLP (features -> params) in Python, export ONNX, load via zentract at runtime
- **Current cluster model**: 64 clusters, k=3 inverse-distance blend, +3.2 zensim vs baseline
- **Target**: Continuous prediction (no cluster quantization), better generalization

### 3. Better Image Comparison Metric

**DONE (core infrastructure, 2026-03-10):**
- `regional.rs` module: `RegionalFeatures::extract()` + `RegionalComparison::compare()`
- 5 luminance zones × 32-bin L histograms + chroma mean
- 4 chroma zones × 32-bin L histograms
- 6 hue sectors × 32-bin a + b histograms
- Weighted aggregate score (midtones > extremes, skin > sky, saturated > neutral)

**TODO:** Integrate into parity/comparison examples, validate against zensim on real data

### 4. ImageMagick Compatibility (`worktree-feature-requests` branch)

**Architecture**: `WorkingSpace::Srgb` on `PipelineConfig` controls scatter/gather only (sRGB passthrough instead of Oklab conversion). Separate filter types for sRGB math — each filter does one thing, no dual-behavior branching.

**`PlaneSemantics`** enum on `Filter` trait: `Any` (generic spatial ops), `Oklab` (default, Oklab-native), `Rgb` (sRGB compat). Pipeline validates at push time.

**sRGB compat filters** (`src/filters/srgb_compat.rs`):
- `LinearContrast` — `(v-0.5)*factor+0.5` per plane
- `LinearBrightness` — `v+offset` per plane
- `SigmoidalContrast` — S-curve for `-sigmoidal-contrast`
- `HslSaturate` — RGB→HSL→scale S→RGB (unclamped S, RGB clamp)
- `LumaGrayscale` — Rec.709 luma
- `ChannelPosterize` — quantize all planes uniformly
- `ChannelSolarize` — threshold inversion on all planes
- `ChannelSharpen` — USM on all planes
- `DifferenceEmboss` — blur→directional difference→bias
- `GaussianMotionBlur` — Gaussian-weighted line kernel

**New generic filters** (Issues #2, #6):
- `Convolve` — separable + matrix convolution with factory kernels
- `MotionBlur` / `ZoomBlur` — directional and radial blur
- `Posterize` / `Solarize` — Oklab-native versions (L-only or L+chroma)
- `Morphology` — erode, dilate, open, close, tophat, blackhat
- `PolarWarp` — swirl, implode, wave, barrel distortion

**IM formula notes** (empirically verified against IM 6.9.11 Q16):
- `-brightness-contrast BxC`: brightness = additive `B/100`, contrast = `slope = tan(π*(1+C/100)/4)` then `output = slope*(input-0.5)+0.5` clamped
- `-modulate 100,S,100`: HSL with **unclamped S** — let S exceed 1.0, clamp final RGB
- `-posterize N`: `round(v*(N-1))/(N-1)` per channel — our formula matches exactly
- `-solarize N%`: `if v > threshold { 1-v }` per channel — matches exactly
- `-emboss N`: NOT a 3x3 kernel — blur(sigma=N) then directional difference + bias
- `-edge N`: morphological edge detection, not Sobel gradient

**Zensim agreement scores** (100=identical, 5 test images):

| Operation | srgb_vs_im |
|-----------|-----------|
| Morphology | 99 |
| Solarize | 99 |
| Brightness | 95-99 |
| Contrast | 95 |
| Saturation | 94-95 |
| Grayscale | 94 |
| Blur | 72 |
| Sharpen | 67-69 |
| Posterize | 40 |

Remaining gaps: blur/sharpen kernel radius convention (our `ceil(3σ)` vs IM's ~`ceil(2.5σ)`), posterize rounding on some images, emboss/edge use fundamentally different algorithms.

## Known Issues

- zencodecs local build broken (missing `ImageFormat::Jp2` variant) — worktree strips it from dev-deps (same as CI via superwork)
- Issue #5 (auto-filter banding) still open — needs two-pass architecture for strip processing
