# Qualification runner

Step helpers for a serialized qualification run, and synthetic controls that
prove the helpers fail closed. These are the files run 20d used, byte for byte
(sha256 in `benchmarks/pulse_animation_qualify_2026-10-28.md`), except the
fixture, noted below.

| File | Purpose |
|---|---|
| `helpers.sh` | `step`, `tests`, `expect_fail`, `ic_step`, `at`, `stage`, `disk_ok`, sourced by a per-run driver |
| `ic_compare.py` | Classifies a strict-Clippy log against a known-lint baseline |
| `runner-selftest.sh` | 24 controls (13 for `tests`, 11 for `ic_compare.py`) with a stub cargo; `just qualify-selftest` |
| `imageflow-compat-clippy-baseline-60127aaf.tsv` | The 65 strict-Clippy lints `imageflow-compat` carried at `60127aaf` |
| `fixtures/ic-20c.log` | Run 20c's `imageflow-compat` strict-Clippy log: its diagnostic, location and summary lines, verbatim and in order |

## What fails a step

- `tests NAME FILTER ARGS…` builds one test binary and runs each test alone. It
  fails on a nonzero build status, an inventory other than exactly one test
  executable, a failed or empty `--list`, a failed `--list --ignored`, a filtered
  test the binary does not list or ignores, a test that exits nonzero or times
  out, and a test that exits 0 without reporting one passed test.
  `#[ignore]` tests are recorded as `ignored-not-run`, never as passes, and a
  step whose every test is ignored fails with "no test ran".
- `ic_step NAME BASELINE ARGS…` keeps Clippy's raw status and log. A log of
  known baseline lints only is still a strict-Clippy **FAIL**; anything else
  fails the run: an exit status other than 0 or 101, a diagnostic without a
  location, a compiler error code, a missing or miscounted
  `could not compile … due to N previous errors` summary, or a new lint.
- `expect_fail NAME TEST ARGS…` requires a behavioral failure. Its pattern
  (`range end index|must be refused`) is run 20d's; a compile failure, a
  timeout or a pass is UNEXPECTED.

The baseline names each lint by file, line and column, so it describes one
source state. Regenerate it, with its commit in the name, whenever
`src/imageflow_compat` changes; never to absorb a new lint.

The fixture keeps only the lines `ic_compare.py` reads, so it fits in the repo
(7,289 of 37,915 bytes). `ic_compare.py` classifies both files identically:
baseline-only, 65 lints, 0 new. The full log's sha256 was
`535de6012e9dd1aa1a89d096cf9280be87a402c607390ecee9fe5db76721862f`.
