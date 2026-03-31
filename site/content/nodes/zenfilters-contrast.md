+++
title = "Contrast"
description = "zenfilters.contrast — filter node"
weight = 400

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.contrast"
role = "filter"
group = "tone"
stage = "Filters"
+++

Power-curve contrast adjustment pivoted at middle grey.  Uses a power curve that pivots at the perceptual equivalent of 18.42% middle grey in Oklab space. Positive values increase contrast.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `amount` | float (-1.0 – 1.0) | 0.0 | Contrast strength (positive = increase, negative = flatten) |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `s.contrast` | — | `amount` |

**Example:** `?s.contrast=value`

