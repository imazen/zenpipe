+++
title = "Bloom"
description = "zenfilters.bloom — filter node"
weight = 90

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.bloom"
role = "filter"
group = "effects"
+++

Soft glow from bright areas via screen blending.  Extracts pixels above a luminance threshold, blurs them with a large Gaussian kernel, and adds the result back. Produces natural-looking soft glow around bright light sources.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `amount` | float (0.0 – 1.0) | 0.0 | Bloom intensity (0 = off, 1 = full) |
| `sigma` | float (2.0 – 100.0) | 20.0 | Bloom spread (larger = softer, wider glow) (px) |
| `threshold` | float (0.0 – 1.0) | 0.699999988079071 | Luminance threshold for bloom contribution |

