#!/usr/bin/env bash
# Swap one native repository's git pin everywhere this repo names it, then
# re-resolve every graph root's lock and report what moved. Builds nothing.
#
#   scripts/repin-native.sh <repo> <old-rev> <new-rev> <crate,crate,...>
#
# <repo> is the GitHub repository under imazen (zenjxl, zenavif, zenextras...);
# the crate list names that repository's own packages, which are expected to
# move. Every manifest with `imazen/<repo>", rev = "<old>"` is rewritten: the
# graph roots' [patch] tables and member dependency lines alike (zencodecs'
# zenavif-parse, for one). tools/jpeg-png-consumer pins this repo's published
# history and moves separately; wasm-size-shim is a path-only build.
#
# Each lock is refreshed with `cargo metadata` (minimal re-resolution, not
# `cargo update`). The script then prints every package outside the named crates
# and the zencodec core family whose identity (name, version, source, checksum)
# changed, and exits non-zero if any did: unrelated graph movement is reviewed,
# never accepted silently. It finishes with check-core-family.py (one family per
# graph) and check-decoder-pins.py (the zenavif and rav1d-safe pins agree).
# The rewritten files stay in place for review either way.
set -euo pipefail
REPO=${1:?repository under imazen}; OLD=${2:?old rev}; NEW=${3:?new rev}; CRATES=${4:?comma-separated crates}
for r in "$OLD" "$NEW"; do
  [[ $r =~ ^[0-9a-f]{40}$ ]] || { echo "need full 40-hex revisions" >&2; exit 2; }
done
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
WORK=target/repin-native/$REPO
mkdir -p "$WORK"
mapfile -t MANIFESTS < <(grep -rlF "imazen/$REPO\", rev = \"$OLD\"" --include=Cargo.toml . \
  | grep -v -e '^./target/' -e '^./tools/jpeg-png-consumer/' -e '^./wasm-size-shim/' | sort)
[ ${#MANIFESTS[@]} -gt 0 ] || { echo "no manifest pins imazen/$REPO at $OLD" >&2; exit 3; }
for m in "${MANIFESTS[@]}"; do
  sed -i "s|imazen/$REPO\", rev = \"$OLD\"|imazen/$REPO\", rev = \"$NEW\"|g" "$m"
  echo "$m: imazen/$REPO -> $NEW"
done
status=0
for lock in Cargo.lock fuzz/Cargo.lock demo/crate/Cargo.lock tools/metadata-services/Cargo.lock zencodecs/fuzz/Cargo.lock; do
  name=$(dirname "$lock" | tr / _)
  cp "$lock" "$WORK/$name.before"
  cargo metadata --format-version 1 --manifest-path "$(dirname "$lock")/Cargo.toml" \
    > /dev/null 2> "$WORK/$name.resolve.err" || { cat "$WORK/$name.resolve.err" >&2; exit 5; }
  python3 - "$WORK/$name.before" "$lock" "$CRATES" <<'EOF' || status=1
import sys, tomllib
allowed = set(sys.argv[3].split(",")) | {"zencodec", "zencodec-io", "zencodec-coroutine", "zenmeta"}
def load(path):
    data = tomllib.load(open(path, "rb"))
    packages = data.get("package", []) + data.get("patch", {}).get("unused", [])
    return {(p["name"], p["version"], p.get("source") or "", p.get("checksum") or "")
            for p in packages if p["name"] not in allowed}
before, after = load(sys.argv[1]), load(sys.argv[2])
for key in sorted(before ^ after):
    print(f"  {'-' if key in before else '+'} {key[0]} {key[1]} {key[2]}")
print(f"{sys.argv[2]}: {'nothing outside the named crates moved' if before == after else 'OTHER PACKAGES MOVED: review before keeping'}")
sys.exit(0 if before == after else 1)
EOF
done
python3 scripts/check-core-family.py || status=1
python3 scripts/check-decoder-pins.py || status=1
exit $status
