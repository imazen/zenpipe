+++
title = "Decode Webp"
description = "zenwebp.decode — decode node"
weight = 100

[taxonomies]
tags = ["webp", "decode"]

[extra]
node_id = "zenwebp.decode"
role = "decode"
group = "decode"
stage = "Decode"
+++

WebP decode node.


## Accepted Values

- **`upsampling`**: `bilinear`, `fancy`

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `dithering_strength` | int (0 – 100) | 0 | Dithering Strength *(optional)* |
| `upsampling` | string | — | Upsampling *(optional)* |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `webp.dithering` | `webp.dither` | `dithering_strength` |
| `webp.upsampling` | — | `upsampling` |

**Example:** `?webp.dithering=400&webp.upsampling=value`

