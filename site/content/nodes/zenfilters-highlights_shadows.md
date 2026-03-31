+++
title = "Highlights / Shadows"
description = "zenfilters.highlights_shadows — filter node"
weight = 80

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.highlights_shadows"
role = "filter"
group = "tone_range"
+++

Targeted highlight recovery and shadow lift.  Positive highlights compresses bright areas (recovery). Positive shadows lifts dark areas (fill light). Custom thresholds control where transitions begin.

## Parameters

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `highlight_threshold` | float (0.5 – 0.949999988079071) | 0.699999988079071 | L value above which pixels are in the highlight zone |
| `shadow_threshold` | float (0.05000000074505806 – 0.5) | 0.30000001192092896 | L value below which pixels are in the shadow zone |

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `highlights` | float (-1.0 – 1.0) | 0.0 | Highlight recovery (positive = compress, negative = boost) |
| `shadows` | float (-1.0 – 1.0) | 0.0 | Shadow recovery (positive = lift, negative = deepen) |

