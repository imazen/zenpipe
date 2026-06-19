#!/usr/bin/env bash
# Demonstrates the goal of zenpipe#68: a batch image converter as a plain bash
# script over `zencodecs` — no bespoke Rust. This is the shape `hdr-corpus-convert`
# collapses to once the CLI gains HEIC decode + an `--hdr reconstruct` mode.
#
# TODAY this handles the SDR / transcode majority (PNG/JPEG/WebP/AVIF/JXL).
# The HDR (gain-map HEIC / Ultra-HDR-JPEG → PQ PNG) path is pending #68.
#
# Usage: batch-convert.sh <zencodecs-bin> <src-dir> <out-dir> [<out-ext>]
set -euo pipefail
BIN=${1:?path to zencodecs binary}
SRC=${2:?source dir}
OUT=${3:?output dir}
EXT=${4:-png}            # target format by extension

find "$SRC" -type f \( -iname '*.png' -o -iname '*.jpg' -o -iname '*.jpeg' \
        -o -iname '*.webp' -o -iname '*.avif' -o -iname '*.jxl' -o -iname '*.gif' -o -iname '*.bmp' \) \
| while IFS= read -r src; do
    rel=${src#"$SRC"/}
    dst="$OUT/${rel%.*}.$EXT"
    # Resumable: skip files already converted.
    [ -f "$dst" ] && continue
    mkdir -p "$(dirname "$dst")"
    "$BIN" convert "$src" "$dst" --quiet \
        || echo "  FAIL $rel" >&2
done
echo "done -> $OUT"
