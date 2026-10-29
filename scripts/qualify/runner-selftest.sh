#!/usr/bin/env bash
# runner-selftest.sh: synthetic controls for helpers.sh and ic_compare.py with
# a stub cargo and stub test binaries; builds nothing. Every failure mode must
# fail the step and the positive controls must pass. Exits 0 only when all
# controls behave.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$HOME/tmp"
WORK=$(mktemp -d "$HOME/tmp/runner-selftest.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/bin" "$WORK/exe"
cat > "$WORK/bin/cargo" <<'EOF'
#!/usr/bin/env bash
# Stub cargo: report STUB_ARTIFACTS as test executables, then exit STUB_RC.
for exe in $STUB_ARTIFACTS; do
  printf '{"reason":"compiler-artifact","executable":"%s","profile":{"test":true},"manifest_path":"%s/Cargo.toml"}\n' "$exe" "$(dirname "$exe")"
done
exit "${STUB_RC:-0}"
EOF
cat > "$WORK/exe/t-0000" <<'EOF'
#!/usr/bin/env bash
# Stub libtest binary: STUB_MODE is pass, listfail, ignlistfail, sleep, zero
# or fail; STUB_IGNORED names a test that --list --ignored reports.
if [ "$1" = "--list" ]; then
  if [ "${4:-}" = "--ignored" ]; then
    [ "$STUB_MODE" = ignlistfail ] && exit 2
    [ -n "${STUB_IGNORED:-}" ] && printf '%s: test\n' "$STUB_IGNORED"
    exit 0
  fi
  [ "$STUB_MODE" = listfail ] && exit 2
  printf 'alpha: test\nbeta: test\n'
  exit 0
fi
case "$STUB_MODE" in
  pass) echo "test result: ok. 1 passed; 0 failed"; exit 0 ;;
  sleep) sleep 5; exit 0 ;;
  zero) echo "test result: ok. 0 passed; 0 failed"; exit 0 ;;
  fail) echo "test result: FAILED. 0 passed; 1 failed"; exit 101 ;;
esac
exit 3
EOF
chmod +x "$WORK/bin/cargo" "$WORK/exe/t-0000"
cp "$WORK/exe/t-0000" "$WORK/exe/u-0000"
MIN_FREE_KIB=0
source "$HERE/helpers.sh"
bad=0

# tests_control NAME EXPECT FILTER: run tests() with the STUB_* environment.
tests_control() {
  local name=$1 expect=$2 filter=$3
  local run="$WORK/run-$name"
  mkdir -p "$run/logs"
  local status
  status=$(PATH="$WORK/bin:$PATH" RUN="$run" STATUS=0 bash -c '
    source "$1/helpers.sh"; STATUS=0; MIN_FREE_KIB=0
    tests "$2" "$3" -p stub > "$RUN/out.txt" 2>&1
    echo "$STATUS"' _ "$HERE" "$name" "$filter")
  local got=pass; [ "$status" = 0 ] || got=fail
  if [ "$got" = "$expect" ]; then echo "ok    tests/$name: $got ($(grep -h '^=== ' "$run/out.txt" | tail -1 | cut -c1-110))"
  else echo "WRONG tests/$name: expected $expect, got $got"; sed 's/^/      /' "$run/out.txt" | head -8; bad=1; fi
}

# ic_control NAME EXPECT RC LOG: classify LOG with exit status RC.
ic_control() {
  local name=$1 expect=$2 rc=$3 log=$4
  python3 "$HERE/ic_compare.py" "$log" "$HERE/imageflow-compat-clippy-baseline-60127aaf.tsv" "$rc" "$WORK/ic-$name.txt" > "$WORK/ic-$name.out" 2>&1
  local cmp=$?
  local got=pass; [ "$cmp" -eq 0 ] || got=fail
  if [ "$got" = "$expect" ]; then echo "ok    ic/$name: $got ($(head -1 "$WORK/ic-$name.out"))"
  else echo "WRONG ic/$name: expected $expect, got $got"; sed 's/^/      /' "$WORK/ic-$name.out" | head -6; bad=1; fi
}

T="$WORK/exe/t-0000"; U="$WORK/exe/u-0000"
printf 't\talpha\n' > "$WORK/filter-ok.tsv"
printf 't\tgamma\n' > "$WORK/filter-missing.tsv"
STUB_ARTIFACTS="$T" STUB_RC=0 STUB_MODE=pass tests_control pass pass all
STUB_ARTIFACTS="$T" STUB_RC=0 STUB_MODE=pass tests_control filter-ok pass "$WORK/filter-ok.tsv"
STUB_ARTIFACTS="$T" STUB_RC=101 STUB_MODE=pass tests_control build-partial-then-failure fail all
STUB_ARTIFACTS="$T $U" STUB_RC=0 STUB_MODE=pass tests_control two-executables fail all
STUB_ARTIFACTS="" STUB_RC=0 STUB_MODE=pass tests_control no-executable fail all
STUB_ARTIFACTS="$T" STUB_RC=0 STUB_MODE=listfail tests_control list-failure fail all
STUB_ARTIFACTS="$T" STUB_RC=0 STUB_MODE=sleep TEST_TIMEOUT=1 tests_control timeout fail all
STUB_ARTIFACTS="$T" STUB_RC=0 STUB_MODE=zero tests_control exit0-without-running fail all
STUB_ARTIFACTS="$T" STUB_RC=0 STUB_MODE=fail tests_control test-fails fail all
STUB_ARTIFACTS="$T" STUB_RC=0 STUB_MODE=pass tests_control filter-names-unlisted-test fail "$WORK/filter-missing.tsv"
STUB_ARTIFACTS="$T" STUB_RC=0 STUB_MODE=pass STUB_IGNORED=beta tests_control ignored-test-reported-not-passed pass all
STUB_ARTIFACTS="$T" STUB_RC=0 STUB_MODE=ignlistfail tests_control ignored-list-failure fail all
printf 't\tbeta\n' > "$WORK/filter-ignored.tsv"
STUB_ARTIFACTS="$T" STUB_RC=0 STUB_MODE=pass STUB_IGNORED=beta tests_control filter-names-ignored-test fail "$WORK/filter-ignored.tsv"

REAL="$HERE/fixtures/ic-20c.log"
: > "$WORK/empty.log"
ic_control real-baseline-only pass 101 "$REAL"
ic_control clean pass 0 "$WORK/empty.log"
sed 's/^error: could not compile `zenpipe` (lib) due to 65 previous errors$/error: something broke\n&/' "$REAL" > "$WORK/unlocated.log"
ic_control unlocated-error fail 101 "$WORK/unlocated.log"
printf 'error[E0425]: cannot find value `x` in this scope\n   --> src/lib.rs:1:1\n\nerror: could not compile `zenpipe` (lib) due to 1 previous error\n' > "$WORK/compiler.log"
ic_control compiler-error fail 101 "$WORK/compiler.log"
printf 'error: failed to load manifest for workspace member `/x`\n' > "$WORK/tool.log"
ic_control tool-failure-no-lints fail 101 "$WORK/tool.log"
ic_control timeout fail 124 "$WORK/empty.log"
ic_control crash-status fail 139 "$REAL"
sed 's/due to 65 previous errors/due to 66 previous errors/' "$REAL" > "$WORK/count.log"
ic_control summary-count-mismatch fail 101 "$WORK/count.log"
sed 's/^error: could not compile `zenpipe` (lib) due to 65 previous errors$/error: a new lint nobody has seen\n   --> src\/job.rs:1:1\n\nerror: could not compile `zenpipe` (lib) due to 66 previous errors/' "$REAL" > "$WORK/new.log"
ic_control new-lint fail 101 "$WORK/new.log"
grep -v '^error: could not compile' "$REAL" > "$WORK/nosummary.log"
ic_control missing-summary fail 101 "$WORK/nosummary.log"
printf 'error: could not compile `zenpipe` (lib) due to 0 previous errors\n' > "$WORK/zero.log"
ic_control failure-without-lints fail 101 "$WORK/zero.log"

if [ "$bad" -eq 0 ]; then echo "runner self-test: all controls behave"; else echo "runner self-test: FAILED"; fi
exit "$bad"
