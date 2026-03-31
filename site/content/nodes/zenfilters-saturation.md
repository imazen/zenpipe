+++
title = "Saturation"
description = "zenfilters.saturation — filter node"
weight = 50

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.saturation"
role = "filter"
group = "color"
+++

Uniform chroma scaling on Oklab a/b channels.  Scales chroma by a constant factor. 1.0 = no change, 0.0 = grayscale, 2.0 = double saturation.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `factor` | float (0.0 – 2.0) | 1.0 | Saturation multiplier (0 = grayscale, 1 = unchanged, 2 = double) (×) |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `s.saturation` | — | `factor` |

**Example:** `?s.saturation=1.0`

