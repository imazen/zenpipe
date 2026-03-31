# ⚙️ Edge Detect

> **ID:** `zenfilters.edge_detect` · **Role:** filter · **Group:** detail
> **Tags:** `edge`, `detect`, `sobel`, `canny`

Edge detection on the L (lightness) channel.  Replaces L with gradient magnitude (Sobel/Laplacian) or binary edges (Canny), normalized to [0, 1]. Chroma channels are zeroed to produce a grayscale edge map.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `mode` | int (0 – 2) | 0 | Detection mode (0 = Sobel, 1 = Laplacian, 2 = Canny) |
| `strength` | float (0.10000000149011612 – 5.0) | 1.0 | Sobel/Laplacian: output scaling. Canny: Gaussian blur sigma. (×) |
