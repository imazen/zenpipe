+++
title = "Sigmoid"
description = "zenfilters.sigmoid — filter node"
weight = 60

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.sigmoid"
role = "filter"
group = "tone"
+++

S-curve tone mapping with skew and chroma compression.  Uses the generalized sigmoid f(x) = x^c / (x^c + (1-x)^c). Contrast controls steepness, skew shifts the midpoint, and chroma_compression adapts saturation to luminance changes.

## Parameters

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `chroma_compression` | float (0.0 – 1.0) | 0.0 | How much chroma adapts to luminance changes (0 = L-only, 1 = full) |

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `contrast` | float (0.5 – 3.0) | 1.0 | S-curve steepness (1 = identity, >1 = more contrast) |
| `skew` | float (0.10000000149011612 – 0.8999999761581421) | 0.5 | Midpoint bias (0.5 = symmetric, <0.5 = darken, >0.5 = brighten) |

