# Gentle white-background noise removal — the validated recipe

**This is the one.** The method for removing sub-pixel render noise from near-white
studio/product backgrounds **gently** — without touching the product, its contact
shadows, or creating any edge line. Validated on the AI product corpus
(`/mnt/v/zen/ai-corpus/products`, 2026-05-31): **713/750 cleaned, 0 errors**,
shadows preserved, no halos.

Implementation: [`examples/ai_corpus_flatten.rs`](examples/ai_corpus_flatten.rs)
→ `white_snap()`. It can be promoted to a library filter (a `BackgroundClean`
node) later; the algorithm below is the reference.

## Recipe

1. **Background-type guard.** Median of the border's min-channel; if it's below
   `skip_floor` (235) the background isn't near-white → **skip the whole image**
   (never partially-clean a gray/colored backdrop — that blotches).

2. **Measure the image's own white.** Mean and std of the border's near-white
   pixels (min-channel ≥ 244). Clean diffusion renders have std < 1 — a *tiny*
   range around an "average white" of ~252–254.

3. **Eligibility = a tiny band around that white.** A pixel is background only if
   its min-channel ≥ `thresh`, where `thresh = white_mean − (5 + 4·white_std)`,
   clamped to `[244, 252]`. Anything darker — shadow, product, cream/off-white
   product — is below the band and never eligible.

4. **Connectivity (the change-allowed mask).** 4-connected flood fill from the
   image border through eligible pixels. Only border-connected near-white counts
   as background; a near-white region *inside* the product is never reached, and
   a light/cream product halts the fill at its own (slightly-darker) edge.

5. **Snap to PURE white (255), feathered two ways** so there is never a hard
   boundary:
   - **Luminance feather** across `[thresh, thresh+ramp]` (ramp = 6): pixels at
     the bottom of the band are barely touched, so a soft penumbra eases in
     rather than stepping at the threshold contour.
   - **Spatial feather** across `[0, shadow_radius]` (≈ **64 px**), where distance
     is a chamfer transform from the nearest non-background ("no-go") pixel: the
     snap eases from full 255 in the open field down to **zero** as it approaches
     any shadow/product. The `255 → original` transition is spread over the
     radius, so it is imperceptible. **The feather only ever touches background
     (flood-mask) pixels; it fades to zero at the no-go boundary and never
     modifies product or shadow pixels.**

6. **Re-run safety.** Write-once `_orig_<name>` backup; always process *from* the
   pristine original; the flattened result takes the original filename;
   non-candidates are restored from backup and marked `_skip_`. Repeated runs are
   idempotent and never degrade or clobber the original.

## Why — approaches that failed first (don't repeat)

- **`BackgroundFlatten` (the library filter) at full strength** flooded light /
  cream products and lifted green products toward white — its edge-seeded fill
  treated them as background → mangled interiors.
- **A hard mask clip** (restore original outside a near-white mask) produced hard
  shadow edges at the mask boundary.
- **Pure-255 snap with only a luminance feather** left a faint **halo line** where
  the flat 255 met the shadow's gradient (a Mach band).
- **Snapping to the measured off-white** killed the line but the off-white
  **clashed with the white page** — not acceptable.
- **The fix:** pure 255 **+** a *large* spatial feather (~64 px), so the
  brightening transition is gradual and invisible while the open background stays
  truly white.

## Parameters (example CLI)

| flag | default | meaning |
|------|---------|---------|
| `--skip-floor` | 235 | border min-channel median below this ⇒ not a white bg ⇒ skip |
| `--white-ramp` | 6 | luminance feather width above `thresh` |
| `--shadow-radius` | 64 | spatial feather radius (px) easing the snap away from shadows/products |

## Scope / non-goals

- Cleans **near-white** backgrounds only; colored/gradient backdrops are skipped
  by design.
- Does **not** posterize or touch the subject — it is a *gentle* denoise of the
  background, not a cutout or a cartoonifier.
