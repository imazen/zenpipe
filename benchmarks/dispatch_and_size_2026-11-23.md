# zenpipe Stage-1 Dispatch & Tail Vectorization (`2026-11-23`)

Baseline: `8a3d78be` (`main`, `chore(deps): upgrade zenfilters and workspace locks to archmage 0.9.33`).
Scope: Stage-1 internal size, dispatch, and tail cleanup inside `zenfilters` and `zencodecs` (zero public API or default-feature changes; external `[LEAF]` dependency locks are updated separately in Stage 2).

## Changes

1. **`zenfilters/src/simd/x86.rs` — Cold out-of-line scalar tails:**
   - Outlined the non-8-aligned remainder loops in `hue_rotate_impl_v3`, `vibrance_impl_v3`, and `fused_adjust_impl_v3` into `#[cold] #[inline(never)]` scalar helpers (`hue_rotate_tail_scalar`, `vibrance_tail_scalar`, `fused_adjust_ab_tail_scalar`).
   - Prevents LLVM from vectorizing small scalar remainder loops inside AVX2 `#[arcane]` bodies using masked `vmaskmovps` / `vmaskmovpd` loads/stores.
2. **`zencodecs/src/dispatch.rs` — Non-generic pipeline tails:**
   - Extracted non-generic `#[inline(never)]` `run_one_shot_pipeline` (`OneShotEncodeTail<'a, '_>` callback), `plan_streaming_encode`, and `finish_streaming_encoder` from `build_from_config<C, F>` and `build_streaming_from_config<C, F>`.
   - Preserves identical descriptor negotiation, tone-mapping descriptor handoff (`codec_supported` in `ToneMappingEncoder`), color fitting, and metadata policy behavior.
3. **`benches/dispatch_and_filters.rs` (new benchmark):**
   - Exercises the odd-width scalar-tail code paths (`517x387`, remainder 7) across `hue_rotate`, `vibrance`, and `fused_adjust`, plus one-shot and streaming `zencodecs::EncodeRequest` paths (`128x96`) across JPEG, PNG, WebP, and BMP.

## Size, Symbol & Disassembly Attribution (x86_64)

### `dispatch_and_filters` (`RUSTFLAGS="-C strip=none"`, local i265)

| Metric | BEFORE (`8a3d78be`) | AFTER | Delta |
|---|---:|---:|---:|
| `.text` (`size`) | `6,466,918 B` | `6,401,054 B` | **−65,864 B (−1.02%)** |
| `__simd_inner_*` symbols | 0 | 0 | 0 |
| `vmaskmov` total | 14 | 2 | **−12** |
| — `zenfilters::simd::x86` | 12 (`vibrance` 4, `hue_rotate` 4, `fused_adjust` 4) | 0 | **−12** |
| — `zenpixels_convert` (external pin) | 2 | 2 | 0 |
| `build_streaming_from_config<C, F>` (6 codecs) | `49,549 B` | `22,555 B` (+ `5,497 B` shared helpers) | **−21,497 B** |
| `build_from_config` `call_once` shims (6 codecs) | `86,251 B` | `973 B` (+ `11,023 B` `run_one_shot_pipeline` + `29,886 B` closures) | **−44,369 B** |

### `zenpipe-cmd` (`--release -p zenpipe-cmd --bin zenpipe --features zenpipe/nodes-avif`, `r5600g`)

| Metric | BEFORE | AFTER | Delta |
|---|---:|---:|---:|
| Binary file size | `25,651,976 B` | `25,533,624 B` | **−118,352 B (−0.46%)** |
| `.text` (`size`) | `21,090,869 B` | `20,977,809 B` | **−113,060 B (−0.54%)** |
| `objdump -d` `vmaskmov` count | 30 | 30 | 0 (upstream deps locked in Stage 2) |

## Benchmarks (`r5600g`, Ryzen 5 5600G, `taskset -c 4`, `--rounds=30`, no `target-cpu=native`)

Host pinned to core 4 (`cpu_effective_set=[0..11]`, `cpu_thread_effective=[4]`, `cpu_policy=auto`, `--format=llm`). Note: `kernel_tiers` and `dispatch_and_filters` BEFORE ran against unmodified `8a3d78be` + benchmark files at `/home/lilith/tmp/bench-lanes-2026-11-23/zenpipe-before`.

### `dispatch_and_filters` (directly exercises changed tail & dispatch code)

| Group / Arm | BEFORE (`mean ±mad`) | AFTER (`mean ±mad`) | BEFORE throughput | AFTER throughput |
|---|---:|---:|---:|---:|
| `hue_rotate_tail7` (517×387) | `67.2 ±2.9 µs` `[55.7–83.8]` | `73.9 ±2.2 µs` `[56.1–96.9]` | `2.98 Gpx/s` | `2.71 Gpx/s` (overlap) |
| `vibrance_tail7` (517×387) | `246.7 ±5.2 µs` | `245.6 ±5.6 µs` | `811 Mpx/s` | `815 Mpx/s` |
| `fused_adjust_tail7` (517×387) | `618.5 ±14.1 µs` | `622.9 ±12.0 µs` | `323 Mpx/s` | `321 Mpx/s` |
| `oneshot_jpeg` (128×96) | `934.6 ±55.5 µs` | `741.3 ±35.3 µs` | `13.1 Mpx/s` | `16.6 Mpx/s` |
| `oneshot_png` (128×96) | `5847.2 ±266.0 µs` | `5695.8 ±162.4 µs` | `2.10 Mpx/s` | `2.16 Mpx/s` |
| `oneshot_webp` (128×96) | `1185.2 ±57.4 µs` | `1217.6 ±67.3 µs` | `10.4 Mpx/s` | `10.1 Mpx/s` |
| `oneshot_bmp` (128×96) | `27.3 ±14.9 µs` | `11.6 ±2.2 µs` | `450 Mpx/s` | `1.05 Gpx/s` |
| `stream_jpeg` (128×96) | `1012.4 ±29.3 µs` | `757.5 ±44.6 µs` | `12.1 Mpx/s` | `16.2 Mpx/s` |
| `stream_png` (128×96) | `5520.8 ±219.7 µs` | `5754.8 ±255.8 µs` | `2.23 Mpx/s` | `2.14 Mpx/s` |
| `stream_webp` (128×96) | `1319.6 ±30.2 µs` | `1237.9 ±69.8 µs` | `9.31 Mpx/s` | `9.93 Mpx/s` |
| `stream_bmp` (128×96) | `17.9 ±4.2 µs` | `10.8 ±0.7 µs` | `685 Mpx/s` | `1.14 Gpx/s` |

### `zenfilters` `kernel_tiers` (`v3(avx2)` arm)

| Kernel | BEFORE | AFTER | BEFORE `ops/s` | AFTER `ops/s` |
|---|---:|---:|---:|---:|
| `hue_rotate` (`v3`) | `193.8 ±23.9 µs` `[165.4–230.9]` | `211.4 ±15.2 µs` `[180.3–251.0]` | `5.41 G` | `4.96 G` (CI overlap) |
| `vibrance` (`v3`) | `1.1 ±0.0 ms` `[1.1–1.1]` | `1.2 ±0.0 ms` `[1.1–1.4]` | `980 M` | `888 M` (CI overlap) |
| `brilliance_apply` (`v3`) | `705.7 ±35.5 µs` | `684.8 ±36.7 µs` | `1.49 G` | `1.53 G` |
| `adaptive_sharpen_apply` (`v3`) | `861.3 ±20.6 µs` | `854.6 ±23.2 µs` | `1.22 G` | `1.23 G` |
| `power_contrast_plane` (`v3`) | `693.1 ±10.0 µs` | `611.5 ±19.1 µs` | `1.51 G` | `1.71 G` |

Verdict: Zero measurable throughput regression on modified filters (`vibrance_tail7`, `fused_adjust_tail7`, `hue_rotate`/`vibrance` CIs overlap) and lower binary/object size (`zenpipe` `.text -113,060 B`, `dispatch_and_filters` `.text -65,864 B`, `zenfilters` `vmaskmov 12 -> 0`). Logs saved in `/home/lilith/tmp/bench-lanes-2026-11-23/results/zenpipe_{before,after}.log`.

## Verification Gates

- `cargo fmt --check -p zenfilters -p zencodecs -p zenpipe`: PASS
- `cargo clippy -p zenfilters -p zencodecs -p zenpipe --all-targets -- -D warnings`: PASS
- `cargo check -p zenfilters --no-default-features` and `-p zencodecs --no-default-features`: PASS
- `just api-doc-check`: PASS
- `just test-standalone-wasm`: PASS
- `cargo test --release` across `zenfilters`, `zencodecs` (`--no-fail-fast`, including `tests/transcode_matrix.rs` `120/120` cells), and `zenpipe`: `1,611 passed, 1 failed, 58 ignored`. The single failure (`zencodecs` `tests/checked_orientation.rs`, `left: (4, 7, Identity)`, `right: (7, 4, Rotate90)`) also failed identically on unmodified baseline `8a3d78be`.
