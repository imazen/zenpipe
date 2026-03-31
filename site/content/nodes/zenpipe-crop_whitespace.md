+++
title = "Crop Whitespace"
description = "zenpipe.crop_whitespace — resize node"
weight = 300

[taxonomies]
tags = ["crop", "whitespace", "trim", "content", "analysis"]

[extra]
node_id = "zenpipe.crop_whitespace"
role = "resize"
group = "analysis"
stage = "Resize & Layout"
+++

Detect and crop uniform borders (whitespace trimming).  Materializes the upstream image, scans inward from each edge to find where pixel values diverge from the border color, then crops to the detected content bounds plus optional padding.  RIAPI: `?trim.threshold=80&trim.percentpadding=0.5` JSON: `{ "threshold": 80, "percent_padding": 0.5 }`

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `percent_padding` | float (0.0 – 50.0) | 0.0 | Padding around detected content as a percentage of content dimensions.  0.0 = tight crop, 0.5 = 0.5% padding on each side. (%) |
| `threshold` | int (0 – 255) | 80 | Color distance threshold (0–255).  Pixels within this distance of the border color are considered "whitespace". Lower = stricter, higher = more tolerant. |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `trim.percentpadding` | — | `percent_padding` |
| `trim.threshold` | — | `threshold` |

**Example:** `?trim.percentpadding=value&trim.threshold=80`

