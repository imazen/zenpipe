<!-- GENERATED FROM README.md by zenutils gen-readme-crates.sh — DO NOT EDIT. -->

# zenfilters

Photo filter pipeline in Oklab perceptual color space with SIMD acceleration via [archmage](https://github.com/imazen/archmage).

55+ filters with broad coverage of Lightroom and darktable adjustments for tone, color, detail, and effects. 34 built-in film look presets using tensor-compressed 3D LUTs (163 KB total). ASC CDL, .cube LUT loading, hue-qualified curves. Self-describing parameter schemas for automatic UI generation.

Rust 1.93+, 2024 edition. `#![forbid(unsafe_code)]` — entirely safe Rust. Part of the
[zenpipe](https://github.com/imazen/zenpipe) monorepo (its standalone repository now
redirects here).

**[Browse the Film Look Gallery](https://imazen.github.io/zenpipe/)** — interactive before/after comparisons for all 34 presets.

## Architecture

```text
Input (linear RGB f32 or sRGB u8)
  → scatter: deinterleave to planar Oklab (L, a, b planes)
    → filter stack: each filter modifies planes in-place
      → gamut mapping: compress out-of-gamut colors
        → gather: reinterleave to output format
```

Oklab is perceptually uniform — arithmetic operations produce visually proportional changes. Splitting into contiguous f32 planes means luminance-only filters (exposure, contrast, curves) process one plane at full SIMD width.

### Strip processing

All processing uses L3-cache-friendly horizontal strips (~4 MB working set). Neighborhood filters use overlapping halo rows. At 4K with clarity + sharpen (halo ~50px), the working set is ~9 MB per strip vs ~100 MB full-frame.

### SIMD

AVX2 f32x8 dispatch via archmage for all hot paths:
- Scatter/gather (RGB→Oklab→RGB conversion)
- Gaussian blur (FIR horizontal, stackblur vertical with 8-column tiles)
- FusedAdjust (11 per-pixel operations in one pass)
- Wavelet threshold + accumulate (noise reduction)
- Adaptive sharpen energy gating
- All per-pixel plane operations (scale, offset, power contrast, sigmoid, vibrance)

Fast math: `pow_lowp_unchecked` (~1% precision, 2× faster than midp) for contrast, sigmoid, and vibrance power curves. `cbrt_lowp` for Oklab conversion.

## Quick start

```rust
use zenfilters::{Pipeline, PipelineConfig, FilterContext};
use zenfilters::filters::*;

let mut pipeline = Pipeline::new(PipelineConfig::default())?;

let mut exposure = Exposure::default();
exposure.stops = 0.5;
pipeline.push(Box::new(exposure));

let mut clarity = Clarity::default();
clarity.amount = 0.3;
pipeline.push(Box::new(clarity));

let mut ctx = FilterContext::new();
let (w, h) = (1920, 1080);
let src = vec![0.5f32; w * h * 3];
let mut dst = vec![0.0f32; w * h * 3];
pipeline.apply(&src, &mut dst, w as u32, h as u32, 3, &mut ctx)?;
```

### `apply` data contract (read this before feeding it a decoded image)

`Pipeline::apply(src, dst, width, height, channels, ctx)`:

- **Layout: interleaved, not planar.** Despite the crate's planar-Oklab internals,
  `src`/`dst` are *interleaved* (`R,G,B,R,G,B,…`, or `R,G,B,A,…`). `apply` scatters
  to planar Oklab and gathers back internally — the planar layout never reaches the
  caller.
- **Colorspace: linear RGB, *not* sRGB-encoded.** Values are linear-light f32,
  nominally `[0, 1]`. Feeding sRGB-encoded f32 (mid-grey `128/255 ≈ 0.502`) where
  linear is expected (mid-grey ≈ `0.216`) silently wrecks every tone/contrast/gamut
  computation — there is no runtime check, so get this right. `WorkingSpace`
  (default `Oklab`) selects only the *internal* processing space; the f32
  input/output contract is linear RGB regardless of it.
- **`channels`: `3` (RGB) or `4` (RGBA).** With `4`, the alpha plane is carried
  through unmodified except by alpha-aware filters.
- **No stride.** Buffers must be tightly packed: `src.len()` and `dst.len()` must be
  `≥ width * height * channels`. There is no row-stride / row-padding parameter; a
  shorter buffer returns `PipelineError::BufferSize`.
- **`dst` comes back in the same space and layout as `src`** — interleaved linear
  RGB(A) f32. `dst` may alias nothing of `src` (pass a separate buffer).

#### Getting from a decoded RGB8 image to `apply` and back

`apply` itself takes only linear f32, so a decoded sRGB-u8 image needs a conversion
on each side. Three supported on-ramps, lowest-effort first:

1. **`apply_to_buffer(&pipeline, &input, convert_back, &mut ctx)`** (re-exported at
   the crate root) is the high-level path: hand it a `zenpixels::PixelBuffer` (which
   carries the descriptor — layout, transfer function, channel type) and it does
   linearization, HDR normalization, scatter, filter, gather, and re-encode for you.
   With `convert_back = false` it returns linear f32 RGB(A) for further processing.
2. **Manual linear-f32 path:** decode to sRGB-u8, convert to linear f32 with
   [`linear-srgb`](https://lib.rs/crates/linear-srgb)'s
   `default::srgb_u8_to_linear_slice`, run `apply`, then re-encode with
   `default::linear_to_srgb_u8_slice`. (`linear-srgb` is already a dependency.)
3. **In-crate scatter/gather:** the crate re-exports `scatter_srgb_u8_to_oklab` and
   `gather_oklab_to_srgb_u8` (a fused sRGB-u8 ⇄ Oklab path that skips the
   intermediate linear-f32 buffer) for callers driving the planar API directly via
   `apply_planar`.

(For ImageMagick-style math directly on sRGB-encoded values — no linearization, no
Oklab — see the `srgb-compat` and `srgb-filters` features below.)

### Errors

`Pipeline::new` and `apply` return `Result<_, whereat::At<PipelineError>>` — the
error plus the source location it was raised at, which is what a server wants in
structured logs. Pull both with the `whereat::At` accessors (`PipelineError` is
`#[non_exhaustive]`, so keep a wildcard arm):

```rust
use zenfilters::PipelineError;

match pipeline.apply(&src, &mut dst, w, h, 3, &mut ctx) {
    Ok(()) => {}
    Err(e) => {
        let loc = e.location();   // Option<&core::panic::Location> — file:line
        match e.error() {         // &PipelineError
            PipelineError::BufferSize { expected, actual } => { /* wrong buffer length */ }
            PipelineError::UnsupportedPrimaries(_) => { /* unsupported color primaries */ }
            _ => {}
        }
        eprintln!("filter pipeline failed at {loc:?}: {e}");
    }
}
```

### Cancellation

A server bounding a long filter stack can cancel cooperatively.
`apply_with_stop(src, dst, width, height, channels, ctx, stop)` takes a
[`&dyn enough::Stop`](https://lib.rs/crates/enough) as the final argument
(`apply` delegates to it with `enough::Unstoppable`, so the uncancellable path
is byte-identical). The token is polled only at outer-loop boundaries — between
scatter/gather strips and between filters, never inside the per-pixel inner
loops — so cancellation costs nothing in the hot path. On cancellation it
returns `PipelineError::Cancelled(enough::StopReason)`. There is a matching
`apply_planar_with_stop` for the planar entry point.

## Presets

19 built-in presets with intensity blending:

```rust
use zenfilters::presets;

let preset = &presets::builtin_presets()[0]; // "Vivid"
let pipe = preset.build_pipeline_at(0.6);   // 60% intensity
pipe.apply(&src, &mut dst, w, h, 3, &mut ctx)?;
```

Categories: Enhance (Vivid, Enhance, Clean), Warm (Warm, Golden Hour), Cool, Portrait (Portrait, Portrait Warm), Landscape, Film (Vintage, Film Warm, Film Cool, Faded), Cinematic (Cinematic, Moody), B&W (Classic, High Contrast, Film, Sepia).

Presets support tone curves, sigmoid, clarity, sharpening, grain, vignette, bloom, and B&W modes. Intensity blending lerps each parameter toward its identity value.

Presets serialize to JSON (with the `serde` feature) for storage and sharing.

## Film looks

34 built-in film look presets, each a mathematical RGB→RGB transform stored as a rank-8 tensor decomposition (~5 KB per look, 163 KB total). Max error across all presets: 8 levels @8bit; 22 of 34 are indistinguishable from the source LUT.

```rust
use zenfilters::filters::{FilmLook, FilmPreset};
use zenfilters::{Filter, FilterContext, OklabPlanes};

let mut look = FilmLook::new(FilmPreset::Kodachrome);
look.strength = 0.8;
look.apply(&mut planes, &mut FilterContext::new());
```

**Classic negative:** Portra, Kodak Gold, Ektar, Superia, Pro 400H

**Slide film:** Velvia, Provia, Kodachrome, Ektachrome

**Cinema:** Print 2383, 500T Tungsten

**Digital:** Classic Chrome, Classic Negative, Cool Chrome

**Creative:** Bleach Bypass, Cross Process, Teal & Orange, Faded Film, Golden Hour, Noir, Technicolor, Matte

**Cinematic moods:** Cyberpunk Neon, Desert Crush, Green Code, French Whimsy, Arctic Light, Neon Noir, Dusty Americana, Moonlit Blue, Cold Case, Desert Spice, Candy Pop, Blockbuster

Also supports loading arbitrary .cube 3D LUTs, ASC CDL color correction, and hue-qualified curves (Hue vs Sat, Hue vs Hue, Hue vs Lum, Lum vs Sat).

## Parameter schemas

Every filter is self-describing for automatic UI generation:

```rust
use zenfilters::param_schema::Describe;
use zenfilters::filters::AdaptiveSharpen;

let schema = AdaptiveSharpen::schema();
// schema.name = "adaptive_sharpen"
// schema.label = "Adaptive Sharpen"
// schema.group = FilterGroup::Detail
// schema.params[0] = ParamDesc {
//     name: "amount", label: "Amount",
//     kind: Float { min: 0.0, max: 2.0, default: 0.0, identity: 0.0, step: 0.05 },
//     unit: "×", section: "Main", slider: SliderMapping::Linear
// }
```

Each parameter carries: name, label, tooltip, type (Float/Int/Bool/FloatArray), range, default, identity point, step size, unit, UI section, and slider mapping.

Data binding via `get_param`/`set_param` by name:

```rust
filter.set_param("amount", ParamValue::Float(0.5));
let val = filter.get_param("amount"); // Some(Float(0.5))
```

`Filter::schema()` (or `<FilterType>::schema()`) is the authoritative,
machine-readable source for every field, range, default, and identity point — the
table below is a hand reference for the most common filters. Each struct is
`#[non_exhaustive]`, so construct via `Default::default()` then set fields.

### Common filter fields

| Filter | Field | Type | Identity / default | Notes |
|--------|-------|------|--------------------|-------|
| `Exposure` | `stops` | `f32` | `0.0` | Linear-light stops; ±. |
| `Contrast` | `amount` | `f32` | `0.0` | `1.0` = strong, `-1.0` = flatten. Pivots at Oklab middle grey. (**`amount`, not `factor`.**) |
| `Saturation` | `factor` | `f32` | `1.0` | `0.0` = grayscale, `2.0` = double. (**`factor`, not `amount`.**) |
| `Vibrance` | `amount` | `f32` | `0.0` | `1.0` = full boost. |
| `Vibrance` | `protection` | `f32` | `2.0` | Higher = more protection for already-saturated colors. |
| `Clarity` | `sigma` | `f32` | `4.0` | Fine-scale blur σ; coarse blur is `4× sigma`. |
| `Clarity` | `amount` | `f32` | `0.0` | `+` enhances texture, `-` softens; `0.3`–`1.0` natural. |
| `Clarity` | `adaptive` | `bool` | `false` | Variance-gated: more clarity in flat regions, less in textured. |

### Slider mappings

Some parameters have non-linear perceptual response. The `slider` module provides
free-function pairs (`*_from_slider` maps a UI slider value → internal parameter,
`*_to_slider` inverts it) so equal slider increments produce equal perceived
changes. Available pairs: `contrast_*`, `saturation_*`, `dehaze_*`,
`ltm_compression_*`, `nr_strength_*`, `bilateral_range_*`, `sharpen_noise_floor_*`.
(`Contrast::from_slider` and `Saturation::from_slider` are convenience constructors
wrapping `contrast_from_slider` / `saturation_from_slider`.)

| Mapping | Parameters | Effect |
|---------|-----------|--------|
| `Linear` | Most params | Direct 1:1 |
| `SquareFromSlider` | Contrast, dehaze, NR, LTM compression | First half = useful range |
| `FactorCentered` | Saturation | 0.5 = identity, 0 = gray, 1 = double |

```rust
use zenfilters::slider;
let internal = slider::contrast_from_slider(0.5); // → 0.25 (moderate)
let back = slider::contrast_to_slider(internal);   // → 0.5
```

## Filter compatibility

Machine-readable rules prevent common mistakes:

```rust
use zenfilters::filter_compat::{validate_pipeline, FilterTag};

let tags = [FilterTag::Sigmoid, FilterTag::DtSigmoid];
let issues = validate_pipeline(&tags);
// → error: "tone_mapper: 2 filters active, use only one"
```

**Exclusive groups** (use only one): tone mappers, sharpeners, smoothers.

**Ordering constraints**: denoise before sharpen, recovery before tuning, tone map before contrast.

**Range conflicts** with max-combined-intensity thresholds: Sigmoid + Contrast (0.6), LocalToneMap + Clarity (0.7), Saturation + GamutExpand (0.6).

## Resize-aware filtering

Filters declare when they should run relative to a resize:

| Phase | Filters | Why |
|-------|---------|-----|
| **PreResize** | CA, noise reduction, sharpen, clarity, texture, bilateral, dehaze | Pixel-space sigma; need full-res detail |
| **PostResize** | Grain, vignette, bloom | Spatial effects relative to output frame |
| **Either** | Exposure, contrast, curves, saturation, vibrance, color grading | Per-pixel, no spatial dependency |

### Resolution-independent parameters

Set `reference_width` so parameters work identically at any resolution. Define values once (e.g., for 4K), and the pipeline scales them automatically:

```rust
let mut pipe = Pipeline::new(PipelineConfig {
    reference_width: Some(3840), // params calibrated for 4K
    ..Default::default()
})?;
pipe.push(Box::new(Clarity { sigma: 4.0, amount: 0.3 }));
pipe.push(Box::new(Exposure { stops: 0.3 }));
pipe.push(Box::new(Grain { amount: 0.2, size: 1.5, seed: 0 }));
```

**Without resize** — scale everything for the actual resolution:
```rust
pipe.scale_to_width(1920);  // clarity σ→2.0, grain size→0.75
pipe.apply(&src, &mut dst, 1920, 1080, 3, &mut ctx)?;
```

**With resize** — one call scales each half for the resolution it runs at, then splits:
```rust
let (pre, post) = pipe.split_scaled(3840, 1920);
// pre: clarity σ=4.0 (scaled for 3840 input)
// post: grain size=0.75 (scaled for 1920 output)

pre.apply(&src, &mut buf, 3840, 2160, 3, &mut ctx)?;
// ... zenresize ...
post.apply(&resized, &mut dst, 1920, 1080, 3, &mut ctx)?;
```

**Without scaling** — use raw pixel values, split only:
```rust
let (pre, post) = pipe.split_for_resize();
```

Three methods, composable: `scale_to_width()`, `split_for_resize()`, `split_scaled()`. Presets, autotune, and user edits all work through the same system.

## Filters (49)

### Tone & Exposure (16)

| Filter | Description |
|--------|-------------|
| `Exposure` | Linear light exposure in stops |
| `AutoExposure` | Geometric mean normalization |
| `Contrast` | Midtone-pivoted power curve |
| `HighlightsShadows` | Highlight/shadow recovery with quadratic masks |
| `WhitesBlacks` | Smoothstep-weighted extreme luminance control |
| `BlackPoint` | Level remapping with optional soft-clip headroom (low end) |
| `WhitePoint` | Level remapping with optional soft-clip headroom (high end) |
| `HighlightRecovery` | Histogram-adaptive soft-knee compression |
| `ShadowLift` | Histogram-adaptive toe lift |
| `ToneCurve` | Monotone cubic Hermite (Fritsch-Carlson) |
| `ParametricCurve` | 4-zone Lightroom-style parametric curve |
| `ChannelCurves` | Per-channel R/G/B LUTs in sRGB space |
| `Levels` | Input/output range remap with gamma |
| `Sigmoid` | Generalized sigmoid with chroma compression |
| `BasecurveToneMap` | Camera-specific tone curves (14 cameras + 16 makers) |
| `ToneEqualizer` | 9-zone guided-filter luminance adjustment (darktable equivalent) |
| `LocalToneMap` | Base/detail decomposition with pivoted gamma |

### Sharpening & Detail (7)

| Filter | Description |
|--------|-------------|
| `AdaptiveSharpen` | Noise-gated USM with Lightroom's 4 controls (amount, radius, detail, masking) |
| `Sharpen` | Basic unsharp mask |
| `Clarity` | Two-band mid-frequency local contrast |
| `Texture` | Fine detail enhancement (finer scale than clarity) |
| `Brilliance` | S-curve local adaptation (smoothstep-weighted) |
| `Bloom` | Soft-knee highlight glow with screen blending |
| `EdgeDetect` | Sobel / Laplacian / Canny edge detection on L channel |

### Noise Reduction (4)

| Filter | Description |
|--------|-------------|
| `NoiseReduction` | Wavelet (à trous) with BayesShrink optimal thresholding |
| `Bilateral` | Guided filter (O(1)/pixel, edge-preserving) |
| `Blur` | Gaussian blur (SIMD stackblur for σ≥6, FIR for small σ) |
| `MedianBlur` | Neighborhood median for salt-and-pepper noise; L-only or all channels |

### Color (11)

| Filter | Description |
|--------|-------------|
| `Temperature` | Oklab b channel offset (warm/cool) |
| `Tint` | Oklab a channel offset (green/magenta) |
| `Saturation` | Uniform chroma scale |
| `Vibrance` | Chroma-protective saturation (boosts muted colors, protects skin) |
| `HueRotate` | 2D rotation in a/b plane |
| `HslAdjust` | Per-hue H/S/L adjustments (8 ranges) |
| `ColorGrading` | Shadow/midtone/highlight split-toning |
| `CameraCalibration` | R/G/B primary hue+sat shifts, shadow tint |
| `ColorMatrix` | 5×5 affine transform in linear RGB |
| `GamutExpand` | Hue-selective P3 chroma expansion |
| `BwMixer` | Chroma-aware B&W channel mixer (8 weights) |

### Effects (9)

| Filter | Description |
|--------|-------------|
| `Grain` | Deterministic film grain with midtone response curve |
| `Vignette` | Radial luminance darkening |
| `Devignette` | Radial lens correction (brightening) |
| `Dehaze` | Dark channel prior analog in Oklab |
| `ChromaticAberration` | Radial chroma plane shift (bilinear) |
| `Grayscale` | Luminance-only conversion |
| `Sepia` | Warm monotone toning |
| `Invert` | Luminance and chroma inversion |
| `Alpha` | Alpha channel multiplier (fade / transparency) |

### Compositing & Masking (1)

| Filter | Description |
|--------|-------------|
| `MaskedFilter` | Wraps any filter with a spatial mask: linear gradient, radial gradient, or luminance range |

### Performance (1)

| Filter | Description |
|--------|-------------|
| `FusedAdjust` | 11 per-pixel ops in one SIMD pass (exposure, contrast, H/S, dehaze, temp, tint, sat, vibrance, BP/WP) |

### Experimental (feature = `"experimental"`)

| Filter | Description |
|--------|-------------|
| `Warp` | 3×3 projective matrix transform (rotation, deskew, affine, perspective); bilinear / Catmull-Rom / Lanczos-3 |

Additionally, `dt_sigmoid` and `cat16` modules provide free functions for darktable-compatible sigmoid tone mapping and CAT16 chromatic adaptation.


## Algorithms

| Component | Algorithm | Reference |
|-----------|-----------|-----------|
| Blur (σ≥6) | SIMD stackblur, 8-column f32x8 vertical | Klingemann 2004 |
| Blur (σ<6) | Separable FIR with AVX2 FMA | — |
| Noise reduction | À trous wavelet + BayesShrink | Chang et al., IEEE TIP 2000 |
| Bilateral | Guided filter, O(1)/pixel | He et al., TPAMI 2013 |
| Tone equalizer | Guided filter mask + zone LUT | Pierre (darktable) 2019 |
| Brilliance | Smoothstep S-curve local adaptation | — |
| Tone curves | Fritsch-Carlson monotone cubic Hermite | Fritsch & Carlson 1980 |
| Contrast | Anchored power curve at Oklab middle grey | darktable basicadj |

All LUTs use 1024 entries (10-bit, 4 KB each) — balances curve fidelity against L1 cache pressure when multiple curves are active.

## Features

| Feature | Description |
|---------|-------------|
| `serde` | Serialize/deserialize all filter structs, schemas, presets, compat types |
| `srgb-compat` | ImageMagick-style `WorkingSpace::Srgb` / `WorkingSpace::LinearRgb` configs + sRGB-math filter types (`LinearContrast`, `HslSaturate`, `LumaGrayscale`, …) that operate directly on encoded values, bypassing the Oklab roundtrip |
| `srgb-filters` | Standalone sRGB u8 per-pixel filter functions (`color_adjust`, `color_matrix`, `sharpen`, `blur`) on `PixelBuffer` / `PixelSliceMut`, no Oklab roundtrip |
| `experimental` | Auto-tuning, fused interleaved path, film look gallery tool |
| `zennode` | Node graph definitions for zenpipe integration |

## Limitations

- **Single-threaded.** No rayon, no threading. Callers handle parallelism.
- **Full-frame materialization** for neighborhood filters (clarity, sharpen, noise reduction) when strip processing is not used.
- **DtSigmoid and Cat16** are utility modules with free functions, not Pipeline-composable filters.
- **Not yet implemented:** Lens Blur (depth-based bokeh), Transform/Upright (perspective correction), Lens Distortion (barrel/pincushion), Blend Layers (compositing).

## License

Dual-licensed: [AGPL-3.0](https://github.com/imazen/zenpipe/blob/main/LICENSE-AGPL3) or [commercial](https://github.com/imazen/zenpipe/blob/main/LICENSE-COMMERCIAL).

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

See [LICENSE-COMMERCIAL](https://github.com/imazen/zenpipe/blob/main/LICENSE-COMMERCIAL) for details.

## Image tech I maintain

| | |
|:--|:--|
| **Codecs** ¹ | [zenjpeg] · [zenpng] · [zenwebp] · [zengif] · [zenavif] · [zenjxl] · [zenjxl-decoder] · [jxl-encoder] · [zenbitmaps] · [heic] · [zentiff] · [zenpdf] · [zensvg] · [zenjp2] · [zenraw] · [ultrahdr] |
| Codec internals | [zenrav1e] · [rav1d-safe] · [zenravif] · [zenavif-parse] · [zenavif-serialize] |
| Compression | [zenflate] · [zenzop] · [zenzstd] |
| Processing | [zenresize] · [zenquant] · [zenblend] · **zenfilters** · [zensally] · [zentone] |
| Pixels & color | [zenpixels] · [zenpixels-convert] · [linear-srgb] · [garb] · [zenyuv] |
| Pipeline & framework | [zenpipe] · [zencodec] · [zencodecs] · [zenlayout] · [zennode] · [zenwasm] · [zentract] |
| Metrics | [zensim] · [fast-ssim2] · [butteraugli] · [zenmetrics] · [resamplescope-rs] |
| Pickers & ML | [zenanalyze] · [zenpredict] · [zenpicker] · [zenanalyze-api] |
| Test corpora | [codec-corpus] · [imazen-26] |
| Products | [Imageflow] image engine ([.NET][imageflow-dotnet] · [Node][imageflow-node] · [Go][imageflow-go]) · [Imageflow Server] · [ImageResizer] (C#) |

<sub>¹ pure-Rust, `#![forbid(unsafe_code)]` codecs, as of 2026</sub>

### General Rust awesomeness

[zenbench] · [archmage] · [magetypes] · [enough] · [whereat] · [cargo-copter] · [zenutils]

[Open source](https://www.imazen.io/open-source) · [@imazen](https://github.com/imazen) · [@lilith](https://github.com/lilith) · [lib.rs/~lilith](https://lib.rs/~lilith)

[zenjpeg]: https://github.com/imazen/zenjpeg
[zenpng]: https://github.com/imazen/zenpng
[zenwebp]: https://github.com/imazen/zenwebp
[zengif]: https://github.com/imazen/zengif
[zenavif]: https://github.com/imazen/zenavif
[zenjxl]: https://github.com/imazen/zenjxl
[zenjxl-decoder]: https://github.com/imazen/zenjxl-decoder
[jxl-encoder]: https://github.com/imazen/jxl-encoder
[zenbitmaps]: https://github.com/imazen/zenbitmaps
[heic]: https://github.com/imazen/heic
[zentiff]: https://github.com/imazen/zenextras
[zenpdf]: https://github.com/imazen/zenextras
[zensvg]: https://github.com/imazen/zenextras
[zenjp2]: https://github.com/imazen/zenextras
[zenraw]: https://github.com/imazen/zenraw
[ultrahdr]: https://github.com/imazen/ultrahdr
[zenrav1e]: https://github.com/imazen/zenrav1e
[rav1d-safe]: https://github.com/imazen/rav1d-safe
[zenravif]: https://github.com/imazen/cavif-rs
[zenavif-parse]: https://github.com/imazen/zenavif
[zenavif-serialize]: https://github.com/imazen/zenavif
[zenflate]: https://github.com/imazen/zenflate
[zenzop]: https://github.com/imazen/zenzop
[zenzstd]: https://github.com/imazen/zenzstd
[zenresize]: https://github.com/imazen/zenresize
[zenquant]: https://github.com/imazen/zenquant
[zenblend]: https://github.com/imazen/zenblend
[zensally]: https://github.com/imazen/zensally
[zentone]: https://github.com/imazen/zentone
[zenpixels]: https://github.com/imazen/zenpixels
[zenpixels-convert]: https://github.com/imazen/zenpixels
[linear-srgb]: https://github.com/imazen/linear-srgb
[garb]: https://github.com/imazen/garb
[zenyuv]: https://github.com/imazen/zenjpeg
[zenpipe]: https://github.com/imazen/zenpipe
[zencodec]: https://github.com/imazen/zencodec
[zencodecs]: https://github.com/imazen/zenpipe
[zenlayout]: https://github.com/imazen/zenpipe
[zennode]: https://github.com/imazen/zennode
[zenwasm]: https://github.com/imazen/zenwasm
[zentract]: https://github.com/imazen/zentract
[zensim]: https://github.com/imazen/zensim
[fast-ssim2]: https://github.com/imazen/fast-ssim2
[butteraugli]: https://github.com/imazen/butteraugli
[zenmetrics]: https://github.com/imazen/zenmetrics
[resamplescope-rs]: https://github.com/imazen/resamplescope-rs
[zenanalyze]: https://github.com/imazen/zenanalyze
[zenpredict]: https://github.com/imazen/zenanalyze
[zenpicker]: https://github.com/imazen/zenanalyze
[zenanalyze-api]: https://github.com/imazen/zenanalyze
[codec-corpus]: https://github.com/imazen/codec-corpus
[imazen-26]: https://github.com/imazen/imazen-26
[zenbench]: https://github.com/imazen/zenbench
[archmage]: https://github.com/imazen/archmage
[magetypes]: https://github.com/imazen/archmage
[enough]: https://github.com/imazen/enough
[whereat]: https://github.com/lilith/whereat
[cargo-copter]: https://github.com/imazen/cargo-copter
[zenutils]: https://github.com/imazen/zenutils
[Imageflow]: https://github.com/imazen/imageflow
[Imageflow Server]: https://github.com/imazen/imageflow-dotnet-server
[ImageResizer]: https://github.com/imazen/resizer
[imageflow-dotnet]: https://github.com/imazen/imageflow-dotnet
[imageflow-node]: https://github.com/imazen/imageflow-node
[imageflow-go]: https://github.com/imazen/imageflow-go
