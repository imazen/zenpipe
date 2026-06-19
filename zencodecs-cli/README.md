# zencodecs

A minimal, capable image **transcoding CLI** over the [`zencodecs`](../zencodecs) engine —
MxN any→any, lossless, and *minimally-lossless* (zensim IQA). The library does all the
codec work; this binary is just argument parsing + file IO, so batch jobs are a plain
bash script, not bespoke Rust.

## Install / build

```sh
cargo build -p zencodecs-cli --release   # produces `zencodecs`
```

## Usage

```sh
# Transcode — output format inferred from the extension (or --format)
zencodecs convert in.jpg out.webp --quality 80
zencodecs convert in.png out.jxl  --lossless
zencodecs convert in.jpg out.avif --quality 50

# Minimally-lossless: smallest size meeting a zensim-A score (0–100) vs the original
zencodecs convert in.jpg out.jxl  --target-quality 90

# Web publishing: strip GPS/camera/timestamps, keep orientation + color
zencodecs convert in.jpg out.webp --metadata web

# Alpha → opaque: composite over a matte
zencodecs convert in.png out.jpg  --matte 255,255,255

# Probe: format + dimensions + supplements, as JSON
zencodecs probe in.heic
```

## Flags (`convert`)

| flag | meaning |
|---|---|
| `--format <f>` | force output format (png/jpeg/webp/avif/jxl/gif/bmp); else from the output extension |
| `--quality <0..100>` | lossy quality, codec-calibrated |
| `--lossless` | encode losslessly |
| `--target-quality <0..100>` | minimally-lossless: smallest size meeting this zensim-A score |
| `--metadata <exact\|preserve\|web\|color>` | metadata retention (default `exact`) |
| `--matte <R,G,B>` | matte for alpha→opaque (default white) |
| `-q, --quiet` | suppress the per-file summary |

One file in, one file out, clean exit codes — compose with `find`/`xargs` for batches
(see [`examples/batch-convert.sh`](examples/batch-convert.sh)).

## Supported formats

Decode + encode: JPEG, PNG, WebP, JXL, GIF, BMP. Decode: AVIF. Coefficient-domain
(no pixel round-trip): JPEG↔JPEG recompress, JPEG↔JXL lossless.

HEIC decode and an HDR-reconstruct (`--hdr`) mode are tracked in
[zenpipe#68](https://github.com/imazen/zenpipe/issues/68).
