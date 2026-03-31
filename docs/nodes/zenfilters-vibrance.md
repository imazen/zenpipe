# ⚙️ Vibrance

> **ID:** `zenfilters.vibrance` · **Role:** filter · **Group:** color

Smart saturation that protects already-saturated colors.  Boosts chroma of low-saturation pixels more than high-saturation ones, preventing skin tone and sky clipping.

## Parameters

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `amount` | float (-1.0 – 1.0) | 0.0 | Vibrance boost (0 = off, 1 = full) |

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `protection` | float (0.5 – 4.0) | 2.0 | Protection exponent for already-saturated colors |
