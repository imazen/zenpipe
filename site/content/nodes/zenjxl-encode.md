+++
title = "Encode Jxl"
description = "zenjxl.encode — encode node"
weight = 700

[taxonomies]
tags = ["jxl", "jpeg-xl", "encode", "lossy", "lossless", "hdr", "codec"]

[extra]
node_id = "zenjxl.encode"
role = "encode"
group = "encode"
stage = "Encode"
+++

JPEG XL encoder configuration.

## Parameters

### Quality

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `distance` | float (0.0 – 25.0) | 1.0 | Distance (butteraugli) *(optional)* |
| `jxl_quality` | float (0.0 – 100.0) | 75.0 | JXL Quality *(optional)* |

### Speed

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `effort` | int (0 – 10) | 7 | Generic effort (0 = fastest, 10 = best compression).  Mapped to libjxl speed tiers: 0-1 map to tier 1 (Lightning), 10 maps to tier 10 (Tortoise). *(optional)* |

### Mode

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `lossless` | bool | False | Lossless *(optional)* |

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `noise` | bool | False | Noise *(optional)* |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `jxl.distance` | `jxl.d` | `distance` |
| `jxl.effort` | `jxl.e` | `effort` |
| `jxl.quality` | `jxl.q` | `jxl_quality` |
| `jxl.lossless` | — | `lossless` |
| `jxl.noise` | — | `noise` |

**Example:** `?jxl.distance=1.0&jxl.effort=7&jxl.quality=75.0`

