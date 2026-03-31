+++
title = "Alpha"
description = "zenfilters.alpha — filter node"
weight = 90

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.alpha"
role = "filter"
group = "effects"
+++

Alpha channel scaling for transparency adjustment.  Multiplies all alpha values by a constant factor. Useful for fade effects or global opacity changes. If no alpha channel exists, this is a no-op.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `factor` | float (0.0 – 1.0) | 1.0 | Alpha multiplier (0 = fully transparent, 1 = unchanged) |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `s.alpha` | — | `factor` |

**Example:** `?s.alpha=1.0`

