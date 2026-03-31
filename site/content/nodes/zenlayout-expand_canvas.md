+++
title = "Expand Canvas"
description = "zenlayout.expand_canvas — resize node"
weight = 100

[taxonomies]
tags = ["pad", "canvas", "geometry"]

[extra]
node_id = "zenlayout.expand_canvas"
role = "resize"
group = "canvas"
+++

Expand the canvas by adding padding around the image.  Adds specified pixel amounts to each side. The fill color defaults to "transparent" (premultiplied zero). Accepts CSS-style named colors or hex values.  JSON: `{ "left": 10, "top": 10, "right": 10, "bottom": 10, "color": "white" }`

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `bottom` | int (0 – 4294967295) | 0 | Bottom padding in pixels. (px) |
| `color` | string | transparent | Fill color for the expanded area.  Accepts "transparent", "white", "black", or hex "#RRGGBB" / "#RRGGBBAA". |
| `left` | int (0 – 4294967295) | 0 | Left padding in pixels. (px) |
| `right` | int (0 – 4294967295) | 0 | Right padding in pixels. (px) |
| `top` | int (0 – 4294967295) | 0 | Top padding in pixels. (px) |

