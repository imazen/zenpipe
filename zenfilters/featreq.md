# Feature Requests from zenimage

## Done

- ~~WhitePoint: Soft Clip with Headroom~~ — shipped (headroom field)
- ~~HighlightsShadows: Configurable Thresholds~~ — shipped (shadow_threshold, highlight_threshold)
- ~~SimpleDehaze~~ — rejected (inferior algorithm, not real dehaze)

## 1. HDR Tone Mapping Operators

**Priority: High**

zenimage has three global HDR→SDR tone mapping ops (~630 lines) that should live in
zenfilters. All are point operations on linear RGB f32 with unbounded input range.
They use BT.709 luminance for color preservation (scale RGB by L_out/L_in ratio).

### FilmicTonemap

ACES-style filmic S-curve. Standard in games and film pipelines.

```
Parameters: exposure (pre-scale), whitepoint (HDR peak → 1.0)
Algorithm:
  x = pixel * exposure / whitepoint
  L = BT.709 luminance
  L' = (L*(2.51*L + 0.03)) / (L*(2.43*L + 0.59) + 0.14)
  RGB' = RGB_scaled * (L' / L)
```

### ReinhardTonemap

Extended Reinhard. Standard in photography pipelines.

```
Parameters: exposure (pre-scale), l_max (maximum luminance)
Algorithm:
  L = BT.709 luminance * exposure
  L' = (L / l_max) / (1 + L / l_max)
  RGB' = RGB_in * (L' / L)
```

### Bt2390Tonemap

ITU BT.2390 EETF (Electrical-Electrical Transfer Function). Broadcast standard
for HLG and HDR10 content.

```
Parameters: source_peak (HDR peak nits), target_peak (SDR target, default 1.0)
Algorithm:
  e = RGB / source_peak
  L = BT.709 luminance
  ks = (1.5 * target_peak / source_peak - 0.5).clamp(0, 1)
  Below ks: pass through
  Above ks: cubic Hermite spline soft rolloff (Catmull-Rom)
  RGB' = e_out * target_peak
```

### Design considerations

These operate in **linear RGB**, not Oklab. They need access to the raw HDR values
before any perceptual transform. Options:

1. **Pre-scatter filter** — apply before the Oklab scatter step in the pipeline
2. **Separate HDR pipeline stage** — new trait or pipeline phase for linear RGB ops
3. **Raw-plane filter** — operate on the L plane treating it as linear luminance
   (loses the color-ratio preservation, probably wrong)

Option 1 is cleanest: `Pipeline` gains an optional pre-scatter hook that runs on
the interleaved linear RGB buffer before scatter_to_oklab. The tone mapper normalizes
HDR→SDR range, then normal Oklab filters (exposure, contrast, etc.) operate on SDR.

## 2. Gamut Mapping / Expansion

**Priority: Medium**

zenimage has ~1200 lines of dead gamut code in `graphics/gamut.rs` that was never
wired up. Rather than revive it, zenfilters should provide proper gamut operations.

### What's needed

**Gamut compression (out-of-gamut → in-gamut):**
zenfilters already has `GamutMapping::SoftCompress` in the pipeline gather step.
This may be sufficient. Verify it handles Display P3 → sRGB compression correctly.

**Gamut expansion (sRGB → wider gamut for HDR displays):**
- sRGB → Display P3 (most common, Apple ecosystem)
- sRGB → BT.2020 (broadcast HDR)
- Intelligent chroma expansion at gamut boundary (not just matrix multiply)

Approaches from the dead zenimage code (for reference, not necessarily worth porting):
- `Oklch` chroma boost at boundary (fast, modest expansion)
- 3D LUT with trilinear interpolation (pre-computed, O(1))
- MLP neural network (3→32→32→3 with ReLU, 5KB weights, best quality)

The Oklch approach is probably the right one for zenfilters — it's simple, works in
the existing Oklab pipeline, and produces good results for sRGB→P3 expansion.

### ICC Profile Constants

zenimage also has embedded ICC profiles (Display P3 v2/v4, Adobe RGB, Rec.2020,
ProPhoto). These are CC0-licensed from Compact-ICC-Profiles. Only Display P3 v4
is actually used. These should live in zenpixels or a dedicated profiles crate,
not in a filter library.
