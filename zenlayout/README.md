# zenlayout [![ci](https://img.shields.io/github/actions/workflow/status/imazen/zenlayout/ci.yml?style=flat-square&label=CI)](https://github.com/imazen/zenlayout/actions/workflows/ci.yml) [![crates.io](https://img.shields.io/crates/v/zenlayout?style=flat-square)](https://crates.io/crates/zenlayout) [![docs.rs](https://img.shields.io/docsrs/zenlayout?style=flat-square)](https://docs.rs/zenlayout) [![msrv](https://img.shields.io/badge/MSRV-1.89-blue?style=flat-square)](https://doc.rust-lang.org/cargo/reference/manifest.html#the-rust-version-field) [![license](https://img.shields.io/crates/l/zenlayout?style=flat-square)](https://github.com/imazen/zenlayout#license)

zenlayout is a pure-geometry image layout engine for computing resize dimensions, crop regions, and canvas placement.

`no_std` compatible, `#![forbid(unsafe_code)]`.

```toml
[dependencies]
zenlayout = "0.2"
# With RIAPI query string parsing:
zenlayout = { version = "0.2", features = ["riapi"] }
```

## What it does

Given source dimensions and a set of commands (orient, crop, region, constrain, pad), zenlayout computes every dimension, crop rect, and placement offset needed to produce the output. It handles EXIF orientation, aspect-ratio-aware scaling, codec alignment (JPEG MCU boundaries), and gain map / secondary plane spatial locking.

What it doesn't do: touch pixels. That's your resize engine's job.

## Quick start

```rust
use zenlayout::{Pipeline, DecoderOffer, OutputLimits, Subsampling};

let (ideal, request) = Pipeline::new(4000, 3000)
    .auto_orient(6)            // EXIF orientation 6 = 90 CW
    .fit(800, 600)             // fit within 800x600
    .output_limits(OutputLimits {
        align: Some(Subsampling::S420.mcu_align()),
        ..Default::default()
    })
    .plan()
    .unwrap();

// Pass `request` to your decoder, get back what it actually did
let offer = DecoderOffer::full_decode(4000, 3000);
let plan = ideal.finalize(&request, &offer);

// plan.resize_to, plan.canvas, plan.remaining_orientation, etc.
// contain everything the resize engine needs
```

### RIAPI query strings

With the `riapi` feature, parse URL query strings directly:

```rust
use zenlayout::riapi;

let result = riapi::parse("w=800&h=600&mode=crop&scale=both");
let pipeline = result.instructions
    .to_pipeline(4000, 3000, None)
    .expect("valid pipeline");

let (ideal, _request) = pipeline.plan().expect("valid layout");
assert_eq!(ideal.layout.resize_to.width, 800);
assert_eq!(ideal.layout.resize_to.height, 600);
```

Supported parameters: `w`/`h`, `maxwidth`/`maxheight`, `mode`, `scale`, `crop`, `anchor`, `zoom`/`dpr`, `srotate`/`rotate`, `flip`/`sflip`, `autorotate`, `bgcolor`, `c.gravity`, `c.focus`, `c.zoom`, `c.finalmode`. Non-layout keys (format, quality, effects) are preserved in `Instructions::extras()` for downstream consumers.

### Smart crop parameters

These parameters control content-aware crop positioning. They require a crop-producing mode (`mode=crop`) with target dimensions (`w`/`h`).

| Parameter | Values | Description |
|-----------|--------|-------------|
| `c.gravity=x,y` | Percentages 0-100 | Focal point for crop alignment. `c.gravity=30,70` shifts the crop window toward 30% from left, 70% from top. |
| `c.focus=x1,y1,x2,y2` | Percentages 0-100 | Focus rectangle that must stay visible when cropping. |
| `c.focus=x1,y1,x2,y2;...` | Semicolon-separated | Multiple focus rectangles. Also supports flat comma groups of 4. |
| `c.focus=x,y` | Percentages 0-100 | Focal point (2-value shorthand, equivalent to `c.gravity`). |
| `c.focus=faces` | Keyword | Trigger face detection (requires ML engine). Silently ignored when unavailable. |
| `c.focus=saliency` | Keyword | Trigger saliency detection. Silently ignored when unavailable. |
| `c.focus=auto` | Keyword | Trigger faces + saliency detection. Silently ignored when unavailable. |
| `c.zoom=true` | Boolean | Maximal crop mode — zoom tight on the subject. Default: `false` (minimal crop, keeps most content). |
| `c.finalmode=pad\|crop\|max\|stretch` | Mode name | Override the constraint mode after smart crop is applied. |

Example: `?w=400&h=400&mode=crop&c.focus=20,30,80,90` crops to a square while keeping the region from 20-80% width, 30-90% height visible.

Parsed into `Instructions::c_focus` (`CFocus` enum), `c_zoom` (`Option<bool>`), and `c_finalmode` (`Option<String>`). Back-compatible with ImageResizer's CropAround plugin syntax.

## Processing pipeline

The `Pipeline` builder processes operations in a fixed order, regardless of the order setters are called. **Last-setter-wins**: calling the same category twice replaces the previous value (standard builder pattern). Orientation is the exception — it always composes algebraically.

```text
Pipeline processing order

1. ORIENT -- All orientation commands (auto_orient, rotate, flip) compose
   into a single source transform via D4 group algebra. This happens
   regardless of where they appear -- there is no "post-resize flip."
   Source dimensions transform to post-orientation space.

     .auto_orient(6).rotate_90() = Rotate90 . Rotate90 = Rotate180
     .fit(800, 600).flip_h()     = flip source, then fit (not "fit then flip")

2. REGION or CROP -- Define the effective source. Crop and region share a
   single slot; setting either replaces the other.
   - Crop: select a rectangle within the source (origin + size)
   - Region: viewport into infinite canvas (edge coords; can crop, pad, or both)
   Crop converts to Region internally.

3. CONSTRAIN -- Resize the effective source to target dimensions. The 9
   constraint modes control aspect ratio handling.
   - Fit/FitCrop/FitPad/Distort will upscale small images
   - Within/WithinCrop/WithinPad will not
   - PadWithin never upscales but always pads to target canvas
   - Single-axis constraints derive the missing dimension from aspect ratio

4. PAD -- Add explicit padding around the constrained result. Additive on
   canvas dimensions. Padding does NOT collapse -- pad_uniform(10, color)
   always adds exactly 10px on each side regardless of other commands.

5. LIMITS -- Safety limits applied to the final canvas:
   a. max -- proportional downscale if canvas exceeds max (security cap)
   b. min -- proportional upscale if canvas below min (quality floor)
   c. align -- snap to codec multiples (may slightly exceed max/drop below min)
   Max always wins over min.
```

**Sequential mode** (`compute_layout_sequential()`): same operations, but commands execute in order. Orient still fuses into a source transform. Multiple crop/region compose (each refines the previous). **Last** constrain wins. Post-constrain crop/pad adjusts the output canvas.

## Two-phase layout

Layout computation splits into two phases to support decoder negotiation (JPEG prescaling, partial decode, hardware orientation):

```text
    Commands + Source
          |
          v
    +--------------+     +--------------+
    |compute_layout|---->|DecoderRequest|----> Decoder
    +--------------+     +--------------+        |
          |                                      |
          v                                      v
    +-----------+       +-------------+    +------------+
    |IdealLayout|------>|  finalize() |<---|DecoderOffer|
    +-----------+       +-------------+    +------------+
                              |
                              v
                        +----------+
                        |LayoutPlan| -- final operations
                        +----------+
```

**Phase 1** (`Pipeline::plan()` or `compute_layout()`) computes the ideal layout assuming a full decode. It returns an `IdealLayout` (what the output should look like) and a `DecoderRequest` (hints for the decoder — crop region, target size, orientation).

**Phase 2** (`IdealLayout::finalize()`) takes a `DecoderOffer` describing what the decoder actually did (maybe it prescaled to 1/8, applied orientation, or cropped to MCU boundaries). It compensates for the difference and returns a `LayoutPlan` with the remaining work: what to trim, resize, orient, and place on the canvas.

If your decoder doesn't support any of that, pass `DecoderOffer::full_decode(w, h)`.

## Constraint modes

Nine modes control how source dimensions map to target dimensions:

| Mode | Behavior | Aspect ratio | May upscale |
|------|----------|-------------|-------------|
| `Fit` | Scale to fit within target | Preserved | **Yes** |
| `Within` | Like Fit, but never upscales | Preserved | No |
| `FitCrop` | Scale to fill target, crop overflow | Preserved | **Yes** |
| `WithinCrop` | Like FitCrop, but never upscales | Preserved | No |
| `FitPad` | Scale to fit, pad to exact target | Preserved | **Yes** |
| `WithinPad` | Like FitPad, but never upscales | Preserved | No |
| `PadWithin` | Never upscale, always pad to target canvas | Preserved | No |
| `Distort` | Scale to exact target dimensions | Stretched | **Yes** |
| `AspectCrop` | Crop to target aspect ratio, no scaling | Preserved | No |

`WithinPad` vs `PadWithin`: when the source is smaller than the target, `WithinPad` returns the image at its original size (identity — no canvas expansion). `PadWithin` always returns the target canvas dimensions with the image centered on it.

```text
    Source 4:3, Target 1:1 (square):

    Fit           Within         FitCrop       FitPad
    +---+         +---+          +---+         +-----+
    |   |         |   |          | # |         |     |
    |   |         |   |          | # |         | ### |
    |   |         |   |(smaller) | # |         |     |
    +---+         +---+          +---+         +-----+
    exact size    <= source      fills+crops    fits+pads
```

Single-axis constraints are supported: `Constraint::width_only()` and `Constraint::height_only()` derive the other dimension from the source aspect ratio.

## Orientation

`Orientation` models the D4 dihedral group — 4 rotations x 2 flip states = 8 elements, matching EXIF orientations 1-8.

| Orientation | = Rotation  | + FlipH? | Swaps axes? |
|-------------|-------------|----------|-------------|
| Identity    | 0           | no       | no          |
| FlipH       | 0           | yes      | no          |
| Rotate180   | 180         | no       | no          |
| FlipV       | 180         | yes      | no          |
| Transpose   | 90 CW       | yes      | yes         |
| Rotate90    | 90 CW       | no       | yes         |
| Transverse  | 270 CW      | yes      | yes         |
| Rotate270   | 270 CW      | no       | yes         |

Orientations compose algebraically and are verified against the D4 Cayley table:

```rust
use zenlayout::Orientation;

let exif6 = Orientation::from_exif(6).unwrap(); // 90 CW
let combined = exif6.compose(Orientation::FlipH);
assert_eq!(combined, Orientation::Transpose);   // EXIF 5

// Inverse undoes:
assert_eq!(exif6.compose(exif6.inverse()), Orientation::Identity);
```

**All orientation commands fuse into a single source transform**, regardless of where they appear in the pipeline. There is no "post-resize flip" — orientation is always applied to the source. In sequential mode, if an axis-swapping orientation (Rotate90/270, Transpose, Transverse) appears after a constraint, the constraint's target dimensions are swapped to compensate, producing correct output geometry.

## Region

`Region` defines a viewport rectangle in source coordinates. It unifies crop and pad into a single concept:

- Viewport smaller than source = crop
- Viewport extending beyond source = pad (filled with `color`)
- Viewport entirely outside source = blank canvas

Coordinates use **edge positions** (left, top, right, bottom), not origin + size. `Region::crop(10, 10, 90, 90)` selects an 80x80 area. This differs from `SourceCrop::pixels(10, 10, 80, 80)` which uses origin + size for the same region.

Each edge is a `RegionCoord`: a percentage of source dimension plus a pixel offset. This allows expressions like "10% from the left edge" or "50 pixels past the right edge".

```rust
use zenlayout::{Pipeline, Region, RegionCoord, CanvasColor};

// 50px padding on all sides
let (ideal, _) = Pipeline::new(800, 600)
    .region(Region::padded(50, CanvasColor::white()))
    .plan()
    .unwrap();
// Canvas: 900x700, source at (50, 50)

// Mixed crop+pad: extend left, crop right
let (ideal, _) = Pipeline::new(800, 600)
    .region_viewport(-50, 0, 600, 600, CanvasColor::black())
    .plan()
    .unwrap();
// Canvas: 650x600, 600x600 of source at (50, 0)

// Percentage-based crop: 10% from each edge
let reg = Region {
    left: RegionCoord::pct(0.1),
    top: RegionCoord::pct(0.1),
    right: RegionCoord::pct(0.9),
    bottom: RegionCoord::pct(0.9),
    color: CanvasColor::Transparent,
};
```

`SourceCrop` converts to `Region` internally via `to_region()`. Region and Crop share a single slot — setting either replaces the other.

When a Region is combined with a constraint, the constraint operates on the overlap between the viewport and the source. The viewport's padding areas scale proportionally.

## Sequential mode

For scripting use cases where command order matters, use `compute_layout_sequential()` with a `Command` slice:

```rust
use zenlayout::{compute_layout_sequential, Command, SourceCrop};

let commands = [
    Command::Crop(SourceCrop::pixels(100, 100, 600, 400)),
    Command::Crop(SourceCrop::pixels(50, 50, 500, 300)),  // refines the first crop
];
let (ideal, _) = compute_layout_sequential(&commands, 800, 600, None).unwrap();
```

Sequential mode differences from fixed mode:
- **Orient**: still fuses into a single source transform regardless of position
- **Crop/Region**: compose sequentially (second crop refines the first)
- **Constrain**: last one wins
- **Post-constrain crop/pad**: adjusts the output canvas, not the source
- **Limits**: applied once at the end (same as fixed)

Both modes produce a single `Layout` — one crop, one resize, one canvas. "Sequential" refers to the command evaluation order, not multi-pass pixel processing. Requires `alloc` feature.

## Secondary planes

For gain maps, depth maps, or alpha planes that share spatial extent with the primary image but live at a different resolution:

```rust
use zenlayout::{Pipeline, DecoderOffer, Size};

// SDR: 4000x3000, gain map: 1000x750 (1/4 scale)
let (sdr_ideal, sdr_req) = Pipeline::new(4000, 3000)
    .auto_orient(6)
    .crop_pixels(100, 100, 2000, 2000)
    .fit(800, 800)
    .plan()
    .unwrap();

// Derive gain map plan -- automatically maintains the source ratio
let (gm_ideal, gm_req) = sdr_ideal.derive_secondary(
    Size::new(4000, 3000),  // primary source
    Size::new(1000, 750),   // gain map source
    None,                   // auto: 1/4 of SDR output
);

// Each decoder independently handles its capabilities
let sdr_plan = sdr_ideal.finalize(&sdr_req, &DecoderOffer::full_decode(4000, 3000));
let gm_plan = gm_ideal.finalize(&gm_req, &DecoderOffer::full_decode(1000, 750));

// Both plans produce spatially-locked results
assert_eq!(sdr_plan.remaining_orientation, gm_plan.remaining_orientation);
```

Source crop coordinates are scaled from primary to secondary space with round-outward logic (origin floors, extent ceils) to ensure full spatial coverage.

## Codec layout

`CodecLayout` computes per-plane geometry for YCbCr encoders:

```rust
use zenlayout::{CodecLayout, Subsampling, Size};

let codec = CodecLayout::new(Size::new(800, 608), Subsampling::S420);

// Luma plane
assert_eq!(codec.luma.extended, Size::new(800, 608));
assert_eq!(codec.luma.blocks_w, 100); // 800 / 8

// Chroma plane (half resolution for 4:2:0)
assert_eq!(codec.chroma.extended, Size::new(400, 304));

// MCU grid
assert_eq!(codec.mcu_size, Size::new(16, 16));
assert_eq!(codec.mcu_cols, 50);

// Feed rows in chunks of this size to the encoder
assert_eq!(codec.luma_rows_per_mcu, 16);
```

## Feature flags

| Flag | Default | Implies | Description |
|------|---------|---------|-------------|
| `std` | **yes** | `alloc` | Standard library. Enables `Error` impl for `LayoutError`. |
| `alloc` | via `std` | — | Heap allocation (`Vec`, `BTreeMap`). Enables `compute_layout_sequential`. |
| `riapi` | no | `alloc` | RIAPI query string parsing (`?w=800&h=600&mode=crop`). |
| `svg` | no | `std` | SVG visualization of layout pipeline steps. |
| `smart-crop` | no | `alloc` | Content-aware cropping (experimental, API unstable). |

The core API (`Pipeline`, `Constraint::compute()`, `compute_layout()`) works with zero features — `no_std`, no heap. `Pipeline::plan()` makes zero heap allocations.

## Error handling

`LayoutError` is returned from `Constraint::compute()`, `Pipeline::plan()`, and `Instructions::to_pipeline()`:

| Variant | Cause |
|---------|-------|
| `ZeroSourceDimension` | Source image has zero width or height |
| `ZeroTargetDimension` | Target width or height is zero |
| `ZeroRegionDimension` | Region viewport has zero or negative area |
| `NonFiniteFloat` | A float parameter is NaN or infinity |

NaN/Inf values are rejected at all API boundaries — in the RIAPI parser, at `Constraint::compute()` entry, and at `Instructions::to_pipeline()` entry.

## Limitations

- Only integer coordinates (no subpixel positioning)
- Sequential mode requires `alloc` feature
- `smart-crop` feature is experimental, API unstable
- No pixel operations — geometry only

## Image tech I maintain

| | |
|:--|:--|
| State of the art codecs* | [zenjpeg] · [zenpng] · [zenwebp] · [zengif] · [zenavif] ([rav1d-safe] · [zenrav1e] · [zenavif-parse] · [zenavif-serialize]) · [zenjxl] ([jxl-encoder] · [zenjxl-decoder]) · [zentiff] · [zenbitmaps] · [heic] · [zenraw] · [zenpdf] · [ultrahdr] · [mozjpeg-rs] · [webpx] |
| Compression | [zenflate] · [zenzop] |
| Processing | [zenresize] · [zenfilters] · [zenquant] · [zenblend] |
| Metrics | [zensim] · [fast-ssim2] · [butteraugli] · [resamplescope-rs] · [codec-eval] · [codec-corpus] |
| Pixel types & color | [zenpixels] · [zenpixels-convert] · [linear-srgb] · [garb] |
| Pipeline | [zenpipe] · [zencodec] · [zencodecs] · **zenlayout** · [zennode] |
| ImageResizer | [ImageResizer] (C#) — 24M+ NuGet downloads across all packages |
| [Imageflow][] | Image optimization engine (Rust) — [.NET][imageflow-dotnet] · [node][imageflow-node] · [go][imageflow-go] — 9M+ NuGet downloads across all packages |
| [Imageflow Server][] | [The fast, safe image server](https://www.imazen.io/) (Rust+C#) — 552K+ NuGet downloads, deployed by Fortune 500s and major brands |

<sub>* as of 2026</sub>

### General Rust awesomeness

[archmage] · [magetypes] · [enough] · [whereat] · [zenbench] · [cargo-copter]

[And other projects](https://www.imazen.io/open-source) · [GitHub @imazen](https://github.com/imazen) · [GitHub @lilith](https://github.com/lilith) · [lib.rs/~lilith](https://lib.rs/~lilith) · [NuGet](https://www.nuget.org/profiles/imazen) (over 30 million downloads / 87 packages)

## License

Dual-licensed: [AGPL-3.0](LICENSE-AGPL3) or [commercial](LICENSE-COMMERCIAL).

I've maintained and developed open-source image server software — and the 40+
library ecosystem it depends on — full-time since 2011. Fifteen years of
continual maintenance, backwards compatibility, support, and the (very rare)
security patch. That kind of stability requires sustainable funding, and
dual-licensing is how we make it work without venture capital or rug-pulls.
Support sustainable and secure software; swap patch tuesday for patch leap-year.

[Our open-source products](https://www.imazen.io/open-source)

**Your options:**

- **Startup license** — $1 if your company has under $1M revenue and fewer
  than 5 employees. [Get a key →](https://www.imazen.io/pricing)
- **Commercial subscription** — Governed by the Imazen Site-wide Subscription
  License v1.1 or later. Apache 2.0-like terms, no source-sharing requirement.
  Sliding scale by company size.
  [Pricing & 60-day free trial →](https://www.imazen.io/pricing)
- **AGPL v3** — Free and open. Share your source if you distribute.

See [LICENSE-COMMERCIAL](LICENSE-COMMERCIAL) for details.

[zenjpeg]: https://github.com/imazen/zenjpeg
[zenpng]: https://github.com/imazen/zenpng
[zenwebp]: https://github.com/imazen/zenwebp
[zengif]: https://github.com/imazen/zengif
[zenavif]: https://github.com/imazen/zenavif
[zenjxl]: https://github.com/imazen/zenjxl
[zentiff]: https://github.com/imazen/zentiff
[zenbitmaps]: https://github.com/imazen/zenbitmaps
[heic]: https://github.com/imazen/heic-decoder-rs
[zenraw]: https://github.com/imazen/zenraw
[zenpdf]: https://github.com/imazen/zenpdf
[ultrahdr]: https://github.com/imazen/ultrahdr
[jxl-encoder]: https://github.com/imazen/jxl-encoder
[zenjxl-decoder]: https://github.com/imazen/zenjxl-decoder
[rav1d-safe]: https://github.com/imazen/rav1d-safe
[zenrav1e]: https://github.com/imazen/zenrav1e
[mozjpeg-rs]: https://github.com/imazen/mozjpeg-rs
[zenavif-parse]: https://github.com/imazen/zenavif-parse
[zenavif-serialize]: https://github.com/imazen/zenavif-serialize
[webpx]: https://github.com/imazen/webpx
[zenflate]: https://github.com/imazen/zenflate
[zenzop]: https://github.com/imazen/zenzop
[zenresize]: https://github.com/imazen/zenresize
[zenfilters]: https://github.com/imazen/zenfilters
[zenquant]: https://github.com/imazen/zenquant
[zenblend]: https://github.com/imazen/zenblend
[zensim]: https://github.com/imazen/zensim
[fast-ssim2]: https://github.com/imazen/fast-ssim2
[butteraugli]: https://github.com/imazen/butteraugli
[zenpixels]: https://github.com/imazen/zenpixels
[zenpixels-convert]: https://github.com/imazen/zenpixels
[linear-srgb]: https://github.com/imazen/linear-srgb
[garb]: https://github.com/imazen/garb
[zenpipe]: https://github.com/imazen/zenpipe
[zencodec]: https://github.com/imazen/zencodec
[zencodecs]: https://github.com/imazen/zencodecs
[zennode]: https://github.com/imazen/zennode
[Imageflow]: https://github.com/imazen/imageflow
[Imageflow Server]: https://github.com/imazen/imageflow-server
[imageflow-dotnet]: https://github.com/imazen/imageflow-dotnet
[imageflow-node]: https://github.com/imazen/imageflow-node
[imageflow-go]: https://github.com/imazen/imageflow-go
[ImageResizer]: https://github.com/imazen/resizer
[archmage]: https://github.com/imazen/archmage
[magetypes]: https://github.com/imazen/archmage
[enough]: https://github.com/imazen/enough
[whereat]: https://github.com/lilith/whereat
[zenbench]: https://github.com/imazen/zenbench
[cargo-copter]: https://github.com/imazen/cargo-copter
[resamplescope-rs]: https://github.com/imazen/resamplescope-rs
[codec-eval]: https://github.com/imazen/codec-eval
[codec-corpus]: https://github.com/imazen/codec-corpus
