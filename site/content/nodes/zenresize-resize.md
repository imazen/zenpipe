+++
title = "Resize"
description = "zenresize.resize — resize node"
weight = 30

[taxonomies]
tags = ["resize", "scale", "resample"]

[extra]
node_id = "zenresize.resize"
role = "resize"
group = "geometry"
+++

Forced resize to exact dimensions (no layout planning).  Unlike [`Constrain`] which applies constraint modes (fit, crop, pad), this resizes unconditionally to the specified width × height. Used when the caller has already determined the target dimensions.  Skipped at compile time when input dimensions match target (identity resize).  JSON: `{ "w": 400, "h": 300, "filter": "robidoux" }` RIAPI: Not directly exposed — use Constrain for querystring-driven resize.

## Parameters

### Quality

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `filter` | string | — | Resampling filter name (e.g., "robidoux", "lanczos", "mitchell"). Empty string = default (Robidoux). |
| `sharpen` | float (0.0 – 100.0) | 0.0 | Post-resize sharpening percentage (0–100). 0 = no sharpening. (%) |

### Dimensions

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `h` | int (1 – 65535) | 1 | Target height in pixels. (px) |
| `w` | int (1 – 65535) | 1 | Target width in pixels. (px) |

