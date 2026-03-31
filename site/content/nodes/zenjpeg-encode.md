+++
title = "Encode Jpeg"
description = "zenjpeg.encode — encode node"
weight = 20

[taxonomies]
tags = ["jpeg", "jpg", "encode", "lossy"]

[extra]
node_id = "zenjpeg.encode"
role = "encode"
group = "encode"
+++

JPEG encoder configuration as a self-documenting pipeline node.  Schema-only definition for pipeline registry. Conversion to native zenjpeg config types happens in the bridge layer via `ParamMap`.

## Parameters

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `aq` | bool | True | Adaptive Quantization *(optional)* |
| `deringing` | bool | True | Deringing *(optional)* |

### Color

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `chroma_downsampling` | string | average | Chroma Downsampling *(optional)* |
| `color_space` | string | ycbcr | Color Space *(optional)* |
| `subsampling` | string | quarter | Chroma Subsampling *(optional)* |

### Quality

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `effort` | int (0 – 2) | 1 | Effort *(optional)* |
| `quality` | float (0.0 – 100.0) | 85.0 | Quality *(optional)* |

### Encoding

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `quant_tables` | string | jpegli | Quantization Tables *(optional)* |
| `scan_mode` | string | progressive | Scan Mode *(optional)* |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `jpeg.aq` | — | `aq` |
| `jpeg.chroma_method` | — | `chroma_downsampling` |
| `jpeg.colorspace` | — | `color_space` |
| `jpeg.deringing` | — | `deringing` |
| `jpeg.effort` | — | `effort` |
| `jpeg.quality` | `jpeg.q` | `quality` |
| `jpeg.tables` | — | `quant_tables` |
| `jpeg.progressive` | `jpeg.mode` | `scan_mode` |
| `jpeg.subsampling` | `jpeg.ss` | `subsampling` |

**Example:** `?jpeg.aq=True&jpeg.chroma_method=average&jpeg.colorspace=ycbcr`

