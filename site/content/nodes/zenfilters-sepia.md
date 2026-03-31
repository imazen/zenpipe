+++
title = "Sepia"
description = "zenfilters.sepia — filter node"
weight = 400

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.sepia"
role = "filter"
group = "color"
stage = "Filters"
+++

Sepia tone effect in perceptual Oklab space.  Desaturates the image, then applies a warm brown tint by shifting the a and b channels toward the sepia point.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `amount` | float (0.0 – 1.0) | 1.0 | Sepia strength (0 = grayscale, 1 = full sepia) |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `s.sepia` | — | `amount` |

**Example:** `?s.sepia=1.0`

