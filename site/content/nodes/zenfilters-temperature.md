+++
title = "Temperature"
description = "zenfilters.temperature — filter node"
weight = 50

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.temperature"
role = "filter"
group = "color"
+++

Color temperature adjustment (warm/cool) via Oklab b shift.  Positive values warm the image (shift toward yellow/orange). Negative values cool it (shift toward blue).

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `shift` | float (-1.0 – 1.0) | 0.0 | Color temperature shift (negative = cool, positive = warm) |

