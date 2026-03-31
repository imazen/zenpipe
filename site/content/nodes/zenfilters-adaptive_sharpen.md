+++
title = "Adaptive Sharpen"
description = "zenfilters.adaptive_sharpen — filter node"
weight = 40

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.adaptive_sharpen"
role = "filter"
group = "detail"
+++

Noise-gated sharpening with detail and masking controls.  Measures local texture energy and only sharpens where there is actual detail to enhance, leaving flat areas (sky, skin) unaffected.

## Parameters

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `amount` | float (0.0 – 2.0) | 0.0 | Sharpening strength (×) |
| `detail` | float (0.0 – 1.0) | 0.5 | Edge-only (0) to full detail (1) sharpening |
| `sigma` | float (0.5 – 3.0) | 1.0 | Detail extraction scale (smaller = finer detail) (px) |

### Masking

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `masking` | float (0.0 – 1.0) | 0.0 | Restrict sharpening to stronger edges |

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `noise_floor` | float (0.0010000000474974513 – 0.019999999552965164) | 0.004999999888241291 | Threshold below which detail is treated as noise |

