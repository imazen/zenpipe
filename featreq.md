# Feature Requests from zenimage

zenimage delegates 9 of 12 Oklab adjustment ops to zenfilters. The remaining 3
have semantic mismatches that prevent delegation. Adding these would let zenimage
drop the last ~600 lines of manual per-pixel Oklab code.

## 1. WhitePoint: Soft Clip with Headroom

**Priority: Medium**

zenimage's `WhitePointOp` applies a soft asymptotic rolloff above the white point,
not a hard scale. This preserves highlight detail in HDR-to-SDR workflows.

Current zenfilters `WhitePoint` just scales L by `1/level`. zenimage does:

```rust
if l <= white_point {
    l  // pass through
} else {
    // asymptotic approach: wp + headroom * (1 - exp(-excess * k))
    let headroom = white_point * headroom_fraction;
    let excess = l - white_point;
    let k = 3.0 / headroom.max(0.01);
    white_point + headroom * (1.0 - (-excess * k).exp())
}
```

**Needed:** Add a `headroom: f32` field to `WhitePoint` (default 0.0 = current behavior,
>0.0 = soft rolloff above level). When headroom is 0, keep the current fast path.

## 2. HighlightsShadows: Configurable Thresholds

**Priority: Medium**

zenimage's `HighlightsShadowsOp` has configurable `shadow_threshold` (default 0.3)
and `highlight_threshold` (default 0.7) with smooth quadratic mask transitions.
The zenfilters `HighlightsShadows` filter has fixed internal thresholds.

**Needed:** Add `shadow_threshold: f32` and `highlight_threshold: f32` fields
to `HighlightsShadows` (defaults matching current behavior). The mask functions:

```rust
fn shadow_mask(l: f32, threshold: f32) -> f32 {
    if l >= threshold { 0.0 }
    else { let t = 1.0 - l / threshold; t * t }
}

fn highlight_mask(l: f32, threshold: f32) -> f32 {
    if l <= threshold { 0.0 }
    else { let t = (l - threshold) / (1.0 - threshold); t * t }
}
```

Also confirm the SIMD `highlights_shadows()` function uses the same
shadow/highlight adjustment formula as zenimage (blend toward mid-grey target).

## 3. Dehaze: Per-Pixel (Non-Spatial) Variant

**Priority: Low**

zenfilters `Dehaze` uses a spatial dark-channel-prior approach (Gaussian blur
for atmosphere estimation). zenimage's `DehazeOp` is a simpler per-pixel effect:

1. Shadow lift: `amount * 0.15 * (1 - L)^2`
2. S-curve contrast boost: `amount * 0.4`
3. Chroma boost: `a,b *= 1 + amount * 0.3`

This is the imageflow-compatible dehaze — fast, predictable, no neighborhood
requirement. The spatial `Dehaze` is better for real haze removal but is a
different operation with different use cases.

**Options:**
- Add a `SimpleDehaze` filter (per-pixel, no neighborhood) alongside the existing spatial `Dehaze`
- Or add a `spatial: bool` field to `Dehaze` that switches between the two algorithms

The per-pixel variant is a point operation (`is_neighborhood() = false`), so it
benefits from strip processing and is much cheaper than the spatial version.
