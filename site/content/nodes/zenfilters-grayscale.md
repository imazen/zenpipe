+++
title = "Grayscale"
description = "zenfilters.grayscale — filter node"
weight = 400

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.grayscale"
role = "filter"
group = "color"
stage = "Filters"
+++

Convert to grayscale by zeroing chroma channels.  In Oklab, grayscale means a=0, b=0. The perceived luminance is already encoded in the L channel, so there is no information loss.


## Accepted Values

- **`algorithm`**: `oklab`, `ntsc`, `bt709`, `flat`, `ry`

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `algorithm` | string | oklab | Grayscale algorithm. All produce identical results in Oklab space (zero chroma), but different luma coefficients when applied in sRGB. Values: "oklab" (default), "ntsc", "bt709", "flat", "ry" |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `s.grayscale` | — | `algorithm` |

**Example:** `?s.grayscale=oklab`

