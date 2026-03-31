+++
title = "Grain"
description = "zenfilters.grain — filter node"
weight = 400

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.grain"
role = "filter"
group = "effects"
stage = "Filters"
+++

Film grain simulation with luminance-adaptive response.  Adds synthetic grain to the luminance channel. Grain intensity varies with luminance: stronger in midtones, weaker in deep shadows and bright highlights, like real film.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `amount` | float (0.0 – 1.0) | 0.0 | Grain intensity (0 = none, 1 = heavy) |
| `seed` | int (0 – 65535) | 0 | Random seed for grain pattern |
| `size` | float (1.0 – 5.0) | 1.0 | Grain spatial frequency (1 = fine, 2+ = coarser) (px) |

