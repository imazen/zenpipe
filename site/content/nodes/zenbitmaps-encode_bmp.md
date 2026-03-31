+++
title = "Encode Bmp"
description = "zenbitmaps.encode_bmp — encode node"
weight = 700

[taxonomies]
tags = ["codec", "bmp", "lossless", "encode"]

[extra]
node_id = "zenbitmaps.encode_bmp"
role = "encode"
group = "encode"
stage = "Encode"
+++

BMP encoding with bit depth selection.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `bits` | int (1 – 32) | 24 | Bit Depth (bits) |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `bmp.bits` | `bits` | `bits` |

**Example:** `?bmp.bits=24`

