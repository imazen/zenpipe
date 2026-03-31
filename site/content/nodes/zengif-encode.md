+++
title = "Encode Gif"
description = "zengif.encode — encode node"
weight = 20

[taxonomies]
tags = ["gif", "encode", "animation", "palette"]

[extra]
node_id = "zengif.encode"
role = "encode"
group = "encode"
+++

GIF encoder settings.

## Parameters

### Quality

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `dithering` | float (0.0 – 1.0) | 0.5 | Dithering *(optional)* |
| `lossy_tolerance` | float (0.0 – 255.0) | 0.0 | Lossy Tolerance *(optional)* |
| `quality` | float (1.0 – 100.0) | 80.0 | Palette Quality *(optional)* |

### Animation

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `loop_count` | string | infinite | Loop |
| `palette_error_threshold` | float (0.0 – 50.0) | 5.0 | Palette Error Threshold *(optional)* |
| `shared_palette` | bool | True | Shared Palette *(optional)* |

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `quantizer` | string | auto | Quantizer |
| `use_transparency` | bool | True | Use Transparency *(optional)* |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `gif.dithering` | `gif.dither` | `dithering` |
| `gif.loop` | — | `loop_count` |
| `gif.lossy` | — | `lossy_tolerance` |
| `gif.palette_threshold` | — | `palette_error_threshold` |
| `gif.quality` | — | `quality` |
| `gif.quantizer` | — | `quantizer` |
| `gif.shared_palette` | — | `shared_palette` |
| `gif.use_transparency` | `gif.transparency` | `use_transparency` |

**Example:** `?gif.dithering=0.5&gif.loop=infinite&gif.lossy=value`

