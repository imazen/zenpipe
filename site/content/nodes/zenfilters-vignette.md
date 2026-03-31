+++
title = "Vignette"
description = "zenfilters.vignette — filter node"
weight = 400

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.vignette"
role = "filter"
group = "effects"
stage = "Filters"
+++

Post-crop vignette: darken or lighten image edges.  Applies a radial falloff from center to edges. Positive strength darkens edges (classic vignette), negative brightens.

## Parameters

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `feather` | float (0.0 – 1.0) | 0.5 | Transition softness (0 = hard, 1 = very soft) |
| `midpoint` | float (0.0 – 1.0) | 0.5 | Distance from center where effect starts (0 = center, 1 = corners) |
| `strength` | float (-1.0 – 1.0) | 0.0 | Vignette strength (positive = darken edges, negative = brighten) |

### Shape

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `roundness` | float (0.0 – 1.0) | 1.0 | Shape (1 = circular, 0 = rectangular) |

