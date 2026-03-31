+++
title = "Hue Rotate"
description = "zenfilters.hue_rotate — filter node"
weight = 400

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.hue_rotate"
role = "filter"
group = "color"
stage = "Filters"
+++

Hue rotation in Oklab a/b plane.  Rotates colors around the hue circle by the specified angle in degrees. Preserves lightness and chroma.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `degrees` | float (-180.0 – 180.0) | 0.0 | Rotation angle in degrees (°) |

