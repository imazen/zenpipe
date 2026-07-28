# Per-kernel NEON isolation — 2026-07-28

Platform: Apple Silicon (aarch64, NEON), darwin 25.5.0
Bench: `benches/kernel_tiers.rs` (zenbench, interleaved arms), 1 MP plane per kernel

`color_grading.rs` and `row_batch.rs` measure whole filter pipelines. An aggregate cannot
reveal a single kernel SLOWER than its own scalar fallback — the faster kernels average it
away. That failure mode was found and fixed in garb, zensim, zentone, zenpng and zenresize
during the same aarch64 sweep, so these 11 kernels were checked individually.

## Result: no losers

| kernel | NEON | scalar | ratio | bound |
|---|---|---|---|---|
| power_contrast_plane | 1.1 ms | 3.1 ms | **2.82×** | transcendental (pow) |
| scatter_srgb_u8_to_oklab | 2.0 ms | 3.4 ms | **1.71×** | convert + matrix |
| gather_oklab_to_srgb_u8 | 2.9 ms | 4.5 ms | **1.55×** | convert + LUT |
| highlights_shadows | 424 µs | 648 µs | 1.53× | arithmetic |
| vibrance | 1.4 ms | 1.9 ms | 1.37× | arithmetic |
| sigmoid_tone_map_plane | **1.5 ms** | 2.2 ms | **1.40×** | transcendental (pow) — see below |
| unsharp_fuse | 158 µs | 159 µs | 1.00× | **memory bandwidth** |
| square_plane | 124 µs | 124 µs | 1.00× | **memory bandwidth** |
| subtract_planes | 161 µs | 162 µs | 1.01× | **memory bandwidth** |
| scale_plane | 121 µs | 121 µs | 1.00× | **memory bandwidth** |
| hue_rotate | 216 µs | 220 µs | 1.02× | **memory bandwidth** |

## A real loser was found here, and fixed

`sigmoid_tone_map_plane` was the one kernel measuring SLOWER on NEON than its own
autovectorized scalar fallback: **2.3 ms NEON vs 2.0 ms scalar**, i.e. scalar 11.6–14.1%
faster, reproduced across runs. This is precisely the failure the per-kernel bench exists to
surface and that a pipeline-level aggregate hides.

Cause: three `f32x8::recip()` calls per element (bias denominator, `x_safe`, and
`1 + powered`). On this core `recip()` is a reciprocal *estimate* plus Newton refinement,
which costs more than the hardware divide it replaces — and is less accurate.

Fix: use true division. Results:

| | before | after |
|---|---|---|
| NEON | 2.3 ms | **1.5 ms** |
| vs scalar tier | **0.87× (losing)** | **1.40× (winning)** |
| tier divergence | 130,125 / 262,144 samples | 20,301 / 262,144 |
| worst ULP delta | 22 | 18 |

So it is 1.53× faster **and** measurably more accurate — division is exactly rounded where the
estimate is not, so the NEON path moved toward the scalar reference rather than away from it.

**Output changes very slightly** (~1e-6 on a [0,1] tone curve). That is a precision *gain*, not
a loss, but it is a change and is called out here rather than buried.

**Blast radius is NEON and WASM only.** `wide_simd.rs` is `#[magetypes(neon, wasm128)]` and
only `simd/neon.rs` and `simd/wasm128.rs` consume it; x86 has a separate path and is untouched.

### Then the other two sites were measured — and both were also losing

`brilliance_apply` (:477) and the adaptive-sharpen gate (:1115/:1117) have the identical
`x * y.recip()` shape. Added to this bench rather than changed on inference, and both turned
out to be NEON regressions too:

| kernel | NEON before | NEON after | vs scalar: before → after |
|---|---|---|---|
| adaptive_sharpen_apply | 559.7 µs | **317.1 µs** (1.77×) | 0.69× losing → **1.17× winning** |
| brilliance_apply | 607.3 µs | **467.6 µs** (1.30×) | 0.98× losing → **1.31× winning** |
| sigmoid_tone_map_plane | 2.3 ms | **1.4 ms** (1.64×) | 0.87× losing → **1.44× winning** |

`adaptive_sharpen_apply` was the worst: forced-scalar was 28–34% faster than the hand-written
NEON path.

### The underlying fact, measured directly

A standalone microbenchmark of the primitive (`~/tmp/recipbench`, raw NEON intrinsics,
1 M elements) settles why:

```
divide         : 0.107 ms/Melem   exact
recip 1-Newton : 0.125 ms/Melem   1.17x slower, 135 ULP error
recip 2-Newton : 0.145 ms/Melem   1.35x slower,   2 ULP error
```

**On this core `vdivq_f32` beats `vrecpeq_f32` + Newton on BOTH speed and accuracy.** The
reciprocal-estimate trick is a legacy x86/older-ARM optimization that is counterproductive
here. Any `.recip()` in a NEON kernel is paying for the privilege of being wrong.

Workspace scan for the same pattern (`.recip()` / `rcp_approx` outside tests): zenfilters 16,
zenpipe 13, zenjxl-decoder 8, jxl-encoder 6, zensim 5, zenmetrics 3, zenanalyze 1. Only the
zenfilters ones are fixed here — the others live in bodies that also generate x86 tiers, where
the tradeoff is different (x86 divide latency is higher) and unmeasurable on this host.

## Reading the 1.00× rows — they are NOT a gap

`scale_plane` moves 1 M f32 in and out in 121 µs = ~69 GB/s, which is this host's measured
single-core memory-bandwidth ceiling. Same for `square_plane`, `subtract_planes`,
`unsharp_fuse` and `hue_rotate` (4.9–8.7 Gops/s, all at the wall). Both arms are saturated;
there is no arithmetic left for SIMD to remove, and a hand-written kernel cannot beat the
memory system.

On aarch64 NEON is *baseline*, so the "scalar" arm is the magetypes scalar tier **with LLVM
autovectorization** — not unvectorized code. A ratio near 1.00 therefore means both arms
compiled to equivalent work, which is the expected and correct outcome for a
bandwidth-bound elementwise pass.

## Where the remaining headroom actually is

The only kernels not at the bandwidth wall are the two transcendental-bound ones:

- `sigmoid_tone_map_plane` — 401 Mops/s, only 1.12× over scalar
- `power_contrast_plane` — 943 Mops/s, 2.82× over scalar

Both are dominated by `f32x8::pow_lowp_unchecked`, i.e. `exp(c · ln(x))` per element with a
loop-invariant exponent. The sigmoid kernel additionally does three reciprocals per element
(bias denominator, `x_safe.recip()`, and `(1 + powered).recip()`).

**That headroom is in magetypes, not here.** Making these faster means a faster SIMD `pow`
(or a reciprocal with a documented accuracy contract), which is a change to a foundational
crate's public API and would alter filter output. Not attempted in a NEON sweep; recorded so
the next session starts from the measurement rather than re-deriving it.

## Note on the bench

`archmage` is added as a **dev-dependency** with `testable_dispatch` so the baseline NEON
token can be disabled. Feature unification applies that to test/bench builds only, so
consumers are unaffected. Without it the bench prints "SIMD tier not toggleable" and skips
rather than silently reporting the SIMD path under both labels — which is how the first run
of this bench behaved before the dev-dep was added.
