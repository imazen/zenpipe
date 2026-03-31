+++
title = "Crop Margins"
description = "zenlayout.crop_margins — orient node"
weight = 30

[taxonomies]
tags = ["crop", "margins", "geometry"]

[extra]
node_id = "zenlayout.crop_margins"
role = "orient"
group = "geometry"
+++

Crop the image by removing percentage-based margins from each side.  Each value is a fraction of the corresponding source dimension to remove. CSS-style ordering: top, right, bottom, left.  JSON: `{ "top": 0.1, "right": 0.05, "bottom": 0.1, "left": 0.05 }`

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `bottom` | float (0.0 – 0.5) | 0.0 | Bottom margin as fraction of source height to remove. |
| `left` | float (0.0 – 0.5) | 0.0 | Left margin as fraction of source width to remove. |
| `right` | float (0.0 – 0.5) | 0.0 | Right margin as fraction of source width to remove. |
| `top` | float (0.0 – 0.5) | 0.0 | Top margin as fraction of source height to remove. |

