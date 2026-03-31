# ⚙️ Decode Jxl

> **ID:** `zenjxl.decode` · **Role:** decode · **Group:** decode
> **Tags:** `jxl`, `jpeg-xl`, `decode`, `codec`

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
