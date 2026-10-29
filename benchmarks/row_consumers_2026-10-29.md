# Native row consumers and timeout causes — 2026-10-29

The consumer started at `b6e4f801`; the frozen pin-only export was
`938b5330052c074104397720b454e5476f508394`. The core remains `3f4fd14f`,
zenpixels `f23ac753`, and the native JXL encoder `11c6eb0c`.

All six manifest/lock roots now use WebP `2e58d1e5`, bitmaps `9d71b356`
and JXL `a3a93aaf`. A semantic comparison against the original TOML locks,
after substituting those three revision IDs back, is exactly equal in all six
roots. No other package, dependency edge, checksum or feature changed.
Cargo resolution and the core-family/decoder-pin checks passed.

## Native qualification

Run `just test-repaired-row-output`. It supplements, without replacing,
`just test-dispatched-output` and the explicit independent-decoder gate.

On Linux ARM64 with Rust 1.96.0, the five existing `streamed_bytes` tests pass
for JPEG, PNG, WebP, JXL, PNM, Farbfeld and TGA. Thirteen RGB/RGBA cases compare
one-shot bytes with undrained row encoding, writer drains, uneven prefix drains
and output-buffer transfer. Inputs have poisoned row padding. The same gate
retains empty-canvas, repeated/mismatched declaration, row-count and exact/tiny
output-budget assertions. Strict library and test Clippy passes.

This is same-native byte equivalence for those configurations, not an
independent decode oracle, all-format qualification, a performance benchmark,
or a workspace-memory bound. GIF, AVIF, TIFF, BMP, QOI and HDR were not enabled
in this focused run; the full dispatch recipe remains the broader gate.

## Typed timeout regression

`TranscodeSink` stringified native errors from `begin` and `push_rows`.
The failing-first JPEG control printed `begin: zenjpeg: operation timed out`
but could not recover its native error with `zencodec::find_cause`.
Both hooks now propagate the existing boxed cause directly, without new API.

The final regression checks JPEG and PNG declaration and completion after a
caller deadline expires, requiring typed `Stopped(TimedOut)`. The existing
Pulse target also passes: JPEG encode/decode exercised 5/54 stop points;
PNG encode/decode exercised 8/65, with no wrong result. The five row tests and
strict Clippy pass again after the change.

Two initial versions of the new fixture wrongly required a buffered codec to
observe its deadline during row delivery. Both failed before the fixture was
corrected to include native finalization. No pre-existing assertion changed.
This repair preserves error identity; it does not add checkpoints or promise
preemption, immediate output, or bounded cancellation latency.

## Run receipts and limitations

All heavy commands used a 3 GiB cap and two jobs. Resource receipts describe
validation processes, not codec memory requirements:

- Native row tests and strict Clippy: rc0, 438s, peak RSS 0.80 GiB,
  minimum available 13657 MiB, peak load 2.00.
- Original cause regression: expected rc101, 4s, peak RSS 0.21 GiB.
- Final cause + existing Pulse + row tests and strict Clippy: rc0, 4s,
  peak RSS 0.21 GiB, minimum available 14508 MiB, peak load 0.43.

Dependency fetching twice hit a disk reserve guard. No native test was counted
from those attempts. The first outer timeout left a nested metadata command;
its recorded timeout PID was terminated and exit verified. Later runs used
process-group cleanup. No benchmark or timing comparison is inferred.

An offline targeted fuzz-lock update deduplicated moxcms 0.9.0 into 0.8.1;
that unrelated change was not accepted. The original graph was retained with
only the three source-ID substitutions, then validated by Cargo with `--locked`.
The WASM-size graph's offline attempt lacked a cached source; online resolution
passed. The preserved fuzz graph's offline metadata attempt lacked font-types;
its subsequent online `--locked` validation passed. No dependencies were omitted.

Full logs are retained by their hashes below; no executables are archived.

| Log | SHA-256 |
|---|---|
| full.log | `199ccd2c93c995c4aebb39bfb71e8e8c1e97a8513381945b6ba5ba05ede9897b` |
| retry.log | `e0ecc673d99a80abc30cf0d7ee203d40713c884287f83e04c19c4ed39ba702f2` |
| root-test.log | `39610b244fe2b73d44bca8aa56d98132eeae25bd8762cc64d8eaf4dc41b231cd` |
| cause-baseline.log | `2e3dfd7d3b7934d4d9678f426b75d1b809ff1daf57820625986c40466cc3c729` |
| cause-fixed.log | `e496e2eb42d97ff464afbf6cae0438241c7ed45612c7761d00d6cb0463843b5c` |
| cause-fixed2.log | `5939818249cb5885b9f19582ac539b6e5a439ac6325498cd349298e82c19e475` |
| cause-fixed3.log | `d8a29824f28468654f972bcf39403b8336aa9cf4ca3d1a4ad9ca6e972e2a198b` |
| finish-graphs.log | `6488f120feb0e9ef14b5736bf38208f3c2baafb6d7f69b270afb8d59addffd8a` |
| offline-graphs.log | `7fdd0b7e6c3eda674c9d2f0be9df1b832ddc4b42f8cf231dcea2c335d8941579` |
| wasm-lock.log | `6975517ef8d5aeabe059d63d2373dc22a1d2552b70f6ce0fafaa7d25e45cecf6` |
| wasm-online.log | `60a22c5a905c1b7bcd9dbda2607c26f3d7124edb58a07bfd9926075597b54735` |
| fuzz-preserved.log | `a7f49ac74534e21c2a3df145e01129b5ba4fd2141eff722a2b6157877c22d4df` |
| fuzz-online.log | `2b44ede835866cdfc956181a33ce6addca363c6d71c58bc7eaa9cbf7eab42d77` |
