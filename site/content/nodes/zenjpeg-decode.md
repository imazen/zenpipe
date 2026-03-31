+++
title = "Decode Jpeg"
description = "zenjpeg.decode — decode node"
weight = 10

[taxonomies]
tags = ["jpeg", "jpg", "decode"]

[extra]
node_id = "zenjpeg.decode"
role = "decode"
group = "decode"
+++

JPEG decoder configuration.

## Parameters

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `auto_orient` | bool | False | Auto Orient |
| `strictness` | string | balanced | Strictness |

### Limits

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `max_megapixels` | int (0 – 10000) | 100 | Max Megapixels (MP) *(optional)* |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `jpeg.orient` | `jpeg.auto_orient` | `auto_orient` |
| `jpeg.max_megapixels` | — | `max_megapixels` |
| `jpeg.strictness` | — | `strictness` |

**Example:** `?jpeg.orient=value&jpeg.max_megapixels=100&jpeg.strictness=balanced`

