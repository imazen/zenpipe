#!/usr/bin/env bash
# Isolated publication qualification; caller must first obtain the heavy slot.
# Keep the canonical full API profile and every existing browser assertion.
set -uo pipefail
results=${1:?absolute results directory required}
core=${2:?immutable core guide export required}
core_revision=${3:-${GITHUB_SHA:-}}
[[ "$core_revision" =~ ^[0-9a-f]{40}$ ]] || {
    echo 'pass the immutable core revision for the archived guide' >&2; exit 2;
}
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
step preview-protocol timeout 45s just checked-preview-source
step native timeout 600s just checked-publication-check
step api-generate timeout 1200s just api-doc
# Generation and check are distinct gates; neither substitutes a reduced profile.
step api-check timeout 600s just api-doc-check
# Use the guide's committed dependency lock, including its highlighter. The
# demo accepts this Playwright version through its existing ^1.59.1 range.
step core-tools timeout 120s npm ci --prefix "$core/docs" --ignore-scripts
mkdir -p "$results/browser-tools"
python3 - "$core/docs/package.json" "$results/browser-tools/package.json" <<'PYTOOLS'
from pathlib import Path
import json,re,sys
version = json.loads(Path(sys.argv[1]).read_text())['devDependencies']['playwright']
assert re.fullmatch(r'\d+\.\d+\.\d+', version), 'guide must pin a complete Playwright version'
Path(sys.argv[2]).write_text(json.dumps({'private': True, 'dependencies': {'playwright': version, '@playwright/test': version}}) + '\n')
PYTOOLS
step browser-tools timeout 120s npm install --prefix "$results/browser-tools" --ignore-scripts --no-save --package-lock=false
step browser-runtime timeout 180s env PLAYWRIGHT_SKIP_BROWSER_GC=1 "$core/docs/node_modules/.bin/playwright" install chromium
# Disposable exports only. No shared checkout or cached binary is changed.
step browser-links timeout 45s python3 - "$root" "$results/browser-tools/node_modules" <<'PYLINKS'
from pathlib import Path
import sys
link = Path(sys.argv[1]) / 'demo/node_modules'
assert not link.exists() and not link.is_symlink(), f'pre-existing tools at {link}'
link.symlink_to(Path(sys.argv[2]), target_is_directory=True)
assert not (Path(sys.argv[1]) / 'demo/pkg').exists(), 'demo profile is explicitly mock/browser-canvas, not a WASM qualification'
PYLINKS
step core-guide timeout 300s env GITHUB_SHA="$core_revision" bash -c '
    set -euo pipefail
    cd "$1"
    python3 - "$2" <<'"'"'PY'"'"'
import socket,sys
with socket.socket() as s:
    s.bind(("127.0.0.1",int(sys.argv[1])))
PY
    SITE_SCREENSHOTS="$3" just docs-site-browser-check "$2"
' _ "$core" 3547 "$results/core-browser"
demo_browser() {
    local demo=$1 output=$2; shift 2
    (
    set -euo pipefail
    cd "$demo"
    python3 - <<'PY'
import socket
with socket.socket() as s:
    # A completed prior browser leaves TIME_WAIT, not a live server.
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("127.0.0.1",3847))
PY
    exec ./node_modules/.bin/playwright test --config playwright.config.js --project chromium --workers 1 --output "$output" "$@"
    )
}
export -f demo_browser
step demo-preview timeout 90s bash -c 'demo_browser "$@"' _ "$root/demo" "$results/demo-preview" tests/encode-preview.spec.js
step demo-browser timeout 600s bash -c 'demo_browser "$@"' _ "$root/demo" "$results/demo-browser"
printf 'done status=%s\n' "$status" > "$results/stage.txt"
exit "$status"
