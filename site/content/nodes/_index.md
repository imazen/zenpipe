+++
title = "Node Reference"
description = "All 91 pipeline nodes — decode, encode, resize, filter, composite, and more"
sort_by = "weight"
template = "section.html"
+++

Auto-generated from [zennode](https://github.com/imazen/zennode) schemas. Each node documents its parameters, defaults, and RIAPI querystring keys.

## Pipeline Flow

```
Decode → Orient → Crop/Region → Resize/Constrain → Filters → Composite → Encode
```

## Node Groups

| Group | Count | Description |
|-------|-------|-------------|
| **geometry** | 16 | Crop, flip, rotate, orient, region, constrain, resize |
| **color** | 15 | Saturation, grayscale, sepia, HSL, color grading, channel curves |
| **encode** | 11 | JPEG, PNG, WebP, AVIF, JXL, GIF, TIFF, BMP |
| **detail** | 10 | Sharpen, blur, clarity, noise reduction, bilateral, edge detect |
| **effects** | 8 | Alpha, bloom, chromatic aberration, grain, vignette, dehaze |
| **tone_range** | 8 | Highlights/shadows, black/white point, tone equalizer, local tone map |
| **tone** | 6 | Contrast, exposure, fused adjust, parametric curve, sigmoid, tone curve |
| **decode** | 5 | JPEG, WebP, JXL, HEIC |
| **tone_map** | 3 | Basecurve, dt_sigmoid, levels |
| **canvas** | 3 | Expand canvas, fill rect, round corners |
| **composite** | 2 | Composite, overlay |
| **analysis** | 1 | Crop whitespace |
| **auto** | 1 | Auto exposure |
| **layout** | 1 | Output limits |
| **quantize** | 1 | Palette quantization |

Browse the sidebar or use search to find specific nodes.
