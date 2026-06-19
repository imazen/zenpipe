#!/usr/bin/env bash
# The `hdr-corpus-convert` Rust crate, as a bash script over `zencodecs` (zenpipe#68).
#
# For each image under <corpus-root>, mirroring the tree into <out-dir>:
#   * PNG            → copied verbatim as <stem>.sdr.png (it already IS its SDR rendition)
#   * JPEG / HEIC    → <stem>.sdr.png  (display-oriented, cICP-tagged SDR base)
#   * gain-map source→ <stem>.hdr.png  (BT.2100 PQ HDR reconstruction, cICP + cLLI)
#
# Verified pixel-identical (ImageMagick AE=0) to the corpus-convert reference on
# Apple/Samsung HEIC (SDR+HDR, oriented) and JPEG. The CLI auto-orients by default
# and carries the resolved source color (e.g. Apple Display-P3) onto the SDR cICP.
#
# Usage: convert-hdr-corpus.sh <zencodecs-bin> <corpus-root> <out-dir>
set -euo pipefail
BIN=${1:?path to zencodecs binary}
ROOT=${2:?corpus root}
OUT=${3:?output dir}

find "$ROOT" -type f \( -iname '*.png' -o -iname '*.jpg' -o -iname '*.jpeg' -o -iname '*.heic' -o -iname '*.heif' \) \
| while IFS= read -r src; do
    rel=${src#"$ROOT"/}; dir=$(dirname "$rel"); stem=$(basename "${rel%.*}")
    dst="$OUT/$dir"; mkdir -p "$dst"
    sdr="$dst/$stem.sdr.png"
    [ -f "$sdr" ] && continue                       # resumable

    case "${src,,}" in
        *.png)  cp "$src" "$sdr" ;;                  # PNG is its own SDR rendition
        *)      "$BIN" convert "$src" "$sdr" --quiet || { echo "  FAIL sdr $rel" >&2; continue; } ;;
    esac

    # HDR rendition only when the container advertises a gain map.
    if "$BIN" probe "$src" 2>/dev/null | grep -q '"gain_map":true'; then
        "$BIN" convert "$src" "$dst/$stem.hdr.png" --hdr --quiet || echo "  FAIL hdr $rel" >&2
    fi
done
echo "done -> $OUT"
