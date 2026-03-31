# ⚙️ Invert

> **ID:** `zenfilters.invert` · **Role:** filter · **Group:** effects

Color inversion in Oklab space.  Inverts lightness (L' = 1.0 - L) and negates chroma (a' = -a, b' = -b). Produces a perceptually correct negative.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enabled` | bool | True | Enable/disable. Always true when the node is present. Exists to enable RIAPI s.invert=true querystring support. |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `s.invert` | — | `enabled` |

**Example:** `?s.invert=True`
