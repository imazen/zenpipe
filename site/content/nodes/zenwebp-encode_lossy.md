+++
title = "Encode Webp Lossy"
description = "zenwebp.encode_lossy — encode node"
weight = 700

[taxonomies]
tags = ["webp", "lossy", "encode"]

[extra]
node_id = "zenwebp.encode_lossy"
role = "encode"
group = "encode"
stage = "Encode"
+++

WebP lossy (VP8) encode node.


## Accepted Values

- **`preset`**: `default`, `picture`, `photo`, `drawing`, `icon`, `text`

## Parameters

### Alpha

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `alpha_quality` | int (0 – 100) | 0 | Alpha Quality *(optional)* |

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `effort` | float (0.0 – 100.0) | 0.0 | Effort *(optional)* |
| `preset` | string | — | Preset *(optional)* |
| `quality` | float (0.0 – 100.0) | 0.0 | Quality *(optional)* |
| `sharp_yuv` | bool | False | Sharp YUV *(optional)* |

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `filter_sharpness` | int (0 – 7) | 0 | Filter Sharpness *(optional)* |
| `filter_strength` | int (0 – 100) | 0 | Filter Strength *(optional)* |
| `segments` | int (1 – 4) | 0 | Segments *(optional)* |
| `sns_strength` | int (0 – 100) | 0 | SNS Strength *(optional)* |

### Target

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `target_psnr` | float (0.0 – 100.0) | 0.0 | Target PSNR *(optional)* |
| `target_size` | int (0 – 4294967295) | 0 | Target Size *(optional)* |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `webp.alpha_quality` | `webp.aq` | `alpha_quality` |
| `webp.effort` | — | `effort` |
| `webp.sharpness` | — | `filter_sharpness` |
| `webp.filter` | — | `filter_strength` |
| `webp.preset` | — | `preset` |
| `webp.quality` | `webp.q` | `quality` |
| `webp.segments` | — | `segments` |
| `webp.sharp_yuv` | — | `sharp_yuv` |
| `webp.sns` | — | `sns_strength` |
| `webp.target_psnr` | — | `target_psnr` |
| `webp.target_size` | — | `target_size` |

**Example:** `?webp.alpha_quality=400&webp.effort=value&webp.sharpness=400`

