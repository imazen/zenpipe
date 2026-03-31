+++
title = "Encode Webp Lossless"
description = "zenwebp.encode_lossless — encode node"
weight = 20

[taxonomies]
tags = ["webp", "lossless", "encode"]

[extra]
node_id = "zenwebp.encode_lossless"
role = "encode"
group = "encode"
+++

WebP lossless (VP8L) encode node.

## Parameters

### Alpha

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `alpha_quality` | int (0 – 100) | 0 | Alpha Quality *(optional)* |

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `effort` | float (0.0 – 100.0) | 0.0 | Compression Effort *(optional)* |
| `method` | int (0 – 6) | 0 | VP8L compression method (0 = fastest, 6 = best). Separate from effort — method controls the algorithm tier, effort controls quality within that tier. *(optional)* |
| `near_lossless` | int (0 – 100) | 0 | Near-Lossless *(optional)* |

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `exact` | bool | False | Exact *(optional)* |

### Target

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `target_size` | int (0 – 4294967295) | 0 | Target Size *(optional)* |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `webp.alpha_quality` | `webp.aq` | `alpha_quality` |
| `webp.effort` | — | `effort` |
| `webp.exact` | — | `exact` |
| `webp.method` | — | `method` |
| `webp.near_lossless` | `webp.nl` | `near_lossless` |
| `webp.target_size` | — | `target_size` |

**Example:** `?webp.alpha_quality=400&webp.effort=value&webp.exact=value`

