+++
title = "Encode Png"
description = "zenpng.encode — encode node"
weight = 20

[taxonomies]
tags = ["codec", "png", "lossless", "encode"]

[extra]
node_id = "zenpng.encode"
role = "encode"
group = "encode"
+++

PNG encoding with quality, lossless mode, and compression options.

## Parameters

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `lossless` | bool | True | Lossless *(optional)* |
| `min_quality` | int (0 – 100) | 0 | Min Quality *(optional)* |
| `png_quality` | int (0 – 100) | 0 | PNG Quality *(optional)* |

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `max_deflate` | bool | False | Max Deflate *(optional)* |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `png.lossless` | — | `lossless` |
| `png.max_deflate` | — | `max_deflate` |
| `png.min_quality` | — | `min_quality` |
| `png.quality` | — | `png_quality` |

**Example:** `?png.lossless=True&png.max_deflate=value&png.min_quality=400`

