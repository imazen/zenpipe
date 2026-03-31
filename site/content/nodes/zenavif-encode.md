+++
title = "Avif Encode"
description = "zenavif.encode — encode node"
weight = 700

[taxonomies]
tags = ["avif", "encode", "av1"]

[extra]
node_id = "zenavif.encode"
role = "encode"
group = "encode"
stage = "Encode"
+++

AVIF encoding node.


## Accepted Values

- **`bit_depth`**: `auto`, `8`, `10`, `12`
- **`color_model`**: `ycbcr`, `rgb`
- **`alpha_color_mode`**: `clean`, `dirty`, `premultiplied`

## Parameters

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `alpha_color_mode` | string | clean | Alpha Color Mode *(optional)* |
| `color_model` | string | ycbcr | Color Model *(optional)* |
| `lossless` | bool | False | Lossless *(optional)* |

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `alpha_quality` | float (0.0 – 100.0) | 0.0 | Alpha Quality *(optional)* |
| `bit_depth` | string | auto | Bit Depth *(optional)* |
| `quality` | float (1.0 – 100.0) | 75.0 | Quality *(optional)* |
| `speed` | int (1 – 10) | 4 | Speed *(optional)* |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `avif.alpha_color_mode` | `avif.alpha_mode` | `alpha_color_mode` |
| `avif.alpha_quality` | `avif.aq` | `alpha_quality` |
| `avif.depth` | — | `bit_depth` |
| `avif.color_model` | — | `color_model` |
| `avif.lossless` | — | `lossless` |
| `avif.q` | `avif.quality` | `quality` |
| `avif.speed` | — | `speed` |

**Example:** `?avif.alpha_color_mode=clean&avif.alpha_quality=value&avif.depth=auto`

