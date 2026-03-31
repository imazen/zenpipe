+++
title = "Camera Calibration"
description = "zenfilters.camera_calibration — filter node"
weight = 50

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.camera_calibration"
role = "filter"
group = "color"
+++

Camera calibration -- primary color hue and saturation calibration with shadow tint.  Equivalent to Lightroom's Camera Calibration panel. Adjusts how the camera's RGB primaries map to final color.

## Parameters

### Blue Primary

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `blue_hue` | float (-60.0 – 60.0) | 0.0 | Blue primary hue shift (°) |
| `blue_saturation` | float (0.0 – 3.0) | 1.0 | Blue primary saturation scale (×) |

### Green Primary

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `green_hue` | float (-60.0 – 60.0) | 0.0 | Green primary hue shift (°) |
| `green_saturation` | float (0.0 – 3.0) | 1.0 | Green primary saturation scale (×) |

### Red Primary

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `red_hue` | float (-60.0 – 60.0) | 0.0 | Red primary hue shift (°) |
| `red_saturation` | float (0.0 – 3.0) | 1.0 | Red primary saturation scale (×) |

### Shadow

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `shadow_tint` | float (-1.0 – 1.0) | 0.0 | Shadow tint: green-magenta balance in shadows |

