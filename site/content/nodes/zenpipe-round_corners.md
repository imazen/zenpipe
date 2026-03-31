+++
title = "Round Corners"
description = "zenpipe.round_corners — filter node"
weight = 100

[taxonomies]
tags = ["corners", "rounded", "mask", "border-radius"]

[extra]
node_id = "zenpipe.round_corners"
role = "filter"
group = "canvas"
+++

Apply rounded corners with anti-aliased masking.  Generates a `RoundedRectMask` (via zenblend) and applies it to the alpha channel. Transparent corners reveal the background color, or remain transparent for PNG/WebP/AVIF output.  Supports uniform radius, per-corner radii, percentage-based radii, and circle mode (elliptical crop for non-square images).  JSON: `{ "radius": 20.0, "bg_color": [0, 0, 0, 0] }`

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `bg_a` | int (0 – 255) | 0 | Background color alpha channel. 0 = transparent (preserve alpha). |
| `bg_b` | int (0 – 255) | 0 | Background color blue channel. |
| `bg_g` | int (0 – 255) | 0 | Background color green channel. |
| `bg_r` | int (0 – 255) | 0 | Background color red channel (for compositing transparent corners). |
| `mode` | string | pixels | Rounding mode: "pixels" (default), "percentage", "circle", "pixels_custom", "percentage_custom". |
| `radius` | float (0.0 – 10000.0) | 0.0 | Corner radius in pixels (uniform). Clamped to min(width, height) / 2. Used when mode is "pixels" (default) or as fallback.  RIAPI: `?s.roundcorners=20` (single value) or `?s.roundcorners=10,20,30,40` (TL,TR,BR,BL) (px) |
| `radius_bl` | float (0.0 – 10000.0) | -1.0 | Bottom-left corner radius (for per-corner modes). |
| `radius_br` | float (0.0 – 10000.0) | -1.0 | Bottom-right corner radius (for per-corner modes). |
| `radius_tl` | float (0.0 – 10000.0) | -1.0 | Top-left corner radius (for per-corner modes). |
| `radius_tr` | float (0.0 – 10000.0) | -1.0 | Top-right corner radius (for per-corner modes). |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `s.roundcorners` | — | `radius` |

**Example:** `?s.roundcorners=value`

