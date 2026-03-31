+++
title = "Hsl Adjust"
description = "zenfilters.hsl_adjust — filter node"
weight = 50

[taxonomies]
tags = ["color", "hsl"]

[extra]
node_id = "zenfilters.hsl_adjust"
role = "filter"
group = "color"
+++

Per-color hue, saturation, and luminance adjustment

## Parameters

### Hue

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `hue` | array | [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] | Hue shift per color range in degrees (°) |

### Luminance

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `luminance` | array | [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] | Luminance offset per color range |

### Saturation

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `saturation` | array | [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0] | Saturation multiplier per color range (×) |

