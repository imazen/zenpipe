+++
title = "Basecurve Tone Map"
description = "zenfilters.basecurve_tonemap — filter node"
weight = 400

[taxonomies]
tags = ["tonemap", "camera", "basecurve"]

[extra]
node_id = "zenfilters.basecurve_tonemap"
role = "filter"
group = "tone_map"
stage = "Filters"
+++

Camera-matched basecurve tone mapping from darktable presets


## Accepted Values

- **`preset`**: `linear`, `film_like`, `nikon`, `canon`, `sony`

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `chroma_compression` | float (0.0 – 1.0) | 0.4000000059604645 | Chroma compression strength (0=L-only, 1=full RGB-like desaturation) |
| `preset` | string | — | Camera preset name (e.g., "nikon_d7000", "canon_eos_5d_mark_ii") |

