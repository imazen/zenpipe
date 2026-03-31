# ⚙️ Sharpen

> **ID:** `zenfilters.sharpen` · **Role:** filter · **Group:** detail

Unsharp mask sharpening on L channel.  Like clarity but with a smaller sigma for fine detail enhancement. Sharpening in Oklab L avoids color fringing at high-contrast edges.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `amount` | float (0.0 – 2.0) | 0.0 | Sharpening strength (×) |
| `sigma` | float (0.5 – 3.0) | 1.0 | Blur sigma for detail extraction (px) |
