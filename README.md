# zenfilters ![CI](https://img.shields.io/github/actions/workflow/status/imazen/zenfilters/ci.yml?style=for-the-badge) ![MSRV](https://img.shields.io/badge/MSRV-1.93-blue?style=for-the-badge) ![License](https://img.shields.io/badge/license-AGPL--3.0--only%20OR%20Commercial-blue?style=for-the-badge)

Photo filter pipeline in Oklab perceptual color space with SIMD acceleration via [archmage](https://github.com/imazen/archmage).

55+ filters with broad coverage of Lightroom and darktable adjustments for tone, color, detail, and effects. 34 built-in film look presets using tensor-compressed 3D LUTs (163 KB total). ASC CDL, .cube LUT loading, hue-qualified curves. Self-describing parameter schemas for automatic UI generation.

Rust 1.93+, 2024 edition. `#![forbid(unsafe_code)]` — entirely safe Rust.

**[Browse the Film Look Gallery](https://imazen.github.io/zenfilters/)** — interactive before/after comparisons for all 34 presets.

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

### Slider mappings

Some parameters have non-linear perceptual response. The `slider` module provides mapping functions so equal slider increments produce equal perceived changes:

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

## Performance

All numbers single-threaded on x86-64 AVX2.

| Pipeline | 2K (1920×1080) | 4K (3840×2160) | 8K (7680×4320) |
|----------|---------------|----------------|----------------|
| Per-pixel (FusedAdjust) | 9 ms | 37 ms | 147 ms |
| Clarity (blur + unsharp) | 20 ms | 88 ms | 418 ms |
| Realistic (adjust + clarity + sharpen) | 39 ms | 174 ms | 773 ms |
| Heavy (clarity + texture + NR) | 87 ms | 413 ms | 1.6 s |

Blur performance (σ=16, SIMD stackblur):
| Resolution | Time | Throughput |
|-----------|------|-----------|
| 2K | 4.0 ms | 520 Mpx/s |
| 4K | 28 ms | 295 Mpx/s |
| 8K | 111 ms | 300 Mpx/s |

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
| `srgb-filters` | Direct sRGB u8 per-pixel filters (no Oklab roundtrip) |
| `experimental` | Auto-tuning, fused interleaved path, film look gallery tool |
| `zennode` | Node graph definitions for zenpipe integration |

## Limitations

- **Single-threaded.** No rayon, no threading. Callers handle parallelism.
- **Full-frame materialization** for neighborhood filters (clarity, sharpen, noise reduction) when strip processing is not used.
- **DtSigmoid and Cat16** are utility modules with free functions, not Pipeline-composable filters.
- **Not yet implemented:** Lens Blur (depth-based bokeh), Transform/Upright (perspective correction), Lens Distortion (barrel/pincushion), Blend Layers (compositing).

## Image tech I maintain

| | |
|:--|:--|
| State of the art codecs<sup>[1]</sup> | [zenjpeg] · [zenpng] · [zenwebp] · [zengif] · [zenavif] ([rav1d-safe] · [zenrav1e] · [zenavif-parse] · [zenavif-serialize]) · [zenjxl] ([jxl-encoder] · [zenjxl-decoder]) · [zentiff] · [zenbitmaps] · [heic] · [zenraw] · [zenpdf] · [ultrahdr] · [mozjpeg-rs] · [webpx] |
| Compression | [zenflate] · [zenzop] |
| Processing | [zenresize] · **zenfilters** · [zenquant] · [zenblend] |
| Metrics | [zensim] · [fast-ssim2] · [butteraugli] · [resamplescope-rs] · [codec-eval] · [codec-corpus] |
| Pixel types & color | [zenpixels] · [zenpixels-convert] · [linear-srgb] · [garb] |
| Pipeline | [zenpipe] · [zencodec] · [zencodecs] · [zenlayout] · [zennode] |
| ImageResizer | [ImageResizer] (C#) — 24M+ NuGet downloads across all packages |
| [Imageflow][] | Image optimization engine (Rust) — [.NET][imageflow-dotnet] · [node][imageflow-node] · [go][imageflow-go] — 9M+ NuGet downloads across all packages |
| [Imageflow Server][] | [The fast, safe image server](https://www.imazen.io/) (Rust+C#) — 552K+ NuGet downloads, deployed by Fortune 500s and major brands |

<sup>[1]</sup> <sub>as of 2026</sub>

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
[zenlayout]: https://github.com/imazen/zenlayout
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
