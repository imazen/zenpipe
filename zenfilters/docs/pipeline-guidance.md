# Pipeline Guidance: Optimization, Validation, and Correction

## Principle

If the output is identical, optimize unconditionally. No flags, no configuration, no "levels." Identity elimination, affine coalescing, and commutative reordering are **free** — the pixels don't change.

Validation (wrong order, incompatible combinations) is separate. That's about catching user errors, not optimization.

## Unconditional Optimizations (same output guaranteed)

### 1. Identity Elimination

Filters at their identity value produce no change. Remove them before processing.

Add `fn is_identity(&self) -> bool` to the `Filter` trait (default `false`). `Pipeline::optimize()` strips identity filters. This skips the function call, enables better fusion, and simplifies the pipeline for debugging.

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

Most filters already check this internally and early-return. The trait method lets the pipeline skip them entirely — no dispatch, no plane access.

### 2. Affine Coalescing

Any chain of per-channel affine transforms (scale + offset) collapses to a single transform with identical output.

```
LinearContrast{0.3} then LinearBrightness{0.1}:
  pass 1: y = 1.3 * x - 0.15
  pass 2: z = y + 0.1
  combined: z = 1.3 * x - 0.05  (one pass, same pixels)
```

More generally: any sequence of `output = a*input + b` operations composes as `(a₁*a₂, a₁*b₂ + b₁)`. This works for sRGB contrast + brightness chains.

For Oklab: `FusedAdjust` already coalesces Exposure + Contrast + Saturation + Temperature + Tint + HighlightsShadows + Dehaze + BlackPoint + WhitePoint into one SIMD pass. If the pipeline contains any subset of these as separate filters, replace with a single `FusedAdjust`.

### 3. Duplicate Collapse

Two consecutive same-type filters with composable parameters:

- `Saturation{1.5}` → `Saturation{1.5}` = `Saturation{2.25}` (multiply factors)
- `Exposure{0.5}` → `Exposure{0.3}` = `Exposure{0.8}` (add stops)
- `LinearBrightness{0.1}` → `LinearBrightness{0.05}` = `LinearBrightness{0.15}` (add offsets)
- `HueRotate{30}` → `HueRotate{15}` = `HueRotate{45}` (add angles)

### 4. Per-Pixel / Neighborhood Partitioning

Group per-pixel filters together, separate from neighborhood filters. This enables strip processing for the per-pixel group (L3-cache-friendly) while neighborhood filters get full-frame access.

```
Before: [Exposure, Blur, Contrast, Sharpen, Saturation]
After:  [Exposure, Contrast, Saturation] → strip  then  [Blur, Sharpen] → full-frame
```

The reordering is safe only when the per-pixel filters commute (don't depend on each other's output in a way that changes with reordering). For independent per-channel operations (exposure on L, saturation on a/b), reordering is always safe. For operations that both touch L (exposure + contrast), order matters — don't reorder.

Rule: only reorder across the per-pixel/neighborhood boundary when `channel_access()` sets don't overlap.

## Validation (user error detection)

Separate from optimization. These catch mistakes that would produce wrong output.

### Push-time (current)

- `PlaneSemantics` mismatch → panic. Oklab filter in sRGB pipeline is always a bug.

### On-demand (`validate_pipeline()`)

Already implemented in `filter_compat.rs`:
- Exclusive group violations (two tone mappers → error)
- Wrong ordering (sharpen before denoise → error)
- Range conflicts (high saturation + high gamut expansion → warning)
- Sharpen without AdaptiveSharpen → info

### What to add

- WorkingSpace compat check (already done via PlaneSemantics at push time)
- Redundant filter warning (two Contrast filters — intentional or mistake?)
- Resize phase validation (PostResize filter before resize operation)

## Debug Mode

`Pipeline::set_debug(true)` disables all unconditional optimizations. Filters run exactly as pushed — no identity elimination, no coalescing, no reordering. Every filter's `apply()` is called even if `is_identity()` returns true.

Use for:
- Test suites that verify individual filter behavior
- Benchmarking individual filter cost (not fused)
- Debugging unexpected output (is the optimization changing something?)
- Regression testing against non-optimized output

The debug flag is on `Pipeline`, not global. Production and test pipelines can coexist.

## Correction (user error fixing)

These change filter order, which changes the pipeline. The caller must opt in.

### Level 1: Minimum fix — satisfy constraints

Move the fewest filters possible to satisfy `ORDER_CONSTRAINTS`. Preserve user intent as much as possible — if the user put sharpen before denoise, just swap those two. Don't reorganize everything.

```
Input:  [Exposure, Sharpen, NoiseReduction, Contrast]
Output: [Exposure, NoiseReduction, Sharpen, Contrast]
                   ^^^^^^^^^^^^^^^^^^^^^^^^^^^
                   swapped: 1 move to fix the constraint violation
```

**Algorithm:** Walk constraints, find violations, bubble the offending filter to satisfy the constraint. Minimal displacement. Returns a list of moves made.

**When to use:** URL/KV parameter parsing where order isn't specified. Automated pipelines that compose filters from different sources. "Fix my mistakes but don't rethink my pipeline."

### Level 2: Optimal reorder — canonical ordering

Reorder the entire pipeline to match the recommended order (§ below). This produces the best quality output regardless of how the filters were originally arranged.

```
Input:  [Saturation, Sharpen, Exposure, NoiseReduction, Grain]
Output: [NoiseReduction, Exposure, Saturation, Sharpen, Grain]
         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
         full reorder to canonical: denoise → tone → color → detail → effects
```

**Algorithm:** Assign each filter a canonical position based on its `FilterTag` and the recommended order table. Stable sort by position. Neighborhood filters stay in their relative order within the same position group.

**When to use:** "Build me the best pipeline from these filters." Node graph compilation. First-time setup from a preset.

### Recommended filter order (canonical)

```
 1. Tone mapping     (Sigmoid, DtSigmoid, Basecurve)
 2. Noise reduction  (NoiseReduction, Bilateral, MedianBlur)
 3. Recovery         (HighlightRecovery, ShadowLift)
 4. Tone             (Exposure, BlackPoint, WhitePoint, Levels)
 5. Contrast         (Contrast, ParametricCurve, ToneCurve)
 6. Tone range       (HighlightsShadows, WhitesBlacks, ToneEqualizer)
 7. Local detail     (LocalToneMap, Clarity, Texture, Brilliance)
 8. Color            (Temperature, Tint, Saturation, Vibrance, HslAdjust)
 9. Color grading    (ColorGrading, HueCurves, ChannelCurves, FilmLook)
10. Detail           (Sharpen, AdaptiveSharpen)
    ─── resize ───
11. Output effects   (Grain, Vignette, Bloom, ChromaticAberration)
12. Geometric        (Warp, Rotate, PolarWarp)
```

### ResizePhase auto-split

`Pipeline::split_at_resize()` already exists. Partitions into pre-resize and post-resize groups based on `ResizePhase`. This is safe — the split point is well-defined.

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
