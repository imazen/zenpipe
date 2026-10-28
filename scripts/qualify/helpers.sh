# helpers.sh: the run's step helpers, sourced by run.sh and by
# runner-selftest.sh (which drives them against stub cargo and test binaries).
# Callers set RUN, STATUS, MIN_FREE_KIB and, optionally, TEST_TIMEOUT and
# BUILD_TIMEOUT.
TEST_TIMEOUT=${TEST_TIMEOUT:-600}
BUILD_TIMEOUT=${BUILD_TIMEOUT:-3600}
HELPERS_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

stage() { echo "$1 $(date -u +%T)" > "$RUN/stage.txt"; }

disk_ok() {
  local avail
  avail=$(df --output=avail -k /home | tail -1 | tr -d ' ')
  if [ "$avail" -lt "$MIN_FREE_KIB" ]; then
    echo "=== $1 NOT RUN: ${avail} KiB free < ${MIN_FREE_KIB} KiB floor; stopping the run" >&2
    printf '%s\tstopped-disk\t%s KiB\t%s\n' "$1" "$avail" "$(date -u +%T)" >> "$RUN/steps.tsv"
    exit 3
  fi
  echo "$avail"
}

# step NAME TIMEOUT CMD...: full output in logs/NAME.log, summary in run.log.
step() {
  local name=$1 limit=$2; shift 2
  local avail; avail=$(disk_ok "$name") || exit 3
  stage "$name"
  echo "=== $name: $* | start $(date -u +%T) | free ${avail} KiB"
  timeout "$limit" "$@" > "$RUN/logs/$name.log" 2>&1
  local rc=$?
  grep -E '^(error|warning)(\[|:)|^test result:|^FAILED|^OK --|^=== |panicked' "$RUN/logs/$name.log" | head -60
  echo "=== $name rc=$rc | free $(df --output=avail -k /home | tail -1 | tr -d ' ') KiB | end $(date -u +%T)"
  printf '%s\t%s\t%s\n' "$name" "$rc" "$(date -u +%T)" >> "$RUN/steps.tsv"
  if [ "$rc" -ne 0 ]; then STATUS=1; fi
}

# tests NAME FILTER CARGO_TEST_ARGS...: build the one test binary the arguments
# select, then run each listed test alone from its package directory. FILTER
# is "all" or a file of "<binary-stem>\t<test>" lines, every one of which must
# be listed and run. Tests marked #[ignore] are listed with --list --ignored,
# not run, and reported as ignored: never counted as passes. The step fails on:
# a nonzero build status (artifacts from a partial build are never used), an
# unreadable inventory or one with other than exactly one test executable, a
# failed or empty --list, a failed --list --ignored, a filtered test the binary
# does not list or ignores, a test that exits nonzero or times out, or a test
# that exits 0 without reporting exactly one passed test.
tests() {
  local name=$1 filter=$2; shift 2
  local avail; avail=$(disk_ok "$name") || exit 3
  stage "$name"
  echo "=== $name (each test alone): cargo test $* | start $(date -u +%T)"
  mkdir -p "$RUN/logs/$name"
  local verdict="" total=0 failed=0
  timeout "$BUILD_TIMEOUT" cargo test --color never "$@" --no-run --message-format=json \
    > "$RUN/logs/$name.build.json" 2> "$RUN/logs/$name.build.log"
  local build_rc=$?
  local pairs="" dir="" exe="" bin="" stem="" names="" ignored="" list_rc=0 ign_rc=0
  if [ "$build_rc" -ne 0 ]; then
    verdict="build failed rc=$build_rc"
  else
    pairs=$(jq -r 'select(.reason=="compiler-artifact" and .executable != null and .profile.test)
      | (.manifest_path | sub("/Cargo.toml$"; "")) + "\t" + .executable' "$RUN/logs/$name.build.json")
    local jq_rc=$?
    local count; count=$(printf '%s' "$pairs" | grep -c .)
    if [ "$jq_rc" -ne 0 ]; then
      verdict="inventory unreadable (jq rc=$jq_rc)"
    elif [ "$count" -ne 1 ]; then
      verdict="inventory has $count test executables, expected 1"
    fi
  fi
  if [ -z "$verdict" ]; then
    IFS=$'\t' read -r dir exe <<< "$pairs"
    bin=$(basename "$exe")
    stem=${bin%-*}
    local list
    list=$(cd "$dir" && "$exe" --list --format terse 2> "$RUN/logs/$name/list.err")
    list_rc=$?
    names=$(printf '%s\n' "$list" | sed -n 's/: test$//p')
    local ilist
    ilist=$(cd "$dir" && "$exe" --list --format terse --ignored 2> "$RUN/logs/$name/list-ignored.err")
    ign_rc=$?
    ignored=$(printf '%s\n' "$ilist" | sed -n 's/: test$//p')
    if [ "$list_rc" -ne 0 ]; then
      verdict="--list failed rc=$list_rc"
    elif [ "$ign_rc" -ne 0 ]; then
      verdict="--list --ignored failed rc=$ign_rc"
    elif [ -z "$names" ]; then
      verdict="--list named no tests"
    fi
  fi
  if [ -z "$verdict" ] && [ "$filter" != all ]; then
    local missing=0 fstem ftest
    while IFS=$'\t' read -r fstem ftest; do
      [ -n "$ftest" ] || continue
      if [ "$fstem" != "$stem" ] || ! printf '%s\n' "$names" | grep -qxF "$ftest" \
        || printf '%s\n' "$ignored" | grep -qxF "$ftest"; then
        echo "    MISSING (or ignored) $fstem $ftest"
        missing=$((missing + 1))
      fi
    done < "$filter"
    [ "$missing" -eq 0 ] || verdict="the filter names $missing test(s) the binary does not list or ignores"
  fi
  local nignored=0
  if [ -z "$verdict" ]; then
    local t rc log
    while read -r t; do
      [ -n "$t" ] || continue
      if [ "$filter" != all ] && ! grep -qxF "$stem"$'\t'"$t" "$filter"; then continue; fi
      if printf '%s\n' "$ignored" | grep -qxF "$t"; then
        nignored=$((nignored + 1))
        printf '%s\t%s\t%s\t%s\n' "$name" "$bin" "$t" "ignored-not-run" >> "$RUN/tests.tsv"
        echo "    $bin $t ignored (#[ignore]; not run)"
        continue
      fi
      total=$((total + 1))
      log="$RUN/logs/$name/$bin--$t.log"
      ( cd "$dir" && CARGO_MANIFEST_DIR="$dir" timeout "$TEST_TIMEOUT" "$exe" "$t" --exact --nocapture --test-threads 1 ) > "$log" 2>&1
      rc=$?
      if [ "$rc" -eq 0 ] && ! grep -q '^test result: ok\. 1 passed;' "$log"; then
        rc=98  # exited 0 without running exactly this one test
      fi
      [ "$rc" -ne 0 ] && failed=$((failed + 1))
      printf '%s\t%s\t%s\t%s\n' "$name" "$bin" "$t" "$rc" >> "$RUN/tests.tsv"
      echo "    $bin $t rc=$rc"
    done <<< "$names"
    [ "$total" -gt 0 ] || verdict="no test ran"
  fi
  if [ -n "$verdict" ]; then
    echo "=== $name: FAILED: $verdict | end $(date -u +%T)"
    printf '%s\tFAILED: %s\t%s\n' "$name" "$verdict" "$(date -u +%T)" >> "$RUN/steps.tsv"
    STATUS=1
    return 0
  fi
  echo "=== $name: $failed of $total tests failed; $nignored ignored (not run) | end $(date -u +%T)"
  printf '%s\t%s/%s-failed; %s-ignored-not-run\t%s\n' "$name" "$failed" "$total" "$nignored" "$(date -u +%T)" >> "$RUN/steps.tsv"
  if [ "$failed" -ne 0 ]; then STATUS=1; fi
}

# at REV LABEL: make REV the working copy and prove it.
at() {
  jj edit "$1" > /dev/null 2>&1 || { echo "jj edit $1 failed" >&2; exit 4; }
  local rev; rev=$(jj log -r @ --no-graph -T commit_id)
  echo "source ($2): @ $rev" | tee -a "$RUN/heads.txt"
  [ "$rev" = "$1" ] || { echo "@ is $rev, expected $1; not running" >&2; exit 4; }
}

# expect_fail NAME TEST CARGO_TEST_ARGS...: the test must fail behaviorally at T
# (the slicing panic or the "must be refused" assertion); anything else,
# including a compile failure, a timeout or a pass, is UNEXPECTED.
expect_fail() {
  local name=$1 test=$2; shift 2
  disk_ok "$name" > /dev/null || exit 3
  stage "$name"
  echo "=== $name (expected to FAIL) | start $(date -u +%T)"
  timeout 3600 cargo test --color never "$@" -- "$test" --exact --nocapture > "$RUN/logs/$name.log" 2>&1
  local rc=$?
  grep -E "panicked|range end index|must be refused|test result:|^error" "$RUN/logs/$name.log" | head -8
  local verdict
  if [ "$rc" -ne 0 ] && grep -qE "range end index|must be refused" "$RUN/logs/$name.log"; then verdict="failed-as-expected"; else verdict="UNEXPECTED"; STATUS=1; fi
  echo "=== $name rc=$rc $verdict | end $(date -u +%T)"
  printf '%s\t%s\t%s\t%s\n' "$name" "$rc" "$verdict" "$(date -u +%T)" >> "$RUN/steps.tsv"
}

# ic_step NAME BASELINE CLIPPY_ARGS...: strict Clippy whose raw status and log
# are kept. The run fails unless ic_compare.py classifies the log as clean or as
# known baseline lints only; a baseline-only result is still recorded as a
# strict-Clippy FAILURE, never as green.
ic_step() {
  local name=$1 baseline=$2; shift 2
  disk_ok "$name" > /dev/null || exit 3
  stage "$name"
  echo "=== $name: cargo clippy $* | start $(date -u +%T)"
  timeout 3600 cargo clippy --color never --keep-going "$@" > "$RUN/logs/$name.log" 2>&1
  local rc=$?
  python3 "$HELPERS_DIR/ic_compare.py" "$RUN/logs/$name.log" "$baseline" "$rc" "$RUN/$name.compare.txt"
  local cmp=$?
  local summary; summary=$(head -1 "$RUN/$name.compare.txt" 2>/dev/null)
  local verdict
  if [ "$cmp" -ne 0 ]; then
    verdict="FAIL (not baseline-only: see $name.compare.txt)"; STATUS=1
  elif [ "$rc" -eq 0 ]; then
    verdict="clean"
  else
    verdict="FAIL strict Clippy (raw rc $rc; known baseline lints only)"
  fi
  echo "=== $name rc=$rc $verdict | $summary"
  printf '%s\t%s\t%s; %s\t%s\n' "$name" "$rc" "$verdict" "$summary" "$(date -u +%T)" >> "$RUN/steps.tsv"
}
