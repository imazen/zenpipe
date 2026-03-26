# WASM SIMD128 Audit Findings

Audit of the zen image processing stack for wasm32 SIMD support, conducted 2026-03-25.

## Summary

We audited and optimized 10 crates for wasm simd128 performance. The biggest wins came from zenfilters (3.8x on blur via real `f32x8` SIMD) and the biggest remaining gaps are algorithmic (DEFLATE, VP8 entropy coding) where SIMD can't help.

All benchmarks: 4K (3840x2160) RGBA gradient, wasmtime with `--wasm simd`, `-C target-feature=+simd128`, `opt-level=s`, LTO.

## Before/After (WASM simd128, 4K)

| Operation | Before | After | Speedup |
|---|---|---|---|
| Resize 4K→720p | 468ms | 355ms | 1.3x |
| Filters per-pixel (exp+sat+con) | 281ms | 112ms | **2.5x** |
| Filters neighborhood (blur+sharp) | 1158ms | 306ms | **3.8x** |
| JPEG encode Q85 | 274ms | 267ms | ~1.0x |
| JPEG decode | 96ms | 92ms | ~1.0x |
| PNG encode | 2767ms | 2590ms | 1.07x |

## WASM vs Native (same 128-bit SIMD width)

Native x86_64 with SSE2 baseline (no AVX2) uses the same 128-bit SIMD width as wasm simd128. Comparing these isolates wasmtime JIT overhead from SIMD width differences.

| Operation | Native SSE2 | WASM simd128 | Overhead |
|---|---|---|---|
| Resize | 50ms | 355ms | 7.1x |
| Filters per-pixel | 44ms | 112ms | 2.5x |
| Filters blur+sharp | 199ms | 306ms | 1.5x |
| JPEG encode | 50ms | 267ms | 5.3x |
| PNG encode | 1090ms | 2590ms | 2.4x |

The overhead varies from 1.5x (filters, which have dense SIMD utilization) to 7.1x (resize, which is memory-bandwidth-bound with scattered access patterns). This is wasmtime JIT codegen quality and wasm calling convention overhead, not a SIMD gap.

## Profiling methodology

`wasmtime --profile=guest` with an unstripped binary produces Firefox Profiler-compatible JSON with function-level self-time sampling. Parse with:

```bash
wasmtime --profile=guest,profile.json --wasm simd binary.wasm
# View at https://profiler.firefox.com/ or parse the JSON
```

For fair native comparison, `archmage::dangerously_disable_tokens_except_wasm(true)` disables all archmage SIMD tokens, forcing `incant!` to select scalar fallbacks. Set via `SCALAR_ONLY=1` env var in the demo. Note: this only affects archmage-dispatched code. Crates using `wide` or `multiversion` still get SSE2 auto-vectorization on x86_64 since SSE2 is the baseline.

## Profile breakdown (top self-time, 4K full pipeline)

| % | Function | Status |
|---|---|---|
| 10.6% | zenpng encoder filter | `#[autoversion]` added, but global simd128 flag already auto-vectorizes |
| 9.6% | zenflate DEFLATE compress | Algorithmic (LZ77 match finding), not SIMD-amenable |
| 9.5% | zenwebp VP8 encode | Already has hand-written wasm SIMD (`simd_wasm.rs`) |
| 6.3% | zenflate matchfinder slide_window | `#[autoversion]` on `matchfinder_rebase` (i16 saturating add) |
| 5.2% | zenresize vertical f16 filter | magetypes f32x4, could add wasm128 intrinsics |
| 4.1% | zenfilters Gaussian blur | Real f32x8 SIMD via `#[magetypes(neon, wasm128)]` |
| 4.1% | zenpng bigrams_score | Hash table lookups, inherently serial |
| 3.8% | zenpng bigram_entropy_score | Same |

## What we changed

### zenfilters (biggest win: 3.8x)

Refactored `src/simd/neon.rs` (1793 lines) into shared `src/simd/wide_simd.rs` (1505 lines) using `#[magetypes(neon, wasm128)]` to generate both NEON and wasm128 variants from one source. The `#[magetypes]` macro does text substitution of the `Token` identifier, so `f32x8<Token>` becomes `f32x8<NeonToken>` or `f32x8<Wasm128Token>` — both polyfilled as 2x128-bit. All 22 SIMD functions (scatter/gather Oklab, fused adjust, Gaussian blur, hue rotate, etc.) now have real vectorized wasm128 code.

### linear-srgb

Added `wasm128_slice_tiers!` macro generating 4-wide f32x4 slice conversion functions. 10 `incant!` calls updated from `[v4, v3, scalar]` to `[v4, v3, wasm128, scalar]`. Uses the existing rational polynomial approximation at 4-wide, not lookup tables.

### zenpng

Hand-written wasm128 intrinsics for all 6 decoder unfilter operations (Up, Sub bpp3/4, Avg bpp4, Paeth bpp3/4). Up filter uses `i8x16_add` for 16 bytes/iteration. Paeth uses branchless predictor with `i16x8_abs` + `v128_bitselect` — direct port of the x86 V2 approach. Also `#[autoversion]` on encoder filter functions.

### zenflate

`#[autoversion]` on `matchfinder_rebase` (i16 saturating add on 64K array) and `matchfinder_init`.

### zenresize

Migrated `wide_kernels.rs` from `wide` crate to `magetypes` generic types with `#[magetypes(neon, wasm128)]`. Removed `wide` dependency. Added wasm128-intrinsic i16 horizontal convolution kernel using `v128_load` + `u16x8_extend_low/high` + `i32x4_dot_i16x8` + `i8x16_shuffle` — mirrors the x86 AVX2 kernel at 128-bit width. All via archmage `#[rite(wasm128, import_intrinsics)]` — no unsafe code.

### zenjpeg

Migrated from `multiversed`/`multiversion` to archmage `#[autoversion]` across 29 functions. Removed `multiversion` and `multiversed` dependencies. Unified SIMD dispatch under archmage across the entire zen codec stack.

### jxl-encoder-rs

Added `#[arcane]` wasm128 wrappers for all 5 AC strategy search functions + reconstruction. Re-exported 22 existing wasm128 SIMD primitives from jxl_simd.

### zengif

Implemented `GifStreamingDecoder` — decodes first frame fully, then yields rows in 16-row batches via `next_batch()`. GIF is frame-based, not row-based, so this bridges the zencodec streaming decode API.

## Key patterns

### `#[magetypes(neon, wasm128)]` — shared SIMD code generation

The archmage `#[magetypes]` macro does text substitution of `Token` with the concrete token type per tier. This generates tier-suffixed function variants (`_neon`, `_wasm128`) from one source, each with the right `#[arcane]` and `cfg` guards. The `f32x8` type is polyfilled as 2xf32x4 for both NEON and wasm128.

### `#[autoversion]` — placement matters

`#[autoversion]` generates a runtime dispatch trampoline. Put it on outer loops (per-row, per-block-batch), not per-pixel/per-sample functions. A 10ns trampoline on a 5ns function body is a 3x overhead. Rule of thumb: fine if called less than ~100 times per scanline.

### `wide` crate vs `magetypes`

`wide::f32x4` uses `cfg(target_feature="simd128")` — a compile-time check resolved by the global `-C target-feature=+simd128` flag. This means `#[autoversion]` and `#[arcane]` per-function `target_feature` attributes don't help `wide` types — they're already SIMD or not based on the global flag. `magetypes` types take a token parameter, so they always produce the right instructions for the token's tier.

### wasm128 intrinsics via archmage

`#[rite(wasm128, import_intrinsics)]` makes `core::arch::wasm32::*` intrinsics safe to call within the function body. No `unsafe` keyword needed. This enables hand-written wasm SIMD while maintaining `#![forbid(unsafe_code)]`.

## What doesn't help

- **`#[autoversion]` with global `-C target-feature=+simd128`**: When the global flag is set, LLVM already auto-vectorizes with simd128. Per-function `target_feature` adds trampoline overhead for zero benefit. Only helps when the global flag is NOT set (e.g., multi-target binaries).

- **SIMD for DEFLATE/LZ77**: Match finding is hash-chain traversal with data-dependent branches. Huffman coding is bit-level serial. The only SIMD-friendly part (adler32 checksum) was already optimized.

- **SIMD for entropy estimation**: Bigram frequency counting uses hash table random access — inherently serial. Autoversion doesn't change the bottleneck.

## Binary size

cdylib with all codecs (JPEG, PNG, WebP, GIF, JXL, AVIF decode), LTO, simd128, opt-level=s:
- Raw: 5.1 MB
- Gzip: 1.7 MB
