# ⚙️ Brilliance

> **ID:** `zenfilters.brilliance` · **Role:** filter · **Group:** detail

Adaptive local contrast based on local average luminance.  Unlike clarity, brilliance adjusts each pixel relative to its local average -- lifting shadows and compressing highlights selectively. Produces natural dynamic range compression similar to Apple's Brilliance slider.

## Parameters

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `amount` | float (-1.0 – 1.0) | 0.0 | Overall effect strength |
| `sigma` | float (2.0 – 50.0) | 10.0 | Blur sigma for computing local average (px) |

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `highlight_strength` | float (0.0 – 1.0) | 0.4000000059604645 | Highlight compression strength |
| `shadow_strength` | float (0.0 – 1.0) | 0.6000000238418579 | Shadow lift strength |
