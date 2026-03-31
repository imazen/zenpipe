+++
title = "Bw Mixer"
description = "zenfilters.bw_mixer — filter node"
weight = 400

[taxonomies]
tags = ["color", "grayscale", "bw"]

[extra]
node_id = "zenfilters.bw_mixer"
role = "filter"
group = "color"
stage = "Filters"
+++

Grayscale conversion with per-color luminance weights

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `weights` | array | [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0] | Weight per color range (proportional to chroma) (×) |

