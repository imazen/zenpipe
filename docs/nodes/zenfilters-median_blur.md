# ⚙️ Median Blur

> **ID:** `zenfilters.median_blur` · **Role:** filter · **Group:** detail
> **Tags:** `median`, `denoise`, `impulse`, `edge-preserving`

Median filter for impulse noise removal (preserves edges).  Replaces each pixel with the median of its neighborhood. Unlike Gaussian blur, the median filter preserves edges while removing salt-and-pepper noise.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `filter_chroma` | bool | False | Also apply median to color channels (a, b) |
| `radius` | int (1 – 5) | 1 | Neighborhood radius (1 = 3x3, 2 = 5x5, 3 = 7x7) (px) |
