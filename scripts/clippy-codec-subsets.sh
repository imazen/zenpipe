#!/usr/bin/env bash
# Strict Clippy on each single-codec feature subset that a zenpipe node feature
# builds.
#
# zenpipe's `nodes-<codec>` features each compile zencodecs with `zennode` plus
# exactly one codec feature, a combination no other check builds. A decode-only
# subset (`jxl-decode`, 2026-10-28) carried 13 warnings that only `-D warnings`
# on that exact combination shows. For each NODE=CODEC pair this lints
#   1. zencodecs with `zennode,CODEC`: lib, tests, examples and benches;
#   2. zenpipe with `std,NODE`: its node code, plus zencodecs' lib in exactly
#      the subset the node feature enables.
# Every pair runs even after a failure, and `--keep-going` lints every target
# of a pair past the first that fails; the exit status is non-zero if any
# failed, and the failures are listed at the end.
#
#   scripts/clippy-codec-subsets.sh [NODE=CODEC ...]
#
# With no arguments the pairs come from zenpipe's Cargo.toml: every `nodes-*`
# feature that enables exactly one `zencodecs/<feature>`.
set -uo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
if [ $# -eq 0 ]; then
  mapfile -t PAIRS < <(python3 - <<'EOF'
import tomllib
features = tomllib.load(open("Cargo.toml", "rb"))["features"]
for name, enables in sorted(features.items()):
    codecs = [e.split("/", 1)[1] for e in enables if e.startswith("zencodecs/")]
    if name.startswith("nodes-") and len(codecs) == 1:
        print(f"{name}={codecs[0]}")
EOF
  )
  [ ${#PAIRS[@]} -gt 0 ] || { echo "no nodes-* feature enables a zencodecs codec" >&2; exit 2; }
  set -- "${PAIRS[@]}"
fi
failed=()
for pair in "$@"; do
  node=${pair%%=*}
  codec=${pair#*=}
  if [ -z "$node" ] || [ -z "$codec" ] || [ "$node" = "$pair" ]; then
    echo "expected NODE=CODEC, got '$pair'" >&2
    exit 2
  fi
  echo "=== $node: zencodecs zennode,$codec"
  cargo clippy -p zencodecs --no-default-features --features "zennode,$codec" --all-targets --keep-going --locked -- -D warnings \
    || failed+=("zencodecs --features zennode,$codec")
  echo "=== $node: zenpipe std,$node"
  cargo clippy -p zenpipe --no-default-features --features "std,$node" --lib --keep-going --locked -- -D warnings \
    || failed+=("zenpipe --features std,$node")
done
if [ ${#failed[@]} -gt 0 ]; then
  printf 'FAILED: %s\n' "${failed[@]}"
  exit 1
fi
echo "OK -- $# node codec subsets pass strict Clippy"
