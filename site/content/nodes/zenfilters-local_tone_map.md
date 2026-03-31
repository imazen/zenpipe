+++
title = "Local Tone Map"
description = "zenfilters.local_tone_map — filter node"
weight = 400

[taxonomies]
tags = ["tonemap", "hdr", "local", "dynamic range"]

[extra]
node_id = "zenfilters.local_tone_map"
role = "filter"
group = "tone_range"
stage = "Filters"
+++

Local tone mapping: compresses dynamic range while preserving local contrast.  Separates the image into a base layer (large-scale luminance) and detail layer (local texture), compresses the base, and recombines. Core of faux HDR processing from a single exposure.

## Parameters

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `compression` | float (0.0 – 1.0) | 0.0 | Dynamic range compression strength |
| `detail_boost` | float (0.5 – 3.0) | 1.0 | Local detail enhancement factor (×) |

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `sigma` | float (5.0 – 100.0) | 30.0 | Base layer extraction sigma (larger = coarser separation) (px) |

