+++
title = "Highlight Recovery"
description = "zenfilters.highlight_recovery — filter node"
weight = 80

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.highlight_recovery"
role = "filter"
group = "tone_range"
+++

Automatic soft-clip recovery for blown highlights.  Analyzes the L histogram to detect blown highlight content, then applies a proportional soft knee compression. Images with properly exposed highlights are barely affected.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `strength` | float (0.0 – 1.0) | 0.0 | Recovery strength (0 = off, 1 = full) |

