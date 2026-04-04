# zenpipe CLI — Executable Spec

**Goal**: Replace ImageMagick, libvips, and Pillow with a single binary that's faster, safer, and simpler.

**Philosophy**: One obvious way to do everything. No 500-page man page. If you can say it in English, you can type it as a command.

---

## 1. Core Syntax

```bash
zenpipe <input> [operations...] <output>
```

That's it. Input on the left, output on the right, operations in between. Every operation is a verb.

```bash
# Resize a JPEG
zenpipe photo.jpg --resize 800x600 out.webp

# The format is inferred from the extension. Always.
zenpipe photo.jpg out.png          # convert JPEG → PNG
zenpipe photo.jpg out.jxl          # convert JPEG → JXL
zenpipe photo.jpg out.avif         # convert JPEG → AVIF

# No operation = smart convert (preserves quality, optimal settings)
zenpipe photo.jpg out.webp

# stdin/stdout
cat photo.jpg | zenpipe - --resize 800 - > out.webp
curl https://example.com/img.jpg | zenpipe - --resize 400 out.webp
```

---

## 2. Operations

Operations chain left to right. Each takes the output of the previous.

### 2.1 Geometry

```bash
--resize 800              # fit within 800×800, preserve aspect
--resize 800x600          # fit within 800×600
--resize 800x600!         # exact (distort if needed)
--resize 800x600^         # fill (crop to fit)
--resize 800x600#         # pad (letterbox to fit)
--resize 50%              # scale by percentage
--resize 800 --filter lanczos   # explicit filter (default: robidoux)

--crop 100,100,800,600    # x,y,w,h in pixels
--crop 10%,10%,80%,80%    # percentage crop
--crop auto               # auto whitespace crop (content detection)

--rotate 90               # cardinal (pixel-perfect, no interpolation)
--rotate 2.5              # arbitrary (robidoux interpolation)
--rotate auto             # auto-deskew (detect and correct)

--flip h                  # horizontal
--flip v                  # vertical

--orient auto             # apply EXIF orientation and strip tag
--orient 6                # force EXIF orientation value

--pad 20                  # add 20px padding (white)
--pad 20 --bg black       # padding with color
--pad 10,20,10,20         # top,right,bottom,left
```

### 2.2 Filters

```bash
--exposure 1.5            # stops (photographic, +1 = 2× brighter)
--contrast 0.3            # -1..1
--brightness 10           # simple linear (like CSS, -100..100)
--saturation 1.2          # factor (1.0 = unchanged)
--vibrance 0.5            # 0..1
--temperature 0.2         # warm/cool shift
--tint 0.1                # green/magenta shift

--clarity 0.5             # local contrast
--sharpen 0.3             # unsharp mask amount
--sharpen 0.3,1.0         # amount,sigma
--blur 2.0                # gaussian sigma
--denoise 0.5             # luminance noise reduction

--highlights -0.3         # recover highlights
--shadows 0.3             # lift shadows
--black-point 0.05        # crush blacks
--white-point 0.95        # clip whites

--vignette 0.3            # strength
--dehaze 0.5              # dehazing strength
--grain 0.2               # film grain amount

--auto-enhance            # auto exposure + levels + clarity + vibrance
--auto-levels             # histogram stretch
--auto-exposure           # scene-adaptive exposure correction
--auto-wb                 # auto white balance
```

### 2.3 Color

```bash
--grayscale               # luminance-weighted B&W
--sepia 0.5               # sepia toning (0..1)
--invert                  # color inversion

--preset portra           # film look preset
--preset velvia,0.7       # preset with intensity

--colorspace srgb         # convert to sRGB
--colorspace p3           # convert to Display P3
--colorspace rec2020      # convert to Rec.2020
```

### 2.4 Document

```bash
--deskew                  # auto-detect and correct rotation
--perspective             # auto-detect document quad and rectify
--binarize                # Otsu threshold for B&W documents
--clean-doc               # full pipeline: deskew + perspective + crop auto + auto-levels
```

### 2.5 Compositing

```bash
--overlay logo.png,10,10           # overlay at x,y
--overlay logo.png,right,bottom    # named anchor
--overlay logo.png,center,0.5      # centered, 50% opacity
--watermark logo.png               # smart watermark (auto-position, auto-opacity)

--canvas 1920x1080 --bg white      # create canvas, place image centered
--fill red                         # fill with solid color (for testing)
```

---

## 3. Output Control

### 3.1 Format (Inferred from Extension)

```bash
out.jpg   out.jpeg        # JPEG
out.webp                  # WebP
out.png                   # PNG
out.jxl                   # JPEG XL
out.avif                  # AVIF
out.gif                   # GIF
out.bmp                   # BMP
out.tiff                  # TIFF
out.qoi                   # QOI
out.ppm                   # PPM
```

### 3.2 Quality / Codec Options

```bash
--quality 85              # universal quality (0-100, maps to codec-native)
--effort 7                # compression effort (0-10, speed vs size)
--lossless                # lossless mode (WebP, JXL, AVIF, PNG)
--near-lossless 80        # near-lossless (WebP, JXL)

# Codec-specific (rarely needed)
--jpeg-subsampling 420    # 444, 422, 420
--jpeg-progressive        # progressive JPEG
--png-depth 16            # 16-bit PNG
--jxl-distance 1.0        # butteraugli distance (overrides --quality)
--avif-speed 4            # rav1e speed (1-10)
--gif-dither 0.5          # dithering strength
```

### 3.3 Metadata

```bash
--strip                   # strip all metadata
--strip exif              # strip EXIF only
--strip icc               # strip ICC profile
--keep exif,icc           # keep only these (strip everything else)
--preserve                # keep everything (default)

--icc srgb                # embed/convert to sRGB ICC profile
--icc p3                  # embed/convert to Display P3
```

### 3.4 HDR / Gain Map

```bash
--hdr preserve            # keep gain map if present (default)
--hdr strip               # discard gain map, output SDR only
--hdr tonemap             # apply gain map → HDR output
--hdr reconstruct         # reconstruct gain map from HDR source
```

---

## 4. Batch Processing

```bash
# All JPEGs in a directory → WebP at 80 quality
zenpipe 'photos/*.jpg' --resize 1600 --quality 80 'out/{name}.webp'

# Placeholders in output path:
#   {name}  — input filename without extension
#   {ext}   — input extension
#   {dir}   — input directory
#   {n}     — sequence number
#   {w}     — output width
#   {h}     — output height

# Multiple outputs from one input (srcset)
zenpipe photo.jpg --resize 400 'out/{name}-sm.webp' \
                  --resize 800 'out/{name}-md.webp' \
                  --resize 1600 'out/{name}-lg.webp'

# Parallel processing
zenpipe 'photos/*.jpg' --resize 800 'out/{name}.webp' --jobs 8

# Recursive
zenpipe 'photos/**/*.jpg' --resize 800 'out/{name}.webp'

# Dry run (show what would be done)
zenpipe 'photos/*.jpg' --resize 800 'out/{name}.webp' --dry-run
```

---

## 5. Info / Inspect

```bash
# Image info (dimensions, format, color space, metadata)
zenpipe info photo.jpg
# Output:
#   photo.jpg: JPEG 4000×3000, sRGB, 8-bit
#   EXIF: Canon EOS R5, 24mm f/2.8, ISO 100
#   ICC: sRGB IEC61966-2.1
#   Size: 2.4 MB (1.6 bpp)
#   Gain map: none

# JSON output for scripting
zenpipe info photo.jpg --json

# Probe multiple files
zenpipe info 'photos/*.jpg' --json > manifest.json

# Compare two images (perceptual difference)
zenpipe compare a.jpg b.jpg
# Output:
#   SSIMULACRA2: 84.3 (good)
#   Butteraugli: 1.2
#   PSNR: 38.2 dB
```

---

## 6. Recipes

```bash
# Save a recipe
zenpipe photo.jpg --exposure 0.5 --contrast 0.3 --preset portra \
        --save-recipe sunset.json out.jpg

# Apply a recipe to another image
zenpipe other.jpg --recipe sunset.json out.jpg

# Apply a recipe to a batch
zenpipe 'photos/*.jpg' --recipe sunset.json 'out/{name}.webp'

# Export current pipeline as imageflow querystring
zenpipe photo.jpg --exposure 0.5 --resize 800 --print-qs
# Output: ?s.brightness=0.5&w=800&format=jpg

# Apply an imageflow/RIAPI querystring
zenpipe photo.jpg --qs "w=800&h=600&mode=crop&format=webp" out.webp
```

---

## 7. Srcset Generation

```bash
# Generate responsive image set
zenpipe photo.jpg --srcset 400,800,1200,1600 --quality 80 'out/{name}-{w}w.webp'

# With multiple formats
zenpipe photo.jpg --srcset 400,800,1200 --formats webp,avif,jxl 'out/{name}-{w}w.{ext}'

# Generate HTML srcset attribute
zenpipe photo.jpg --srcset 400,800,1200 --quality 80 'out/{name}-{w}w.webp' --print-srcset
# Output:
#   <img srcset="photo-400w.webp 400w, photo-800w.webp 800w, photo-1200w.webp 1200w"
#        sizes="(max-width: 800px) 100vw, 800px"
#        src="photo-800w.webp" alt="">

# With named crop sets
zenpipe photo.jpg --srcset 400,800 --crop-set hero:16:9,thumb:1:1 \
        'out/{name}-{crop}-{w}w.webp'
```

---

## 8. Pipeline Visualization

```bash
# Show the pipeline graph (what zenpipe will do)
zenpipe photo.jpg --resize 800 --exposure 0.5 out.webp --explain
# Output:
#   1. Decode JPEG (4000×3000, sRGB, 8-bit)
#   2. Resize 800×600 (robidoux, within)
#   3. Exposure +0.5 stops (Oklab f32)
#   4. Convert Oklab f32 → sRGB u8
#   5. Encode WebP lossy (quality 80, effort 5)
#   Estimated: 45ms, ~120 KB output

# Trace mode (detailed timing per step)
zenpipe photo.jpg --resize 800 out.webp --trace
# Output:
#   decode:     12.3ms (4000×3000 → RGBA8)
#   resize:      8.1ms (4000×3000 → 800×600, robidoux)
#   encode:     24.7ms (WebP lossy q80)
#   total:      45.1ms
#   output:     118 KB (1.57 bpp)
```

---

## 9. Exit Codes & Errors

```
0   Success
1   Input error (file not found, unsupported format)
2   Operation error (invalid parameter, pipeline failure)
3   Output error (write failed, disk full)
4   Partial failure (batch mode: some files failed)
```

Errors go to stderr with context:
```
error: resize: width 0 is invalid (must be 1-65535)
  → zenpipe photo.jpg --resize 0 out.jpg
                                ^
```

---

## 10. Comparison with Existing Tools

### What zenpipe replaces

```bash
# ImageMagick
convert photo.jpg -resize 800x600 -quality 85 out.webp
# zenpipe
zenpipe photo.jpg --resize 800x600 --quality 85 out.webp

# libvips
vipsthumbnail photo.jpg -s 800 -o out.webp[Q=85]
# zenpipe
zenpipe photo.jpg --resize 800 --quality 85 out.webp

# Pillow (Python)
python -c "from PIL import Image; Image.open('photo.jpg').resize((800,600)).save('out.webp',quality=85)"
# zenpipe
zenpipe photo.jpg --resize 800x600 --quality 85 out.webp

# ffmpeg (for images)
ffmpeg -i photo.jpg -vf scale=800:-1 out.webp
# zenpipe
zenpipe photo.jpg --resize 800 out.webp
```

### What zenpipe adds that others don't

```bash
# Perceptual photo filters (not just pixel math)
zenpipe photo.jpg --clarity 0.5 --vibrance 0.3 --dehaze 0.4 out.jpg

# Film look presets
zenpipe photo.jpg --preset portra,0.8 out.jpg

# Auto document cleanup
zenpipe scan.jpg --clean-doc out.png

# Gain map / HDR pipeline
zenpipe hdr.jxl --hdr tonemap out.jpg

# JPEG XL (no separate tool needed)
zenpipe photo.jpg --quality 80 --effort 7 out.jxl

# Pipeline caching (Session reuse across batch)
zenpipe 'photos/*.jpg' --resize 800 --exposure 0.5 'out/{name}.webp'
# Second run with different filter: only re-runs filters, reuses resize cache

# Srcset + crop sets in one command
zenpipe photo.jpg --srcset 400,800,1200 --crop-set hero:16:9 'out/{name}-{crop}-{w}w.webp'
```

---

## 11. Library / Sidecar API

For programmatic use (replacing Pillow as a library):

### 11.1 C API (imageflow_abi compatible)

```c
// Create a pipeline
zen_pipeline *p = zen_pipeline_new();
zen_pipeline_decode(p, bytes, len);
zen_pipeline_resize(p, 800, 600, ZEN_FIT_WITHIN);
zen_pipeline_exposure(p, 0.5);
zen_pipeline_encode(p, ZEN_FORMAT_WEBP, 85);

// Execute
zen_result *r = zen_pipeline_execute(p);
// r->data, r->len, r->width, r->height

zen_result_free(r);
zen_pipeline_free(p);
```

### 11.2 Rust API (direct)

```rust
use zenpipe::cli::{Pipeline, Fit};

let output = Pipeline::open("photo.jpg")?
    .resize(800, 600, Fit::Within)
    .exposure(0.5)
    .encode_webp(85)
    .execute()?;

std::fs::write("out.webp", &output.data)?;
```

### 11.3 Python Bindings (Pillow replacement)

```python
import zenpipe

# Simple — mirrors CLI syntax
img = zenpipe.open("photo.jpg")
img = img.resize(800, 600)
img = img.exposure(0.5)
img.save("out.webp", quality=85)

# Or one-liner
zenpipe.process("photo.jpg", resize=800, exposure=0.5, output="out.webp")

# Batch
zenpipe.batch("photos/*.jpg", resize=800, quality=80, output="out/{name}.webp")

# NumPy interop (for data science)
arr = zenpipe.open("photo.jpg").to_numpy()  # → (H, W, 4) uint8 RGBA
img = zenpipe.from_numpy(arr)
img.save("out.png")

# Pillow interop
from PIL import Image
pil_img = Image.open("photo.jpg")
zen_img = zenpipe.from_pil(pil_img)
zen_img = zen_img.auto_enhance().preset("portra")
pil_out = zen_img.to_pil()
```

### 11.4 What the Library Adds Over the CLI

| Feature | CLI | Library |
|---------|-----|---------|
| Pipeline construction | Arguments | Method chaining |
| In-memory I/O | stdin/stdout | Byte buffers |
| Pixel access | No | NumPy arrays, raw buffers |
| Session reuse | Per-batch | Explicit, across calls |
| Streaming decode | Automatic | Pull-based Source trait |
| Custom filters | No | Implement Filter trait |
| Embedding | Subprocess | In-process, zero-copy |
| Async | No | tokio/async-std support |

---

## 12. Build & Distribution

```bash
# Install from crates.io
cargo install zenpipe-cli

# Or download binary
curl -sSL https://zenpipe.dev/install.sh | sh

# Or Homebrew
brew install zenpipe

# Or Docker
docker run --rm -v $(pwd):/data zenpipe photo.jpg --resize 800 out.webp

# Build from source
git clone https://github.com/imazen/zenpipe
cd zenpipe && cargo build --release --bin zenpipe
```

Binary size target: <15 MB (all codecs statically linked, no runtime deps).
Supported: Linux (x86_64, aarch64), macOS (arm64, x86_64), Windows (x86_64, aarch64).

---

## 13. Design Principles

1. **Input → operations → output**. Always. No modes, no subcommands for basic operations.
2. **Format from extension**. Never `--format webp`. The extension IS the format.
3. **Sensible defaults**. `--resize 800` does the right thing. No required flags.
4. **Universal quality**. `--quality 85` means the same perceptual quality regardless of format. Internally calibrated per codec.
5. **Errors are actionable**. Show what went wrong, where, and what to do instead.
6. **Fast by default**. Streaming pipeline, SIMD, session caching. No temp files.
7. **No ImageMagick legacy**. No `-` prefix for flags. No arcane geometry syntax. No 700 operators.
8. **Composable**. Pipes work. JSON output for scripting. Exit codes are meaningful.
9. **Safe**. Memory-safe (Rust), no buffer overflows, resource limits by default. `#![forbid(unsafe_code)]`.
10. **One binary**. No shared libraries, no runtime deps, no Python, no Java.
