#!/usr/bin/env bash
# seed_corpus.sh — Populate fuzz seed corpus from local + external sources.
#
# Usage:
#   ./seed_corpus.sh              # Full seed (local + external)
#   ./seed_corpus.sh --local-only # Skip external downloads
#
# External sources are fetched by default. Pass --local-only to skip.
# Re-running is safe; existing files are not re-downloaded.

set -eo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CORPUS_DIR="$SCRIPT_DIR/corpus/seed"
ZEN_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
EXTERNAL_CACHE="$SCRIPT_DIR/.external-cache"

LOCAL_ONLY=false
for arg in "$@"; do
    case "$arg" in
        --local-only) LOCAL_ONLY=true ;;
        *) echo "Unknown arg: $arg"; exit 1 ;;
    esac
done

mkdir -p "$CORPUS_DIR"

echo "=== Phase 1: Local seeds from sibling crates ==="

copy_seeds() {
    local src="$1" dst_subdir="$2"
    local dst="$CORPUS_DIR/$dst_subdir"
    if [ -d "$src" ]; then
        mkdir -p "$dst"
        local count
        count=$(find "$src" -maxdepth 1 -type f | wc -l)
        if [ "$count" -gt 0 ]; then
            cp -n "$src"/* "$dst/" 2>/dev/null || true
            echo "  $dst_subdir: copied from $src ($count files)"
        fi
    fi
}

copy_glob() {
    local pattern="$1" dst_subdir="$2"
    local dst="$CORPUS_DIR/$dst_subdir"
    mkdir -p "$dst"
    local count=0
    # Use a subshell with nullglob to handle missing files gracefully.
    while IFS= read -r -d '' f; do
        cp -n "$f" "$dst/" 2>/dev/null || true
        count=$((count + 1))
    done < <(find $(dirname "$pattern") -maxdepth 1 -name "$(basename "$pattern")" -type f -print0 2>/dev/null || true)
    [ "$count" -gt 0 ] && echo "  $dst_subdir: copied $count files from glob" || true
}

# JPEG seeds
copy_seeds "$ZEN_DIR/zenjpeg/zenjpeg/fuzz/corpus/seed" "jpeg"
copy_glob "$ZEN_DIR/zencodecs/tests/images/*.jpg" "jpeg"

# PNG seeds
copy_seeds "$ZEN_DIR/zenpng/fuzz/corpus/fuzz_decode" "png"

# GIF seeds
copy_seeds "$ZEN_DIR/zengif/fuzz/corpus/fuzz_decode" "gif"
copy_seeds "$ZEN_DIR/zengif/tests/corpus/image-rs/simple" "gif"

# WebP seeds — check for fuzz corpus or test fixtures
copy_seeds "$ZEN_DIR/zenwebp/fuzz/corpus/fuzz_decode" "webp"
copy_glob "$ZEN_DIR/zenwebp/tests/fixtures/*.webp" "webp"

# AVIF seeds
copy_seeds "$ZEN_DIR/zenavif/fuzz/corpus/fuzz_decode" "avif"

# JXL seeds
copy_glob "$ZEN_DIR/zenjxl-decoder/resources/test/*.jxl" "jxl"
# Also check worktree copies
copy_glob "$ZEN_DIR/zenjxl-decoder/.claude/worktrees/*/zenjxl-decoder/resources/test/*.jxl" "jxl"

# HEIC seeds
copy_seeds "$ZEN_DIR/heic/fuzz/corpus/fuzz_probe" "heic"
copy_glob "$ZEN_DIR/heic/tests/fixtures/*.heic" "heic"
copy_glob "$ZEN_DIR/heic/tests/fixtures/*.heif" "heic"

# BMP/PNM/Farbfeld seeds
copy_seeds "$ZEN_DIR/zenbitmaps/fuzz/corpus/fuzz_decode" "bitmaps"
copy_glob "$ZEN_DIR/zenbitmaps/tests/fixtures/*" "bitmaps"
copy_glob "$ZEN_DIR/zenbitmaps/tests/bmp-fixtures/*.bmp" "bitmaps"

# TIFF seeds
copy_glob "$ZEN_DIR/zentiff/tests/fixtures/*.tiff" "tiff"
copy_glob "$ZEN_DIR/zentiff/tests/fixtures/*.tif" "tiff"

# EXIF — create minimal test blobs
EXIF_DIR="$CORPUS_DIR/exif"
mkdir -p "$EXIF_DIR"
# Minimal TIFF header (little-endian, 0 entries)
printf 'II\x2a\x00\x08\x00\x00\x00\x00\x00' > "$EXIF_DIR/minimal_le.tiff"
# Minimal TIFF header (big-endian, 0 entries)
printf 'MM\x00\x2a\x00\x00\x00\x08\x00\x00' > "$EXIF_DIR/minimal_be.tiff"
# JPEG-style EXIF prefix + minimal TIFF
printf 'Exif\x00\x00II\x2a\x00\x08\x00\x00\x00\x00\x00' > "$EXIF_DIR/jpeg_exif_prefix.bin"
echo "  exif: created 3 minimal test blobs"

# Create mixed/ directory with samples from all formats
echo ""
echo "=== Creating mixed/ corpus (all formats) ==="
MIXED_DIR="$CORPUS_DIR/mixed"
mkdir -p "$MIXED_DIR"
for subdir in jpeg png gif webp avif jxl heic bitmaps tiff exif; do
    src="$CORPUS_DIR/$subdir"
    [ -d "$src" ] || continue
    # Take up to 5 files from each format for mixed corpus
    count=0
    for f in "$src"/*; do
        [ -f "$f" ] || continue
        cp -n "$f" "$MIXED_DIR/" 2>/dev/null || true
        count=$((count + 1))
        [ "$count" -ge 5 ] && break
    done
done
echo "  mixed: $(find "$MIXED_DIR" -type f | wc -l) files"

if [ "$LOCAL_ONLY" = true ]; then
    echo ""
    echo "=== Skipping external corpora (--local-only) ==="
    echo ""
    echo "Done. Total seed files: $(find "$CORPUS_DIR" -type f | wc -l)"
    exit 0
fi

echo ""
echo "=== Phase 2: External corpora ==="

mkdir -p "$EXTERNAL_CACHE"

fetch_github_corpus() {
    local repo="$1" subpath="$2" dst_subdir="$3" branch="${4:-master}"
    local cache_dir="$EXTERNAL_CACHE/$(echo "$repo" | tr '/' '_')"
    local dst="$CORPUS_DIR/$dst_subdir"

    if [ -d "$cache_dir" ]; then
        echo "  $dst_subdir: using cached $repo"
    else
        echo "  $dst_subdir: cloning $repo (sparse)..."
        git clone --depth 1 --filter=blob:limit=1m --sparse \
            "https://github.com/$repo.git" "$cache_dir" -b "$branch" 2>/dev/null || {
            echo "  WARNING: failed to clone $repo — skipping"
            return
        }
        (cd "$cache_dir" && git sparse-checkout set "$subpath" 2>/dev/null) || true
    fi

    mkdir -p "$dst"
    if [ -d "$cache_dir/$subpath" ]; then
        find "$cache_dir/$subpath" -maxdepth 2 -type f \
            \( -name "*.jpg" -o -name "*.jpeg" -o -name "*.png" -o -name "*.gif" \
               -o -name "*.webp" -o -name "*.avif" -o -name "*.jxl" -o -name "*.bmp" \
               -o -name "*.tiff" -o -name "*.tif" -o -name "*.heic" -o -name "*.heif" \
               -o -name "*.ppm" -o -name "*.pgm" -o -name "*.pam" -o -name "*.pnm" \
               -o -name "*.qoi" -o -name "*.tga" -o -name "*.hdr" -o -name "*.ff" \
               -o -size -100k \) \
            -exec cp -n {} "$dst/" \; 2>/dev/null
        local count
        count=$(find "$dst" -type f | wc -l)
        echo "  $dst_subdir: $count files total"
    fi
}

# dvyukov/go-fuzz-corpus — GIF, PNG, JPEG
fetch_github_corpus "dvyukov/go-fuzz-corpus" "gif/corpus" "gif" "master"
fetch_github_corpus "dvyukov/go-fuzz-corpus" "png/corpus" "png" "master"
fetch_github_corpus "dvyukov/go-fuzz-corpus" "jpeg/corpus" "jpeg" "master"

# libjpeg-turbo fuzz seeds
fetch_github_corpus "libjpeg-turbo/fuzz" "seed_corpus" "jpeg" "main"

# image-rs test images
fetch_github_corpus "image-rs/image" "tests/images" "mixed" "main"

echo ""
echo "=== Phase 3: Refresh mixed/ with external additions ==="
for subdir in jpeg png gif webp avif jxl heic bitmaps tiff; do
    src="$CORPUS_DIR/$subdir"
    [ -d "$src" ] || continue
    count=0
    for f in "$src"/*; do
        [ -f "$f" ] || continue
        cp -n "$f" "$MIXED_DIR/" 2>/dev/null || true
        count=$((count + 1))
        [ "$count" -ge 10 ] && break
    done
done
echo "  mixed: $(find "$MIXED_DIR" -type f | wc -l) files total"

echo ""
echo "=== Phase 4: OSS-Fuzz corpus backups ==="

OSS_FUZZ_CACHE="$EXTERNAL_CACHE/oss-fuzz"
mkdir -p "$OSS_FUZZ_CACHE"

fetch_ossfuzz_zip() {
    local project="$1" fuzzer="$2" dst_subdir="$3"
    local zip="$OSS_FUZZ_CACHE/${project}_${fuzzer}.zip"
    local dst="$CORPUS_DIR/$dst_subdir"
    local url="https://storage.googleapis.com/${project}-backup.clusterfuzz-external.appspot.com/corpus/libFuzzer/${project}_${fuzzer}/public.zip"

    if [ -f "$zip" ] && [ "$(stat -c%s "$zip" 2>/dev/null || stat -f%z "$zip" 2>/dev/null)" -gt 1000 ]; then
        echo "  $dst_subdir: using cached $project/$fuzzer"
    else
        echo "  $dst_subdir: downloading $project/$fuzzer..."
        curl -sL --retry 3 --retry-delay 5 -o "$zip" "$url" 2>/dev/null || {
            echo "  WARNING: failed to download $project/$fuzzer — skipping"
            rm -f "$zip"
            return
        }
        # Verify it's a real zip, not an XML error
        if ! file "$zip" | grep -q "Zip archive"; then
            echo "  WARNING: $project/$fuzzer returned non-zip response — skipping"
            rm -f "$zip"
            return
        fi
    fi

    mkdir -p "$dst"
    unzip -qo "$zip" -d "$dst" 2>/dev/null || true
    local count
    count=$(find "$dst" -type f | wc -l)
    echo "  $dst_subdir: $count files total"
}

fetch_ossfuzz_zip "libjpeg-turbo" "cjpeg_fuzzer" "jpeg"
fetch_ossfuzz_zip "libpng" "read_fuzzer" "png"
fetch_ossfuzz_zip "libjxl" "djxl_fuzzer" "jxl"

echo ""
echo "=== Phase 5: Final mixed/ refresh ==="
for subdir in jpeg png gif webp avif jxl heic bitmaps tiff; do
    src="$CORPUS_DIR/$subdir"
    [ -d "$src" ] || continue
    count=0
    for f in "$src"/*; do
        [ -f "$f" ] || continue
        cp -n "$f" "$MIXED_DIR/" 2>/dev/null || true
        count=$((count + 1))
        [ "$count" -ge 20 ] && break
    done
done
echo "  mixed: $(find "$MIXED_DIR" -type f | wc -l) files total"

echo ""
echo "Done. Total seed files: $(find "$CORPUS_DIR" -type f | wc -l)"
