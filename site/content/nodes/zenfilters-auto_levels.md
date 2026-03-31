+++
title = "Auto Levels"
description = "zenfilters.auto_levels — filter node"
weight = 400

[taxonomies]
tags = ["auto", "levels", "normalize", "histogram", "stretch"]

[extra]
node_id = "zenfilters.auto_levels"
role = "filter"
group = "auto"
stage = "Filters"
+++

Auto levels: stretch the luminance histogram to fill [0, 1].  Scans the L plane to find cutoff points, then remaps luminance so the low cutoff maps to 0 and the high cutoff maps to 1. Equivalent to ImageMagick `-auto-level`, with smart outlier-resistant plateau detection, optional midpoint gamma correction, chroma scaling, and cast removal.

## Parameters

### Range

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `clip_high` | float (0.0 – 0.10000000149011612) | 0.0 | Fraction of pixels to clip at the bright end (0 = smart plateau detection) |
| `clip_low` | float (0.0 – 0.10000000149011612) | 0.0 | Fraction of pixels to clip at the dark end (0 = smart plateau detection) |

### Color

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `remove_cast` | bool | False | Subtract mean(a) and mean(b) to neutralize color cast |
| `scale_chroma` | bool | False | Scale a/b channels by the same factor as L (raises saturation on stretch) |

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `strength` | float (0.0 – 1.0) | 1.0 | Blend strength (0 = off, 1 = full stretch) |

### Tone

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `target_midpoint` | float (0.0 – 1.0) | 0.0 | Move the median luminance to this value via gamma correction (0 = off) |

