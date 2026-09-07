# ARM codec audit — 2026-09-06

Coverage is **18 of 18 format families measured**: JPEG, JPEG XL, PNG, WebP,
GIF, AVIF, RAW/DNG, HEIC, TIFF, JPEG 2000, PDF, SVG, PNM, farbfeld, BMP, QOI,
TGA and Radiance HDR. These are the explicit modes and fixtures in the reports,
not every format variant. The stale PDF-estimator assertion is corrected:
335 expanded adapter tests pass, zero fail; 79 existing ignored tests remain.
The three explicitly enabled AVIF corpus tests pass separately.

Pushed audit work spans 19 repositories, including the shared SIMD generator and the AV1 encoder/decoder backends. No release or crates.io publication was made. The measurements below are from an Apple M4 Pro, Rust 1.98 / LLVM 22, runtime dispatch without target-cpu=native. Heavy local work was serialized under nice -n19 with four build/Rayon/OMP threads. Individual reports retain commands, fixtures, confidence intervals and limits.

| Repository | Changes from this audit | Measurement / validation | Pushed audit commit |
|---|---|---|---|
| [archmage](https://github.com/imazen/archmage) | Seven-way add-green/codegen comparison | 16-lane magetypes and direct NEON emit identical assembly; 32 lanes lower to two NEON vectors | `2fcc66e8` |
| [zenwebp](https://github.com/imazen/zenwebp) | 32-lane add-green with 16-lane tail; fixed-row transform-distortion loads | Add-green 5.8–8.3x scalar over four measured sizes; transform-distortion bounds checks 32 to 14, with native/WASM/parity validation | `018753cf` |
| [zenjpeg](https://github.com/imazen/zenjpeg) | Inline generic DCT entry; deterministic fixtures | Generic DCT 65.4 to 56.3 ns in separate builds; scalar 194 ns. One 1MP Q85 4:2:0 encode 24.69/31.41 ms NEON/scalar; decode 11.62/15.78 ms | `2c7f67f9` |
| [zengif](https://github.com/imazen/zengif) | Native paired benchmarks, fixtures and exact parity checks; no production change | Existing palette unroll wins all ten measured size/content cases. Q80 encode 738.21/993.44 us at 64² and 20.77/22.67 ms at 512², NEON/scalar; identical encoded bytes | `8cd4c9ef` |
| [zenpng](https://github.com/imazen/zenpng) | Enable faster exact NEON Avg filter; correct full-scan benchmarks and labels | Avg 1920px 4.3 us vs scalar 6.43 us, old NEON 12.42 us. Full scans: 28 paired SIMD comparisons win over four sizes; exhaustive byte-pair and unaligned checks | `fdaa8de6` |
| [jxl-encoder](https://github.com/imazen/jxl-encoder) | Fixed-array DCT16 helper; truthful entropy/rectangular measurements; provision required v0.12 reference tools in CI | DCT16² 581.8 to 413.9 ns in separate builds, scalar 722.3/767.4 ns. Four rectangular forward/inverse paths beat scalar. New tool provisioning passed 1589 library + 489 integration tests locally | `6fcf14b6` |
| [zenjxl-decoder](https://github.com/imazen/zenjxl-decoder) | Native kernel/whole-decode profiling; correct per-fixture throughput groups | 12 vector-capable inverse shapes beat scalar, 2x2 is a scalar control. Green Queen modular ties scalar; fixed-error-array experiment rejected after no timing gain. No production decoder change | `17dc3030` |
| [zenjxl](https://github.com/imazen/zenjxl) | Wrapper integration audit and repeatable feature-enabled recipe | 25 default tests, 139 with zencodec/expert features; wrapper delegates to the two JXL backends. No independent wrapper SIMD change | `4a2c021b` |
| [zenavif](https://github.com/imazen/zenavif) | Native conversion/decode comparisons; integrate improved AOM encode/DSP and fixed film-grain decoder pins | Three whole-decode fixtures have exact cross-tier pixels. 45 selected encode integration tests pass with the AOM update, including 8/10/12-bit checks. Film-grain fixture now passes exact 1/2/4/8-thread pixels; 203 default tests pass. No whole-encode speedup claimed | `43fc5874` |
| [zenav1-aom](https://github.com/imazen/zenav1-aom) | Measure actual dispatch; direct smooth-V stores; fixed four-column smooth/Paeth paths | Full intra scalar/SIMD differential matrix passes. Four-column Paeth batch 65.48 us vs unchanged scalar 173.35 us; generated NEON arithmetic and one 64-bit row store | `a7b1ab13` |
| [zenrav1e](https://github.com/imazen/zenrav1e) | Rust/NEON-assembly benchmark and parity audit; no production change | Speed8/qindex100, 256² and 512²: Rust 155.56/601.71 ms vs NEON assembly 116.21/448.50 ms in separate builds; identical OBU bytes and reconstruction for the fixtures | `60594682` (master) |
| [rav1d-safe](https://github.com/imazen/rav1d-safe) | ARM tier benchmark; fix x86 film-grain row reservations exposed by CI | Five IVF fixtures / 52 frames retain exact cross-tier pixels. New x86 row borrowing removes multi-row reservations; native Zen5 and ARM validation plus CI passed | `e73811f5` |
| [zenav1-svt](https://github.com/imazen/zenav1-svt) | Repair oracle coverage; packed SATD4 row loads and separate NEON SSE4/SSE8 path | Full suite 2556/2556 and whole-encoder spotcheck 104/104 pass, zero skipped; production quantizer unchanged. SATD4 41.6/44.7 ns, SSE4 31.7/50.1 ns and SSE8 42.7/134 ns NEON/scalar; post-rebase 255 DSP + 104 encoder cells pass; production CI green | `73d4fe35` |
| [zenraw](https://github.com/imazen/zenraw) | Preserve signed zero and NaN payloads in vector normalization; exact paired tiers for both backends | Six whole-decode cells tie with exact tier bytes. Direct scalar normalization auto-vectorizes and ties NEON at five larger sizes; powf/NEF entropy/Malvar dominate sampled integer RAW decode. CI green | `50f9c694` |
| [heic](https://github.com/imazen/heic) | Fix signed overflow in NEON/AVX2/WASM residual addition; remove three 2052-byte coefficient return copies | Regression passes native ARM, x86 through Rosetta and WASM; 108 native library tests pass. Exact before/after RGBA on three fixtures. Native means 45.2→40.4, 24.5→22.9, 10.4→10.1 ms in separate builds; native/scalar ties remain | `c45113e4` |
| [zenextras](https://github.com/imazen/zenextras) | Four-family native decode/encode size grid, exact JP2 references, paired TIFF conversion benchmarks | TIFF slice writes win all 16 comparisons; float slice code auto-vectorizes. Native backend costs are not labeled SIMD/scalar gains | `4e56317b` |
| [zenbitmaps](https://github.com/imazen/zenbitmaps) | RGB8 BMP row conversion and fixed-output RGBA16 farbfeld; six-family paired benchmarks | 48 paired comparisons complete. At 4096², BMP 40.3 to 3.6 ms and farbfeld 31.0 to 7.1 ms in separate builds. Exact byte tests and strict all-target clippy pass; full bitmap CI green | `32376ef8` |
| [ravif](https://github.com/imazen/cavif-rs) | Validate assembly, pure-Rust and expert/stop modes; update native/WASM backend pin to audited zenrav1e | Updated backend passes strict pure-Rust clippy and all three integration configurations. Existing backend chroma/top-right fixes are not attributed to this audit | `a7e9fcc5` |
| [zenpipe / zencodecs](https://github.com/imazen/zenpipe) | Feature-expanded adapter audit; repair CI dependency setup and fuzz decoder pin; correct the PDF estimator regression | Full no-fail-fast run: 335 passed, zero failures, 79 existing ignored tests. JP2 adapter remains a compile-error feature | `76cfc8dc` |

These are bounded measurements, not speed guarantees across every image, quality, platform or optional feature. Separate-build timings are not paired before/after confidence intervals. Scalar fallback may auto-vectorize: AVIF YUV conversion and AOM SAD show vector instructions in both token states, explaining their ties. Tiny-kernel dispatch boundaries, row stores and bounds-check structure explain several measured regressions more directly than a missing ARM intrinsic.

## Detailed records

Most reports live at `<repo>/benchmarks/arm_audit_2026-09-06/README.md`; SVT uses `rust/benchmarks/arm_audit_2026-09-06/README.md`. WebP's record is [IMPROVEMENTS.md](https://github.com/imazen/zenwebp/blob/018753cf/benchmarks/arm_codegen_2026-09-05/IMPROVEMENTS.md). Links below resolve to local records:

- [JPEG](https://github.com/imazen/zenjpeg/blob/2c7f67f9/benchmarks/arm_audit_2026-09-06/README.md), [GIF](https://github.com/imazen/zengif/blob/8cd4c9ef/benchmarks/arm_audit_2026-09-06/README.md), [PNG](https://github.com/imazen/zenpng/blob/fdaa8de6/benchmarks/arm_audit_2026-09-06/README.md)
- [JXL encoder](https://github.com/imazen/jxl-encoder/blob/6fcf14b6/benchmarks/arm_audit_2026-09-06/README.md), [JXL decoder](https://github.com/imazen/zenjxl-decoder/blob/17dc3030/benchmarks/arm_audit_2026-09-06/README.md), [JXL wrapper](https://github.com/imazen/zenjxl/blob/4a2c021b/benchmarks/arm_audit_2026-09-06/README.md)
- [AVIF](https://github.com/imazen/zenavif/blob/e164f9e8/benchmarks/arm_audit_2026-09-06/README.md), [AOM](https://github.com/imazen/zenav1-aom/blob/a7b1ab13/benchmarks/arm_audit_2026-09-06/README.md), [SVT](https://github.com/imazen/zenav1-svt/blob/73d4fe35/rust/benchmarks/arm_audit_2026-09-06/README.md)

## Provenance

The hashes identify the pushed audit state, which can include preceding audit commits; they are not all single production patches. Concurrent JXL reference-version/resampling work, SVT V3 Hadamard/DC-fill work and an archmage macro fix are not attributed to this audit. A pre-existing rav1d-safe stack was inadvertently advanced to main earlier; that was reported immediately, preserved and verified, followed by the row-reservation fix when x86 CI exposed its regression. The audit does not claim authorship of that stack.

SVT's focused oracle repair now passes: broad scalar checks are retained,
C NEON's wide-input divergence is asserted explicitly, and 12 actual PD0
transform shapes × 16 residual patterns × 256 qindices match both C paths.
This is measured producer coverage, not a universal coefficient-bound proof.
See [oracle resolution](https://github.com/imazen/zenav1-svt/blob/73d4fe35/rust/benchmarks/arm_audit_2026-09-06/oracle-resolution.md).

CI update: AOM run [34033961158](https://github.com/imazen/zenav1-aom/actions/runs/34033961158) is fully green at `a7b1ab13`, including both ARM differential modes, x86, Windows ARM, macOS Intel and i686. PNG and JXL decoder main CI are also green; AVIF integration and fuzz CI are also green at `e164f9e8` ([CI](https://github.com/imazen/zenavif/actions/runs/34035488054)). JXL wrapper CI is fully green at `4a2c021b` ([CI](https://github.com/imazen/zenjxl/actions/runs/34034940970)). HEIC copy-removal CI [34068532578](https://github.com/imazen/heic/actions/runs/34068532578), fuzz and MediaCodec runtime checks all passed at `c45113e4`. RAW CI [34067865334](https://github.com/imazen/zenraw/actions/runs/34067865334) and bitmap CI [34067620276](https://github.com/imazen/zenbitmaps/actions/runs/34067620276) also passed. The JXL encoder repair passed stable manual run [34037117019](https://github.com/imazen/jxl-encoder/actions/runs/34037117019) at `4d06cb5c`, which contains `6fcf14b6`; the original push run was superseded by a concurrent benchmark-only commit.


## Authorized takeover, 22:04 UTC

The user authorized taking over the blocked checkouts, reviewing inherited source
changes before inclusion, and determining/reporting SVT's correct oracle target.
RAW/DNG, TIFF, JPEG 2000, PDF and SVG measurements are finished for the stated grids.

Inherited changes were preserved without deletion:

- zenraw `48e6dc0b`: changelog and registry-only apidoc/fuzz lockfile updates;
  no codec source edits. Audit starts from remote `20220cf3`.
- zenextras `5815563e`: three fuzz libfuzzer requirement expansions to 0.4.13
  and two nested lock updates, including the existing zensvg whereat dependency;
  no codec source edits. Audit starts from remote `e17bd6ca`.
- zenpipe `6a68ba94`: changelog-only dependency/API notes; kept separately
  because their historical claims are not newly verified. Audit starts from
  remote `12b468e3`.
- ravif `ef80b57f`: CI comments and changelog are already on remote main,
  not unpublished WIP. No source edits to rescue. The asserted GitHub fork
  acknowledgement explanation is not established by the quoted API evidence.
- SVT synced to `614c1e8b`. The original wide-input quantizer failure reproduces
  at qindex 0, Tx4x4: Rust/C-scalar EOB 16 versus C-NEON EOB 14.

JXL encoder manual CI [34037117019](https://github.com/imazen/jxl-encoder/actions/runs/34037117019)
completed successfully, including Windows ARM and macOS Intel.

## Continuing findings

- SVT small-block baseline: SATD4, SSE4 and SSE8 lose to scalar. A four-byte
  load change and separate SSE4/SSE8 paths pass 255 DSP tests; focused
  measurements favor the new NEON paths. Results are pushed as `3fa5c9a9`; production CI [34066431136](https://github.com/imazen/zenav1-svt/actions/runs/34066431136) is fully green.
- RAW normalization changes negative zero in vector chunks but preserves it
  in scalar tails. The new bitwise regression reproduces the mismatch;
  ordered compare/blend clamping passes 87 native library tests. ARM, x86 through Rosetta and WASM checks pass. Final direct scalar fallback auto-vectorizes and ties native at the five larger measured lengths; exact clamp semantics are preserved.
- zenextras' four families have native measurements at four output sizes.
  JPEG 2000 uses a CID22 512-pixel photograph, with 1024/4096 upsampled size
  controls; those are not native-resolution photographs. No independent
  wrapper SIMD tier exists. Lossless JP2 bytes match all four retained RGB references. TIFF slice writes win all 16 existing push-loop comparisons; assembly confirms CMYK float vectorization.
- zencodecs still declares `jp2-decode` as a compile-error stub despite the
  standalone zenjp2 implementation. This integration gap is distinct from
  the standalone decoder's measured ARM cost.

Zenextras audit CI [34066669454](https://github.com/imazen/zenextras/actions/runs/34066669454) is fully green at `4e56317b`. SVT oracle repair CI [34064068968](https://github.com/imazen/zenav1-svt/actions/runs/34064068968) is fully green; the subsequent small-kernel optimization run [34066431136](https://github.com/imazen/zenav1-svt/actions/runs/34066431136) is also fully green.

## Remaining ARM and integration gaps

- HEIC's IDCT16/32 NEON wrappers still call scalar routines. The sampled fixture
  primarily spends time in residual/CABAC work; the copy removal targets measured
  sites. It does not establish that those larger IDCTs are unimportant for every file.
- RAW integer-NEF whole decode is dominated by powf, entropy and Malvar work in
  the sampled profiles. No approximate math was substituted for exact pixels.
- JXL modular prediction/entropy dominates the profiled fixture. The tested layout
  experiment was preserved and rejected after measurements did not improve it.
- JP2 works in standalone zenjp2; zencodecs' `jp2-decode` is still an unwired stub.
- zencodecs currently uses zenavif 0.1.7/`11033c95`; audited zenavif is 0.2.0.
  This audit does not silently migrate the adapter across that version boundary.
  Current zenavif 0.2.0 now selects `e73811f5`: its previous `66f58fa6` pin
  reproduced [rav1d-safe#526](https://github.com/imazen/rav1d-safe/issues/526)
  on a real film-grain AVIF. The updated wrapper passes exact pixels at
  1/2/4/8 threads, three decodes per setting. The retained zencodecs 0.1.7
  dependency remains a separate, explicitly tested configuration.
- The PDF estimator test formerly expected `unknown()` despite the backend returning
  an estimate. The corrected test compares dispatch to the backend's direct
  result and is explicitly enabled in native CI.

## Shared code generation conclusion

The inspected 16-lane magetypes kernel emits the same NEON instructions as its
direct implementation; the audit did not establish a shared generator defect.
Several codec losses came from function boundaries, bounds checks, scalar row
assembly or growing output buffers. Scalar Rust also auto-vectorizes: a runtime
scalar label does not guarantee scalar machine instructions. The successful
changes preserve arithmetic and improve the actual generated loop or data movement.

Zenpipe's latest pre-audit CI [33844317596](https://github.com/imazen/zenpipe/actions/runs/33844317596)
was already failing: the API/i686 jobs cannot resolve the optional sibling-only
`zenavif_tuner` path in a bare checkout, and Format reports `src/avif_autotune.rs`
reflow. The setup repair below resolves those failing jobs; the full workflow at `59fe3a54` passes. Those failures were separate
from the native PDF assertion and from the standalone codec measurements.

Ravif backend-update CI [34068668993](https://github.com/imazen/cavif-rs/actions/runs/34068668993) is fully green at `a7e9fcc5`, including Windows ARM, macOS Intel, i686 and both x86 assembly jobs.

Zenpipe update: the three explicitly enabled AVIF corpus tests pass, zero
ignored, against the retained production decoder pin. The fuzz workflow
[34069397883](https://github.com/imazen/zenpipe/actions/runs/34069397883)
is fully green at `6e46dfed`. This commit repairs the missing sibling setup
and mismatched fuzz decoder pin; `84d1de35` repairs the formatting failure.
Main CI [34069770114](https://github.com/imazen/zenpipe/actions/runs/34069770114) is fully green at `59fe3a54`. The PDF assertion is corrected and passes locally.

At `6e46dfed`, the repaired public-API snapshot and i686 cross jobs both pass. Fuzz, Format, MSRV and decoder-pin gates pass too; all native jobs passed in the subsequent `59fe3a54` run.

SVT final benchmark-group CI [34068813843](https://github.com/imazen/zenav1-svt/actions/runs/34068813843) is fully green at `73d4fe35`, including the full x86 differential/conformance job.

Final integration updates: PDF regression commit `76cfc8dc` passes 335 local
adapter tests and the deliberate broken-dispatch mutation check. Its
[fuzz CI](https://github.com/imazen/zenpipe/actions/runs/34074302453) passed;
[native CI](https://github.com/imazen/zenpipe/actions/runs/34074303306) is still running.
AVIF decoder integration `43fc5874` passes 203 default tests (9 existing ignored),
strict library/example clippy and the explicit film-grain thread parity check;
[CI](https://github.com/imazen/zenavif/actions/runs/34075059287) has started.
These pending runs are not reported as green. All 19 earlier audit commits
were independently verified reachable on their remote default branches;
[verification record](remote-verification.tsv).
