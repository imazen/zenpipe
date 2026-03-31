+++
title = "Dehaze"
description = "zenfilters.dehaze — filter node"
weight = 400

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.dehaze"
role = "filter"
group = "effects"
stage = "Filters"
+++

Spatially-adaptive haze removal using dark channel prior.  Uses a dark channel prior analog in Oklab space to estimate and remove atmospheric haze. Hazy regions get strong correction while clear regions are barely affected.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `strength` | float (0.0 – 1.0) | 0.0 | Dehaze correction strength |

