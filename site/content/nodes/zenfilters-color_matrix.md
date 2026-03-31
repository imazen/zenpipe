+++
title = "Color Matrix"
description = "zenfilters.color_matrix — filter node"
weight = 50

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.color_matrix"
role = "filter"
group = "color"
+++

5x5 color matrix applied in linear RGB space.  Transforms `[R, G, B, A, 1]` -> `[R', G', B', A', 1]` using a row-major 5x5 matrix (25 elements). The 5th column is the bias/offset. The filter converts Oklab -> linear RGB, applies the matrix, then converts back.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `matrix` | array | [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] | Row-major 5x5 color matrix (25 floats) |

