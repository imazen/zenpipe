+++
title = "Clarity"
description = "zenfilters.clarity — filter node"
weight = 40

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.clarity"
role = "filter"
group = "detail"
+++

Multi-scale local contrast enhancement on L channel.  Uses a two-band decomposition to isolate the mid-frequency "clarity" band, avoiding both noise amplification and halos.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `amount` | float (-1.0 – 1.0) | 0.0 | Enhancement amount (positive = enhance, negative = soften) |
| `sigma` | float (1.0 – 16.0) | 4.0 | Fine-scale blur sigma (coarse blur is 4x this) (px) |

