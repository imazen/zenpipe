+++
title = "Region Viewport"
description = "zenlayout.region — orient node"
weight = 200

[taxonomies]
tags = ["region", "viewport", "crop", "pad", "geometry"]

[extra]
node_id = "zenlayout.region"
role = "orient"
group = "geometry"
stage = "Orient & Crop"
+++

Viewport into the source image, unifying crop and pad.  Defines a rectangular window using edge coordinates, each expressed as a percentage of the source dimension plus a pixel offset: `resolved = source_dim * pct + px`.  - Viewport smaller than source = crop - Viewport extending beyond source = pad (filled with color) - Viewport entirely outside source = blank canvas  Coordinates are **edge-based** (left, top, right, bottom), not origin + size.  Examples: - Crop 10px from each edge: left_px=10, top_px=10, right_pct=1.0, right_px=-10, bottom_pct=1.0, bottom_px=-10 - Add 20px padding: left_px=-20, top_px=-20, right_pct=1.0, right_px=20, bottom_pct=1.0, bottom_px=20 - Center 50% of image: left_pct=0.25, top_pct=0.25, right_pct=0.75, bottom_pct=0.75

## Parameters

### Bottom

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `bottom_pct` | float (-1.0 – 2.0) | 1.0 | Bottom edge: fraction of source height (1.0 = far edge). |
| `bottom_px` | int (-65535 – 65535) | 0 | Bottom edge: pixel offset added after percentage. (px) |

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `color` | string | transparent | Fill color for areas outside the source image.  Accepts "transparent", "white", "black", or hex "#RRGGBB" / "#RRGGBBAA". |

### Left

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `left_pct` | float (-1.0 – 2.0) | 0.0 | Left edge: fraction of source width (0.0 = origin). |
| `left_px` | int (-65535 – 65535) | 0 | Left edge: pixel offset added after percentage. (px) |

### Right

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `right_pct` | float (-1.0 – 2.0) | 1.0 | Right edge: fraction of source width (1.0 = far edge). |
| `right_px` | int (-65535 – 65535) | 0 | Right edge: pixel offset added after percentage. (px) |

### Top

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `top_pct` | float (-1.0 – 2.0) | 0.0 | Top edge: fraction of source height (0.0 = origin). |
| `top_px` | int (-65535 – 65535) | 0 | Top edge: pixel offset added after percentage. (px) |

