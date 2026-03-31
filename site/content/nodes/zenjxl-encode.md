+++
title = "Encode Jxl"
description = "zenjxl.encode — encode node"
weight = 20

[taxonomies]
tags = ["jxl", "jpeg-xl", "encode", "lossy", "lossless", "hdr", "codec"]

[extra]
node_id = "zenjxl.encode"
role = "encode"
group = "encode"
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
| `effort` | int (1 – 10) | 7 | Effort *(optional)* |

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

