# zencodecs

A minimal, capable image **transcoding CLI** over the [`zencodecs`](../zencodecs) engine —
MxN any→any, lossless, and *minimally-lossless* (by zensim IQA score, or by size). The
library does all the codec work; this binary is just argument parsing + file IO, so batch
jobs are a plain bash script, not bespoke Rust.

## Install / build

```sh
cargo build -p zencodecs-cli --release   # produces `zencodecs`
```

## Usage

```sh
# Transcode — output format inferred from the extension (or --format)
zencodecs convert in.jpg out.webp --quality 80
zencodecs convert in.png out.jxl  --lossless
zencodecs convert in.jpg out.avif --quality 50 --speed realtime

# Minimally-lossless by IQA: smallest size meeting a zensim-A score (0–100) vs the original
zencodecs convert in.jpg out.jxl  --target-quality 90

# Minimally-lossless by size: keep the lossless encode when it is at most 1.5× the
# lossy one (at --quality, or the codec default), else fall back to lossy
zencodecs convert in.png out.webp --quality 80 --lossless-if-cheaper 1.5

# Web publishing: strip GPS/camera/timestamps, keep orientation + color
zencodecs convert in.jpg out.webp --metadata web

# Alpha → opaque: composite over a matte
zencodecs convert in.png out.jpg  --matte 255,255,255

# HDR rendition of a gain-map source (HEIC / Ultra-HDR JPEG): BT.2100 PQ PNG with cICP+cLLI
zencodecs convert in.heic out-hdr.png --hdr
# …and the SDR base rendition of the same file (auto-oriented, Display P3 cICP kept)
zencodecs convert in.heic out-sdr.png

# Probe: format + dimensions + supplements, as JSON
zencodecs probe in.heic
```

## Flags (`convert`)

| flag | meaning |
|---|---|
| `--format <f>` | force output format (png/jpeg/webp/avif/jxl/gif/bmp); else from the output extension |
| `--quality <0..100>` | lossy quality, codec-calibrated |
| `--lossless` | encode losslessly |
| `--lossless-if-cheaper [FACTOR]` | encode lossless *and* lossy; keep lossless when ≤ FACTOR × the lossy size (default 1.5) |
| `--target-quality <0..100>` | minimally-lossless: smallest size meeting this zensim-A score |
| `--speed <fastest\|realtime\|offline\|offline-max>` | per-codec effort preset (`zencodecs::EncodeSpeed`); `fastest` is single-threaded |
| `--metadata <exact\|preserve\|web\|color>` | metadata retention (default `exact`) |
| `--matte <R,G,B>` | matte for alpha→opaque (default white) |
| `--hdr` | reconstruct the gain-map HDR rendition to a PQ PNG (output is always PNG) |
| `--keep-orientation` | keep the EXIF orientation tag instead of baking it into the pixels (default: auto-orient) |
| `-q, --quiet` | suppress the per-file summary |

One file in, one file out, clean exit codes — compose with `find`/`xargs` for batches:
[`examples/batch-convert.sh`](examples/batch-convert.sh) (recursive, resumable) and
[`examples/convert-hdr-corpus.sh`](examples/convert-hdr-corpus.sh) (SDR + PQ-HDR renditions
of a gain-map corpus — the bash replacement for the former `hdr-corpus-convert` crate,
pixel-identical to it on the reference set).

## Supported formats

Decode + encode: JPEG, PNG, WebP, JXL, GIF, BMP. Decode only: AVIF, HEIC (incl. Apple /
Samsung gain-map HDR). Coefficient-domain (no pixel round-trip): JPEG↔JPEG recompress,
JPEG↔JXL lossless.

`--hdr` targets BT.2100 PQ only; there is no HLG output mode. The `probe` JSON reports
`gain_map` / `depth_map` presence but not the HDR transfer of the reconstructed rendition.
