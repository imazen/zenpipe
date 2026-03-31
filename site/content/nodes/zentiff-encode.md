+++
title = "Encode Tiff"
description = "zentiff.encode — encode node"
weight = 700

[taxonomies]
tags = ["codec", "tiff", "lossless", "encode"]

[extra]
node_id = "zentiff.encode"
role = "encode"
group = "encode"
stage = "Encode"
+++

TIFF encoding with compression and predictor options.


## Accepted Values

- **`compression`**: `uncompressed`, `lzw`, `deflate`, `packbits`
- **`predictor`**: `none`, `horizontal`

## Parameters

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `big_tiff` | bool | False | Big Tiff |

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `compression` | string | lzw | Compression |
| `predictor` | string | horizontal | Predictor |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `tiff.big_tiff` | — | `big_tiff` |
| `tiff.compression` | — | `compression` |
| `tiff.predictor` | — | `predictor` |

**Example:** `?tiff.big_tiff=value&tiff.compression=lzw&tiff.predictor=horizontal`

