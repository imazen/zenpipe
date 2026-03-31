+++
title = "RIAPI Querystring Reference"
description = "All supported querystring keys for image processing URLs"
weight = 20
+++

All supported querystring keys for image processing URLs.

## Quick Reference

```
?w=800&h=600&mode=crop&format=webp&qp=high&accept.webp=true
```

## Quality & Format

### Quality Intent

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `qp` | -- | string | Quality profile: "lowest", "low", "medium_low", "medium", "good", "high", "highest", "lossless", or numeric 0-100 |
| `quality` | -- | number | Legacy quality fallback (0-100). Prefer `qp` for new code |
| `format` | `thumbnail` | string | Output format: "jpeg", "png", "webp", "gif", "avif", "jxl", "keep", or "" (auto) |
| `qp.dpr` | `qp.dppx`, `dpr`, `dppx` | number | Device pixel ratio for quality adjustment |
| `lossless` | -- | string | "true", "false", or "keep" (match source) |
| `accept.webp` | -- | boolean | Allow WebP output |
| `accept.avif` | -- | boolean | Allow AVIF output |
| `accept.jxl` | -- | boolean | Allow JPEG XL output |
| `accept.color_profiles` | -- | boolean | Allow non-sRGB color profiles |

## Resize & Layout

### Constrain

| Key | Aliases | Type | Description |
|-----|---------|------|-------------|
| `w` | `width`, `maxwidth` | integer | Target width in pixels |
| `h` | `height`, `maxheight` | integer | Target height in pixels |
| `mode` | -- | string | Constraint mode: "distort", "within" (default), "fit", "within_crop"/"crop", "fit_crop", "fit_pad"/"pad", "within_pad", "pad_within", "aspect_crop", "larger_than" |
| `scale` | -- | string | "down", "up", "both" (default), "canvas" |
| `zoom` | `dpr`, `dppx` | number | Dimension multiplier (2.0 = double w and h) |
| `anchor` | -- | string | Crop/pad anchor: "center", "top_left", "top", "top_right", "left", "right", "bottom_left", "bottom", "bottom_right" |
| `bgcolor` | `canvas_color` | string | Padding fill color: "transparent", "white", "black", or hex "#RRGGBB"/"#RRGGBBAA" |
| `matte` | `matte_color`, `s.matte` | string | Alpha compositing background color |
| `down.filter` | -- | string | Downscale filter (31 available): "robidoux", "lanczos", "mitchell", etc. |
| `up.filter` | -- | string | Upscale filter (same names as down.filter) |
| `down.colorspace` | `up.colorspace` | string | Resampling color space: "linear" (default) or "srgb" |
| `f.sharpen` | `unsharp` | number | Post-resize unsharp mask (0-100) |
| `lobe_ratio` | `kernel_lobe_ratio` | number | Zero-cost kernel sharpening via negative lobe ratio |
| `resample_when` | -- | string | "size_differs" (default), "size_differs_or_sharpening_requested", "always" |
| `sharpen_when` | -- | string | "downscaling" (default), "upscaling", "size_differs", "always" |

### Geometry

| Key | Type | Description |
|-----|------|-------------|
| `srotate` | integer | EXIF orientation value (1-8) |
| `trim.threshold` | integer | Whitespace crop: color distance threshold (0-255) |
| `trim.percentpadding` | number | Padding around detected content (percentage) |
| `s.roundcorners` | number | Corner radius in pixels, or "TL,TR,BR,BL" |

## Filters

| Key | Type | Description |
|-----|------|-------------|
| `s.alpha` | number | Alpha multiplier (0 = transparent, 1 = unchanged) |
| `s.contrast` | number | Contrast strength (positive = increase, negative = flatten) |
| `s.brightness` | number | Exposure compensation in stops |
| `s.grayscale` | string | Algorithm: "oklab" (default), "ntsc", "bt709", "flat", "ry" |
| `s.invert` | boolean | Invert colors |
| `s.saturation` | number | Saturation multiplier (0 = grayscale, 1 = unchanged) |
| `s.sepia` | number | Sepia strength (0 = grayscale, 1 = full) |

## Codec-Specific Parameters

### JPEG Encode

| Key | Aliases | Description |
|-----|---------|-------------|
| `jpeg.quality` | `jpeg.q` | Quality (0-100) |
| `jpeg.effort` | -- | Compression effort |
| `jpeg.colorspace` | -- | Color space |
| `jpeg.subsampling` | `jpeg.ss` | Chroma subsampling |
| `jpeg.progressive` | `jpeg.mode` | Scan mode |
| `jpeg.tables` | -- | Quantization tables |
| `jpeg.deringing` | -- | Deringing |
| `jpeg.aq` | -- | Adaptive quantization |

### MozJPEG Encode

| Key | Aliases | Description |
|-----|---------|-------------|
| `mozjpeg.quality` | `mozjpeg.q` | Quality (0-100) |
| `mozjpeg.effort` | -- | Compression effort |
| `mozjpeg.subsampling` | `mozjpeg.ss` | Chroma subsampling |

### PNG Encode

| Key | Description |
|-----|-------------|
| `png.quality` | Quality |
| `png.min_quality` | Minimum quality |
| `png.effort` | Compression effort (0-12): 0=none, 5=thorough, 10=maniac, 12=extreme |
| `png.lossless` | Lossless mode |
| `png.max_deflate` | Maximum deflate compression |

### WebP Lossy

| Key | Aliases | Description |
|-----|---------|-------------|
| `webp.quality` | `webp.q` | Quality |
| `webp.effort` | -- | Effort |
| `webp.preset` | -- | Preset |
| `webp.sharp_yuv` | -- | Sharp YUV |
| `webp.alpha_quality` | `webp.aq` | Alpha quality |
| `webp.target_size` | -- | Target size (bytes) |
| `webp.target_psnr` | -- | Target PSNR |
| `webp.segments` | -- | Segments |
| `webp.sns` | -- | SNS strength |
| `webp.filter` | -- | Filter strength |
| `webp.sharpness` | -- | Filter sharpness |

### WebP Lossless

| Key | Aliases | Description |
|-----|---------|-------------|
| `webp.effort` | -- | Compression effort |
| `webp.method` | -- | VP8L method (0-6) |
| `webp.near_lossless` | `webp.nl` | Near-lossless |
| `webp.exact` | -- | Exact mode |
| `webp.alpha_quality` | `webp.aq` | Alpha quality |
| `webp.target_size` | -- | Target size (bytes) |

### AVIF Encode

| Key | Aliases | Description |
|-----|---------|-------------|
| `avif.q` | `avif.quality` | Quality |
| `avif.effort` | -- | Generic effort (0-10, higher = slower/better) |
| `avif.speed` | -- | rav1e speed (1-10, higher = faster, inverse of effort) |
| `avif.alpha_quality` | `avif.aq` | Alpha quality |
| `avif.depth` | -- | Bit depth |
| `avif.color_model` | -- | Color model |
| `avif.alpha_color_mode` | `avif.alpha_mode` | Alpha color mode |
| `avif.lossless` | -- | Lossless |

### JPEG XL Encode

| Key | Aliases | Description |
|-----|---------|-------------|
| `jxl.quality` | `jxl.q` | Quality |
| `jxl.distance` | `jxl.d` | Distance |
| `jxl.lossless` | -- | Lossless |
| `jxl.effort` | `jxl.e` | Effort |
| `jxl.noise` | -- | Noise |

### GIF Encode

| Key | Aliases | Description |
|-----|---------|-------------|
| `gif.quality` | -- | Palette quality |
| `gif.dithering` | `gif.dither` | Dithering |
| `gif.lossy` | -- | Lossy tolerance |
| `gif.quantizer` | -- | Quantizer |
| `gif.shared_palette` | -- | Shared palette |
| `gif.palette_threshold` | -- | Palette error threshold |
| `gif.loop` | -- | Loop mode |
| `gif.use_transparency` | `gif.transparency` | Use transparency |

### TIFF Encode

| Key | Description |
|-----|-------------|
| `tiff.compression` | Compression |
| `tiff.predictor` | Predictor |
| `tiff.big_tiff` | Big TIFF |

### BMP Encode

| Key | Aliases | Description |
|-----|---------|-------------|
| `bmp.bits` | `bits` | Bit depth |

### HEIC Decode

| Key | Description |
|-----|-------------|
| `heic.gain_map` | Extract gain map |
| `heic.depth` | Extract depth map |

### JPEG Decode

| Key | Aliases | Description |
|-----|---------|-------------|
| `jpeg.strictness` | -- | Strictness |
| `jpeg.orient` | `jpeg.auto_orient` | Auto orient |
| `jpeg.max_megapixels` | -- | Max megapixels |

### JPEG XL Decode

| Key | Description |
|-----|-------------|
| `jxl.orient` | Adjust orientation |
| `jxl.nits` | Intensity target |

### WebP Decode

| Key | Aliases | Description |
|-----|---------|-------------|
| `webp.upsampling` | -- | Upsampling |
| `webp.dithering` | `webp.dither` | Dithering strength |

### Palette Quantize

| Key | Aliases | Description |
|-----|---------|-------------|
| `quant.max_colors` | `max_colors` | Maximum colors |
| `quant.quality` | -- | Quality |
| `quant.dither_strength` | `dither_strength` | Dithering |
