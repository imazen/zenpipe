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

**DONE (2026-05-28, e-commerce white-background flatten):**
- ~~White-background flatten~~ → `BackgroundFlatten` (`src/filters/background_flatten.rs`):
  conservative automated flatten of noisy/uneven near-white product backgrounds.
  Border estimate + applicability gate + **central-subject gate** (rejects bright
  high-key/sky scenes); edge-seeded flood-fill background mask (only border-connected
  background is touched); chamfer-distance feather (effect → 0 at the silhouette);
  low-order surface fit for gradient backgrounds; shadow-preserving soft-knee
  whitening (max-lift cap); chroma neutralization; halo/fringe removal via guided
  filter + overshoot clamp + chroma decontamination. `Describe` schema + 13 unit tests.
- ~~Metric-gated edits~~ → `metric_gate::MetricGated<M>` (`src/metric_gate.rs`):
  apply → score (pluggable `QualityMetric`; any `Fn(&OklabPlanes,&OklabPlanes)->f32`,
  so zensim plugs in as a closure) → binary-search strength back under a JND
  threshold, or skip. `OklabDeltaMetric` zero-dep default. 4 unit tests.
- Validation/demo: `examples/whitebg_corpus.rs` (experimental) — zensim-scored,
  scaled-back/skipped, before/after/diff images + CSV to `/mnt/v/output/zenfilters/whitebg`.

**DONE (2026-05-31, validated gentle white-bg noise removal — "the one"):**
- The blessed recipe for gently removing sub-pixel render noise from near-white
  backgrounds without touching the product/shadows or creating edge lines lives in
  `examples/ai_corpus_flatten.rs::white_snap` and is documented in
  `WHITE-BG-CLEANUP.md`. Recipe: border-median guard → measure the
  image's own tiny white band → border-connected flood (change-allowed mask) →
  snap to **pure 255**, feathered by luminance AND by a **large (~64px) chamfer
  spatial feather** from the nearest non-background pixel (so the 255→shadow
  transition is imperceptible and product/shadow pixels are never touched).
  Validated on the AI product corpus (713/750 cleaned, shadows preserved, no
  halos). `BackgroundFlatten` at full strength over-reached on light/colored
  products — see the doc's "approaches that failed" for why. Candidate to promote
  to a library `BackgroundClean` filter.

**DONE (2026-05-29, AI-clipart cleanup — complement to white-bg flatten):**
- ~~Clip-art waviness/bubble-noise flatten~~ → `ClipartFlatten`
  (`src/filters/clipart_flatten.rs`), v2: flattens nominally-flat colour regions
  of AI clipart while keeping crisp edges + intentional shading.
  - **Stage 1 (default, `cartoon`=0):** ease L/a/b toward an edge-preserving
    **guided-filter base** (reuses `guided_filter::guided_filter_plane`). Removes
    low-variance waviness, keeps edges + smooth shading, no bilateral staircase /
    gradient-reversal. Params `strength`, `waviness_scale` (guided σ), `flatness`
    (guided eps).
  - **Stage 2 (`cartoon`>0):** quantize the guided base → 4-connected regions per
    palette colour → snap flat-fill interiors to the region mean, gated by
    `region_flatness × boundary_keep(chamfer-dist)` so shaded regions + edges
    survive. Params `palette_size`, `color_tolerance` (centroid-merge dist),
    `edge_feather`.
  - **`zenquant` feature** (opt-in): cartoon-snap palette via zenquant (perceptual
    OKLab, dither off) — verified more *faithful* than built-in k-means on the
    clipart corpus (preserves intended shading/detail better). Path:
    Oklab guided base → BT.709 sRGB8 → `zenquant::quantize`. Built-in k-means is
    the default (no_std/wasm-safe).
  - `Describe` schema + 7 tests (default + zenquant). Demo
    `examples/clipart_flatten_demo.rs` (red diff heatmaps, `--cartoon`,
    `--features experimental,zenquant`). Chains after `BackgroundFlatten`; wrap in
    `MetricGated` for a subtlety guarantee. Research: `/mnt/v/output/zenfilters/clipart-cleanup-research.md`.
  - v1→v2 was driven by reviewing red-diff heatmaps: v1's membership gate protected
    the waviness peaks themselves; the guided-base approach fixed it.

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
