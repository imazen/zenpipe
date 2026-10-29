#!/usr/bin/env bash
# Source exports only. A fresh serialized heavy grant is required before launch.
set -uo pipefail
results=${1:?absolute new results directory required}
core=${2:?immutable core guide export required}
expected=${3:?absolute graph expectation JSON required}
case "$results:$core:$expected" in /*:/*:/*) ;; *) exit 2 ;; esac
[[ ! -e .git && ! -e .jj && -f "$expected" ]] || exit 2
mkdir -p "$results"
[[ ! -e "$results/steps.tsv" ]] || { echo 'prior receipts exist' >&2; exit 2; }
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
# Keep all original locks, including the snapshot runner and WASM size harness.
step preserve-locks timeout 45s python3 - "$results" <<'PY'
from pathlib import Path
import hashlib,json,sys
out = Path(sys.argv[1]) / 'before'
out.mkdir()
hashes = {}
for name in ['Cargo.lock','fuzz/Cargo.lock','demo/crate/Cargo.lock','tools/metadata-services/Cargo.lock','zencodecs/fuzz/Cargo.lock','apidoc/Cargo.lock','wasm-size-shim/Cargo.lock']:
    data = Path(name).read_bytes()
    dest = out / name
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_bytes(data)
    hashes[name] = hashlib.sha256(data).hexdigest()
(out / 'SHA256.json').write_text(json.dumps(hashes,indent=2)+'\n')
PY
[[ -f "$results/before/SHA256.json" ]] || exit 1
# Resolve before checking the new decoder expectations. No cargo update sweep:
# preserve existing compatible versions and retain every metadata failure.
for manifest in Cargo.toml fuzz/Cargo.toml demo/crate/Cargo.toml tools/metadata-services/Cargo.toml zencodecs/fuzz/Cargo.toml apidoc/Cargo.toml wasm-size-shim/Cargo.toml; do
    label=${manifest//\//-}
    step "resolve-$label" timeout 300s bash -c 'exec cargo metadata --format-version 1 --all-features --manifest-path "$1" > "$2"' _ "$manifest" "$results/metadata-$label.json"
done
step full-api-graph timeout 300s bash -c 'exec cargo metadata --format-version 1 --locked --features zencodecs/all,zencodecs/jp2-decode,zencodecs/metadata,zencodecs/checked-transcode-async > "$1"' _ "$results/metadata-full-api.json"
step graph-identities timeout 45s python3 scripts/qualify/publication-graph.py "$PWD" "$expected" "$results/before" "$results"
step existing-core-gate timeout 45s python3 scripts/check-core-family.py
step existing-decoder-gate timeout 45s python3 scripts/check-decoder-pins.py
# First compiler diagnostic: preserve the complete canonical API feature profile
# and raw compiler spans, before asking the snapshot wrapper to summarize it.
step full-profile-diagnostic timeout 900s just checked-publication-diagnostic "$results/full-profile"
# Preserve the entire canonical feature profile, including known unmigrated
# HEIC/raw controls. Extras are additional gates, not profile substitutions.
step full-native-check timeout 900s cargo check -p zencodecs --no-default-features --features all,jp2-decode,metadata,checked-transcode-async,tiff,pdf-decode,svg --locked
step avif-capability timeout 600s cargo test -p zencodecs --no-default-features --features std,cms,jpeg,png,avif-decode,avif-encode,jpeg-ultrahdr --test avif_capability --locked
step avif-orientation-witness timeout 600s cargo test -p zencodecs --no-default-features --features std,cms,jpeg,png,avif-decode,avif-encode,jpeg-ultrahdr --test avif_capability orientation_pixel_witness --locked
step avif-pixels timeout 600s cargo test -p zencodecs --no-default-features --features all,jp2-decode,metadata,checked-transcode-async --test regress avif --locked
step extras-consumers timeout 600s cargo test -p zencodecs --no-default-features --features std,cms,tiff,pdf-decode,svg,jp2-decode --test pdf --test svg_decode --test jp2_decode --locked
step full-native-clippy timeout 600s cargo clippy -p zencodecs --no-default-features --features all,jp2-decode,metadata,checked-transcode-async,tiff,pdf-decode,svg --lib --locked -- -D warnings
core_revision=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["core_source"].rsplit("#",1)[1])' "$expected")
step publication-api-browser timeout 3600s bash scripts/qualify/publication-final.sh "$results/final" "$core" "$core_revision"
# Record again after API generation (which invokes nested Cargo commands).
step final-graph-identities timeout 45s python3 scripts/qualify/publication-graph.py "$PWD" "$expected" "$results/before" "$results"
printf 'done status=%s\n' "$status" > "$results/stage.txt"
exit "$status"
