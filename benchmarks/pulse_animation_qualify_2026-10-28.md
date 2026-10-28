# Animated-transcode frame-format fix: qualification run 20d, 2026-10-28

**The run failed (rc 1).** Its two failures are listed first; neither involves
the fix. The fix's functional evidence passed, and an independent review
accepted it without a rerun. Nothing here measures speed or stop latency.

## Revisions

| Role | Commit | Content |
|---|---|---|
| base | `60127aaf` | io-drivers before this stack (PR #108) |
| deps | `66c6488f` | zenwebp `5a6cce27` (zenwebp PR 105 head, Pulse migration) |
| T | `75229b58` | the new tests, without the fix |
| F | `3c688575` | the fix: every frame converts to the encoder's format; `FrameSink` refuses a strip of another format or width |

zencodec core family `6214c93c`. Host r7900x (x86_64 Linux), rustc 1.99.1,
every cargo call `--locked`, warm target directory from run 20c. A per-run
driver (not committed, because it names local paths; sha256 `56b248e4…`)
sourced `scripts/qualify/helpers.sh` and ran the steps below:

```text
run-heavy --mem 3G --jobs 2 -- bash run.sh
run-heavy: done rc=1 75s | peak-RSS 0.40GiB | min-avail 4910MiB | peak-load 1.19
```

## The two failures

1. **`W-stop-latency` failed: no test ran.** All 10 tests in
   `zencodecs/tests/stop_latency.rs` are `#[ignore]` benchmarks, so the step had
   nothing to run. The step should have been recorded-only; that was a mistake
   in designing the run. **Stop latency is unmeasured.** The `#[ignore]`
   attributes are unchanged.
2. **`IC-clippy-lib` failed strict Clippy, raw rc 101.**
   `cargo clippy -p zenpipe --no-default-features --features std,imageflow-compat,nodes-jpeg,nodes-png --lib -- -D warnings`
   reports 65 lints. All 65 are in `src/imageflow_compat/*`, all are in the
   baseline recorded at `60127aaf`
   (`scripts/qualify/imageflow-compat-clippy-baseline-60127aaf.tsv`), and none is
   new. They predate this stack. This is a FAIL, not green.

## Failing first, at T

Each control must fail behaviorally before the fix; a compile failure would not
count. All five panicked at `src/animation.rs:365` slicing a short row (rc 101):

| Step | Test | Panic |
|---|---|---|
| P0a | `frame_sink_refuses_a_strip_in_another_format` | range end index 16, slice length 12 |
| P0b | `animated_webp_frames::imagejob_webp_to_webp_keeps_every_frame_and_duration` | range end index 64, slice length 48 |
| P0c | `frame_sink_refuses_strided_rgb_before_writing` | range end index 16, slice length 12 |
| P0d | `frame_sink_refuses_other_widths_before_writing` | range end index 16, slice length 12 |
| P0e | `animated_webp_frames::imagejob_rgb8_frames_convert_exactly_with_opaque_alpha` | range end index 64, slice length 48 |

## At F

These steps passed (rc 0):
- `check-core-family`, `check-decoder-pins`;
- `cargo metadata --locked` for all five lockfile roots;
- `cargo fmt --all --check`;
- `clippy-codec-subsets nodes-webp=webp`;
- strict Clippy on zencodecs W, all targets;
- `cargo test` on zencodecs W, all targets;
- strict Clippy on the zenpipe default lib;
- strict Clippy on `tests/animation.rs` under AN;
- `cargo test -p zenpipe --lib --tests` with default features.

Steps that ran each test alone (`<test> --exact --test-threads 1`), counting a
pass only on exit 0 with `test result: ok. 1 passed;`:

| Step | Binary | Passed | Failed | `#[ignore]`, not run |
|---|---|---|---|---|
| W-webp-capability | zencodecs `webp_capability` | 9 | 0 | 0 |
| W-stop-and-limits | zencodecs `stop_and_limits` | 25 | 0 | 4 |
| W-stop-latency | zencodecs `stop_latency` | 0 | 0 | 10 (step FAILED) |
| IC-imageflow-compat-info | `imageflow_compat_info` | 2 | 0 | 0 |
| IC-v2-zennode-parity | `v2_zennode_parity` | 59 | 0 | 0 |
| IC-v2-json-jobs | `v2_json_jobs` | 41 | 0 | 0 |
| V-animation | zenpipe `animation` | 20 | 0 | 0 |
| E-trace-migrated | zenpipe `trace_quality` | 7 | 0 | 0 |

Feature sets:

| Set | Package | Features |
|---|---|---|
| W | zencodecs | `std,cms,jpeg,jpeg-ultrahdr,webp,gif,gif-zenquant,png,png-zenquant,jxl-decode,bitmaps-bmp` |
| IC | zenpipe | `std,imageflow-compat,nodes-jpeg,nodes-png` |
| AN | zenpipe | `std,job,nodes-gif,nodes-png,nodes-webp` |
| TR | zenpipe | `std,job,nodes-gif,nodes-png,nodes-webp,nodes-jpeg,nodes-jxl,nodes-bmp` |

## Recorded only

- **`stop_and_limits -- --ignored`.** Four tests carry
  `#[ignore = "PNG codec adapter not yet wired into zencodecs"]`:
  `stop_decode_png`, `stop_encode_png`, `limits_decode_png_pixels` and
  `limits_encode_png_memory`. All four pass when run. The reason looks stale;
  the attributes stay until reviewed.
- **The still-only witness (#111).** It is a probe commit, never pushed; its
  source is in the issue. On every GIF/WebP pair, `zencodecs::transcode()`
  returns Ok with one output frame for a three-frame input. Nothing changes
  until an API proposal is reviewed.

## Independent review

A separate reviewer re-read the 20 per-test animation logs (each: 1 passed,
0 failed, 0 ignored), the 102 compat results and the five failing-first logs. It
accepted the functional evidence for the frame-format fix without a rerun. The
aggregate rc 1, the stop-latency step and the strict-Clippy FAIL stand as
recorded.

## Corrected counts from earlier runs (never published)

An earlier version of the runner counted `#[ignore]` tests as passes:

| Run | Binary | Reported | Actual |
|---|---|---|---|
| 17 | zencodecs `jxl_capability` | 11/11 | 8 passed, 3 ignored |
| 20, 20c | zencodecs `stop_and_limits` | 29/29 | 25 passed, 4 ignored |
| 20, 20c | zencodecs `stop_latency` | 10/10 | none ran, 10 ignored |

The counts published for runs 14 and 19 hold: no per-test log there reports an
ignored test, and their cargo summaries report 0 ignored.

## Limits

- No timing claim: stop latency is unmeasured, and so is throughput.
- No progress-forwarding or reporting pulse is tested.
- `PipeError::Cancelled` carries no stop reason. It is exact only for zenpipe's
  own deadline, whose reason is `Cancelled`. A caller's `TimedOut` and a codec's
  typed stop error are lost; this is an open API concern.
- For transparent lossless frames, the tests compare visible pixels and alpha,
  not RGB under alpha 0, which a lossless encoder may rewrite. That is
  visible-pixel preservation, not bit-exactness of every sample.
- Codecs not yet on the Pulse contract are excluded, not disabled:
  - zenavif `a7638f6e`;
  - heic `ac5c7cb9`;
  - zenextras `109a9ec3` (tiff, pdf, jp2, svg);
  - zenraw `50f9c694`.
  Also excluded: `nodes-all`, the two-engine RIAPI parity suite, the fuzz
  graphs and the API snapshot.
- Final qualification depends on native CI. zenwebp `5a6cce27`'s run
  40686426847 passed on every job.

## Files

- `pulse_animation_qualify_2026-10-28.steps.tsv`: each step's status, as the
  run wrote it.
- `pulse_animation_qualify_2026-10-28.tests.tsv`: each test's status;
  `ignored-not-run` marks an `#[ignore]` test.
- `scripts/qualify/`: the runner (sha256 as run):
  - `helpers.sh` `aa658f22…`
  - `ic_compare.py` `96984981…`
  - `runner-selftest.sh` `e8526309…`
  - baseline `f117c832…`

  Its 24 synthetic controls behaved in this run (step `R-selftest`) and again
  from the repo (`just qualify-selftest`).
