+++
title = "Overlay"
description = "zenpipe.overlay — composite node"
weight = 110

[taxonomies]
tags = ["overlay", "watermark", "logo", "composite"]

[extra]
node_id = "zenpipe.overlay"
role = "composite"
group = "composite"
+++

Overlay a small image (watermark, logo) at absolute coordinates.  Single input (the background). The overlay image data is provided via io_id (loaded from a separate input buffer).  Overlay is auto-converted to premultiplied linear f32. Opacity scales the overlay's alpha channel before compositing.

## Input Ports

| Port | Label | Edge Kind | Required |
|------|-------|-----------|----------|
| `input` | Background image | input | Yes |
| `from_io` | Overlay image (io_id) | input | Yes |

## Parameters

### Blending

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `blend_mode` | string | source_over | Blend mode: "source_over" (default), "multiply", "screen", etc. |
| `opacity` | float (0.0 – 1.0) | 1.0 | Overlay opacity (0.0 = invisible, 1.0 = fully opaque). |

### Source

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `io_id` | int (0 – 1000) | 0 | I/O ID of the overlay image source. |

### Position

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `x` | int (-65535 – 65535) | 0 | X position on the background canvas. (px) |
| `y` | int (-65535 – 65535) | 0 | Y position on the background canvas. (px) |

