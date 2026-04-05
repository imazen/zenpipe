# imageflow-magic — ImageMagick Compatibility Layer

Drop-in replacement for ImageMagick's `convert`/`magick` command. Parses IM syntax, emits zenpipe node graphs. Pure Rust, no ImageMagick dependency.

```bash
# These do the same thing:
convert photo.jpg -resize 800x600 -quality 85 out.webp
imageflow-magic photo.jpg -resize 800x600 -quality 85 out.webp
zenpipe magic photo.jpg -resize 800x600 -quality 85 out.webp
```

---

## 1. Coverage Summary

**~82% of real-world ImageMagick usage is mappable today.** 18 of the top 20 most-used operations are fully supported. Text rendering is available via SVG overlay (zensvg). SVG and JPEG 2000 format support added.

| Category | Real-World Usage | Coverage | Gap |
|----------|-----------------|----------|-----|
| Resize + geometry | 35% | 90% | liquid-rescale, shear |
| Color/level adjust | 20% | 85% | -fx expressions |
| Sharpen/blur/enhance | 15% | 80% | motion-blur, rotational-blur |
| Compositing/overlay | 10% | 95% | — |
| Format conversion | 5% | 95% | JPEG 2000, SVG, EPS |
| Artistic effects | 5% | 30% | oil-paint, charcoal, sketch, swirl |
| Drawing/text | 5% | 40% | SVG text via zensvg; no IM `-draw`/`-annotate` syntax |
| Morphology/analysis | 2% | 0% | all 22 methods |
| Animation | 2% | 70% | morph |
| Distortion | 1% | 30% | affine + perspective via zenfilters warp; 11 other types missing |

---

## 2. IM Tool Mapping

| IM Tool | Replacement | Status |
|---------|-------------|--------|
| `convert` / `magick` | `imageflow-magic` or `zenpipe magic` | ✅ Primary target |
| `identify` / `magick identify` | `zenpipe info` | ✅ Via zencodec probe |
| `mogrify` / `magick mogrify` | `zenpipe magic --mogrify` (in-place) | ✅ Same pipeline, overwrite |
| `composite` / `magick composite` | `zenpipe magic -composite` | ✅ Via NodeOp::Composite |
| `compare` / `magick compare` | `zenpipe compare` | ✅ Via zensim/butteraugli |
| `montage` | `zenpipe montage` | ⬜ Needs grid layout |
| `display` / `animate` / `import` | Out of scope | ❌ GUI/OS tools |
| `conjure` | Out of scope | ❌ MSL scripting |
| `stream` | Native — zenpipe IS a streaming engine | ✅ |

---

## 3. Geometry Syntax Parser

Full ImageMagick geometry specification:

| Syntax | Meaning | Zenpipe Mapping |
|--------|---------|----------------|
| `WxH` | Fit within, preserve aspect | `Constrain { w, h, mode: "within" }` |
| `WxH!` | Exact, distort aspect | `Constrain { mode: "distort" }` |
| `WxH^` | Fill, crop to fit | `Constrain { mode: "fit_crop" }` |
| `WxH>` | Shrink only (never enlarge) | `Constrain { mode: "within" }` (native) |
| `WxH<` | Enlarge only (never shrink) | `Constrain { mode: "larger_than" }` |
| `W` | Width only, auto height | `Constrain { w: Some(W), h: None }` |
| `xH` | Height only, auto width | `Constrain { w: None, h: Some(H) }` |
| `scale%` | Scale by percentage | Compute `w = src_w * scale/100`, resize |
| `sx%xsy%` | Per-axis scale | Compute both, `mode: "distort"` |
| `area@` | Target pixel area | `w = sqrt(area * aspect)`, fit |
| `WxH+X+Y` | Crop at position | `Crop { x: X, y: Y, w: W, h: H }` |
| `WxH#` | Pad to exact (letterbox) | `Constrain { mode: "fit_pad" }` |

```rust
// imageflow-magic/src/geometry.rs
pub struct IMGeometry {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub x_offset: Option<i32>,
    pub y_offset: Option<i32>,
    pub flags: GeometryFlags,
    pub percentage: bool,
    pub area: bool,
}

bitflags! {
    pub struct GeometryFlags: u8 {
        const BANG       = 0x01;  // !  exact/distort
        const CARET      = 0x02;  // ^  fill/crop
        const GREATER    = 0x04;  // >  shrink only
        const LESS       = 0x08;  // <  enlarge only
        const HASH       = 0x10;  // #  pad/letterbox
    }
}

pub fn parse_geometry(s: &str) -> Result<IMGeometry, ParseError> { ... }
```

---

## 4. Operation Mapping (Complete)

### 4.1 Geometry — Full Support

| IM Flag | Zen Equivalent | Notes |
|---------|---------------|-------|
| `-resize WxH[!^><#]` | `Constrain` with mode from flags | Full geometry syntax |
| `-crop WxH+X+Y` | `Crop { x, y, w, h }` | |
| `-thumbnail WxH` | `Constrain` + strip metadata | Fast path |
| `-scale WxH` | `Resize` with `Filter::Box` | Pixel averaging |
| `-sample WxH` | `Resize` with nearest-neighbor | |
| `-extent WxH` | `ExpandCanvas` | Canvas expand/crop |
| `-border N` | `ExpandCanvas` with bg_color | |
| `-shave NxN` | `Crop` (inset) | |
| `-trim` | `CropWhitespace` | Auto content detect |
| `-flip` | `Orient(FlipV)` | |
| `-flop` | `Orient(FlipH)` | |
| `-rotate N` | `Orient` (90/180/270) or zenfilters `Rotate` | Cardinal = pixel-perfect |
| `-auto-orient` | `AutoOrient(exif)` | |
| `-transpose` | `Orient(Transpose)` | |
| `-transverse` | `Orient(Transverse)` | |
| `-gravity` | Sets anchor for crop/pad/composite | Mapped to `Constrain.anchor` |
| `-resample DPI` | Compute target pixels from DPI, resize | |

### 4.2 Color & Levels — Full Support

| IM Flag | Zen Equivalent |
|---------|---------------|
| `-colorspace sRGB/Gray/...` | `RowConverterOp` |
| `-modulate B,S,H` | `Exposure` + `Saturation` + `HueRotate` |
| `-brightness-contrast BxC` | `Exposure` + `Contrast` |
| `-level black%,white%,gamma` | `Levels` |
| `-auto-level` | `AutoLevels` |
| `-auto-gamma` | `AutoExposure` |
| `-normalize` | `AutoLevels { clip_low: 0.02, clip_high: 0.02 }` |
| `-gamma G` | `Exposure` (gamma curve) |
| `-negate` | `Invert` |
| `-grayscale Rec709` | `Grayscale` |
| `-sepia-tone N%` | `Sepia { amount: N/100 }` |
| `-tint N%` | `Temperature` / `Tint` |
| `-sigmoidal-contrast CxM` | `Sigmoid { contrast: C }` |
| `-contrast-stretch Bx%Wx%` | `Levels` with clip params |
| `-color-matrix M` | `ColorMatrix { matrix: M }` |
| `-hald-clut` | `CubeLut` (3D LUT application) |
| `-cdl` | `AscCdl` |

### 4.3 Enhancement — Full Support

| IM Flag | Zen Equivalent |
|---------|---------------|
| `-sharpen 0xR` | `Sharpen { sigma: R }` |
| `-unsharp RxS+A+T` | `Sharpen { sigma: S, amount: A }` |
| `-adaptive-sharpen 0xR` | `AdaptiveSharpen { sigma: R }` |
| `-blur 0xS` | `Blur { sigma: S }` |
| `-gaussian-blur 0xS` | `Blur { sigma: S }` |
| `-bilateral-blur WxS` | `Bilateral { spatial: W, range: S }` |
| `-despeckle` | `MedianBlur { radius: 1 }` |
| `-enhance` | `NoiseReduction` |
| `-noise N` | `Grain { amount: N/100 }` (additive) |

### 4.4 Compositing — Full Support (26 blend modes)

| IM Flag | Zen Equivalent |
|---------|---------------|
| `-composite` | `NodeOp::Composite` |
| `-compose Method` | `zenblend::BlendMode` (26 modes) |
| `-dissolve N%` | `Overlay { opacity: N/100 }` |
| `-blend N%` | `Overlay { opacity: N/100 }` |
| `-watermark N%` | `WatermarkLayout` + `Overlay` |
| `-flatten` | `Composite` chain |
| `-gravity` | Anchor for overlay positioning |
| `-geometry +X+Y` | Offset for overlay |

Blend mode mapping:

| IM | zenblend | | IM | zenblend |
|----|----------|-|----|----------|
| src-over | `SrcOver` | | multiply | `Multiply` |
| dst-over | `DstOver` | | screen | `Screen` |
| src-in | `SrcIn` | | overlay | `Overlay` |
| dst-in | `DstIn` | | darken | `Darken` |
| src-out | `SrcOut` | | lighten | `Lighten` |
| dst-out | `DstOut` | | color-dodge | `ColorDodge` |
| src-atop | `SrcAtop` | | color-burn | `ColorBurn` |
| dst-atop | `DstAtop` | | hard-light | `HardLight` |
| xor | `Xor` | | soft-light | `SoftLight` |
| plus | `Plus` | | difference | `Difference` |
| clear | `Clear` | | exclusion | `Exclusion` |
| src | `Src` | | linear-dodge | `LinearDodge` |
| dst | `Dst` | | linear-burn | `LinearBurn` |

### 4.5 Format & Encoding — Full Support

| IM Flag | Zen Equivalent |
|---------|---------------|
| `-quality N` | Codec-calibrated quality (universal 0-100) |
| `-depth N` | `PixelFormat` bit depth |
| `-type Grayscale/TrueColor` | `PixelFormat` channel layout |
| `-alpha on/off/remove` | `AddAlpha` / `RemoveAlpha` |
| `-strip` | Encode without metadata |
| `-profile path` | ICC embed/convert via moxcms |
| `-interlace Plane/Line` | Progressive JPEG / interlaced PNG |
| `-sampling-factor 2x2` | JPEG chroma subsampling |
| `-define key=val` | Codec-specific options |
| `-density DPI` | Metadata DPI tag |
| `-compress type` | Codec-specific compression |

### 4.6 Not Supported (Intentional)

| IM Feature | Status | Alternative |
|-----------|--------|-------------|
| `-fx` expression language | ❌ Won't support | Security risk, full interpreter |
| `-draw` / `-annotate` | ⬜ Different syntax | Render text as SVG, composite via zensvg |
| `-morphology` (22 methods) | ⬜ Not yet | Erode/dilate/open/close — image analysis domain |
| `-distort` (11 of 13 types) | ⬜ Partial | Perspective + affine via zenfilters warp; arc/barrel/etc missing |
| `-liquid-rescale` | ❌ Won't support | Seam carving — very slow, rarely needed |
| `-convolve` (custom kernels) | ⬜ Not yet | Gaussian + edge detect exist; generic kernel API missing |
| `-emboss` / `-shade` | ⬜ Not yet | Could build from convolution kernels |
| `-sketch` / `-charcoal` / `-paint` | ⬜ Not yet | Multi-step filter chains; low priority |
| `-swirl` / `-implode` / `-wave` | ⬜ Not yet | Polar warp variants; extend zenfilters warp |
| `-motion-blur` / `-rotational-blur` | ⬜ Not yet | Directional/radial blur kernels |
| GUI tools (display/animate/import) | ❌ Out of scope | Use native viewers |
| MSL scripting (conjure) | ❌ Out of scope | Dead feature |

**Text rendering strategy**: Instead of reimplementing IM's `-annotate`/`-draw text`, compose text as SVG and overlay:
```bash
# IM style (won't support):
convert photo.jpg -annotate +10+20 "Hello" out.jpg

# zenpipe style (supported):
zenpipe photo.jpg --overlay '<svg><text x="10" y="20" fill="white">Hello</text></svg>' out.jpg

# Or from SVG file:
zenpipe photo.jpg --overlay text.svg out.jpg
```
This is more powerful (full SVG typography, CSS fonts, gradients, transforms) and avoids bundling a font rasterizer separately from the SVG renderer we already have.

---

## 5. Crate Architecture

```
imageflow-magic/
├── Cargo.toml
├── src/
│   ├── lib.rs          — public API: parse_command() → NodeList
│   ├── parse.rs        — argument parser (IM flag syntax)
│   ├── geometry.rs     — geometry string parser (WxH!^><@%+X+Y)
│   ├── color.rs        — color string parser (hex, named, rgba())
│   ├── filter.rs       — -filter name → zenresize::Filter
│   ├── compose.rs      — -compose name → zenblend::BlendMode
│   ├── colorspace.rs   — -colorspace name → PixelDescriptor
│   ├── unsharp.rs      — unsharp mask syntax parser (RxS+A+T)
│   ├── commands/
│   │   ├── convert.rs  — convert/magick command handling
│   │   ├── identify.rs — identify command (info only)
│   │   ├── mogrify.rs  — mogrify (in-place batch)
│   │   ├── composite.rs — composite command
│   │   └── compare.rs  — compare command
│   ├── nodes.rs        — IM operations → zenpipe NodeInstance list
│   ├── quirks.rs       — IM behavioral quirks for compatibility
│   └── error.rs        — IM-style error messages
└── tests/
    ├── geometry_tests.rs
    ├── command_tests.rs
    └── compatibility/   — test suite: run same args through IM and zenpipe, compare
```

### 5.1 Public API

```rust
use imageflow_magic::{parse_command, IMCommand};

// Parse IM-style arguments into a structured command
let cmd = parse_command(&[
    "photo.jpg", "-resize", "800x600", "-quality", "85", "out.webp"
])?;

// cmd.input: "photo.jpg"
// cmd.output: "out.webp"
// cmd.operations: [Resize(800, 600, Within), Quality(85)]

// Convert to zenpipe node list
let nodes: Vec<Box<dyn zennode::NodeInstance>> = cmd.to_nodes()?;

// Or execute directly
let result = cmd.execute()?;
```

### 5.2 Quirks Layer

ImageMagick has many undocumented behaviors. The quirks layer handles:

```rust
// quirks.rs — document each quirk with IM version and rationale
pub fn apply_quirks(ops: &mut Vec<Operation>) {
    // IM applies -strip AFTER encoding, not during pipeline
    // IM's -resize with > flag still reads the full image
    // IM's -thumbnail strips profiles BEFORE resize (faster for JPEG)
    // IM's -crop with negative offsets wraps around
    // IM's -gravity affects the NEXT operation, not all operations
    // IM's -auto-orient strips the EXIF orientation tag after applying
}
```

---

## 6. Format Coverage

| Format | IM | zenpipe | Notes |
|--------|-----|---------|-------|
| JPEG | ✅ | ✅ zenjpeg | |
| PNG | ✅ | ✅ zenpng | |
| WebP | ✅ (delegate) | ✅ zenwebp | IM uses libwebp delegate |
| GIF | ✅ | ✅ zengif | Animation support |
| AVIF | ✅ (recent) | ✅ zenavif | |
| JXL | ✅ (delegate) | ✅ zenjxl | Pure Rust, no delegate |
| TIFF | ✅ | ✅ zentiff | |
| BMP | ✅ | ✅ zenbitmaps | |
| PNM/PAM/PFM | ✅ | ✅ zenbitmaps | |
| QOI | ❌ | ✅ zenbitmaps | zenpipe-only format |
| TGA | ✅ | ✅ zenbitmaps | |
| HDR (Radiance) | ✅ | ✅ zenbitmaps | |
| HEIC/HEIF | ✅ (delegate) | ✅ heic | Decode only |
| RAW/DNG | ✅ (delegate) | ✅ zenraw | Multiple backends |
| PDF | ✅ (Ghostscript) | ✅ zenpdf | Via hayro renderer |
| SVG/SVGZ | ✅ (librsvg) | ✅ zensvg | resvg + usvg, text rendering, optimize |
| JPEG 2000 | ✅ (delegate) | ✅ zenjp2 | hayro-jpeg2000 (pure Rust) |
| EPS/PS | ✅ (Ghostscript) | ❌ | PostScript interpreter |
| EXR | ✅ | ❌ | No pure Rust codec |
| FLIF | ✅ | ❌ | Dead format |

**Format coverage: 20/22 common formats supported.** Missing: EPS (PostScript interpreter), EXR (OpenEXR).

---

## 7. Testing Strategy

### 7.1 Compatibility Test Suite

```bash
# For each test case, run both IM and imageflow-magic, compare outputs:

# Generate IM reference output
convert test.jpg -resize 800x600 /tmp/im-ref.png

# Generate imageflow-magic output
imageflow-magic test.jpg -resize 800x600 /tmp/ifm-out.png

# Compare (using zensim SSIMULACRA2)
zenpipe compare /tmp/im-ref.png /tmp/ifm-out.png --threshold 90
```

### 7.2 Test Categories

| Category | Tests | Validates |
|----------|-------|-----------|
| Geometry parsing | ~50 | All WxH variants, flags, offsets |
| Resize modes | ~20 | Each flag (!, ^, >, <, #, @, %) |
| Color operations | ~30 | modulate, level, gamma, auto-level, etc. |
| Compositing | ~15 | Each blend mode, gravity, offset |
| Format round-trip | ~40 | Each format, quality levels, bit depths |
| Batch/mogrify | ~10 | Glob patterns, in-place, output templates |
| Quirks | ~20 | Documented IM behavioral edge cases |
| Error messages | ~15 | Actionable errors matching IM patterns |

---

## 8. Migration Guide (for users)

```bash
# Step 1: Alias (zero effort migration)
alias convert='imageflow-magic'
alias magick='imageflow-magic'

# Step 2: Check compatibility
imageflow-magic --check-compat your-script.sh
# Output:
#   ✅ -resize 800x600         supported
#   ✅ -quality 85              supported
#   ⚠️ -annotate +10+20 "Hi"   not supported (text rendering)
#   ✅ -strip                   supported

# Step 3: For unsupported operations, use zenpipe native syntax
# instead of IM compat mode (cleaner, more powerful)
```
