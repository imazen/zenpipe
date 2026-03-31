+++
title = "Color Grading"
description = "zenfilters.color_grading — filter node"
weight = 50

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.color_grading"
role = "filter"
group = "color"
+++

Three-way split-toning for shadows, midtones, and highlights.  Applies different color tints to shadows, midtones, and highlights independently. Colors are specified as Oklab a/b offsets.

## Parameters

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `balance` | float (-1.0 – 1.0) | 0.0 | Balance: shifts the shadow/highlight boundary |

### Highlights

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `highlight_a` | float (-0.10000000149011612 – 0.10000000149011612) | 0.0 | Highlight tint: Oklab a offset |
| `highlight_b` | float (-0.10000000149011612 – 0.10000000149011612) | 0.0 | Highlight tint: Oklab b offset |

### Midtones

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `midtone_a` | float (-0.10000000149011612 – 0.10000000149011612) | 0.0 | Midtone tint: Oklab a offset |
| `midtone_b` | float (-0.10000000149011612 – 0.10000000149011612) | 0.0 | Midtone tint: Oklab b offset |

### Shadows

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `shadow_a` | float (-0.10000000149011612 – 0.10000000149011612) | 0.0 | Shadow tint: Oklab a offset (green-magenta) |
| `shadow_b` | float (-0.10000000149011612 – 0.10000000149011612) | 0.0 | Shadow tint: Oklab b offset (blue-yellow) |

