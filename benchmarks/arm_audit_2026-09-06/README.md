# Native ARM adapter audit, 2026-09-06

Baseline `12b468e3`, Apple M4 Pro, Rust 1.98. The PDF test expectation and CI dependency
setup are corrected; production codec arithmetic is unchanged. Existing changelog WIP is retained separately
as jj snapshot `6a68ba94`.

`just arm-codec-integration-audit` enables the defaults plus std,cms,tiff,svg,
pdf-decode,heic-decode,raw-decode,bitmaps-hdr,bitmaps-qoi,bitmaps-tga. Heavy work
is serialized under nice -n19 and four build/Rayon/OMP/test threads.

The corrected no-fail-fast run passes **335 tests, zero failures**, with 79
existing ignored tests across 23 result summaries including doctests. The
baseline's sole failure was `pdf_custom_format_reaches_zenpdf_estimator`, which
required `ResourceEstimate::unknown()` after zenpdf acquired an estimator.
It now compares dispatch to the backend's direct estimate. Native CI explicitly
enables `std,pdf-decode` and runs the PDF library tests. No production estimate
was changed to accommodate the stale test.

Coverage includes feature-enabled SVG render/error checks, default-format
metadata, stop/limits, and encode/decode integration. Corpus/HDR and latency
cases listed as ignored are not counted as coverage. Existing JXL/raw test
imports produce feature-dependent warnings; no new source warnings are added.

The standalone JP2 decoder was measured in zenextras, but zencodecs still has
an explicit compile-error `jp2-decode` stub. This audit does not implement that
public feature. The AVIF adapter retains zenavif 0.1.7 / `11033c95`, rather than
silently moving to audited zenavif 0.2.0. The historical film-grain pin blocker
[rav1d-safe#526](https://github.com/imazen/rav1d-safe/issues/526) is now closed;
a closure alone does not validate a dependency/API migration.

See [retained logs](logs.pointer.md) and [cross-repository report](CROSS_REPO.md).

Strict feature-expanded library clippy and decoder-pin self-tests/check pass.
The latest pre-audit remote CI fails in bare-checkout API/i686 jobs because the
optional `zenavif_tuner` dependency requires a sibling checkout, and in root
formatting for two reflow-only hunks. These are not failures introduced by the
audit recipe. The exact CI log is retained in the pointer file.

## CI dependency setup repair

The API, i686 and fuzz-regression jobs now use the existing zen-workspace setup
action to provision the optional tuner dependency closure. The i686 container
also receives the sibling directory through Cross's documented
[`CROSS_CONTAINER_OPTS`](https://github.com/cross-rs/cross/blob/main/docs/environment_variables.md)
mount option. Test commands and expectations are unchanged.

The fuzz-target gate retains its committed manifests rather than rewriting them:
its root workspace now clones the missing tuner siblings, and zencodecs/fuzz
uses the same zenavif/parser 0.1.7 git pin as the production adapter. Cargo update
changes only that source, its rav1d transitive pin, and removes a duplicate parser
instance. The prior sibling 0.2.0 patch could not satisfy the 0.1.7 requirement.
Root formatting was repaired separately in `84d1de35`. Full CI at `59fe3a54`
[34069770114](https://github.com/imazen/zenpipe/actions/runs/34069770114) passes,
including Windows ARM, macOS Intel, i686, API snapshots, format and clippy.

## Explicit AVIF corpus invocation

With the same feature set and pinned production decoder, the caller-selected
`--test corpus -- --ignored avif` run passes all three tests, zero ignored:
valid decode, invalid-input no-panic, and AVIF round trip. Test time is 826.45 s
in the unoptimized test profile, not a release performance measurement. The
existing round-trip test attempts at most five candidate source files and may
skip failed source decodes, so this result does not claim five successful files
or exhaustive corpus coverage. The repeatable recipe adds `--show-output` for
future per-test summaries.

Fuzz CI [34069397883](https://github.com/imazen/zenpipe/actions/runs/34069397883)
is fully green after setup/pin repair `6e46dfed`: both fuzz workspaces compile
and the regression seeds pass. The PDF assertion correction is additionally
validated locally in the full feature suite and by strict library clippy.

The PDF tests also moved out of the WebP-gated test module: `pdf-decode` alone
now includes them. A local mutation replacing the production PDF estimate call
with `unknown()` fails the corrected regression; restoring the real dispatch
passes the exact `std,pdf-decode --lib pdf_` CI command. This checks that the
gate both runs and detects the original routing regression.
