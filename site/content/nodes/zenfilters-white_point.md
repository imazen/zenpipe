+++
title = "White Point"
description = "zenfilters.white_point — filter node"
weight = 400

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.white_point"
role = "filter"
group = "tone_range"
stage = "Filters"
+++

White point adjustment on Oklab L channel.  Scales the L range so that `level` maps to L=1.0. Values < 1.0 brighten highlights; values > 1.0 extend the dynamic range. Optional soft-clip headroom compresses super-whites instead of hard clipping.

## Parameters

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `headroom` | float (0.0 – 0.5) | 0.0 | Soft-clip rolloff fraction above white point |

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `level` | float (0.5 – 2.0) | 1.0 | White point level (1.0 = no change, <1 = brighten highlights) |

