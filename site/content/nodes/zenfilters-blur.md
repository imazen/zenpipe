+++
title = "Blur"
description = "zenfilters.blur — filter node"
weight = 40

[taxonomies]
tags = ["blur", "smooth", "gaussian"]

[extra]
node_id = "zenfilters.blur"
role = "filter"
group = "detail"
+++

Full-image Gaussian blur across all Oklab channels.  Unlike the L-only blur used internally by clarity/sharpen, this blurs the entire image (L, a, b, and alpha). Blurring in Oklab avoids the darkening artifacts that sRGB gamma-space blurs produce at color boundaries.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `sigma` | float (0.0 – 100.0) | 0.0 | Gaussian sigma in pixels (larger = more blur) (σ) |

