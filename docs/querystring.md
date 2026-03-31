# RIAPI Querystring Reference

All supported querystring keys for image processing URLs.

## Quick Reference

```
?w=800&h=600&mode=crop&format=webp&qp=high&accept.webp=true
```

### [Decode Heic](nodes/heic-decode.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `heic.gain_map` | — | boolean | Extract Gain Map |
| `heic.depth` | — | boolean | Extract Depth Map |
| `heic.mattes` | — | boolean | Placeholder — not yet wired to the decoder. |
| `heic.thumbnail` | — | boolean | Placeholder — not yet wired to the decoder. |

### [Avif Encode](nodes/zenavif-encode.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `avif.q` | `avif.quality` | number | Quality |
| `avif.effort` | — | integer | Generic effort (0 = fastest, 10 = best compression).  Inverted from rav1e speed: effort 0 = speed 10, effort 10 = speed 1. Prefer this over `speed` for codec-agnostic pipelines. |
| `avif.speed` | — | integer | rav1e-native speed (1 = slowest, 10 = fastest). Inverse of effort. |
| `avif.alpha_quality` | `avif.aq` | number | Alpha Quality |
| `avif.depth` | — | string | Bit Depth |
| `avif.color_model` | — | string | Color Model |
| `avif.alpha_color_mode` | `avif.alpha_mode` | string | Alpha Color Mode |
| `avif.lossless` | — | boolean | Lossless |

### [Encode Bmp](nodes/zenbitmaps-encode_bmp.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `bmp.bits` | `bits` | integer | Bit Depth |

### [Quality Intent Node](nodes/zencodecs-quality_intent.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `qp` | — | string | Quality profile: named preset or numeric 0-100.  Named presets: "lowest", "low", "medium_low", "medium", "good", "high", "highest", "lossless". Numeric: "0" to "100" (codec-specific mapping). |
| `quality` | — | number | Legacy quality fallback (0-100). Used when `qp` is not set.  RIAPI: `?quality=85` — sets a numeric quality as fallback for all codecs. Prefer `qp` (quality profile) for new code. |
| `format` | `thumbnail` | string | Explicit output format. Empty = auto-select from allowed formats.  Values: "jpeg", "png", "webp", "gif", "avif", "jxl", "keep", or "". "keep" preserves the source format. "thumbnail" is an alias for the key. |
| `qp.dpr` | `qp.dppx`, `dpr`, `dppx` | number | Device pixel ratio for quality adjustment.  Higher DPR screens tolerate lower quality (smaller pixels). Default 1.0 = no adjustment. |
| `lossless` | — | string | Global lossless preference. Empty = default (lossy).  Accepts "true", "false", or "keep" (match source losslessness). |
| `accept.webp` | — | boolean | Allow WebP output. Must be explicitly enabled. |
| `accept.avif` | — | boolean | Allow AVIF output. Must be explicitly enabled. |
| `accept.jxl` | — | boolean | Allow JPEG XL output. Must be explicitly enabled. |
| `accept.color_profiles` | — | boolean | Allow non-sRGB color profiles in the output. |

### [Alpha](nodes/zenfilters-alpha.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `s.alpha` | — | number | Alpha multiplier (0 = fully transparent, 1 = unchanged) |

### [Contrast](nodes/zenfilters-contrast.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `s.contrast` | — | number | Contrast strength (positive = increase, negative = flatten) |

### [Exposure](nodes/zenfilters-exposure.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `s.brightness` | — | number | Exposure compensation in stops (+/-)  Note: RIAPI `s.brightness` historically used a -1..1 sRGB offset, not photographic stops. The kv alias is provided for discoverability; callers should be aware of the different scale. |

### [Grayscale](nodes/zenfilters-grayscale.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `s.grayscale` | — | string | Grayscale algorithm. All produce identical results in Oklab space (zero chroma), but different luma coefficients when applied in sRGB. Values: "oklab" (default), "ntsc", "bt709", "flat", "ry" |

### [Invert](nodes/zenfilters-invert.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `s.invert` | — | boolean | Enable/disable. Always true when the node is present. Exists to enable RIAPI s.invert=true querystring support. |

### [Saturation](nodes/zenfilters-saturation.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `s.saturation` | — | number | Saturation multiplier (0 = grayscale, 1 = unchanged, 2 = double) |

### [Sepia](nodes/zenfilters-sepia.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `s.sepia` | — | number | Sepia strength (0 = grayscale, 1 = full sepia) |

### [Encode Gif](nodes/zengif-encode.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `gif.quality` | — | number | Palette Quality |
| `gif.dithering` | `gif.dither` | number | Dithering |
| `gif.lossy` | — | number | Lossy Tolerance |
| `gif.quantizer` | — | string | Quantizer |
| `gif.shared_palette` | — | boolean | Shared Palette |
| `gif.palette_threshold` | — | number | Palette Error Threshold |
| `gif.loop` | — | string | Loop |
| `gif.use_transparency` | `gif.transparency` | boolean | Use Transparency |

### [Decode Jpeg](nodes/zenjpeg-decode.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `jpeg.strictness` | — | string | Strictness |
| `jpeg.orient` | `jpeg.auto_orient` | boolean | Auto Orient |
| `jpeg.max_megapixels` | — | integer | Max Megapixels |

### [Encode Jpeg](nodes/zenjpeg-encode.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `jpeg.quality` | `jpeg.q` | number | Quality |
| `jpeg.effort` | — | integer | Effort |
| `jpeg.colorspace` | — | string | Color Space |
| `jpeg.subsampling` | `jpeg.ss` | string | Chroma Subsampling |
| `jpeg.chroma_method` | — | string | Chroma Downsampling |
| `jpeg.progressive` | `jpeg.mode` | string | Scan Mode |
| `jpeg.tables` | — | string | Quantization Tables |
| `jpeg.deringing` | — | boolean | Deringing |
| `jpeg.aq` | — | boolean | Adaptive Quantization |

### [Encode Mozjpeg](nodes/zenjpeg-encode_mozjpeg.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `mozjpeg.quality` | `mozjpeg.q` | number | Quality |
| `mozjpeg.effort` | — | integer | Effort |
| `mozjpeg.subsampling` | `mozjpeg.ss` | string | Chroma Subsampling |

### [Decode Jxl](nodes/zenjxl-decode.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `jxl.orient` | — | boolean | Adjust Orientation |
| `jxl.nits` | — | number | Intensity Target |

### [Encode Jxl](nodes/zenjxl-encode.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `jxl.quality` | `jxl.q` | number | JXL Quality |
| `jxl.distance` | `jxl.d` | number | Distance |
| `jxl.lossless` | — | boolean | Lossless |
| `jxl.effort` | `jxl.e` | integer | Effort |
| `jxl.noise` | — | boolean | Noise |

### [Orient](nodes/zenlayout-orient.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `srotate` | — | integer | EXIF orientation value (1-8). 1 = no transformation. |

### [Crop Whitespace](nodes/zenpipe-crop_whitespace.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `trim.threshold` | — | integer | Color distance threshold (0–255).  Pixels within this distance of the border color are considered "whitespace". Lower = stricter, higher = more tolerant. |
| `trim.percentpadding` | — | number | Padding around detected content as a percentage of content dimensions.  0.0 = tight crop, 0.5 = 0.5% padding on each side. |

### [Round Corners](nodes/zenpipe-round_corners.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `s.roundcorners` | — | number | Corner radius in pixels (uniform). Clamped to min(width, height) / 2. Used when mode is "pixels" (default) or as fallback.  RIAPI: `?s.roundcorners=20` (single value) or `?s.roundcorners=10,20,30,40` (TL,TR,BR,BL) |

### [Encode Png](nodes/zenpng-encode.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `png.quality` | — | integer | PNG Quality |
| `png.min_quality` | — | integer | Min Quality |
| `png.effort` | — | integer | Generic effort (0 = no compression, 12 = extreme).  Maps to zenpng's Compression enum: 0 = None, 1 = Fastest, 2 = Turbo, 3 = Fast, 4 = Balanced, 5 = Thorough, 6 = High, 7 = Aggressive, 8 = Intense, 9 = Crush, 10 = Maniac, 11 = Brag, 12+ = Minutes. |
| `png.lossless` | — | boolean | Lossless |
| `png.max_deflate` | — | boolean | Max Deflate |

### [Quantize](nodes/zenquant-quantize.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `quant.max_colors` | `max_colors` | integer | Max Colors |
| `quant.quality` | — | string | Quality |
| `quant.dither_strength` | `dither_strength` | number | Dithering |

### [Constrain](nodes/zenresize-constrain.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `w` | `width`, `maxwidth` | integer | Target width in pixels. None = unconstrained (derive from height + aspect ratio).  RIAPI: `?w=800` or `?width=800` or `?maxwidth=800` (legacy, implies mode=within). |
| `h` | `height`, `maxheight` | integer | Target height in pixels. None = unconstrained (derive from width + aspect ratio).  RIAPI: `?h=600` or `?height=600` or `?maxheight=600` (legacy, implies mode=within). |
| `mode` | — | string | Constraint mode controlling how the image fits the target dimensions.  - "distort" / "stretch" — stretch to exact dimensions, ignoring aspect ratio - "within" / "max" — fit inside target, never upscale (default) - "fit" — fit inside target, may upscale - "within_crop" / "crop" — fill target by cropping, never upscale - "fit_crop" — fill target by cropping, may upscale - "fit_pad" / "pad" — fit inside target, pad to exact dimensions - "within_pad" — fit inside target without upscale, pad to exact dimensions - "pad_within" — never upscale, always pad to exact canvas - "aspect_crop" / "aspectcrop" — crop to target aspect ratio without resizing - "larger_than" — upscale if needed to meet target, never downscale |
| `scale` | — | string | Scale control: when to allow scaling.  - "down" / "downscaleonly" — only downscale, never upscale - "up" / "upscaleonly" — only upscale, never downscale - "both" — allow both (default) - "canvas" / "upscalecanvas" — upscale canvas (pad) only  RIAPI: `?scale=down` |
| `zoom` | `dpr`, `dppx` | number | Device pixel ratio / zoom multiplier.  Multiplies target dimensions. 2.0 means w and h are doubled. RIAPI: `?zoom=2`, `?dpr=2x`, `?dppx=1.5` |
| `anchor` | — | string | Named anchor for crop/pad positioning.  Controls which part of the image is preserved when cropping, or where the image is positioned when padding.  Values: "center", "top_left", "top", "top_right", "left", "right", "bottom_left", "bottom", "bottom_right".  Overridden by `gravity_x`/`gravity_y` when both are set. |
| `bgcolor` | `canvas_color` | string | Fill color for exterior padding regions added around the image.  Used by pad modes (fit_pad, within_pad, pad_within) to fill the canvas area outside the image content. Does NOT affect pixels inside the image — use `matte_color` for alpha compositing.  None = transparent. Accepts "transparent", "white", "black", or hex "#RRGGBB" / "#RRGGBBAA". |
| `matte` | `matte_color`, `s.matte` | string | Background color for alpha compositing (matte behind transparent pixels).  Applied during resize to prevent halo artifacts at transparent edges. Separate from `canvas_color` which fills exterior padding regions. When set, transparent pixels are composited against this color before the resampling kernel samples them.  None = no matte (preserve transparency). "white" is common for JPEG output. |
| `down.filter` | — | string | Downscale resampling filter. None = auto (Robidoux).  31 filters available: "robidoux", "robidoux_sharp", "robidoux_fast", "lanczos", "lanczos_sharp", "lanczos2", "lanczos2_sharp", "ginseng", "ginseng_sharp", "mitchell", "catmull_rom", "cubic", "cubic_sharp", "cubic_b_spline", "hermite", "triangle", "linear", "box", "fastest", "jinc", "n_cubic", "n_cubic_sharp", etc. |
| `up.filter` | — | string | Upscale resampling filter. None = auto (Ginseng).  Same filter names as down_filter. |
| `down.colorspace` | `up.colorspace` | string | Color space for resampling math. None = auto ("linear" for most operations).  - "linear" — resize in linear light (gamma-correct, default) - "srgb" — resize in sRGB gamma space (faster, less correct)  RIAPI: `?down.colorspace=linear` or `?up.colorspace=srgb` |
| `f.sharpen` | `unsharp` | number | Post-resize unsharp mask strength (0 = none, 100 = maximum).  Applied as a separate pass AFTER resampling. Adds real computational cost proportional to output dimensions. For zero-cost sharpening that adjusts the resampling kernel itself, use `kernel_lobe_ratio`.  None = no unsharp mask. |
| `lobe_ratio` | `kernel_lobe_ratio` | number | Negative-lobe ratio for kernel sharpening (zero-cost).  Adjusts the resampling kernel's negative lobes during weight computation. 0.0 = flatten (maximum smoothness), above filter's natural ratio = sharpen. Zero additional cost — changes the filter shape, not a separate processing step.  For post-resize unsharp mask (separate pass), use `unsharp_percent`. |
| `resample_when` | — | string | When to apply resampling.  - "size_differs" — only resample when dimensions change (default) - "size_differs_or_sharpening_requested" — resample when dimensions change or when sharpening is requested (allows sharpening without resize) - "always" — always resample, even at identity dimensions |
| `sharpen_when` | — | string | When to apply sharpening.  - "downscaling" — sharpen only when downscaling (default) - "upscaling" — sharpen only when upscaling - "size_differs" — sharpen whenever dimensions change - "always" — always sharpen, even at identity dimensions |

### [Encode Tiff](nodes/zentiff-encode.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `tiff.compression` | — | string | Compression |
| `tiff.predictor` | — | string | Predictor |
| `tiff.big_tiff` | — | boolean | Big Tiff |

### [Decode Webp](nodes/zenwebp-decode.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `webp.upsampling` | — | string | Upsampling |
| `webp.dithering` | `webp.dither` | integer | Dithering Strength |

### [Encode Webp Lossless](nodes/zenwebp-encode_lossless.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `webp.effort` | — | integer | Generic effort (0 = fastest, 10 = best compression).  Maps to WebP method 0-6 internally. |
| `webp.method` | — | integer | VP8L compression method (0 = fastest, 6 = best). Separate from effort — method controls the algorithm tier, effort controls quality within that tier. |
| `webp.near_lossless` | `webp.nl` | integer | Near-Lossless |
| `webp.exact` | — | boolean | Exact |
| `webp.alpha_quality` | `webp.aq` | integer | Alpha Quality |
| `webp.target_size` | — | integer | Target Size |

### [Encode Webp Lossy](nodes/zenwebp-encode_lossy.md)

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `webp.quality` | `webp.q` | number | Quality |
| `webp.effort` | — | integer | Generic effort (0 = fastest, 10 = best compression).  Maps to WebP method 0-6 internally. |
| `webp.preset` | — | string | Preset |
| `webp.sharp_yuv` | — | boolean | Sharp YUV |
| `webp.alpha_quality` | `webp.aq` | integer | Alpha Quality |
| `webp.target_size` | — | integer | Target Size |
| `webp.target_psnr` | — | number | Target PSNR |
| `webp.segments` | — | integer | Segments |
| `webp.sns` | — | integer | SNS Strength |
| `webp.filter` | — | integer | Filter Strength |
| `webp.sharpness` | — | integer | Filter Sharpness |
