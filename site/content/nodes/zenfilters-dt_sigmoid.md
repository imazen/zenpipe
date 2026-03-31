+++
title = "DtSigmoid"
description = "zenfilters.dt_sigmoid — filter node"
weight = 70

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.dt_sigmoid"
role = "filter"
group = "tone_map"
+++

darktable-compatible sigmoid tone mapper.  Implements the generalized log-logistic sigmoid from darktable's sigmoid module. Operates per-channel in linear RGB space (not Oklab).

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `contrast` | float (0.10000000149011612 – 10.0) | 1.5 | Middle-grey contrast |
| `hue_preservation` | float (0.0 – 1.0) | 1.0 | Hue preservation (0.0 = per-channel, 1.0 = full hue preservation) |
| `skew` | float (-1.0 – 1.0) | 0.0 | Contrast skewness (-1 to 1, 0 = symmetric) |

