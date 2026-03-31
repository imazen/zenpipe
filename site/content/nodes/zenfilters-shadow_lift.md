+++
title = "Shadow Lift"
description = "zenfilters.shadow_lift — filter node"
weight = 80

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.shadow_lift"
role = "filter"
group = "tone_range"
+++

Automatic toe-curve recovery for crushed shadows.  Analyzes the L histogram to detect crushed shadow content, then applies a proportional toe lift curve. Images with properly exposed shadows are barely affected.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `strength` | float (0.0 – 1.0) | 0.0 | Lift strength (0 = off, 1 = full) |

