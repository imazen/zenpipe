# ⚙️ Whites / Blacks

> **ID:** `zenfilters.whites_blacks` · **Role:** filter · **Group:** tone_range

Whites and Blacks adjustment -- targeted luminance control for the extreme ends of the histogram.  Unlike BlackPoint/WhitePoint (which remap the entire range), Whites/Blacks apply a smooth, localized adjustment that matches Lightroom's Whites/Blacks sliders.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `blacks` | float (-1.0 – 1.0) | 0.0 | Blacks adjustment (positive = lift shadows) |
| `whites` | float (-1.0 – 1.0) | 0.0 | Whites adjustment (positive = brighten highlights) |
