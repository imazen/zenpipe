+++
title = "Output Limits"
description = "zenlayout.output_limits — resize node"
weight = 140

[taxonomies]
tags = ["limits", "alignment", "layout", "codec"]

[extra]
node_id = "zenlayout.output_limits"
role = "resize"
group = "layout"
+++

Safety limits and codec alignment for output dimensions.  Constrains the final output to max/min dimension bounds and optionally aligns dimensions to codec block boundaries (e.g., MCU multiples for JPEG).  Processing order: max (scale down) → min (scale up) → align (snap). Max always wins over min if they conflict.  JSON: `{ "max_w": 4096, "max_h": 4096, "align_x": 16, "align_y": 16, "align_mode": "extend" }`

## Parameters

### Align

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `align_mode` | string | extend | How to handle alignment snapping.  - "crop" — round down, lose edge pixels - "extend" — round up, replicate edge pixels (default) - "distort" — round to nearest, slight stretch |
| `align_x` | int (0 – 4294967295) | 0 | Horizontal alignment multiple in pixels. 0 = no alignment.  Output width will be snapped to the nearest multiple of this value. Common values: 2 (4:2:2), 8 (DCT block), 16 (4:2:0 MCU). (px) |
| `align_y` | int (0 – 4294967295) | 0 | Vertical alignment multiple in pixels. 0 = no alignment.  Output height will be snapped to the nearest multiple of this value. (px) |

### Max

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `max_h` | int (0 – 4294967295) | 0 | Maximum output height. 0 = no limit. Scales down proportionally if exceeded. (px) |
| `max_w` | int (0 – 4294967295) | 0 | Maximum output width. 0 = no limit. Scales down proportionally if exceeded. (px) |

### Min

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `min_h` | int (0 – 4294967295) | 0 | Minimum output height. 0 = no minimum. Scales up proportionally if below. (px) |
| `min_w` | int (0 – 4294967295) | 0 | Minimum output width. 0 = no minimum. Scales up proportionally if below. (px) |

