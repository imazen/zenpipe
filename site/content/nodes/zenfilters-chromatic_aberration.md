+++
title = "Chromatic Aberration"
description = "zenfilters.chromatic_aberration — filter node"
weight = 90

[taxonomies]
tags = ["lens", "correction", "fringing"]

[extra]
node_id = "zenfilters.chromatic_aberration"
role = "filter"
group = "effects"
+++

Lateral chromatic aberration correction.  Corrects color fringing at image edges caused by lens dispersion. In Oklab, CA manifests as radial displacement of the a (green-red) and b (blue-yellow) planes relative to L. Shifts chroma planes radially to re-align them with luminance.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `shift_a` | float (-0.019999999552965164 – 0.019999999552965164) | 0.0 | Radial shift for the a (green-red) channel |
| `shift_b` | float (-0.019999999552965164 – 0.019999999552965164) | 0.0 | Radial shift for the b (blue-yellow) channel |

