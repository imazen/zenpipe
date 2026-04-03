# Automatic Mode Opportunities

## Current Auto Modes

Already shipping:

| Mode | Filter | Trigger | Algorithm |
|------|--------|---------|-----------|
| Rule-based tune | `auto_tune.rs` | Explicit call | 142 features → 18 params via heuristics |
| Auto Exposure | `auto_exposure.rs` | strength > 0 | Geometric mean → target correction |
| Auto Levels | `auto_levels.rs` | strength > 0 | Histogram stretch + smart plateau detection |
| Adaptive Sharpen | `adaptive_sharpen.rs` | amount > 0 | Noise-gated USM with energy mask |
| Highlight Recovery | `highlight_recovery.rs` | strength > 0 | p95/p99.5 adaptive soft knee |
| Shadow Lift | `shadow_lift.rs` | strength > 0 | p1/p5 adaptive toe curve |
| Brilliance | `brilliance.rs` | amount > 0 | Local average for shadow/highlight adaptation |
| Dehaze | `dehaze.rs` | strength > 0 | Spatial transmission estimation |
| Noise Reduction | `noise_reduction.rs` | luminance > 0 | Per-scale BayesShrink wavelet thresholding |
| Regional Features | `regional.rs` | Explicit call | 5 lum + 4 chroma + 6 hue + 4 texture zones |

## Proposed Auto Modes

### Tier 1: Quick wins (heuristic, <50 lines each)

**1. Auto White Balance** — new filter
- Input: mean_a, mean_b from image analysis (already computed in auto_tune)
- Algorithm: If |mean_a| > 0.02 or |mean_b| > 0.02, apply temperature = -mean_b * scale, tint = -mean_a * scale
- Weight by image saturation — don't over-correct near-neutral scenes
- Strength parameter controls blend: 0 = off, 1 = full correction
- Different from AutoLevels' remove_cast (which is a side-effect, not the primary operation)

**2. Auto Contrast** — new filter
- Input: contrast_ratio = std_l / mean_l, histogram shape
- Algorithm: If ratio < 0.15, apply contrast boost. If > 0.35, compress.
- Use histogram kurtosis to distinguish "flat" (needs contrast) from "bimodal" (needs local tone map)
- Strength parameter: 0 = off, 1 = full adaptive correction

**3. Auto Tone** — composite auto mode
- Combines: AutoExposure + AutoContrast + HighlightRecovery + ShadowLift
- Single strength parameter controls all four
- Internal logic decides which corrections are needed based on image statistics
- This is what most users want from an "auto" button — fix exposure, contrast, highlights, and shadows in one pass
- Implementation: extract features once, apply all corrections in order

**4. Auto Vibrance** — new filter
- Input: per-hue-sector chroma from `regional.rs`
- Algorithm: Compute per-sector saturation deficit relative to a target profile
- Boost muted sectors (desaturated skin, faded sky) more than already-vivid ones
- Strength: 0 = off, 1 = full adaptive boost
- Uses regional hue sectors to avoid boosting noise in neutral areas

**5. Smart Whites/Blacks** — enhance existing WhitesBlacks
- Input: p5, p95 percentiles (already computed in highlight_recovery/shadow_lift)
- Algorithm: Set whites toward p95, blacks toward p5 instead of fixed 0/1
- Only activate when the range is significantly narrower than [0, 1]
- Could be a mode flag on WhitesBlacks: `auto: bool`

### Tier 2: Medium effort (heuristic + spatial analysis, 50-200 lines)

**6. Content-Aware Noise Reduction** — enhance existing NoiseReduction
- Input: texture zones from `regional.rs`
- Algorithm: Apply less NR in textured zones (preserve detail), more in smooth zones
- Current NR uses per-scale thresholding but is spatially uniform
- Enhancement: generate a spatial strength mask from texture zone classification
- Apply mask as a per-pixel blend between filtered and original

**7. Adaptive Clarity** — enhance existing Clarity
- Input: local variance map (already computed for adaptive_sharpen)
- Algorithm: Boost clarity in low-variance regions (flat areas need more local contrast), reduce in textured regions (already have detail)
- Similar to how AdaptiveSharpen gates by energy, but for mid-frequency content

**8. Auto Dehaze Strength** — enhance existing Dehaze
- Input: atmospheric contrast from the existing transmission estimate
- Algorithm: Automatically set strength based on how much haze is detected
- If the image has low spatial contrast variation (everything is equally hazy), go stronger
- If only parts are hazy (e.g., distance), the spatial adaptation already handles it
- Add an `auto_strength: bool` flag that replaces manual strength with computed value

**9. Smart Bloom** — enhance existing Bloom
- Input: highlight percentile analysis
- Algorithm: Auto-set threshold from p90 of L distribution (bloom only top 10%)
- Auto-set amount based on highlight density: more highlights → lower amount per pixel
- Makes bloom work well regardless of image exposure level

**10. Adaptive Bilateral** — enhance existing Bilateral
- Input: edge map (gradient magnitude)
- Algorithm: Auto-set range_sigma based on image noise estimate (MAD of detail coefficients)
- High noise → larger range_sigma (more smoothing)
- Low noise → smaller range_sigma (preserve edges better)
- The spatial_sigma could auto-scale with image resolution

### Tier 3: ML-based (requires training data)

**11. Neural Auto-Tune** — replace cluster model
- Input: 142 features from ImageFeatures
- Model: MLP via zentract ONNX inference
- Training: MIT-Adobe FiveK expert edits
- Output: 18 continuous parameters (no cluster quantization)
- Expected: +2-5 zensim over current cluster model
- Architecture: 142 → 256 → 128 → 18 with ReLU, trained with MSE + zensim loss

**12. Scene Classification** — new
- Input: 142 features + regional features
- Model: Small classifier (portrait, landscape, night, macro, food, architecture, etc.)
- Output: Scene type + confidence
- Use: Route to scene-specific parameter presets or models
- Training: Scene-labeled subset of FiveK or public scene datasets

**13. Per-Hue-Sector Tuning** — enhance auto_tune
- Input: per-sector histograms from regional.rs (already extracted)
- Model: Small per-sector MLP or lookup table
- Output: Per-sector saturation, luminance, and hue shifts
- Training: Expert edits with per-region analysis
- This is what makes pro editors different from auto — they treat skin, sky, and foliage differently

**14. Adaptive Film Look Selection** — new
- Input: 142 features
- Model: Classifier or similarity search
- Output: Best-matching FilmLook preset + strength
- Training: User preference data or A/B testing results
- Would power "suggest a look" functionality

## Implementation Priority

| # | Mode | Effort | Value | Dependencies |
|---|------|--------|-------|-------------|
| 3 | Auto Tone (composite) | 1 day | Very high | None (combines existing) |
| 1 | Auto White Balance | 2 hours | High | None |
| 5 | Smart Whites/Blacks | 2 hours | Medium | None |
| 2 | Auto Contrast | 3 hours | Medium | None |
| 4 | Auto Vibrance | 4 hours | Medium | regional.rs |
| 8 | Auto Dehaze Strength | 2 hours | Medium | None |
| 11 | Neural Auto-Tune | 1 week | Very high | zentract, FiveK dataset |
| 6 | Content-Aware NR | 1 day | High | regional.rs texture zones |
| 7 | Adaptive Clarity | 4 hours | Medium | variance map |
| 9 | Smart Bloom | 2 hours | Low | None |
| 10 | Adaptive Bilateral | 3 hours | Low | noise estimate |
| 12 | Scene Classification | 1 week | High | Training data |
| 13 | Per-Hue Tuning | 1 week | High | regional.rs + training |
| 14 | Film Look Selection | 2 weeks | Medium | User data |

## Architecture Notes

### Auto Tone composite filter

The highest-value addition. Most users want one "fix" button, not 6 separate sliders. Implementation:

```
pub struct AutoTone {
    pub strength: f32,      // 0-1 master blend
    pub preserve_intent: f32, // 0-1 how much to respect intentional exposure
}
```

Internally runs: extract features → decide corrections → apply:
1. AutoExposure (if median far from target)
2. HighlightRecovery (if p95 > 0.85 and highlights clipped)
3. ShadowLift (if p5 < 0.15 and shadows crushed)
4. Contrast correction (if contrast_ratio < 0.15)
5. Color cast removal (if |mean_a| or |mean_b| > 0.03)

The `preserve_intent` parameter controls how aggressively the filter "fixes" things. At 0, it corrects everything it can detect. At 1, it only fixes obvious problems (severe clipping, strong cast). This prevents an intentionally dark moody photo from being brightened.

### Feature extraction caching

Multiple auto filters extract the same features (histograms, percentiles). Currently each filter computes independently. A shared `ImageAnalysis` cache on FilterContext would avoid redundant work:

```
pub struct ImageAnalysis {
    histogram_l: [u32; 1024],
    percentiles: [f32; 7],  // p1, p5, p25, p50, p75, p95, p99
    mean_l: f32,
    mean_a: f32,
    mean_b: f32,
    std_l: f32,
    geo_mean_l: f32,
}
```

Computed once per pipeline.apply() call, cached in FilterContext, invalidated when planes change.
