+++
title = "Devignette"
description = "zenfilters.devignette — filter node"
weight = 400

[taxonomies]
tags = ["lens", "correction", "vignette"]

[extra]
node_id = "zenfilters.devignette"
role = "filter"
group = "effects"
stage = "Filters"
+++

Lens vignetting correction (devignette).  Compensates for the natural light falloff at the edges of a lens. Applies a radial brightness correction that increases toward the corners, based on the cos^4 law of illumination falloff.

## Parameters

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `exponent` | float (1.0 – 8.0) | 4.0 | Falloff exponent (4 = cos^4 law, higher = corners only) |

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `strength` | float (0.0 – 2.0) | 0.0 | Correction strength (1 = full cos^4 compensation) |

