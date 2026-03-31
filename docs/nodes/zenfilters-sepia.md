# ⚙️ Sepia

> **ID:** `zenfilters.sepia` · **Role:** filter · **Group:** color

Sepia tone effect in perceptual Oklab space.  Desaturates the image, then applies a warm brown tint by shifting the a and b channels toward the sepia point.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `amount` | float (0.0 – 1.0) | 1.0 | Sepia strength (0 = grayscale, 1 = full sepia) |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `s.sepia` | — | `amount` |

**Example:** `?s.sepia=1.0`
