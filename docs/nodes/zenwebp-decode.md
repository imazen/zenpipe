# ⚙️ Decode Webp

> **ID:** `zenwebp.decode` · **Role:** decode · **Group:** decode
> **Tags:** `webp`, `decode`

WebP decode node.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `dithering_strength` | int (0 – 100) | 0 | Dithering Strength *(optional)* |
| `upsampling` | string | — | Upsampling *(optional)* |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `webp.dithering` | `webp.dither` | `dithering_strength` |
| `webp.upsampling` | — | `upsampling` |

**Example:** `?webp.dithering=400&webp.upsampling=value`
