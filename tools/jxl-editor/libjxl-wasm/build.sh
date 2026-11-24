#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
EDITOR_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
OUT_DIR="$EDITOR_DIR/dist/wasm/libjxl"
BUILD_DIR="${LIBJXL_WASM_BUILD_DIR:-$EDITOR_DIR/target/libjxl-wasm-build}"
SRC_DIR="$BUILD_DIR/src"
CACHE_DIR="$BUILD_DIR/tarballs"
EMSDK_IMAGE="${EMSDK_IMAGE:-emscripten/emsdk:3.1.74}"

# Pinned libjxl v0.12.0 (a7a9c78) and its exact decoder + lcms2 third_party revisions.
LIBJXL_SHA="a7a9c787341cf703dede03c2009fa460cae5e5df"
BROTLI_SHA="028fb5a23661f123017c060daa546b55cf4bde29"
HIGHWAY_SHA="457c891775a7397bdb0376bb1031e6e027af1c48"
LCMS_SHA="5176347635785e53ee5cee92328f76fda766ecc6"
SKCMS_SHA="96d9171c94b937a1b5f0293de7309ac16311b722"

mkdir -p "$OUT_DIR" "$BUILD_DIR" "$CACHE_DIR"

fetch_github_tarball() {
    local repo="$1"
    local sha="$2"
    local dest="$3"
    local marker="$dest/.fetched-$sha"
    if [[ -f "$marker" ]]; then
        return 0
    fi
    local tgz="$CACHE_DIR/${repo//\//_}_${sha}.tar.gz"
    if [[ ! -f "$tgz" ]]; then
        echo "Downloading $repo @ ${sha:0:10}..." >&2
        curl --fail --silent --show-error --location --retry 2 \
            "https://github.com/$repo/archive/$sha.tar.gz" -o "${tgz}.partial"
        mv "${tgz}.partial" "$tgz"
    fi
    rm -rf "$dest"
    mkdir -p "$dest"
    tar -xzf "$tgz" -C "$dest" --strip-components=1
    touch "$marker"
}

fetch_github_tarball "libjxl/libjxl" "$LIBJXL_SHA" "$SRC_DIR"
fetch_github_tarball "google/brotli" "$BROTLI_SHA" "$SRC_DIR/third_party/brotli"
fetch_github_tarball "google/highway" "$HIGHWAY_SHA" "$SRC_DIR/third_party/highway"
fetch_github_tarball "mm2/Little-CMS" "$LCMS_SHA" "$SRC_DIR/third_party/lcms"
touch "$SRC_DIR/third_party/lcms/.git"
fetch_github_tarball "google/skcms" "$SKCMS_SHA" "$SRC_DIR/third_party/skcms"

JOBS="$(nproc 2>/dev/null || echo 4)"
if (( JOBS > 8 )); then JOBS=$((JOBS - 4)); fi

docker run --rm \
    -u "$(id -u):$(id -g)" \
    -v "$SRC_DIR:/src/libjxl:ro" \
    -v "$SCRIPT_DIR:/src/wrapper:ro" \
    -v "$BUILD_DIR:/build" \
    -v "$OUT_DIR:/out" \
    -e HOME=/build \
    "$EMSDK_IMAGE" \
    bash -lc "
set -euo pipefail
rm -rf /build/cmake && mkdir -p /build/cmake
emcmake cmake -S /src/libjxl -B /build/cmake -G 'Unix Makefiles' \
    -DCMAKE_BUILD_TYPE=MinSizeRel \
    -DBUILD_SHARED_LIBS=OFF \
    -DBUILD_TESTING=OFF \
    -DJPEGXL_ENABLE_TOOLS=OFF \
    -DJPEGXL_ENABLE_DOXYGEN=OFF \
    -DJPEGXL_ENABLE_MANPAGES=OFF \
    -DJPEGXL_ENABLE_BENCHMARK=OFF \
    -DJPEGXL_ENABLE_EXAMPLES=OFF \
    -DJPEGXL_ENABLE_JNI=OFF \
    -DJPEGXL_ENABLE_SJPEG=OFF \
    -DJPEGXL_ENABLE_OPENEXR=OFF \
    -DJPEGXL_ENABLE_JPEGLI=OFF \
    -DJPEGXL_ENABLE_TRANSCODE_JPEG=OFF \
    -DJPEGXL_ENABLE_SKCMS=OFF \
    -DJPEGXL_FORCE_SYSTEM_LCMS2=OFF \
    -DJPEGXL_FORCE_SYSTEM_BROTLI=OFF \
    -DJPEGXL_FORCE_SYSTEM_HWY=OFF \
    -DJPEGXL_BUNDLE_LIBPNG=OFF \
    -DCMAKE_C_FLAGS='-Os -fno-exceptions' \
    -DCMAKE_CXX_FLAGS='-Os -fno-exceptions'

emmake make -C /build/cmake -j ${JOBS} jxl_dec jxl_cms lcms2 brotlidec brotlicommon hwy

em++ -Os -fno-exceptions -std=c++17 \
    -I/src/libjxl/lib/include \
    -I/build/cmake/lib/include \
    /src/wrapper/libjxl_dec_wrapper.cpp \
    /build/cmake/lib/libjxl_dec.a \
    /build/cmake/lib/libjxl_cms.a \
    /build/cmake/third_party/liblcms2.a \
    /build/cmake/third_party/brotli/libbrotlidec.a \
    /build/cmake/third_party/brotli/libbrotlicommon.a \
    /build/cmake/third_party/highway/libhwy.a \
    -sMODULARIZE=1 \
    -sEXPORT_ES6=1 \
    -sDYNAMIC_EXECUTION=0 \
    -sENVIRONMENT=web,worker \
    -sALLOW_MEMORY_GROWTH=1 \
    -sINITIAL_MEMORY=16777216 \
    -sMAXIMUM_MEMORY=536870912 \
    -sFILESYSTEM=0 \
    -sEXPORTED_FUNCTIONS='[\"_malloc\",\"_free\",\"_libjxl_version_str\",\"_libjxl_decode_rgba\",\"_libjxl_result_json\",\"_libjxl_result_pixels\",\"_libjxl_result_pixels_len\",\"_libjxl_free_result\"]' \
    -sEXPORTED_RUNTIME_METHODS='[\"UTF8ToString\",\"HEAPU8\"]' \
    -o /out/libjxl_dec.mjs

ls -lh /out/libjxl_dec.mjs /out/libjxl_dec.wasm
"
