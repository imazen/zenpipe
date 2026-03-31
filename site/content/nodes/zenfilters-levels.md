+++
title = "Levels"
description = "zenfilters.levels — filter node"
weight = 70

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.levels"
role = "filter"
group = "tone_map"
+++

Input/output range remapping with gamma correction.  The classic Photoshop/Lightroom Levels dialog: clip input range, adjust midtone gamma, and remap output range.

## Parameters

### Midtone

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `gamma` | float (0.10000000149011612 – 10.0) | 1.0 | Midtone adjustment (1 = linear, >1 = brighten, <1 = darken) |

### Input

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `in_black` | float (0.0 – 1.0) | 0.0 | Input black point (clip shadows) |
| `in_white` | float (0.0 – 1.0) | 1.0 | Input white point (clip highlights) |

### Output

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `out_black` | float (0.0 – 1.0) | 0.0 | Minimum output luminance |
| `out_white` | float (0.0 – 1.0) | 1.0 | Maximum output luminance |

