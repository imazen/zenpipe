+++
title = "Crop Percent"
description = "zenlayout.crop_percent — orient node"
weight = 200

[taxonomies]
tags = ["crop", "geometry"]

[extra]
node_id = "zenlayout.crop_percent"
role = "orient"
group = "geometry"
stage = "Orient & Crop"
+++

Crop the image using percentage-based coordinates.  All coordinates are fractions of source dimensions (0.0 = origin, 1.0 = full extent). For example, `x=0.1, y=0.1, w=0.8, h=0.8` removes 10% from each edge.  JSON: `{ "x": 0.1, "y": 0.1, "w": 0.8, "h": 0.8 }`

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `h` | float (0.0 – 1.0) | 1.0 | Height as fraction of source height (1.0 = full height). |
| `w` | float (0.0 – 1.0) | 1.0 | Width as fraction of source width (1.0 = full width). |
| `x` | float (0.0 – 1.0) | 0.0 | Left edge as fraction of source width (0.0 = left edge). |
| `y` | float (0.0 – 1.0) | 0.0 | Top edge as fraction of source height (0.0 = top edge). |

