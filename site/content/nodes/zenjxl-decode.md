+++
title = "Decode Jxl"
description = "zenjxl.decode — decode node"
weight = 10

[taxonomies]
tags = ["jxl", "jpeg-xl", "decode", "codec"]

[extra]
node_id = "zenjxl.decode"
role = "decode"
group = "decode"
+++

JPEG XL decoder configuration.

## Parameters

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `adjust_orientation` | bool | True | Adjust Orientation |

### HDR

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `intensity_target` | float (0.0 – 10000.0) | 0.0 | Intensity Target (nits) *(optional)* |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `jxl.orient` | — | `adjust_orientation` |
| `jxl.nits` | — | `intensity_target` |

**Example:** `?jxl.orient=True&jxl.nits=value`

