+++
title = "Parametric Curve"
description = "zenfilters.parametric_curve — filter node"
weight = 400

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.parametric_curve"
role = "filter"
group = "tone"
stage = "Filters"
+++

Parametric tone curve with 4 zone controls and 3 movable dividers.  Zone-based control similar to Lightroom's parametric tone curve panel. Each zone slider pushes the curve up or down within its region.

## Parameters

### Zones

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `darks` | float (-1.0 – 1.0) | 0.0 | Darks (lower midtones) zone adjustment |
| `highlights` | float (-1.0 – 1.0) | 0.0 | Highlights zone adjustment |
| `lights` | float (-1.0 – 1.0) | 0.0 | Lights (upper midtones) zone adjustment |
| `shadows` | float (-1.0 – 1.0) | 0.0 | Shadows zone adjustment |

### Splits

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `split_highlights` | float (0.550000011920929 – 0.949999988079071) | 0.75 | Boundary between lights and highlights zones |
| `split_midtones` | float (0.30000001192092896 – 0.75) | 0.5 | Boundary between darks and lights zones |
| `split_shadows` | float (0.05000000074505806 – 0.44999998807907104) | 0.25 | Boundary between shadows and darks zones |

