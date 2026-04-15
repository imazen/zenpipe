# SIMD Warp Prototype: Results and Analysis

## Benchmark Environment

- Image: 1920x1080 (2.07 Mpix), 5-degree rotation, Robidoux 4x4 kernel
- CPU: WSL2 (x86_64 with AVX2)
- No `-C target-cpu=native` (runtime dispatch via archmage)
- 10 iterations per measurement, 3 warmup iterations

## Results Summary

### Single Plane

| Approach | ms/plane | Mpix/s | vs Scalar |
|----------|----------|--------|-----------|
| Scalar reference | 37.7 | 55.0 | 1.0x |
| SIMD planar (A) | 21.1 | 98.2 | **1.8x** |
| SIMD row-gather (B) | 45.5 | 45.5 | 0.83x |

### Full 3-Plane Warp (L + a + b)

| Approach | ms/3-plane | Mpix/s | vs Scalar |
|----------|------------|--------|-----------|
| Scalar 3x sequential | 113.9 | 54.6 | 1.0x |
| SIMD planar 3x (A) | 66.6 | 93.4 | 1.7x |
| SIMD row-gather 3x (B) | 131.3 | 47.4 | 0.87x |
| Fused 3-plane scalar (C) | 62.6 | 99.4 | 1.82x |
| **Fused 3-plane SIMD (D)** | **27.9** | **222.9** | **4.08x** |

## Approach Analysis

### A: Planar SIMD (8-wide AVX2)

Process 8 consecutive output pixels in parallel per plane. Source coordinates
computed incrementally (add m0/m3 per step). Robidoux kernel evaluated using
Horner's method on f32x8 (branchless via `blend`). Source pixels gathered
individually (no safe AVX2 gather wrapper in magetypes).

**Result: 1.8x per plane.** The SIMD kernel evaluation saves ~40% of the
arithmetic, but the scalar gather (32 random f32 loads per 8 output pixels)
dominates. SIMD advantage is capped by the serial memory access pattern.

### B: Row-Gather (precomputed coordinates)

Precompute all source coordinates and kernel weights for an entire output row,
then sweep through source rows. Intended to be more cache-friendly.

**Result: 0.83x (slower than scalar).** The overhead of materializing temporary
buffers for coordinates and weights outweighs the cache benefit. The working set
for buffers (7 arrays * width * 4 bytes) adds L1 pressure. Not viable.

### C: Fused 3-Plane Scalar

Process all 3 planes in a single per-pixel loop. Source addresses and kernel
weights computed once, then reused for L, a, b gathers.

**Result: 1.82x for 3-plane.** Confirms that address computation is a major
cost. Computing indices once and loading from 3 planes at those addresses
nearly halves the total time. Better cache locality too: each source pixel
neighborhood is only accessed from L1/L2 once.

### D: Fused 3-Plane SIMD (winner)

Combines SIMD kernel evaluation from A with address amortization from C.
For each batch of 8 output pixels:
1. Compute source coordinates via incremental f32x8 (2 adds)
2. Evaluate Robidoux weights in SIMD (Horner + blend, no branches)
3. Normalize weights in SIMD (sum + recip)
4. Extract weights to arrays (to_array)
5. For each pixel, compute 16 linear indices once
6. Gather from L, a, b at those indices (48 loads reusing 16 addresses)
7. Accumulate separable convolution (scalar per pixel, 3 planes fused)
8. Store results via f32x8

**Result: 4.08x for 3-plane.** The two multiplicative speedup factors:
- SIMD kernel evaluation: ~1.8x (same as approach A)
- Address amortization: ~2.3x (same addresses used for 3 planes, better cache)

## Bottleneck Analysis

The dominant cost is **scalar gather**: 16 random f32 loads per pixel from
source planes at non-contiguous addresses. For a 1920x1080 plane, the source
is ~8 MB, larger than L2 cache. Each pixel's 4x4 neighborhood spans 4 rows,
each at a different cache line.

For small rotations (5 deg), adjacent output pixels map to nearly adjacent
source pixels (cos(5) = 0.996), so the same cache lines are reused heavily.
This is why the fused approach wins: the second and third plane loads hit
warm cache lines.

### Why not AVX2 gather (`vpgatherdd`)?

magetypes does not expose `_mm256_i32gather_ps` through a safe wrapper (it
requires raw pointer access, which violates `#![forbid(unsafe_code)]`).
Even if it did, `vpgatherdd` on Haswell/Skylake takes ~20 cycles for 8
scattered loads, which is only marginally faster than 8 scalar loads at
~3 cycles each from L1.

## Correctness

All approaches verified against the scalar reference:
- Max absolute difference: < 1e-5 (due to mul_add vs separate mul + add)
- Zero pixels exceeding the 1e-5 threshold
- Tested with both Clamp and Color background modes

## Current state

- **Approach D (fused 3-plane SIMD)** is wired into `Warp::apply` and
  `Rotate::apply` via `warp_planes_fused()`. Always uses Robidoux kernel.
- **Multi-arch dispatch** via `#[magetypes(v3, neon, wasm128)]` — the fused
  3-plane inner loop generates NEON and WASM128 variants automatically.
  Single-plane alpha uses scalar fallback on non-x86 (future: add SIMD).
- **SIMD path always uses Robidoux** regardless of `WarpInterpolation` setting.
  Other kernels are only supported by the scalar fallback (perspective
  transforms, or builds without `experimental` feature).

## Future work

1. **Fuse alpha as 4th channel** in the fused SIMD loop — currently runs
   as a separate single-plane pass (~7ms overhead at 1080p).
2. **Add bilinear SIMD** for real-time preview (2×2 = 4 loads vs 4×4 = 16).
3. **AVX-512 gather**: `vpgatherdd` with 16-wide could reduce the gather
   bottleneck. Requires safe wrappers in archmage/magetypes.

## Dead end: interleaved RGBA u8

We prototyped an interleaved RGBA u8 warp to skip the Oklab round-trip
for standalone rotation. The hypothesis was wrong:

| Path | Time | Mpix/s |
|------|------|--------|
| Fused 3-plane f32 SIMD | 28ms | 222 |
| RGBA u8 SIMD | 64ms | 32 |
| RGBA u8 scalar | 99ms | 21 |

The interleaved u8 path is **2.3× slower** than planar f32 SIMD even
without counting Oklab conversion overhead (~5ms). Root cause:
4-byte-strided u8 gathers are catastrophically cache-hostile compared
to contiguous f32 plane access. The Oklab cost is dwarfed by the
gather penalty. Code was removed; see git history for the full
implementation.

## Files

- `src/filters/warp_simd.rs` — Planar SIMD approaches A-D + tests (`pub(crate)`)
- `src/filters/warp.rs` — `Rotate`, `Warp`, `RotateMode`, `WarpBackground` public API
- `src/zennode_defs.rs` — `RotateDef`, `WarpDef` node definitions (feature-gated)
- `examples/warp_bench.rs` — Benchmark harness
- All gated behind `feature = "experimental"`
