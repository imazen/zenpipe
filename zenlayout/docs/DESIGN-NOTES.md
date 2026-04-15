# DimensionEffect Design Notes

## Warp/Perspective as DimensionEffect

Spatial transforms (perspective correction, lens distortion, deskew) are not
just "same dims with pixel shuffling" — they can and should change output
dimensions based on a **resolution policy**.

### The problem

A document photo with perspective: the near edge has more pixels (say 1000px
wide) and the far edge has fewer (say 600px). Current zenfilters behavior
keeps input dimensions and warps into the same canvas. This means:

- The narrow (far) edge is **upsampled** → blurry text
- The wide (near) edge is kept at full resolution → wasted work

### Resolution policies for non-uniform transforms

Given a 3×3 projective matrix M mapping output→source, each output pixel
samples from a different-sized source region. The local scale factor varies
across the image. A resolution policy picks the output dimensions:

| Policy | Output size | Quality | Use case |
|--------|-------------|---------|----------|
| `MatchNarrow` | Smallest local scale | Sharp everywhere, smaller output | Document OCR, archival |
| `MatchWide` | Largest local scale | Preserves all source detail, blurry in upsampled regions | Photo editing |
| `MatchArea` | Geometric mean scale | Balanced | General purpose |
| `PreserveInput` | Same as input | Simple, current behavior | Backward compat |
| `Custom(w, h)` | Caller-specified | Full control | UI preview at specific size |

### How this maps to DimensionEffect

`WarpEffect` (a new type in zenlayout or zenfilters) implements `DimensionEffect`:

```rust
struct WarpEffect {
    matrix: [f32; 9],
    policy: ResolutionPolicy,
}

impl DimensionEffect for WarpEffect {
    fn forward(&self, w: u32, h: u32) -> Option<(u32, u32)> {
        // Compute local scale at corners/edges from matrix
        // Apply policy to choose output dims
        Some(compute_output_dims(w, h, &self.matrix, self.policy))
    }
    // ...
}
```

The planner uses this to adjust the resize target. The execution engine
(zenpipe) materializes the warp at the resolved dimensions.

### What lives where

- **zenlayout**: `DimensionEffect` trait, `RotateEffect`, pure math functions,
  `ResolutionPolicy` enum, `compute_warp_output_dims()` (pure geometry from
  matrix corner mapping — no pixel ops)
- **zenfilters**: `Warp` filter that implements the actual pixel transform,
  `WarpEffect` adapter that wraps a matrix + policy into a `DimensionEffect`
- **zenpipe**: chains effects and inserts materialization barriers

### Transforms that change dimensions

| Transform | Dimension behavior |
|-----------|--------------------|
| Rotation (inscribed crop) | Smaller: `short/(long*sin + short*cos)` scale |
| Rotation (expand) | Larger: bounding box `w*cos+h*sin` × `w*sin+h*cos` |
| Perspective correction | Policy-dependent: local scale varies by position |
| Lens distortion (barrel undistort) | Usually smaller (center shrinks) |
| Lens distortion (pincushion undistort) | Usually larger (edges expand) |
| Deskew with fill | Same dims (fill borders) |
| Affine (uniform scale) | Scale factor × input |
| Trim (content-aware) | Unknown until analysis — analysis barrier |

### Relationship to zenresize streaming

zenresize's streaming pipeline (crop→resize→pad→orient) is not affected.
Effects are separate pipeline stages with materialization barriers. The
planner computes the resize target accounting for effects, but zenresize
only sees the resize step with correct pre-computed dimensions.
