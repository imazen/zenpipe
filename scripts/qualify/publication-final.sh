#!/usr/bin/env bash
# Isolated publication qualification; caller must first obtain the heavy slot.
# Keep the canonical full API profile and every existing browser assertion.
set -uo pipefail
results=${1:?absolute results directory required}
core=${2:?immutable core guide export required}
case "$results:$core" in /*:/*) ;; *) echo 'absolute paths required' >&2; exit 2 ;; esac
root=$(pwd)
[[ ! -e .git && ! -e .jj ]] || { echo 'run in an immutable source export' >&2; exit 2; }
[[ ! -e "$core/.git" && ! -e "$core/.jj" && -f "$core/docs/site/browser.test.mjs" ]] || {
    echo 'core guide must be an immutable source export' >&2; exit 2;
}
mkdir -p "$results"
[[ ! -e "$results/steps.tsv" ]] || { echo 'refusing to overwrite prior receipts' >&2; exit 2; }
status=0
step() {
    local label=$1; shift
    printf '%s\t' "$label" >> "$results/commands.tsv"
    printf '%q ' "$@" >> "$results/commands.tsv"
    printf '\n' >> "$results/commands.tsv"
    date -u '+%Y-%m-%dT%H:%M:%SZ' > "$results/last-check.txt"
    printf '%s\n' "$label" > "$results/stage.txt"
    "$@" > "$results/$label.log" 2>&1
    local rc=$?
    printf '%s\t%s\t%s\n' "$label" "$rc" "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" >> "$results/steps.tsv"
    printf '%s rc=%s\n' "$label" "$rc"
    [[ $rc -eq 0 ]] || status=1
}
step source timeout 45s just checked-publication-source
step native timeout 600s just checked-publication-check
step api-generate timeout 1200s just api-doc
# Generation and check are distinct gates; neither substitutes a reduced profile.
step api-check timeout 600s just api-doc-check
mkdir -p "$results/browser-tools"
printf '%s\n' '{"private":true,"dependencies":{"playwright":"1.59.1","@playwright/test":"1.59.1"}}' > "$results/browser-tools/package.json"
step browser-tools timeout 120s npm install --prefix "$results/browser-tools" --ignore-scripts --no-save --package-lock=false
# Disposable exports only. No shared checkout or cached binary is changed.
step browser-links timeout 45s python3 - "$root" "$core" "$results/browser-tools/node_modules" <<'PY'
from pathlib import Path
import sys
for root in [Path(sys.argv[1]) / 'demo', Path(sys.argv[2])]:
    link = root / 'node_modules'
    assert not link.exists() and not link.is_symlink(), f'pre-existing tools at {link}'
    link.symlink_to(Path(sys.argv[3]), target_is_directory=True)
assert not (Path(sys.argv[1]) / 'demo/pkg').exists(), 'demo profile is explicitly mock/browser-canvas, not a WASM qualification'
PY
step core-guide timeout 300s bash -c '
    set -euo pipefail
    cd "$1"
    python3 - "$2" <<'"'"'PY'"'"'
import socket,sys
with socket.socket() as s:
    s.bind(("127.0.0.1",int(sys.argv[1])))
PY
    SITE_SCREENSHOTS="$3" just docs-site-browser-check "$2"
' _ "$core" 3547 "$results/core-browser"
step demo-browser timeout 600s bash -c '
    set -euo pipefail
    cd "$1"
    python3 - <<'"'"'PY'"'"'
import socket
with socket.socket() as s:
    s.bind(("127.0.0.1",3847))
PY
    exec ./node_modules/.bin/playwright test --config playwright.config.js --project chromium --workers 1 --output "$2"
' _ "$root/demo" "$results/demo-browser"
printf 'done status=%s\n' "$status" > "$results/stage.txt"
exit "$status"
