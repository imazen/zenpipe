+++
title = "Black Point"
description = "zenfilters.black_point — filter node"
weight = 400

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.black_point"
role = "filter"
group = "tone_range"
stage = "Filters"
+++

Black point adjustment on Oklab L channel.  Remaps the shadow floor. A black point of 0.05 means values that were L=0.05 become L=0.0, and the range is stretched accordingly.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `level` | float (0.0 – 0.5) | 0.0 | Black point level (0 = no change, 0.1 = crush bottom 10%) |

