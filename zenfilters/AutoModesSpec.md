# Auto Modes Implementation Spec

ML runtime (zentract) is always optional. All heuristic modes work without it.

## Foundation: ImageAnalysis cache

Before any auto filters, add a shared analysis cache to FilterContext so multiple auto filters don't redundantly compute histograms/percentiles. This is the prerequisite for everything else.

### ImageAnalysis struct

```rust
pub struct ImageAnalysis {
    pub histogram_l: [u32; 1024],
    pub percentiles: [f32; 7],  // p1, p5, p25, p50, p75, p95, p99
    pub mean_l: f32,
    pub mean_a: f32,
    pub mean_b: f32,
    pub std_l: f32,
    pub std_a: f32,
    pub std_b: f32,
    pub geo_mean_l: f32,
    pub dynamic_range: f32,     // p99 - p1
    pub contrast_ratio: f32,    // std_l / mean_l
    pub chroma_energy: f32,     // (std_a + std_b) * 0.5
    pixel_count: usize,
    generation: u64,            // incremented on invalidation
}
```

### FilterContext changes

```rust
pub struct FilterContext {
    f32_pool: Vec<Vec<f32>>,
    u8_pool: Vec<Vec<u8>>,
    analysis: Option<ImageAnalysis>,  // cached, invalidated by pipeline between filter groups
}

impl FilterContext {
    /// Get or compute image analysis. Caches result for subsequent filters.
    pub fn analyze(&mut self, planes: &OklabPlanes) -> &ImageAnalysis { ... }
    /// Invalidate cached analysis (call when planes change between filter groups).
    pub fn invalidate_analysis(&mut self) { ... }
}
```

Single pass over L plane computes histogram + mean + variance + geo_mean. Percentiles derived from histogram. Analysis of a/b planes is a second pass. Total: 2 passes over all planes, ~3ms for a 4K image.

### Files
- `src/context.rs` — add ImageAnalysis struct and cache methods
- `src/analysis.rs` — new module, extraction logic

---

## 1. Auto Tone (composite)

The "fix everything" button. Highest value.

### Struct

```rust
pub struct AutoTone {
    /// Master strength. 0 = off, 1 = full correction.
    pub strength: f32,
    /// How much to respect intentional exposure.
    /// 0 = correct everything detected, 1 = only fix severe problems.
    pub preserve_intent: f32,
}
```

### Algorithm

Uses ImageAnalysis cache. Applies corrections in order:

1. **Exposure**: If median < 0.25 or > 0.7, compute correction toward 0.45. Scale by `(1 - preserve_intent)` for mild mis-exposure, full correction for severe.
2. **Highlight recovery**: If p95 > 0.85 AND (p99-p95) < 0.02, apply soft knee. Strength = `0.5 * (1 - preserve_intent * 0.5)`.
3. **Shadow lift**: If p5 < 0.15, apply toe curve. Strength = adaptive from shadow density.
4. **Contrast**: If contrast_ratio < 0.15, apply power contrast. Amount = `0.1 * (1 - preserve_intent)`.
5. **Color cast**: If |mean_b| > 0.03, shift b plane. If |mean_a| > 0.03, shift a plane. Scale by `(1 - preserve_intent * 0.7)` (cast correction is almost always wanted).

Each sub-correction has its own threshold and scaling. The `strength` parameter blends the final result with the original.

### Implementation

This is a **single filter** that does everything inline (no sub-filter instantiation). It reads the analysis cache, computes correction parameters, and applies them in one or two passes over the planes. Per-pixel corrections (exposure, contrast, cast) are fused into a single loop. Highlight/shadow corrections use the existing soft-knee/toe-curve math.

### File
- `src/filters/auto_tone.rs`

---

## 2. Auto White Balance

### Struct

```rust
pub struct AutoWhiteBalance {
    /// Correction strength. 0 = off, 1 = full cast removal.
    pub strength: f32,
}
```

### Algorithm

1. Compute mean_a, mean_b from ImageAnalysis cache
2. Compute cast magnitude: `cast = sqrt(mean_a² + mean_b²)`
3. If cast < 0.01, return (image is neutral)
4. Compute saturation weight: `w = (chroma_energy / 0.10).clamp(0.3, 1.0)` — don't over-correct low-saturation images
5. Apply: `a -= mean_a * strength * w`, `b -= mean_b * strength * w`

### File
- `src/filters/auto_white_balance.rs`

---

## 3. Auto Contrast

### Struct

```rust
pub struct AutoContrast {
    /// Correction strength. 0 = off, 1 = full adaptive correction.
    pub strength: f32,
}
```

### Algorithm

1. Read contrast_ratio from ImageAnalysis
2. If ratio < 0.20: compute boost. `amount = (0.20 - ratio) / 0.20 * 0.3 * strength`
3. If ratio > 0.40: compute compress. `amount = -(ratio - 0.40) / 0.60 * 0.15 * strength`
4. Apply as power contrast on L: `L = pivot * (L/pivot)^(1+amount)` where pivot = median

### File
- `src/filters/auto_contrast.rs`

---

## 4. Auto Vibrance (per-hue adaptive)

### Struct

```rust
pub struct AutoVibrance {
    /// Boost strength. 0 = off, 1 = full adaptive boost.
    pub strength: f32,
}
```

### Algorithm

1. Compute mean chroma per hue sector (6 sectors from Oklab hue angle)
2. Target chroma profile: `[0.08, 0.06, 0.05, 0.06, 0.04, 0.07]` (warm, yellow-green, green-cyan, cool, purple, magenta)
3. Per-pixel: compute hue sector, look up deficit = target - sector_mean
4. If deficit > 0: boost chroma by `deficit * strength * protection` where protection = `(1 - chroma/0.3)`
5. If deficit <= 0: no change (sector already saturated enough)

### File
- `src/filters/auto_vibrance.rs`

---

## 5. Smart Whites/Blacks

Enhance existing WhitesBlacks with auto mode.

### Changes

Add field to WhitesBlacks:
```rust
pub struct WhitesBlacks {
    pub whites: f32,
    pub blacks: f32,
    pub auto_range: bool,  // NEW: auto-detect from percentiles
}
```

When `auto_range = true`:
1. Read p5, p95 from ImageAnalysis
2. If (p95 - p5) < 0.7 (narrow range), auto-set:
   - `effective_whites = (1.0 - p95) * strength`  (push white point up)
   - `effective_blacks = -p5 * strength`  (push black point down)
3. Apply the computed values through existing WhitesBlacks logic

### File
- `src/filters/whites_blacks.rs` (modify existing)

---

## 6. Auto Dehaze Strength

Enhance existing Dehaze with auto-strength mode.

### Changes

Add field:
```rust
pub struct Dehaze {
    pub strength: f32,
    pub auto_strength: bool,  // NEW: auto-detect haze level
}
```

When `auto_strength = true`:
1. Compute atmosphere map (existing blur)
2. Compute transmission variance: `var_t = variance(1 - atm/airlight)`
3. Low variance → uniform haze → higher strength
4. `auto_s = (1.0 - var_t.sqrt() * 3.0).clamp(0.1, 0.8)`
5. Blend: `effective_strength = auto_s * strength`

### File
- `src/filters/dehaze.rs` (modify existing)

---

## 7. Adaptive Clarity

Enhance Clarity with variance-gated application.

### Changes

Add field:
```rust
pub struct Clarity {
    pub sigma: f32,
    pub amount: f32,
    pub adaptive: bool,  // NEW: variance-gated
}
```

When `adaptive = true`:
1. Compute local variance map (small Gaussian blur of (L - blur(L))²)
2. Per-pixel: scale clarity amount by `1.0 - (variance / threshold).min(1.0)`
3. Flat areas get full clarity boost, textured areas get less

### File
- `src/filters/clarity.rs` (modify existing)

---

## 8. Content-Aware NR

Enhance NoiseReduction with spatial mask.

### Changes

Add field:
```rust
pub struct NoiseReduction {
    // ... existing fields ...
    pub content_aware: bool,  // NEW: texture-preserving mask
}
```

When `content_aware = true`:
1. After wavelet shrinkage, compute edge/texture magnitude from L gradients
2. Blend: `output = NR_result * (1 - edge_mask * preserve) + original * edge_mask * preserve`
3. `preserve = 0.5` (default weight toward original in textured regions)

### File
- `src/filters/noise_reduction.rs` (modify existing)

---

## 9. Smart Bloom

Auto-set threshold from histogram.

### Changes

Add field:
```rust
pub struct Bloom {
    pub threshold: f32,
    pub sigma: f32,
    pub amount: f32,
    pub auto_threshold: bool,  // NEW
}
```

When `auto_threshold = true`:
1. Read p90 from ImageAnalysis
2. `threshold = p90` (bloom only top 10% of luminance)
3. Scale amount by highlight density: `effective = amount / (1.0 - p90).max(0.05)`

### File
- `src/filters/bloom.rs` (modify existing)

---

## 10. Adaptive Bilateral

Auto-set range_sigma from noise estimate.

### Changes

```rust
pub struct Bilateral {
    pub spatial_sigma: f32,
    pub range_sigma: f32,
    pub strength: f32,
    pub auto_range: bool,  // NEW
}
```

When `auto_range = true`:
1. Estimate noise: MAD of high-frequency detail (L - blur(L, sigma=1))
2. `range_sigma = noise_estimate * 3.0` (3-sigma rule)
3. Clamp to [0.01, 0.3]

### File
- `src/filters/bilateral.rs` (modify existing)

---

## ML Tier (optional, behind feature flag)

### 11. Neural Auto-Tune

Behind `zentract` feature flag. Falls back to rule-based when unavailable.

```rust
#[cfg(feature = "zentract")]
pub fn neural_tune(features: &ImageFeatures, model: &zentract_api::Model) -> TunedParams { ... }
```

- Architecture: MLP 142 → 256 → 128 → 18 with ReLU
- Training: MSE loss on FiveK expert params + zensim regularization
- Export: ONNX via zentract
- Fallback: `rule_based_tune()` when zentract not available

### 12. Scene Classification

Behind `zentract` feature flag.

```rust
#[cfg(feature = "zentract")]
pub fn classify_scene(features: &ImageFeatures, model: &zentract_api::Model) -> SceneType { ... }

pub enum SceneType { Portrait, Landscape, Night, Macro, Food, Architecture, Event, Unknown }
```

Route to scene-specific preset variants.

### 13-14. Per-Hue Tuning + Film Look Selection

Deferred to post-zentract integration.

---

## Implementation Order

1. **ImageAnalysis cache** (foundation, everything depends on it)
2. **AutoTone** (highest value, uses cache)
3. **AutoWhiteBalance** (quick, uses cache)
4. **AutoContrast** (quick, uses cache)
5. **Smart Whites/Blacks** (enhance existing, uses cache)
6. **AutoVibrance** (uses hue sectors)
7. **Auto Dehaze Strength** (enhance existing)
8. **Adaptive Clarity** (enhance existing)
9. **Smart Bloom** (enhance existing)
10. **Content-Aware NR** (enhance existing)
11. **Adaptive Bilateral** (enhance existing)
12. Neural Auto-Tune (ML tier, zentract dependency)
13-14. Scene classification, per-hue tuning (ML tier)

## Testing

Each new auto filter gets:
- Unit tests with synthetic planes (identity at strength=0, monotonic response)
- Parameter calibration test in `tests/parameter_calibration.rs` (zensim on CID22)
- Integration into `examples/filter_audit.rs` for OpenAI Vision spot-check
