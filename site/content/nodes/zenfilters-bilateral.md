+++
title = "Bilateral Filter"
description = "zenfilters.bilateral — filter node"
weight = 40

[taxonomies]
tags = ["smooth", "denoise", "edge-preserving"]

[extra]
node_id = "zenfilters.bilateral"
role = "filter"
group = "detail"
+++

Edge-preserving smoothing via guided filter.  Uses a guided filter (He et al., TPAMI 2013) with L as the guide image. O(1) per pixel regardless of radius. Produces locally-linear output that preserves edges from the luminance channel while smoothing noise in all three channels.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `range_sigma` | float (0.0010000000474974513 – 0.5) | 0.10000000149011612 | Edge preservation parameter (smaller = sharper edges) |
| `spatial_sigma` | float (0.5 – 20.0) | 2.0 | Smoothing window size (spatial sigma) (px) |
| `strength` | float (0.0 – 1.0) | 0.0 | Blend strength (0 = off, 1 = full smoothing) |

