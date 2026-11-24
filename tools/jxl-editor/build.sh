#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
export CARGO_INCREMENTAL=0
if [[ "$(wasm-bindgen --version)" != "wasm-bindgen 0.2.129" ]]; then
    echo "Install wasm-bindgen-cli 0.2.129 to match the locked Rust dependency." >&2
    exit 1
fi
mkdir -p dist dist/models
cp web/* dist/

# Fetch pinned public-domain NPS sample photo.
sample_url=https://npgallery.nps.gov/GetAsset/fbf8deda-a0c6-422e-b8ff-4275dd14f289/proxymdres.jpg
sample_sha=5b289d44b16bbf92f90ac7ccc089253fe891a83ae68c415f382de81f6e2e5740
sha_matches() {
    [[ -f "$2" ]] && printf '%s  %s\n' "$1" "$2" | shasum -a 256 -c --status
}
if ! sha_matches "$sample_sha" dist/sample.jpg; then
    curl --fail --silent --show-error --location --retry 2 "$sample_url" -o dist/sample.jpg.partial
    sha_matches "$sample_sha" dist/sample.jpg.partial
    mv dist/sample.jpg.partial dist/sample.jpg
fi

# Fetch pinned ZenSR dejpeg_rt24g f16 weights from imazen/zensr (84 KB, zero binary blobs in git).
zensr_url=https://raw.githubusercontent.com/imazen/zensr/4a33019/models/adopted/dejpeg_rt24g/weights_f16.raw
zensr_sha=0de929e1bee7bde444d1d8ab46a8fd0d0ff4fef47daf561095d1661008798b80
if ! sha_matches "$zensr_sha" dist/models/dejpeg_rt24g.f16.raw; then
    curl --fail --silent --show-error --location --retry 2 "$zensr_url" -o dist/models/dejpeg_rt24g.f16.raw.partial
    sha_matches "$zensr_sha" dist/models/dejpeg_rt24g.f16.raw.partial
    mv dist/models/dejpeg_rt24g.f16.raw.partial dist/models/dejpeg_rt24g.f16.raw
fi

# Stage optional built-in ZenSR 2x/4x upscaling f16 checkpoints from local zensr checkout when present.
ZENSR_MODELS_DIR="${ZENSR_MODELS_DIR:-$HOME/work/zen/zensr/models/adopted}"
for pair in \
    "rtc_distill_2x:2b5173ac26b6c1aab53b4b4d34320096421ee2588fa5021f1c9393f1120277b4" \
    "nomosuni_span_2x:b1e56f5f20df3cbcd8c05416bd10802b3b86fcf67abb350ed92fa31319417481" \
    "nomosuni_span_4x:5006aa39926af84bd7e019e1fd8cbf5eb05fefb614badcee53a32f9a8a349a93"
do
    m="${pair%%:*}"
    s="${pair##*:}"
    dst="dist/models/${m}.f16.raw"
    src="${ZENSR_MODELS_DIR}/${m}/weights_f16.raw"
    if ! sha_matches "$s" "$dst" && [[ -f "$src" ]]; then
        cp "$src" "${dst}.partial"
        sha_matches "$s" "${dst}.partial"
        mv "${dst}.partial" "$dst"
    fi
done

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
    RUSTFLAGS="-C target-feature=$features" CARGO_TARGET_DIR="target/$mode" \
        cargo build --locked --release --target wasm32-unknown-unknown --manifest-path native-wasm/Cargo.toml
    RUSTFLAGS="-C target-feature=$features" CARGO_TARGET_DIR="target/$mode" \
        cargo build --locked --release --target wasm32-unknown-unknown --manifest-path zensr-wasm/Cargo.toml
    for module in codecs filters native zensr; do
        wasm-bindgen --target web --no-typescript --out-dir "dist/wasm/$mode" \
            "target/$mode/wasm32-unknown-unknown/release/jxl_editor_${module}.wasm"
    done
done
