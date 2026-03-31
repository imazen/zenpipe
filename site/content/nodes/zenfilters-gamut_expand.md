+++
title = "Gamut Expand"
description = "zenfilters.gamut_expand — filter node"
weight = 400

[taxonomies]
tags = ["color", "gamut", "p3", "wide"]

[extra]
node_id = "zenfilters.gamut_expand"
role = "filter"
group = "color"
stage = "Filters"
+++

Hue-selective chroma boost simulating wider color gamuts (P3).  Selectively boosts chroma in hue regions where Display P3 extends beyond sRGB, producing vivid reds, richer greens, and punchier oranges. Already-saturated colors get less boost (vibrance-style protection).

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `strength` | float (0.0 – 1.0) | 0.0 | Expansion strength (0 = sRGB, 1 = full P3-like expansion) |

