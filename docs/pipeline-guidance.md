# Pipeline Guidance: Validation, Reordering, and Coalescing

## Problem

Users build filter pipelines from UI interactions, URL parameters, or node graphs. They make mistakes: wrong filter order, incompatible combinations, redundant operations, wrong working space. The pipeline should handle these gracefully at configurable levels of strictness.

## Levels of Automatic Correction

### Level 0: Silent (current default for `Pipeline::push`)

No validation. Filters run in push order. Wrong combinations produce bad output silently. Only `PlaneSemantics` is checked (panic on mismatch).

**Who wants this:** Internal code that knows what it's doing. Benchmark harnesses. Tests.

### Level 1: Warn — Report Issues, Don't Fix

What `validate_pipeline()` does today. Returns `Vec<CompatIssue>` with severity (error/warning/info). Caller decides what to do.

**Catches:**
- Exclusive group violations (two tone mappers)
- Wrong ordering (sharpen before denoise)
- Range conflicts (high saturation + high gamut expansion)
- PlaneSemantics / WorkingSpace mismatch

**Who wants this:** Editor UIs that show warnings. CLI tools that print diagnostics. Developers debugging unexpected output.

**What to add:**
- WorkingSpace validation (Oklab filter in sRGB pipeline → error)
- Redundant filter detection (two Contrast filters → warning)
- Identity filter detection (Exposure with stops=0 → info, remove it)

### Level 2: Reorder — Fix Order, Report What Changed

Automatically reorder filters to satisfy `ORDER_CONSTRAINTS`. Return the reordering actions taken so the UI can reflect them.

**Rules (from `filter_compat.rs`):**
```
denoise → sharpen (noise reduction before detail enhancement)
denoise → clarity, texture
median → sharpen, clarity
recovery → highlights/shadows (fix clipping before fine-tuning)
tone map → contrast (scene-referred before display-referred)
```

**Algorithm:** Topological sort with ORDER_CONSTRAINTS as edges. Preserve relative order of unconstrained filters (stable sort). If a cycle exists (shouldn't with current constraints), report error instead of reordering.

**What to add:**
- ResizePhase grouping: auto-split into pre-resize and post-resize groups
- Neighborhood filters grouped to minimize scatter/gather boundaries (all per-pixel first, then neighborhood, to enable strip processing for the per-pixel group)

**Who wants this:** Drag-and-drop UIs where users rearrange filters freely. Node graph compilers. URL/KV parameter parsing where order isn't specified.

### Level 3: Coalesce — Fuse Compatible Operations

Detect sequences of per-pixel operations that can be fused into a single pass, reducing memory bandwidth.

**Fusible operations (Oklab):**
All parameters of `FusedAdjust`: exposure, contrast, highlights, shadows, vibrance, saturation, temperature, tint, dehaze, black_point, white_point. These are currently 11 separate filters but can run as one SIMD pass.

**Fusible operations (sRGB):**
`LinearContrast` + `LinearBrightness` → single `output = factor * input + offset` pass. Any sequence of per-channel affine transforms collapses to one.

**Non-fusible (must stay separate):**
- Neighborhood filters (blur, sharpen, convolution) — need scratch buffers
- Filters that read analysis results (auto-exposure) — depend on prior filter output
- Filters with different PlaneSemantics — can't cross working space boundaries

**Algorithm:**
1. Partition pipeline into runs of consecutive per-pixel filters with same PlaneSemantics
2. For each Oklab run: extract FusedAdjust parameters, replace run with single FusedAdjust
3. For each sRGB run: collapse affine chains (contrast + brightness → single scale+offset)
4. Leave neighborhood filters and analysis-dependent filters in place

**Cost model:**
- Per-pixel filter: ~1 ns/pixel (memory-bound, one pass over L/a/b)
- Neighborhood filter: ~10-100 ns/pixel (compute-bound, multi-pass)
- Scatter/gather: ~2 ns/pixel
- Fusing N per-pixel filters: saves (N-1) × 1 ns/pixel in memory passes

Coalescing 5 per-pixel filters into 1 FusedAdjust saves ~4 ns/pixel. On a 4K image (~8M pixels), that's ~32ms. Worth it for real-time editing, marginal for batch processing.

**Who wants this:** Real-time preview pipelines. WASM demo. Mobile.

### Level 4: Working Space Optimization (future)

For mixed-space graphs (zennode), automatically insert color space conversions and batch same-space operations.

**Algorithm:**
1. Walk the filter graph
2. Group consecutive filters with compatible PlaneSemantics
3. At boundaries, insert `ColorSpaceConvert` nodes
4. Minimize total conversions (NP-hard in general, but greedy works for linear pipelines)

**Cost model:**
- Oklab scatter/gather: ~4 ns/pixel total
- sRGB passthrough: ~1 ns/pixel total
- Each space transition costs one scatter+gather

**Who wants this:** Node graph editors where users mix Oklab and sRGB filters. Automated pipelines that compose operations from different sources.

---

## Identity Elimination

Filters at their identity value (no effect) should be detected and removed before processing. This is free optimization — no quality tradeoff.

| Filter | Identity condition |
|--------|-------------------|
| Exposure | `stops == 0.0` |
| Contrast | `amount == 0.0` |
| Saturation | `factor == 1.0` |
| Vibrance | `amount == 0.0` |
| Temperature | `shift == 0.0` |
| Tint | `shift == 0.0` |
| Blur | `sigma < 0.01` |
| Sharpen | `amount == 0.0` |
| Vignette | `amount == 0.0` |
| Grain | `amount == 0.0` |
| LinearContrast | `amount == 0.0` |
| LinearBrightness | `offset == 0.0` |
| HslSaturate | `factor == 1.0` |

Most filters already check this internally (`if amount.abs() < 1e-6 { return; }`). But checking at push time avoids even the function call overhead and enables better coalescing (skip identity filters when building fused chains).

**Implementation:** Add `fn is_identity(&self) -> bool` to the `Filter` trait with default `false`. Each filter overrides. `Pipeline::push()` can optionally skip identity filters.

---

## Recommended Filter Order

For documentation and Level 2 auto-reordering:

```
1. Tone mapping (Sigmoid, DtSigmoid, Basecurve) — scene → display
2. Noise reduction (NoiseReduction, Bilateral, MedianBlur)
3. Recovery (HighlightRecovery, ShadowLift)
4. Exposure, BlackPoint, WhitePoint
5. Contrast, Levels, ParametricCurve, ToneCurve
6. HighlightsShadows, WhitesBlacks, ToneEqualizer
7. Local tone (LocalToneMap, Clarity, Texture, Brilliance)
8. Color (Temperature, Tint, Saturation, Vibrance, HslAdjust)
9. Color grading (ColorGrading, HueCurves, ChannelCurves, FilmLook)
10. Detail (Sharpen, AdaptiveSharpen)
   ─── resize happens here (if applicable) ───
11. Output effects (Grain, Vignette, Bloom, ChromaticAberration)
12. Geometric (Warp, Rotate, PolarWarp)
```

Steps 1-10 are `ResizePhase::PreResize` or `Either`.
Steps 11-12 are `ResizePhase::PostResize`.

---

## Working Space Guide

| Working space | When to use | Quality | IM equivalent |
|---------------|------------|---------|---------------|
| `Oklab` | Photo editing, color grading, any quality-sensitive work | Best | No direct equivalent |
| `Srgb` | ImageMagick default compat, legacy tool compat | Lowest (gamma artifacts) | Default IM behavior |
| `LinearRgb` | Physically correct blur/compositing, HDR, scientific | Good (linear-correct, not perceptual) | `-colorspace RGB` |

**Rule of thumb:** Use Oklab unless you need to match another tool's output exactly.

---

## Zenpipe Integration

### Current state
`ZenFiltersConverter` in zenpipe creates `Pipeline` with default config (Oklab). No way to specify working space from a zenpipe job.

### What to add
1. `WorkingSpace` field on the zenfilters node group in zenpipe jobs
2. `ZenFiltersConverter` reads it and passes to `PipelineConfig`
3. Node-level `format` attribute in zennode_defs declares preferred space
4. Graph compiler checks compatibility and inserts conversions if needed

### KV/URL parameter mapping (for CDN use)
```
?ws=srgb&contrast=30&brightness=15&sharpen=1
```
Maps to: `PipelineConfig::srgb_compat()` + `LinearContrast{amount}` + `LinearBrightness{offset}` + `ChannelSharpen{sigma, amount}` with IM's tan-slope formula for contrast.

This translation layer lives outside zenfilters — in imageflow_core's RIAPI parser or in a zenpipe job builder. zenfilters provides the primitives; the caller maps parameters.

---

## Implementation Priority

1. **Level 1 enhancements** (identity detection, WorkingSpace validation, redundant filter warning) — small, high value, no behavior change
2. **Usage guide** (this doc, cleaned up and added to the repo) — documentation
3. **Level 2 reordering** — enables drag-and-drop UIs and unordered parameter APIs
4. **Level 3 coalescing** — performance optimization for real-time preview
5. **Zenpipe bridge** — needed for end-to-end sRGB compat
6. **Level 4 graph optimization** — only needed when zennode graphs mix working spaces
