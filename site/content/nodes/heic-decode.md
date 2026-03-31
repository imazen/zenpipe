+++
title = "Decode Heic"
description = "heic.decode — decode node"
weight = 10

[taxonomies]
tags = ["heic", "heif", "hdr", "depth"]

[extra]
node_id = "heic.decode"
role = "decode"
group = "decode"
+++

HEIC/HEIF decode node.

## Parameters

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `decode_thumbnail` | bool | False | Placeholder — not yet wired to the decoder. |

### Supplements

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `extract_depth` | bool | False | Extract Depth Map |
| `extract_gain_map` | bool | False | Extract Gain Map |
| `extract_mattes` | bool | False | Placeholder — not yet wired to the decoder. |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `heic.thumbnail` | — | `decode_thumbnail` |
| `heic.depth` | — | `extract_depth` |
| `heic.gain_map` | — | `extract_gain_map` |
| `heic.mattes` | — | `extract_mattes` |

**Example:** `?heic.thumbnail=value&heic.depth=value&heic.gain_map=value`

