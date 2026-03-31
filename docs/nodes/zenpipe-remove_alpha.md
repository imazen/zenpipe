# ⚙️ Remove Alpha

> **ID:** `zenpipe.remove_alpha` · **Role:** filter · **Group:** color
> **Tags:** `alpha`, `matte`, `composite`, `flatten`

Remove alpha channel by compositing onto a solid matte color.  Produces RGB output suitable for JPEG encoding. The compositing is done in sRGB space (matching browser behavior for CSS background-color).  JSON: `{ "matte_r": 255, "matte_g": 255, "matte_b": 255 }`

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `matte_b` | int (0 – 255) | 255 | Matte blue channel (sRGB, 0–255). |
| `matte_g` | int (0 – 255) | 255 | Matte green channel (sRGB, 0–255). |
| `matte_r` | int (0 – 255) | 255 | Matte red channel (sRGB, 0–255). |
