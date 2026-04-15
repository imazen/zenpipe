# Rayon Parallelism Architecture for zenfilters

## Context

zenfilters processes images sequentially: scatter (RGB->Oklab) -> filter chain -> gather (Oklab->RGB). SIMD vectorizes within each core, but large images (4K+) leave cores idle. Adding rayon support should scale throughput with core count without penalizing the single-threaded path.

## Design Principles

1. **Feature-gated**: All rayon code behind `#[cfg(feature = "rayon")]`. Zero cost when disabled.
2. **Separate API**: New `apply_par()` method, existing `apply()` unchanged.
3. **Two-level parallelism**: Strip-level for per-pixel pipelines, within-filter for neighborhood pipelines.
4. **No trait changes visible without the feature**: `apply_par` on Filter only exists when rayon is on.
5. **No unsafe**: All parallel splits via `par_chunks_mut`, `par_iter`, zip patterns.

## Architecture

### Level 1: Strip Parallelism (per-pixel-only pipelines)

When no neighborhood filters are present, strips are independent (halo=0). Each strip: scatter -> all filters -> gather. Process strips in parallel via `dst.par_chunks_mut(strip_stride)`.

```
Strip 0: [scatter -> exposure -> contrast -> saturation -> gather]  Thread A
Strip 1: [scatter -> exposure -> contrast -> saturation -> gather]  Thread B
Strip 2: [scatter -> exposure -> contrast -> saturation -> gather]  Thread C
Strip 3: [scatter -> exposure -> contrast -> saturation -> gather]  Thread A
```

Each strip gets a fresh `FilterContext` (per-pixel filters don't use scratch buffers, so this is zero-cost). `src` is `&[f32]` (shared reads). `dst` is split into non-overlapping mutable chunks. `&self` on Pipeline/Filter is safe (`Filter: Sync`).

### Level 2: Within-Filter Parallelism (neighborhood pipelines)

When neighborhood filters are present, the pipeline uses full-frame planes. The filter chain must run sequentially (each filter depends on the previous), but three things can parallelize:

1. **Scatter** (RGB->Oklab): Split plane slices into row chunks, scatter in parallel.
2. **Individual filter internals**: Gaussian blur H-pass rows are independent, V-pass rows are independent.
3. **Gather** (Oklab->RGB): Split plane/dst slices into row chunks, gather in parallel.

```
Scatter:  [rows 0-255]  [rows 256-511]  [rows 512-767]  ...  (parallel)
Filter 1: clarity.apply_par()  (internally parallel blur)
Filter 2: sharpen.apply_par()  (internally parallel blur)
Gather:   [rows 0-255]  [rows 256-511]  [rows 512-767]  ...  (parallel)
```

### Parallel Gaussian Blur

The single biggest win. Called by 15 of 17 neighborhood filters.

**FIR path (sigma < 6)**:
- H-pass: `h_buf.par_chunks_mut(width)` - each row reads src (shared) + edge padding, writes one row of h_buf
- V-pass: `dst.par_chunks_mut(width)` - each row reads h_buf (shared, immutable after H-pass), writes one row of dst
- Per-row scratch (padded row buffer) allocated inside the closure (small: width + 2*radius floats)

**Stackblur path (sigma >= 6)**:
- H-pass: `h_buf.par_chunks_mut(width)` - each row has independent circular buffer state
- V-pass: transpose -> `transposed_out.par_chunks_mut(height)` row-parallel -> transpose back
- Per-row scratch (stack buffer) allocated inside the closure (small: 2*radius+1 floats)

### Filter Trait Extension

```rust
pub trait Filter: Send + Sync {
    // ... existing methods unchanged ...

    #[cfg(feature = "rayon")]
    fn apply_par(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        self.apply(planes, ctx)  // default: sequential fallback
    }
}
```

Neighborhood filters override `apply_par` to call `gaussian_blur_plane_par` instead of `gaussian_blur_plane`. Per-pixel filters inherit the default (which just calls `apply`).

### Minimum Size Threshold

`apply_par()` falls back to `apply()` when `width * height < 262_144` (512x512). Below this, rayon scheduling overhead exceeds the computation.

### Thread Pool Control

No custom pool parameter. Users control the pool via rayon's standard `pool.install(|| pipeline.apply_par(...))` pattern. This keeps the API clean and is idiomatic rayon.

### no_std Compatibility

```rust
#![cfg_attr(not(feature = "rayon"), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;
```

When rayon is off: `no_std`. When rayon is on: `std` (rayon requires threads). `extern crate alloc` works in both modes.

## Files to Modify

| File | Changes |
|------|---------|
| `Cargo.toml` | Add `rayon = ["dep:rayon"]` feature, `rayon = { version = "1.10", optional = true }` dep |
| `src/lib.rs` | `#![no_std]` -> `#![cfg_attr(not(feature = "rayon"), no_std)]`, re-export new items |
| `src/filter.rs` | Add `#[cfg(feature = "rayon")] fn apply_par()` default method |
| `src/pipeline.rs` | Add `apply_par()`, `apply_stripped_par()`, `apply_full_frame_par()`, `apply_planar_par()` |
| `src/blur.rs` | Add `gaussian_blur_plane_par()`, `stackblur_plane_par()` |
| `src/filters/clarity.rs` | Override `apply_par` to use `gaussian_blur_plane_par` |
| `src/filters/sharpen.rs` | Override `apply_par` |
| `src/filters/brilliance.rs` | Override `apply_par` |
| `src/filters/bloom.rs` | Override `apply_par` |
| `src/filters/noise_reduction.rs` | Override `apply_par` |
| `src/filters/bilateral.rs` | Override `apply_par` |
| `src/filters/dehaze.rs` | Override `apply_par` |
| `src/filters/texture.rs` | Override `apply_par` |
| `src/filters/adaptive_sharpen.rs` | Override `apply_par` |
| `src/filters/local_tone_map.rs` | Override `apply_par` |
| `src/filters/tone_equalizer.rs` | Override `apply_par` |
| `src/filters/edge_detect.rs` | Override `apply_par` |
| `src/filters/blur.rs` | Override `apply_par` |
| `src/filters/median_blur.rs` | Override `apply_par` (if applicable) |

## Implementation Sequence

1. **Cargo.toml + lib.rs**: Feature gate, rayon dep. Verify compiles with and without `--features rayon`.
2. **blur.rs**: `gaussian_blur_plane_par()` and `stackblur_plane_par()`. This is the core engine - test independently.
3. **filter.rs**: Add `apply_par` default trait method.
4. **pipeline.rs**: `apply_par()` with both stripped and full-frame parallel paths.
5. **Neighborhood filter overrides**: Start with clarity/sharpen/brilliance (most common), then remaining filters.
6. **Tests**: Verify `apply_par` output matches `apply` within tolerance for both per-pixel and neighborhood pipelines.

## Parallel Scatter/Gather Detail

For `apply_full_frame_par`, parallel scatter needs simultaneous mutable access to `planes.l`, `planes.a`, `planes.b`:

```rust
let chunk_rows = strip_height(width, has_alpha, 0);
let row_stride = w;
let src_stride = w * ch;

planes.l.par_chunks_mut(chunk_rows * row_stride)
    .zip(planes.a.par_chunks_mut(chunk_rows * row_stride))
    .zip(planes.b.par_chunks_mut(chunk_rows * row_stride))
    .enumerate()
    .for_each(|(i, ((l_c, a_c), b_c))| {
        let y = i * chunk_rows;
        let rows = l_c.len() / row_stride;
        let off = y * src_stride;
        simd::scatter_oklab(&src[off..off + rows * src_stride], l_c, a_c, b_c, channels, &m1, inv_white);
    });
```

This works because `par_chunks_mut` on three separate `Vec<f32>` fields produces non-overlapping borrows. The zip creates tuples of corresponding chunks.

## Verification

1. `cargo check` and `cargo check --features rayon` both pass
2. `cargo test` (no rayon) passes unchanged
3. `cargo test --features rayon` passes with new parallel tests
4. New test: `apply_par_matches_apply` - run same pipeline through both paths, assert max error < 1e-6
5. New test: `apply_par_neighborhood_matches` - same for neighborhood pipelines
6. Benchmark: `cargo bench --features rayon` comparing `apply` vs `apply_par` at 1080p, 4K, 8K

## Risks & Mitigations

- **SIMD tokens in closures**: archmage tokens are stateless function pointers - safe across threads. No issue.
- **FilterContext in parallel blur**: H-pass scratch (`h_buf`) allocated from `ctx` before parallel region, then split. Per-row scratch allocated locally inside closure.
- **Stackblur V-pass**: Transpose approach avoids strided writes, keeping everything in safe Rust.
- **Feature-gated trait method**: Standard Rust pattern. Callers must enable rayon feature to access `apply_par`.
