+++
title = "Composite"
description = "zenpipe.composite — composite node"
weight = 110

[taxonomies]
tags = ["composite", "blend", "overlay"]

[extra]
node_id = "zenpipe.composite"
role = "composite"
group = "composite"
+++

Composite a foreground image onto a background at a position.  Two inputs required: - **background** (Canvas edge): the base image to draw onto - **foreground** (Input edge): the image being composited  Both inputs are auto-converted to premultiplied linear f32. Default blend mode is Porter-Duff source-over.

## Input Ports

| Port | Label | Edge Kind | Required |
|------|-------|-----------|----------|
| `canvas` | Background canvas | canvas | Yes |
| `input` | Foreground image | input | Yes |

## Parameters

### Blending

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `blend_mode` | string | source_over | Blend mode: "source_over" (default), "multiply", "screen", etc. |

### Position

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `fg_x` | int (0 – 65535) | 0 | X position of the foreground on the background canvas. (px) |
| `fg_y` | int (0 – 65535) | 0 | Y position of the foreground on the background canvas. (px) |

