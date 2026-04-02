# zenpipe WASM Compatibility Audit

> **⚠️ PARTIALLY STALE** — Original audit 2026-03-25. Items 2-4 below have been fixed.
> zenfilters now has full wasm128 + scalar support. The demo editor runs a 5.6 MB
> WASM module with 6 codecs + 43 filters. See `demo/SPEC.md` for current status.

**Date:** 2026-03-25 (updated 2026-04-02)
**Scope:** zenpipe + all recursive dependencies (runtime + dev/codec deps)
**Targets tested:** wasm32-unknown-unknown, wasm32-wasip1

## TL;DR

zenpipe **compiles clean** for all wasm32 targets, in both `std` and `no_std` modes. The dependency tree is overwhelmingly wasm-ready. Remaining gap:

1. **moxcms** has zero wasm SIMD — scalar-only ICC CMS on wasm
2. ~~**zenfilters** needs 6 missing scalar fallbacks~~ **FIXED** — all 28 `incant!` calls use `[v3, neon, wasm128, scalar]`, all 6 scalar fallbacks exist
3. ~~**linear-srgb** public slice API doesn't auto-dispatch to wasm128~~ (token API works)
4. ~~**zenblend** `mask.rs` uses `f32::ceil()`/`floor()`~~ (minor, no_std only)

Binary size after LTO is excellent: **~9 KB** for a thin wrapper, **~710 KB rlib** pre-LTO.

---

## Compilation Status

| Target | Features | Status |
|--------|----------|--------|
| wasm32-unknown-unknown | `std` | **Clean** (warnings only) |
| wasm32-unknown-unknown | no default features | **Clean** |
| wasm32-wasip1 | `std` | **Clean** |

All three produce zero errors. Warnings are deprecation notices in archmage and dead code in zenresize.

---

## Binary Size

### cdylib (post-LTO, opt-level=z, strip=true)

| Target | Features | Raw | gzip |
|--------|----------|-----|------|
| wasm32-unknown-unknown | std + simd128 | **8,909 B** | 3,850 B |
| wasm32-unknown-unknown | no_std + simd128 | **8,789 B** | 3,854 B |
| wasm32-unknown-unknown | std, no simd128 | **8,930 B** | 3,872 B |
| wasm32-wasip1 | std + simd128 | **28,862 B** | 11,919 B |

Note: cdylib sizes reflect a thin wrapper exercising 5 API entry points. Real usage will be larger but LTO eliminates all unreachable code aggressively.

### rlib sizes (pre-LTO, all compiled code)

| Target | Features | Size |
|--------|----------|------|
| wasm32-unknown-unknown | std + simd128 | 710 KB |
| wasm32-unknown-unknown | no_std + simd128 | 651 KB |

### Heaviest pre-LTO dependencies

| Crate | rlib KB | Notes |
|-------|---------|-------|
| magetypes | 7,749 | SIMD type system — compiles out after LTO |
| wide | 5,985 | Portable SIMD — compiles out after LTO |
| pxfm | 3,870 | Math tables — compiles out if unused |
| moxcms | 3,148 | ICC CMS — compiles out without cms-moxcms |
| num-traits | 1,886 | |
| zenresize | 1,614 | |
| zenfilters | 1,508 | |

Key finding: magetypes + wide are 13.7 MB pre-LTO but compile out **entirely** after LTO. simd128 vs no-simd128 is only a 21-byte difference in the final cdylib.

---

## SIMD Support Matrix

### archmage (dispatch framework) — EXCELLENT

First-class wasm support. Two tiers defined:
- **`wasm128`** — `target_feature(enable = "simd128")`, priority 20
- **`wasm128_relaxed`** — `target_feature(enable = "simd128,relaxed-simd")`, priority 21

Default `incant!` tier list: `[v4, v3, neon, wasm128, scalar]`

`#[rite]` and `#[arcane]` macros handle wasm safely — no `unsafe` wrapper needed (wasm validation model traps deterministically). 256-bit polyfill auto-splits to 2×128-bit. Comprehensive test suite (`tests/wasm_intrinsics_exercise.rs` with 100+ tests).

### Per-crate SIMD status

| Crate | wasm128 tier | Scalar fallback | Status |
|-------|-------------|-----------------|--------|
| **archmage** | Native | N/A | First-class |
| **zenresize** | Yes (`simd/wasm128.rs`) | Yes | Full SIMD on wasm |
| **zenblend** | Yes (`simd/wasm128.rs`) | Yes (SrcOver only) | Full SIMD via wide |
| **zenjpeg** | Yes (`encode/wasm_simd.rs`) | Yes | Full SIMD on wasm |
| **garb** | Yes (`bytes/wasm.rs`) | Yes | Full SIMD on wasm |
| **linear-srgb** | Yes (`tokens/x4`) | Yes | wasm128 in token API, not in public slice dispatch |
| **wide** | Yes (`core::arch::wasm32`) | Yes | Native simd128 support |
| **safe_unaligned_simd** | Yes (`src/wasm32.rs`) | N/A | 14+ safe wrapper functions |
| **zenfilters** | **NO** (only `[v3, neon]`) | **6 functions missing** | **Needs work** |
| **moxcms** | **NO** (raw x86/neon intrinsics) | Yes | Scalar-only on wasm |
| **zengif** | N/A (no SIMD needed) | N/A | Pure algorithm |
| **zenlayout** | N/A (no SIMD needed) | N/A | Pure geometry |

---

## Detailed Findings by Crate

### zenresize — EXCELLENT

- Dedicated `src/simd/wasm128.rs` with `Wasm128Token` dispatch
- All 26 kernels routed through `wide_kernels.rs` (shared with NEON)
- `wide` 1.2.0 compiles to native wasm simd128 instructions
- Transfer functions (sRGB, Bt709, PQ, HLG) all have wasm dispatch
- `no_std` with alloc — proper conditional imports
- Tests use `std::fs` for golden files (can't run tests on wasm, but library code is clean)
- Minor: `layout` feature (zenlayout) is in default features — should be opt-in for wasm

### zenblend — GOOD (one bug)

- Has `src/simd/wasm128.rs` delegating to `wide_kernels.rs`
- `wide` f32x4 compiles to real wasm simd128 (not scalar fallback as initially feared)
- SrcOver blend mode: SIMD-accelerated. All other 25 modes: scalar.
- `#![forbid(unsafe_code)]`
- **Bug:** `mask.rs` lines 458/463/565/574 use `f32::ceil()`/`f32::floor()` which don't exist in no_std. Need `libm::ceilf()`/`libm::floorf()`. Only affects `--no-default-features` build.

### zenfilters — NEEDS WORK

- Already `#![no_std]` with alloc — good baseline
- **All 25 `incant!` calls use `[v3, neon]`** — no wasm128, no scalar in tier list
- archmage's default includes wasm128+scalar, but explicit tier lists override defaults
- **6 functions have NO scalar fallback:** scatter_oklab, gather_oklab, scatter_srgb_u8_to_oklab, gather_oklab_to_srgb_u8, hue_rotate, fused_interleaved_adjust
- `cbrt()` in Oklab pipeline has no wasm intrinsic — needs Newton-Raphson or polynomial
- Estimated effort: 8-12h for full wasm128 support, 4-6h for scalar fallbacks alone

### moxcms — PROBLEM CRATE

- Uses raw `std::arch::x86_64::*` and ARM NEON intrinsics directly (not archmage)
- Zero wasm-specific code. Default features enable AVX/SSE/NEON.
- On wasm: scalar-only ICC CMS. No SIMD acceleration.
- Would need significant rewrite to use archmage for wasm SIMD
- Binary impact: 3.1 MB rlib, but compiles out via LTO if `cms-moxcms` feature disabled

### linear-srgb — GOOD (API gap)

- Has native wasm128 support in `tokens::x4` module
- Rational polynomial approach: ~22 wasm f32x4 instructions per 4-value batch (no `powf`)
- Tiny binary footprint: ~3-5 KB. u8 LUT is 1 KB const, u16 LUT is lazy heap-allocated.
- **Gap:** Public `default::*_slice()` dispatches via `incant!([v4, v3, scalar])` — no wasm128. Must use `tokens::x4::*_wasm128()` from `#[arcane]` context.

### zenjpeg — READY

- Dedicated `src/encode/wasm_simd.rs` with Wasm128Token
- `multiversion`/`multiversed` gracefully fall back to scalar on wasm
- `rayon` is feature-gated (`parallel` feature — don't enable on wasm)
- `#![forbid(unsafe_code)]`
- `web-sys` optional feature for wasm console logging

### zengif — READY

- Pure Rust, zero SIMD needed (GIF is algorithmically simple)
- All quantizers (imagequant, quantizr, zenquant) are pure Rust
- No threading. `AtomicUsize` in stats works as no-op on single-threaded wasm.

### zenlayout — READY

- Pure geometry, single dependency (whereat), `no_std` by default
- Deterministic FP math in `float_math.rs`

### Third-party deps — ALL GOOD

| Crate | wasm compat | Notes |
|-------|-------------|-------|
| wide 1.2.0 | Excellent | Native `core::arch::wasm32` simd128 branch |
| safe_unaligned_simd 0.2.5 | Excellent | Dedicated wasm32 module |
| hashbrown 0.15.5 | Full | Pure Rust no_std HashMap |
| foldhash 0.1.5 | Full | Pure Rust, zero deps |
| libm 0.2.16 | Full | Explicit wasm32 intrinsic support |
| num-traits 0.2.19 | Full | libm feature is wasm-safe |
| bytemuck 1.25.0 | Good | Avoid `zeroable_atomics` on wasm32-unknown-unknown |
| imgref 1.12.0 | Full | Generic Rust, no platform code |
| rgb 0.8.53 | Full | Pure Rust pixel types |
| enough 0.4.2 | Good | Atomics work on wasm (no-op in single-threaded) |
| allocator-api2 0.2.21 | Full | Pure Rust allocator abstraction |
| pxfm 0.1.28 | Full | Pure scalar math, no SIMD |
| garb 0.2.5 | Excellent | Has `src/bytes/wasm.rs` with Wasm128Token |
| multiversion 0.8.0 | Partial | No runtime CPU detection on wasm — scalar fallback only |

---

## Recommendations

### Immediate (no code changes needed)

Build command for wasm:
```bash
# With SIMD (recommended for all modern browsers/runtimes)
RUSTFLAGS="-C target-feature=+simd128" cargo build \
  --target wasm32-unknown-unknown \
  --features std \
  --release

# Without SIMD (maximum compatibility)
cargo build \
  --target wasm32-unknown-unknown \
  --features std \
  --release
```

For codec integration:
```bash
RUSTFLAGS="-C target-feature=+simd128" cargo build \
  --target wasm32-unknown-unknown \
  --features std \
  -p zenjpeg --no-default-features --features std,yuv,trellis \
  -p zengif --no-default-features --features std,quantizr \
  --release
```

### Short-term fixes

1. **zenblend:** Replace `f32::ceil()`/`floor()` with `libm::ceilf()`/`libm::floorf()` in `mask.rs` (4 lines)
2. **zenfilters:** Add `scalar` to all 25 `incant!` tier lists: `[v3, neon, scalar]`
3. **zenfilters:** Implement 6 missing scalar fallbacks (scatter/gather_oklab, hue_rotate, fused_interleaved_adjust)

### Medium-term (wasm SIMD optimization)

4. **zenfilters:** Add `wasm128` tier to incant! calls: `[v3, neon, wasm128, scalar]`
5. **zenfilters:** Create `src/simd/wasm128.rs` with 25 wasm128 implementations
6. **zenfilters:** Implement fast `cbrt` for wasm (Newton-Raphson 3-iteration, or `pow_lowp(x, 1.0/3.0)`)
7. **linear-srgb:** Add `wasm128` to public `incant!` slice dispatch: `[v4, v3, wasm128, scalar]`

### Long-term (performance parity)

8. **moxcms:** Rewrite SIMD paths using archmage (currently raw intrinsics). This would give ICC CMS wasm simd128 support.
9. **Install wasm-opt** for post-build optimization (typically 10-20% size reduction)
10. **Add wasm32 CI target** for zenpipe and key deps

### Not needed

- `enough` atomics: work fine on single-threaded wasm (compile to plain loads/stores)
- `bytemuck`: default features are wasm-safe
- `multiversion`: gracefully falls back to scalar, no action needed
- Binary size: LTO eliminates unused code aggressively, pre-LTO bloat is irrelevant

---

## Performance Expectations (wasm vs native x86_64 AVX2)

| Operation | wasm scalar | wasm simd128 | Notes |
|-----------|------------|-------------|-------|
| Resize (zenresize) | ~25% | ~50-60% | Full wasm128 dispatch |
| Blend SrcOver (zenblend) | ~25% | ~50% | via wide f32x4 |
| Blend other modes | ~60% | ~60% | Scalar only (all platforms) |
| sRGB conversion (linear-srgb) | ~60% | ~80% | Rational polynomial, no powf |
| JPEG encode (zenjpeg) | ~25% | ~50-60% | Has wasm_simd.rs |
| GIF encode/decode (zengif) | ~85% | ~85% | Pure algorithm, no SIMD benefit |
| Layout (zenlayout) | ~95% | ~95% | Pure geometry |
| Filters (zenfilters) | **0%** (missing fallbacks) | **0%** (no wasm128 tier) | **Blocked** |
| ICC CMS (moxcms) | ~30% | ~30% | Scalar-only, no wasm SIMD |
| Pixel swizzle (garb) | ~25% | ~50% | Has wasm128 dispatch |

---

## Architecture Summary

```
zenpipe (wasm32)
├── zenresize      ✅ Full wasm128 SIMD
│   ├── archmage   ✅ First-class wasm support
│   ├── wide       ✅ Native wasm simd128
│   └── linear-srgb ✅ wasm128 in token API
├── zenblend       ✅ wasm128 via wide (1 bug in no_std)
├── zenfilters     ❌ No wasm128, 6 missing scalar fallbacks
│   └── linear-srgb ⚠️ Public API misses wasm128 dispatch
├── zenpixels-convert ✅ (without cms-moxcms)
│   ├── garb       ✅ Full wasm128 SIMD
│   └── moxcms     ❌ Scalar-only on wasm (optional)
├── zencodec       ✅ Pure traits
├── hashbrown      ✅ no_std HashMap
├── enough         ✅ Cooperative cancellation
└── Codecs (dev-deps, future runtime deps)
    ├── zenjpeg    ✅ Has wasm_simd.rs
    ├── zengif     ✅ Pure Rust
    └── zenlayout  ✅ Pure geometry
```
