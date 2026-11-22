#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
export CARGO_INCREMENTAL=0
if [[ "$(wasm-bindgen --version)" != "wasm-bindgen 0.2.129" ]]; then
    echo "Install wasm-bindgen-cli 0.2.129 to match the locked Rust dependency." >&2
    exit 1
fi
mkdir -p dist
cp web/* dist/
# The sample is public-domain NPS photography. Pin the exact published JPEG.
sample_url=https://npgallery.nps.gov/GetAsset/fbf8deda-a0c6-422e-b8ff-4275dd14f289/proxymdres.jpg
sample_sha=5b289d44b16bbf92f90ac7ccc089253fe891a83ae68c415f382de81f6e2e5740
sample_matches() {
    [[ -f "$1" ]] && printf '%s  %s\n' "$sample_sha" "$1" | shasum -a 256 -c --status
}
if ! sample_matches dist/sample.jpg; then
    curl --fail --silent --show-error --location --retry 2 "$sample_url" -o dist/sample.jpg.partial
    sample_matches dist/sample.jpg.partial
    mv dist/sample.jpg.partial dist/sample.jpg
fi
if (( $# == 0 )); then set -- relaxed baseline; fi
for mode in "$@"; do
    case "$mode" in
        relaxed) features=+simd128,+relaxed-simd ;;
        baseline) features=-simd128,-relaxed-simd ;;
        *) echo "Unknown WebAssembly build mode: $mode" >&2; exit 1 ;;
    esac
    RUSTFLAGS="-C target-feature=$features" CARGO_TARGET_DIR="target/$mode" \
        cargo build --locked --release --target wasm32-unknown-unknown
    RUSTFLAGS="-C target-feature=$features" CARGO_TARGET_DIR="target/$mode" \
        cargo build --locked --release --target wasm32-unknown-unknown --manifest-path filters-wasm/Cargo.toml
    for module in codecs filters; do
        wasm-bindgen --target web --no-typescript --out-dir "dist/wasm/$mode" \
            "target/$mode/wasm32-unknown-unknown/release/jxl_editor_${module}.wasm"
    done
done
